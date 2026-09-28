//! `watch` against a scripted car: the units it identifies are recorded, every run asks
//! every unit again, and a unit's record is only ever improved by a run — never cut down
//! by one bad read.
//!
//! A car that carries whole PDUs, as the bus tests' scripted car does, under the real
//! [`Bus`] and the real `watch::run` in its plain, no-terminal view. Nothing here is a fact
//! about any car: four units with made-up part numbers — the gateway among them, which
//! does not list itself and is asked anyway — and a fifth the gateway lists that never
//! answers.
//!
//! **Its own test binary, on purpose.** The record goes under `~/.vagcan`, which no test
//! may write into, so this process points `HOME` at a temporary directory before anything
//! reads it — a process-wide change that is only safe while this is the one thread, which a
//! binary with one test on a single-threaded runtime guarantees and a shared unit-test binary
//! could not.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use vag_cli_core::bus::{Budget, Bus};
use vag_cli_core::plan::UnitIdentity;
use vag_cli_diag::watch::{self, Options, View};
use vag_uds_transport::{AsyncIsoTpTransport, CanId, TransportError};

const GATEWAY: u16 = 0x710;
const ENGINE: u16 = 0x7E0;
const GEARBOX: u16 = 0x7E1;
const ESC: u16 = 0x713;
/// Listed by the gateway, never answers.
const SILENT: u16 = 0x70E;
const VIN: &str = "TESTVIN0000000001";

/// What the car answers, and what it was asked.
#[derive(Default)]
struct Script {
	records: BTreeMap<(u16, u16), Vec<u8>>,
	/// `(unit, identifier)` pairs the unit says nothing to this run: a deadline
	/// missed, as a unit still starting up misses one.
	late: BTreeSet<(u16, u16)>,
	/// `(unit, identifier)` pairs the unit refuses outright (`7F 22 31`, no such
	/// identifier): what a unit that does not implement `F19E` answers, every run.
	refused: BTreeSet<(u16, u16)>,
	/// Every request: the unit's request id and the identifier, for `22` reads.
	asked: Vec<(u16, u16)>,
}

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
		let mut script = self.car.script.lock().unwrap();
		if pdu[0] != 0x22 {
			return Ok(vec![0x7F, pdu[0], 0x11]);
		}
		let mut answer = vec![0x62];
		for did in pdu[1..].chunks(2).map(|b| u16::from_be_bytes([b[0], b[1]])) {
			script.asked.push((self.request, did));
			if script.late.contains(&(self.request, did)) {
				return Err(TransportError::Timeout);
			}
			if script.refused.contains(&(self.request, did)) {
				return Ok(vec![0x7F, 0x22, 0x31]);
			}
			if let Some(record) = script.records.get(&(self.request, did)) {
				answer.extend_from_slice(&did.to_be_bytes());
				answer.extend_from_slice(record);
			}
		}
		// A unit that is not there says nothing at all; one that is refuses what it lacks.
		if !script.records.keys().any(|(request, _)| *request == self.request) {
			return Err(TransportError::Timeout);
		}
		match answer.len() {
			1 => Ok(vec![0x7F, 0x22, 0x31]),
			_ => Ok(answer),
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

/// The car, with `late` the identifiers that go unanswered this run.
fn scripted_car(late: &[(u16, u16)]) -> (Car, Arc<Mutex<Script>>) {
	car_with("PART-ESC", late, &[])
}

/// The car with the ESC's part number as `esc_part` — another number is a swapped
/// unit — and `refused` the identifiers a unit does not implement.
fn car_with(esc_part: &str, late: &[(u16, u16)], refused: &[(u16, u16)]) -> (Car, Arc<Mutex<Script>>) {
	let text = |s: &str| s.as_bytes().to_vec();
	let mut records: BTreeMap<(u16, u16), Vec<u8>> = BTreeMap::new();
	records.insert((GATEWAY, vag_uds_client::gateway::INSTALLATION_LIST), installation_list(&[ESC, SILENT]));
	for (unit, part, component, odx, version) in [
		(ENGINE, "PART-ENGINE", "engine", "EV_Engine", "001001"),
		(GEARBOX, "PART-GEAR", "gearbox", "EV_Gear", "001002"),
		(GATEWAY, "PART-GATEWAY", "gateway", "EV_Gatew", "001004"),
		(ESC, esc_part, "esc", "EV_Brake", "001003"),
	] {
		records.insert((unit, 0xF187), text(part));
		records.insert((unit, 0xF197), text(component));
		records.insert((unit, 0xF19E), text(odx));
		records.insert((unit, 0xF1A2), text(version));
	}
	records.insert((ENGINE, 0xF190), text(VIN));
	let script = Arc::new(Mutex::new(Script {
		records,
		late: late.iter().copied().collect(),
		refused: refused.iter().copied().collect(),
		..Script::default()
	}));
	(Car { script: script.clone() }, script)
}

/// How many times `did` was asked of `unit`.
fn asked(script: &Mutex<Script>, unit: u16, did: u16) -> usize {
	script.lock().unwrap().asked.iter().filter(|(u, d)| *u == unit && *d == did).count()
}

/// One `watch` run in the plain view, over for its own deadline.
async fn watch_once(car: Car) {
	let open = async || Ok(Bus::start(car, Budget::default()));
	watch::run(
		open,
		Options {
			preselect: &[],
			hz: None,
			out: None,
			catalogs: "/definitely/not/here",
			view: View::Plain(Some(Duration::from_millis(300))),
		},
	)
	.await
	.expect("a scripted car is watched to its deadline");
}

/// The walk `measure` and `units --identify` make, then the record — every unit asked.
async fn identify_all_and_record(car: Car) {
	let mut progress = vag_cli_core::progress::Line::new();
	let (_, found) = vag_cli_core::units::identify(car, &[ENGINE], &mut progress).await;
	progress.finish();
	vag_cli_core::units::record(VIN, &found).unwrap();
}

fn in_record(record: &std::path::Path, unit: u16) -> UnitIdentity {
	vag_cli_core::units::read_record(record)
		.unwrap()
		.expect("the record was written")
		.into_iter()
		.find(|u| u.request == unit)
		.unwrap_or_else(|| panic!("{unit:03X} is on record"))
}

fn identity(request: u16, part: &str, component: &str, odx: &str, version: &str) -> UnitIdentity {
	UnitIdentity {
		request,
		part_number: Some(part.into()),
		odx_name: Some(odx.into()),
		odx_version: Some(version.into()),
		component: Some(component.into()),
	}
}

#[tokio::test]
async fn watch_records_the_units_it_identified_and_every_run_asks_them_all_again() {
	let home = tempfile::tempdir().unwrap();
	// Before any thread but this one exists: see the module docs.
	unsafe {
		std::env::set_var("HOME", home.path());
		std::env::remove_var(vag_cli_core::project::PROJECT_ENV);
	}
	let record = home.path().join(".vagcan").join("cars").join(VIN).join(vag_cli_core::datadir::UNITS_FILE);

	// First run: every unit is asked — the gateway too, which is in no installation
	// list and has channels like any other unit — and the ESC misses its deadline on
	// the two identifiers that name its ODX file, as a unit still starting up does.
	let (car, script) = scripted_car(&[(ESC, 0xF19E), (ESC, 0xF1A2)]);
	watch_once(car).await;
	for unit in [ENGINE, GEARBOX, GATEWAY, ESC, SILENT] {
		assert_eq!(asked(&script, unit, 0xF187), 1, "{unit:03X} was probed once");
	}
	let recorded = vag_cli_core::units::read_record(&record).unwrap().expect("the record was written");
	let parts: Vec<(u16, Option<&str>, Option<&str>)> = recorded
		.iter()
		.map(|u| (u.request, u.part_number.as_deref(), u.odx_name.as_deref()))
		.collect();
	assert_eq!(
		parts,
		vec![
			(GATEWAY, Some("PART-GATEWAY"), Some("EV_Gatew")),
			(ESC, Some("PART-ESC"), None),
			(ENGINE, Some("PART-ENGINE"), Some("EV_Engine")),
			(GEARBOX, Some("PART-GEAR"), Some("EV_Gear")),
		],
		"the units that answered, and only those, with what each answered"
	);
	let text = std::fs::read_to_string(&record).unwrap();
	assert!(text.contains("\"request\": \"7E1\""), "{text}");

	// Second run, the ESC answering everything: every unit is asked again — a record
	// is for the commands that cannot ask the car, never a reason not to — and the ESC's
	// ODX name and version are on record now. Found in review (2026-09-28): the first
	// run's record was taken as known, the ESC was never asked again, and its missing
	// ODX name was permanent.
	let (car, script) = scripted_car(&[]);
	watch_once(car).await;
	for unit in [ENGINE, GEARBOX, GATEWAY, ESC, SILENT] {
		assert_eq!(asked(&script, unit, 0xF187), 1, "{unit:03X} is asked on every run");
	}
	assert_eq!(asked(&script, ESC, 0xF19E), 1);
	assert_eq!(
		asked(&script, GATEWAY, vag_uds_client::gateway::INSTALLATION_LIST),
		1,
		"the gateway is asked for its list either way"
	);
	assert_eq!(in_record(&record, ESC), identity(ESC, "PART-ESC", "esc", "EV_Brake", "001003"));
	let complete = vag_cli_core::units::read_record(&record).unwrap().unwrap();

	// A `measure`-like walk with a slow engine: it misses every probe this run, so it
	// is not among the units found — and its entry stays as it was. Then `watch` again,
	// the engine answering: the record is what it was, not less.
	let (car, _) = scripted_car(&[(ENGINE, 0xF187), (ENGINE, 0xF197), (ENGINE, 0xF19E), (ENGINE, 0xF1A2)]);
	identify_all_and_record(car).await;
	assert_eq!(
		in_record(&record, ENGINE),
		identity(ENGINE, "PART-ENGINE", "engine", "EV_Engine", "001001")
	);
	// And an engine that answers its part number and nothing else: the rest stays too.
	let (car, _) = scripted_car(&[(ENGINE, 0xF197), (ENGINE, 0xF19E), (ENGINE, 0xF1A2)]);
	identify_all_and_record(car).await;
	assert_eq!(
		in_record(&record, ENGINE),
		identity(ENGINE, "PART-ENGINE", "engine", "EV_Engine", "001001")
	);
	let (car, script) = scripted_car(&[]);
	watch_once(car).await;
	assert_eq!(asked(&script, ENGINE, 0xF187), 1);
	assert_eq!(
		vag_cli_core::units::read_record(&record).unwrap().unwrap(),
		complete,
		"a run only ever adds to the record"
	);

	// The ESC is swapped for one with another part number, which does not implement
	// `F19E` or `F1A2` and refuses them outright, every run. Its entry is replaced
	// whole: the old unit's ODX file must not be bound to the new unit's part number,
	// or the plan carries the old variant's scalings onto a unit that never said it
	// was that variant (found in review, 2026-09-28). Three runs, the same answer.
	for run in 1..=3 {
		let (car, _) = car_with("PART-ESC-OTHER", &[], &[(ESC, 0xF19E), (ESC, 0xF1A2)]);
		watch_once(car).await;
		assert_eq!(
			in_record(&record, ESC),
			UnitIdentity {
				request: ESC,
				part_number: Some("PART-ESC-OTHER".into()),
				odx_name: None,
				odx_version: None,
				component: Some("esc".into()),
			},
			"run {run} after the swap"
		);
	}
	// And the swapped unit answering everything is recorded whole, like any other.
	let (car, _) = car_with("PART-ESC-OTHER", &[], &[]);
	watch_once(car).await;
	assert_eq!(in_record(&record, ESC), identity(ESC, "PART-ESC-OTHER", "esc", "EV_Brake", "001003"));
	// And nothing landed anywhere but under this test's own home.
	assert!(home.path().join(".vagcan").join("cars").join(VIN).is_dir());
}
