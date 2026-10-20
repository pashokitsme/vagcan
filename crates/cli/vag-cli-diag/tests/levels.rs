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
	// Priority, count 3, cycle counter, 123456 km, two bytes, 2026-07-27 00:00:03.
	let record = vec![0x06, 0x03, 0x00, 0x10, 0x01, 0xE2, 0x40, 0x00, 0x00, 0x69, 0xF6, 0x00, 0x03];
	script.extended.insert((ENGINE, [0x00, 0x01, 0x29]), record.clone());
	script.extended.insert((ESC, [0x04, 0x71, 0x20]), record);
	Car {
		script: Arc::new(Mutex::new(script)),
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
	assert_eq!(
		info[0].stdout,
		format!(
			"vin  {VIN}\n\
			 \n\
			 \x20          engine 01    gearbox 02\n\
			 part       PART-ENGINE  PART-GEAR\n\
			 hardware   —            —\n\
			 software   0001         0001\n\
			 component  ENGINE SELF  GEAR SELF\n\
			 serial     —            —\n\
			 coding     01 02        —\n"
		)
	);
	assert!(info[0].stderr.is_empty(), "{:?}", info[0].stderr);

	// Through the bus, as every car command runs: -vv is the wire, and only -vv.
	let on_bus = || async { Ok(vag_cli_core::bus::Bus::start(car(), vag_cli_core::bus::Budget::default())) };
	let wire = levels!("info through the bus", car::info(on_bus).await);
	assert_eq!(wire[0].stdout, info[0].stdout, "the same data through the bus");
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
	assert_eq!(units[0].stdout, "unit  id   part  name\n09    70E\n--    713\n\n2 units\n");

	// `--identify` records the units under this HOME, so the plain listing after it has
	// their part numbers.
	let identify = levels!("units --identify", car::units(open, true).await);
	assert_eq!(
		identify[0].stdout,
		"unit  id   part          component     name\n\
		 09    70E                              (no answer)\n\
		 --    710  PART-GATEWAY  GATEWAY SELF\n\
		 --    713  PART-ESC      ESC SELF\n\
		 01    7E0  PART-ENGINE   ENGINE SELF\n\
		 02    7E1  PART-GEAR     GEAR SELF\n\
		 \n\
		 5 asked · 4 answered · 2 on the gateway's list\n"
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
	assert_eq!(units[0].stdout, "unit  id   part  name\n09    70E\n--    713\n\n2 units\n");

	let block = levels!("units --identify 01", car::identification(open, "01", false).await);
	assert!(block[0].stdout.starts_with("unit  01\n\ndid   name"), "{}", block[0].stdout);
	assert!(block[0].stdout.contains("F187  VW spare part number"), "{}", block[0].stdout);

	// `sensors` names the link it needs; the scripted car is every link.
	let open_on = |_: car::Reach| async { Ok(car()) };
	let sensors = levels!("sensors", car::sensors(open_on, "01").await);
	assert_eq!(
		sensors[0].stdout,
		"did          value  name\n\
		 F405    40.00 °C    Coolant temperature\n\
		 F40C  1000.00 /min  Engine speed\n\
		 F40D     0.00 km/h  Vehicle speed\n\
		 \n\
		 3 of 3 converted · unit 01\n"
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

	// `faults` keeps its keys per project: one, set up from nothing yet, so no code has a name.
	std::fs::create_dir_all(home.path().join(".vagcan/data/TESTPROJECT")).unwrap();
	let faults = levels!("faults", faults::run(open, None, false, false, false, false, None).await);
	assert_eq!(
		faults[0].stdout,
		"unit  id   name         code    dtc  status         km  count  last seen            src  text\n\
		 01    7E0  ENGINE SELF  000129       failing    123456      3  2026-07-27 00:00:03\n\
		 --    713  ESC SELF     047120       confirmed  123456      3  2026-07-27 00:00:03\n\
		 \n\
		 2 stored · 1 failing now · 4 units read\n"
	);
	// A unit whose fault list did not come back is said at level 0, and not counted as
	// read: 70E is on the gateway's list and never answers (review of PR #38).
	assert!(faults[0].stderr.contains("70E  fault list not read: "), "{:?}", faults[0].stderr);
	// The caveat that used to head the listing is a -v note now.
	assert!(!faults[0].stderr.contains("not a diagnosis"), "{:?}", faults[0].stderr);
	assert!(faults[1].stderr.contains("not a diagnosis"), "{:?}", faults[1].stderr);

	let all = levels!("faults --all", faults::run(open, Some("01"), true, true, false, false, None).await);
	assert!(all[0].stdout.contains("000107"), "an unconfirmed code with --all: {}", all[0].stdout);
	assert!(all[0].stdout.ends_with("2 listed · 1 failing now · 1 unit read\n"), "{}", all[0].stdout);
	assert!(
		all[0].stdout.contains("id   code    record  bytes"),
		"--details is data: {}",
		all[0].stdout
	);

	let supported = levels!(
		"faults --supported",
		faults::run(open, Some("01,713"), false, false, true, false, None).await
	);
	assert_eq!(
		supported[0].stdout,
		"unit  id   name         code    number\n\
		 01    7E0  ENGINE SELF  000129     297\n\
		 01    7E0  ENGINE SELF  000107     263\n\
		 --    713  ESC SELF     047120  291104\n\
		 \n\
		 3 codes supported · 2 units read\n"
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
		devices[0].stdout == "path               adapter  device\n/dev/cu.usbmodem1  yes      CANable 2.0 (slcan)\n",
		"{}",
		devices[0].stdout
	);
	assert!(devices[0].stderr.is_empty(), "{:?}", devices[0].stderr);

	// And nothing landed anywhere but under this test's own home.
	assert!(home.path().join(".vagcan").is_dir());
}
