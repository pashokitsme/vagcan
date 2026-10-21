//! Every car command keeps its data on stdout and everything else on stderr, at every
//! level (owner, 2026-10-20: «так и надо, во всех командах»).
//!
//! Each command runs against a scripted car at levels 0, 1 and 2, under
//! `vag_cli_core::verbosity::Capture`, and [`assert_split`] checks the three runs: stdout
//! is the same at every level, a level only adds stderr lines, and no stdout line is on
//! stderr. The level-0 stdout of each is pinned too.
//!
//! The car is made up: units, part numbers and texts that belong to no car.
//!
//! **Its own test binary, on purpose**, as `watch_records_units.rs` is: the commands read
//! and write `~/.vagcan`, so this process points `HOME` at a temporary directory before
//! anything reads it — safe only while this is the one thread, which one test on a
//! current-thread runtime guarantees.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use vag_cli_core::verbosity::{Capture, Captured, assert_split};
use vag_cli_diag::{car, faults};
use vag_uds_transport::{AsyncIsoTpTransport, CanId, TransportError};

const GATEWAY: u16 = 0x710;
const ENGINE: u16 = 0x7E0;
const GEARBOX: u16 = 0x7E1;
const ESC: u16 = 0x713;
/// Listed by the gateway, never answers.
const SILENT: u16 = 0x70E;
const VIN: &str = "TESTVIN0000000003";

/// What the car answers: identifiers per unit, and fault codes per unit.
#[derive(Default)]
struct Script {
	records: BTreeMap<(u16, u16), Vec<u8>>,
	/// `(code, status)` per unit, as `19 02` lists them.
	dtcs: BTreeMap<u16, Vec<([u8; 3], u8)>>,
	/// Extended record 01 per `(unit, code)`.
	extended: BTreeMap<(u16, [u8; 3]), Vec<u8>>,
}

#[derive(Clone)]
struct Car {
	script: Arc<Mutex<Script>>,
}

struct CarChannel {
	car: Car,
	request: u16,
	pdu: Option<Vec<u8>>,
}

impl vag_uds_can::UnitLink for Car {
	type Channel = CarChannel;

	fn to_unit(self, request: CanId, _response: CanId) -> CarChannel {
		let CanId::Standard(request) = request else { panic!("extended id") };
		CarChannel {
			car: self,
			request,
			pdu: None,
		}
	}

	fn release(channel: CarChannel) -> Car {
		channel.car
	}
}

impl AsyncIsoTpTransport for CarChannel {
	async fn send(&mut self, pdu: &[u8]) -> Result<(), TransportError> {
		self.pdu = Some(pdu.to_vec());
		Ok(())
	}

	async fn recv(&mut self, _timeout: Duration) -> Result<Vec<u8>, TransportError> {
		let Some(pdu) = self.pdu.take() else {
			return Err(TransportError::Timeout);
		};
		let script = self.car.script.lock().unwrap();
		// A unit that is not there says nothing at all.
		if !script.records.keys().any(|(request, _)| *request == self.request) {
			return Err(TransportError::Timeout);
		}
		match pdu.as_slice() {
			[0x22, ids @ ..] => {
				let mut answer = vec![0x62];
				for did in ids.chunks(2).map(|b| u16::from_be_bytes([b[0], b[1]])) {
					if let Some(record) = script.records.get(&(self.request, did)) {
						answer.extend_from_slice(&did.to_be_bytes());
						answer.extend_from_slice(record);
					}
				}
				Ok(match answer.len() {
					1 => vec![0x7F, 0x22, 0x31],
					_ => answer,
				})
			}
			[0x19, sub @ (0x02 | 0x0A), ..] => {
				let mut answer = vec![0x59, *sub, 0xFF];
				for (code, status) in script.dtcs.get(&self.request).into_iter().flatten() {
					answer.extend_from_slice(code);
					answer.push(*status);
				}
				Ok(answer)
			}
			[0x19, 0x06, a, b, c, _] => {
				let code = [*a, *b, *c];
				let status = script
					.dtcs
					.get(&self.request)
					.and_then(|codes| codes.iter().find(|(c, _)| *c == code))
					.map_or(0, |(_, s)| *s);
				let mut answer = vec![0x59, 0x06, *a, *b, *c, status];
				if let Some(record) = script.extended.get(&(self.request, code)) {
					answer.push(0x01);
					answer.extend_from_slice(record);
				}
				Ok(answer)
			}
			[sid, ..] => Ok(vec![0x7F, *sid, 0x11]),
			[] => Err(TransportError::Timeout),
		}
	}
}

/// The gateway's installation list with these units' bits set.
fn installation_list(units: &[u16]) -> Vec<u8> {
	let mut bytes = vec![0u8; 32];
	for unit in units {
		let n = (*unit - 0x700) as usize;
		bytes[n / 8] |= 1 << (n % 8);
	}
	bytes
}

fn car() -> Car {
	let text = |s: &str| s.as_bytes().to_vec();
	let mut script = Script::default();
	let records = &mut script.records;
	records.insert((GATEWAY, vag_uds_client::gateway::INSTALLATION_LIST), installation_list(&[ESC, SILENT]));
	for (unit, part, component) in [
		(ENGINE, "PART-ENGINE", "ENGINE SELF"),
		(GEARBOX, "PART-GEAR", "GEAR SELF"),
		(GATEWAY, "PART-GATEWAY", "GATEWAY SELF"),
		(ESC, "PART-ESC", "ESC SELF"),
	] {
		records.insert((unit, 0xF187), text(part));
		records.insert((unit, 0xF197), text(component));
		records.insert((unit, 0xF189), text("0001"));
	}
	records.insert((ENGINE, 0xF190), text(VIN));
	records.insert((ENGINE, 0x0600), vec![0x01, 0x02]);
	// Standing still, coolant 40 °C (0x50 − 40), 1000 rpm (0x0FA0 / 4).
	records.insert((ENGINE, 0xF40D), vec![0x00]);
	records.insert((ENGINE, 0xF405), vec![0x50]);
	records.insert((ENGINE, 0xF40C), vec![0x0F, 0xA0]);
	// The gearbox answers F40D at a width J1979 does not give it.
	records.insert((GEARBOX, 0xF40D), vec![0x9C, 0x02]);
	script.dtcs.insert(ENGINE, vec![([0x00, 0x01, 0x29], 0x09), ([0x00, 0x01, 0x07], 0x10)]);
	script.dtcs.insert(ESC, vec![([0x04, 0x71, 0x20], 0x08)]);
	// The engine and the ESC name their description files, so a freeze-frame layout can be
	// looked up for them.
	records.insert((ENGINE, 0xF19E), text("EV_TestEngine"));
	records.insert((ENGINE, 0xF1A2), text("001001"));
	records.insert((ESC, 0xF19E), text("EV_TestBrake"));
	records.insert((ESC, 0xF1A2), text("001001"));
	// Record 01: priority 6, count 3. Record 02: aging counter 16, 123456 km, time
	// indication, 2026-07-27 00:00:03.
	let standard = [0x06, 0x03, 0x02, 0x10, 0x01, 0xE2, 0x40, 0x00, 0x00, 0x69, 0xF6, 0x00, 0x03];
	// The engine's own record 0x70: 1000 /min, 12.3 V, a reserved byte — described.
	let engine = [&standard[..], &[0x70, 0x0F, 0xA0, 0x7B, 0x00]].concat();
	// The ESC's own record 0x70: VCDS has rows only for another variant of its name.
	let esc = [&standard[..], &[0x70, 0xAA, 0xBB]].concat();
	// Each unit's own stamp, which the standard records are checked against: 123 456 km,
	// the same moment.
	let stamp = vec![0x91, 0x01, 0xE2, 0x40, 0x00, 0x00, 0x69, 0xF6, 0x00, 0x03];
	records.insert((ENGINE, vag_uds_client::dtc::UnitStamp::DID), stamp.clone());
	records.insert((ESC, vag_uds_client::dtc::UnitStamp::DID), stamp);
	script.extended.insert((ENGINE, [0x00, 0x01, 0x29]), engine);
	script.extended.insert((ESC, [0x04, 0x71, 0x20]), esc);
	Car {
		script: Arc::new(Mutex::new(script)),
	}
}

/// One field of the made-up engine's record 0x70, as the cache stores a VCDS freeze-frame
/// row: lead 104, the record's four bytes plus 100.
fn freeze_row(bit: u32, bits: u32, kind: u8, scale: Option<(f64, f64)>, unit: &str, name: &str) -> vag_data_labels::registries::Described {
	use vag_data_labels::registries::{Described, Entry, Value};
	Described {
		section: vag_data_db::FREEZE_FRAME_SECTION,
		entry: Entry {
			row: 1,
			key: 0x70,
			identifier: None,
			ordinal: None,
			lead: Some(104),
			case: None,
			value: Value {
				kind: Some(kind),
				big_endian: true,
				bit_offset: Some(bit),
				bit_length: Some(bits),
				scale,
				..Default::default()
			},
			raw: String::new(),
		},
		name: Some(name.to_string()),
		text_id: None,
		unit: (!unit.is_empty()).then(|| unit.to_string()),
		levels: Vec::new(),
	}
}

/// One record of a made-up variant's extended-data multiplexer, as the ODIS loader reads it.
fn odis_record(number: i32, length: u32, fields: Vec<vag_data_labels::odis::EnvironmentField>) -> vag_data_labels::odis::EnvironmentRecord {
	vag_data_labels::odis::EnvironmentRecord {
		range: Some((number, number)),
		name: None,
		length: Some(length),
		fields,
	}
}

/// One of its fields: unsigned, big-endian, linear.
fn odis_field(name: &str, bit: u32, bits: u32, factor: Option<f64>, unit: Option<&str>) -> vag_data_labels::odis::EnvironmentField {
	vag_data_labels::odis::EnvironmentField {
		name: Some(name.to_string()),
		text_id: None,
		unit: unit.map(str::to_string),
		bit_offset: Some(bit),
		bit_length: Some(bits),
		signed: false,
		big_endian: true,
		scaling: factor.map(|factor| vag_data_labels::Scaling::Linear(vag_data_labels::LinearScale { factor, offset: 0.0 })),
		constant: false,
	}
}

/// Run `$run` (an expression that awaits one command) at levels 0, 1 and 2; check the
/// split and hand back the three captures.
macro_rules! levels {
	($what:expr, $run:expr) => {{
		let mut runs: Vec<Captured> = Vec::new();
		for level in 0..=2u8 {
			let capture = Capture::start_all(level);
			let result: anyhow::Result<()> = $run;
			let captured = capture.finish();
			result.unwrap_or_else(|e| panic!("{} at level {level}: {e:#}\n{captured:?}", $what));
			runs.push(captured);
		}
		assert_split($what, &runs);
		// `VAGCAN_SHOW_LEVELS=1 cargo test --test levels -- --nocapture` prints what each
		// level wrote, for reading the output itself.
		if std::env::var_os("VAGCAN_SHOW_LEVELS").is_some() {
			for (level, run) in runs.iter().enumerate() {
				println!("==== {} -v×{level} stdout\n{}==== stderr\n{}", $what, run.stdout, run.stderr);
			}
		}
		runs
	}};
}

#[tokio::test(flavor = "current_thread")]
async fn every_car_command_keeps_data_on_stdout_and_the_rest_on_stderr_at_every_level() {
	let home = tempfile::tempdir().unwrap();
	// Before any thread but this one exists: see the module docs.
	unsafe {
		std::env::set_var("HOME", home.path());
		// The override a test build's data dir answers to; HOME alone it does not read.
		std::env::set_var(vag_cli_core::datadir::ROOT_ENV, home.path().join(".vagcan"));
		std::env::remove_var(vag_cli_core::project::PROJECT_ENV);
	}
	let open = || async { Ok(car()) };

	let info = levels!("info", car::info(open).await);
	// A card: the car, then the count and when it was read — the minute varies — then a
	// section per unit.
	let (head, rest) = info[0].stdout.split_once("\n").unwrap();
	assert_eq!(head, VIN);
	let (meta, rest) = rest.split_once("\n").unwrap();
	assert!(meta.starts_with("5 control units · read 20"), "the units `--identify` asks: {meta}");
	assert_eq!(
		rest,
		"\n\
		 ENGINE  01 · 7E0\n\
		 \x20 ENGINE SELF\n\
		 \x20 part PART-ENGINE · software 0001 · EV_TestEngine\n\
		 \x20 coding 01 02\n\
		 \n\
		 GEARBOX  02 · 7E1\n\
		 \x20 GEAR SELF\n\
		 \x20 part PART-GEAR · software 0001\n"
	);
	assert_eq!(
		info[0].stderr, "  (use \"vagcan units\" for the other units, \"vagcan faults\" for their fault codes)\n",
		"the hint at every level, right after the card"
	);

	// Through the bus, as every car command runs: -vv is the wire, and only -vv.
	let on_bus = || async { Ok(vag_cli_core::bus::Bus::start(car(), vag_cli_core::bus::Budget::default())) };
	let wire = levels!("info through the bus", car::info(on_bus).await);
	// Every line but the meta line, whose minute may have turned between the two runs.
	let timeless = |text: &str| {
		text
			.lines()
			.enumerate()
			.filter(|(at, _)| *at != 1)
			.map(|(_, l)| l.to_string())
			.collect::<Vec<_>>()
	};
	assert_eq!(timeless(&wire[0].stdout), timeless(&info[0].stdout), "the same data through the bus");
	assert!(!wire[1].stderr.contains("→"), "{:?}", wire[1].stderr);
	assert!(wire[2].stderr.contains("→  7E0  22 F1 90\n"), "the request: {:?}", wire[2].stderr);
	assert!(
		wire[2]
			.stderr
			.contains(&format!("←  7E8  62 F1 90 {}", vag_cli_core::verbosity::hex(VIN.as_bytes()))),
		"the answer: {:?}",
		wire[2].stderr
	);
	assert!(wire[2].stderr.contains("←  7E8  7F 22 31"), "a refusal: {:?}", wire[2].stderr);

	let units = levels!("units", car::units(open, false).await);
	assert_eq!(
		units[0].stdout,
		"2 control units on the gateway's list\n\nUNIT  ID   PART  NAME\n09    70E\n--    713\n"
	);

	// `--identify` records the units under this HOME, so the plain listing after it has
	// their part numbers.
	let identify = levels!("units --identify", car::units(open, true).await);
	assert_eq!(
		identify[0].stdout,
		format!(
			"5 control units on {VIN} · 4 answered · 2 on the gateway's list\n\
			 \n\
			 \x20  UNIT  ID   PART          COMPONENT     NAME\n\
			 ✗  09    70E                              (no answer)\n\
			 ✓  --    710  PART-GATEWAY  GATEWAY SELF\n\
			 ✓  --    713  PART-ESC      ESC SELF\n\
			 ✓  01    7E0  PART-ENGINE   ENGINE SELF\n\
			 ✓  02    7E1  PART-GEAR     GEAR SELF\n"
		)
	);
	assert!(
		home
			.path()
			.join(".vagcan/cars")
			.join(VIN)
			.join(vag_cli_core::datadir::UNITS_FILE)
			.is_file(),
		"recorded under the test's own home"
	);
	// The record does not cover 70E, which never answered, so it is not taken for this
	// car's: the listing is what it was.
	let units = levels!("units after --identify", car::units(open, false).await);
	assert_eq!(
		units[0].stdout,
		"2 control units on the gateway's list\n\nUNIT  ID   PART  NAME\n09    70E\n--    713\n"
	);

	let block = levels!("units --identify 01", car::identification(open, "01", false).await);
	assert!(block[0].stdout.starts_with("unit 01 · 7E0 · ENGINE SELF\n"), "{}", block[0].stdout);
	assert!(block[0].stdout.contains("\n\nIDENTIFICATION\n"), "{}", block[0].stdout);
	assert!(block[0].stdout.contains("F187  VW spare part number"), "{}", block[0].stdout);

	// `sensors` names the link it needs; the scripted car is every link.
	let open_on = |_: car::Reach| async { Ok(car()) };
	let sensors = levels!("sensors", car::sensors(open_on, "01").await);
	assert_eq!(
		sensors[0].stdout,
		"3 of 32 standard parameters answered by unit 01 · 3 converted\n\
		 \n\
		 DID          VALUE  NAME\n\
		 F405    40.00 °C    Coolant temperature\n\
		 F40C  1000.00 /min  Engine speed\n\
		 F40D     0.00 km/h  Vehicle speed\n"
	);
	let gearbox = levels!("sensors --unit 02", car::sensors(open_on, "02").await);
	assert!(
		gearbox[0].stdout.contains("F40D  9C 02 raw  not J1979: 2 bytes, standard 1"),
		"{}",
		gearbox[0].stdout
	);
	assert!(
		gearbox[1].stderr.contains("at another width"),
		"the reason in words is a -v note: {:?}",
		gearbox[1].stderr
	);

	// `faults` reads its keys from the VCDS store: none set up yet, so no code has a name.
	// The store holds the engine's freeze-frame layout, as `setup` stores a VCDS install's
	// (`RF.rod [FFMUX]`): record 0x70, four bytes. Invented rows, no install's.
	let project = home.path().join(".vagcan/data/TESTPROJECT");
	std::fs::create_dir_all(&project).unwrap();
	let cache = project.join("cache.sqlite");
	let store = home.path().join(".vagcan/vcds");
	std::fs::create_dir_all(&store).unwrap();
	let vcds_cache = store.join("cache.sqlite");
	vag_data_db::record_language(&vcds_cache, vag_data_db::VCDS, "/x/vcds-test", "eng").unwrap();
	let rows = [
		freeze_row(0, 16, 0, Some((0.25, 0.0)), "/min", "Engine speed"),
		freeze_row(16, 8, 0, Some((0.1, 0.0)), "V", "Voltage"),
		freeze_row(24, 8, 11, None, "", "Reserved"),
	];
	// The brake's rows are another variant's: a name VCDS would not open for this car.
	let brake = [freeze_row(0, 16, 0, Some((1.0, 0.0)), "", "Pressure")];
	vag_data_db::put_all_vcds_registry_rows(
		&vcds_cache,
		"/x/vcds-test",
		[("EV_TestEngine", &rows[..]), ("EV_TestBrake_XX99", &brake[..])],
	)
	.unwrap();
	let faults = levels!("faults", faults::run(open, None, false, false, false, false, None).await);
	assert_eq!(
		faults[0].stdout,
		"2 stored codes on 2 of 4 units · 1 failing now\n\
		 \n\
		 7E0  ENGINE SELF  PART-ENGINE\n\
		 \x20 ✗ 000129\n\
		 \x20            failing · 3 times · 123456 km · last 2026-07-27 00:00\n\
		 \x20            priority       6\n\
		 \x20            aging counter  16\n\
		 \x20            Engine speed   1000 /min\n\
		 \x20            Voltage        12.3 V\n\
		 \n\
		 713  ESC SELF  PART-ESC\n\
		 \x20 · 047120\n\
		 \x20            stored · 3 times · 123456 km · last 2026-07-27 00:00\n\
		 \x20            priority       6\n\
		 \x20            aging counter  16\n\
		 \x20            record 70      AA BB (no layout for this variant, not decoded)\n"
	);
	assert!(
		faults[1].stderr.contains("7E0  freeze frames described by VCDS's EV_TestEngine"),
		"where the layout came from is a -v note: {:?}",
		faults[1].stderr
	);
	assert!(
		faults[1]
			.stderr
			.contains("713  freeze frames: no layout for this variant (VCDS's EV_TestBrake_XX99 is another variant)"),
		"another variant's rows are named, not used: {:?}",
		faults[1].stderr
	);
	// One `19 06` per stored code and no other fault read: the freeze frame is in it.
	assert!(!faults[2].stderr.contains("19 04"), "{:?}", faults[2].stderr);

	// The same car with an ODIS project alone: the VCDS install dropped, the project's own
	// multiplexer rows written as `setup` writes them. Each unit's records come from ODIS and
	// nothing else — the ESC's too, which the VCDS rows above did not describe.
	let vcds = vag_data_db::sources(&vcds_cache)
		.unwrap()
		.into_iter()
		.find(|s| s.kind == vag_data_db::VCDS)
		.unwrap();
	vag_data_db::drop_source(&vcds_cache, vcds.id).unwrap();
	let engine_records = [odis_record(
		112,
		4,
		vec![
			odis_field("Sim engine speed", 0, 16, Some(0.25), Some("1/min")),
			odis_field("Sim supply voltage", 16, 8, Some(0.1), Some("V")),
		],
	)];
	let brake_records = [odis_record(112, 2, vec![odis_field("Brake pressure", 0, 16, Some(1.0), Some("bar"))])];
	vag_data_db::put_all_odis_freeze_frames(
		&cache,
		"/x/TESTPROJECT",
		[("EV_TestEngine_001", &engine_records[..]), ("EV_TestBrake_001", &brake_records[..])],
	)
	.unwrap();
	let odis_only = levels!("faults, ODIS alone", faults::run(open, None, false, false, false, false, None).await);
	assert!(
		odis_only[0].stdout.contains(
			"\x20            priority            6\n\
			 \x20            aging counter       16\n\
			 \x20            Sim engine speed    1000 1/min\n\
			 \x20            Sim supply voltage  12.3 V\n\
			 \n\
			 713  ESC SELF  PART-ESC\n\
			 \x20 · 047120\n\
			 \x20            stored · 3 times · 123456 km · last 2026-07-27 00:00\n\
			 \x20            priority        6\n\
			 \x20            aging counter   16\n\
			 \x20            Brake pressure  43707 bar\n"
		),
		"{}",
		odis_only[0].stdout
	);
	assert!(
		odis_only[1].stderr.contains("7E0  freeze frames described by ODIS's EV_TestEngine_001"),
		"{:?}",
		odis_only[1].stderr
	);

	// Both sources: the run's is ODIS. VCDS has rows for the ESC's own variant and ODIS
	// none, so the ESC's record 70 stays bytes — VCDS's rows never fill what ODIS lacks.
	vag_data_db::record_language(&vcds_cache, vag_data_db::VCDS, "/x/vcds-test", "eng").unwrap();
	let brake_vcds = [freeze_row(0, 16, 0, Some((1.0, 0.0)), "bar", "Pressure")];
	vag_data_db::put_all_vcds_registry_rows(&vcds_cache, "/x/vcds-test", [("EV_TestBrake", &brake_vcds[..])]).unwrap();
	vag_data_db::put_all_odis_freeze_frames(&cache, "/x/TESTPROJECT", [("EV_TestEngine_001", &engine_records[..])]).unwrap();
	let both = levels!("faults, both sources", faults::run(open, None, false, false, false, false, None).await);
	assert!(
		both[0].stdout.contains(
			"713  ESC SELF  PART-ESC\n\
			 \x20 · 047120\n\
			 \x20            stored · 3 times · 123456 km · last 2026-07-27 00:00\n\
			 \x20            priority       6\n\
			 \x20            aging counter  16\n\
			 \x20            record 70      AA BB (no layout for this variant, not decoded)\n"
		),
		"{}",
		both[0].stdout
	);
	assert!(
		both[0].stdout.contains("Sim engine speed    1000 1/min"),
		"the engine's from ODIS: {}",
		both[0].stdout
	);
	assert!(!both[0].stdout.contains("Pressure"), "VCDS's row read on an ODIS run: {}", both[0].stdout);
	// A unit whose fault list did not come back is said at level 0, and not counted as
	// read: 70E is on the gateway's list and never answers (review of PR #38).
	assert!(faults[0].stderr.contains("warning: 70E fault list not read: "), "{:?}", faults[0].stderr);
	// The caveat that used to head the listing is a -v note now.
	assert!(!faults[0].stderr.contains("not a diagnosis"), "{:?}", faults[0].stderr);
	assert!(faults[1].stderr.contains("not a diagnosis"), "{:?}", faults[1].stderr);

	let all = levels!("faults --all", faults::run(open, Some("01"), true, true, false, false, None).await);
	assert!(all[0].stdout.contains("000107"), "an unconfirmed code with --all: {}", all[0].stdout);
	assert!(
		all[0].stdout.starts_with("2 listed codes on 1 of 1 unit · 1 failing now\n"),
		"{}",
		all[0].stdout
	);
	assert!(
		all[0].stdout.contains("  ○ 000107\n             not confirmed\n"),
		"an unconfirmed code's state: {}",
		all[0].stdout
	);
	assert!(
		all[0].stdout.contains(
			"\n             Sim supply voltage  12.3 V\n\
				 \x20            record 01           06 03 02 10 01 E2 40 00 00 69 F6 00 03 70 0F A0\n\
				 \x20                                7B 00\n"
		),
		"--details is data, under its code, in the freeze frame's key column: {}",
		all[0].stdout
	);

	let supported = levels!(
		"faults --supported",
		faults::run(open, Some("01,713"), false, false, true, false, None).await
	);
	assert_eq!(
		supported[0].stdout,
		"3 codes supported on 2 of 2 units\n\
		 \n\
		 7E0  ENGINE SELF\n\
		 \x20 000129  297\n\
		 \x20 000107  263\n\
		 \n\
		 713  ESC SELF\n\
		 \x20 047120  291104\n"
	);

	// The device listing, as `vagcan devices` prints it.
	let devices = levels!("devices", {
		let found = vec![vag_uds_can::classify_usb("/dev/cu.usbmodem1".into(), 0x16d0, 0x117e, None)];
		vag_cli_core::out!("{}", vag_cli_core::device::render_list(&found));
		for (level, note) in vag_cli_core::device::list_notes(&found) {
			vag_cli_core::verbosity::say(level, &note);
		}
		Ok(())
	});
	assert!(
		devices[0].stdout == "1 serial device · 1 CAN adapter\n\n   PATH               DEVICE\n✓  /dev/cu.usbmodem1  CANable 2.0 (slcan)\n",
		"{}",
		devices[0].stdout
	);
	assert!(devices[0].stderr.is_empty(), "{:?}", devices[0].stderr);

	// And nothing landed anywhere but under this test's own home.
	assert!(home.path().join(".vagcan").is_dir());
}
