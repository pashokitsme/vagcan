//! `sensors` against scripted cars: where it is a sweep, and the guard it carries there.
//!
//! Off the emissions block (`7E0`–`7E7`) `sensors --unit <unit>` asks one unit the standard
//! OBD-II identifiers that nothing binds it to answer — a sweep, guarded as `units
//! --identify <unit>` is (`CLAUDE.md`, Safety; `car::sensors`). Each test proves one part of
//! that guard; the refusal through the dash board and its `--slcan` exception are the
//! device's, proven in `vag-cli`'s `main.rs` with the words this command refuses in. The
//! last test holds `units --identify <unit>` to the same line between a unit going quiet and
//! the link refusing (`anomaly::Answer::Withheld`).
//!
//! The cars are made up: two units and the bytes they answer, none of them any car's. A
//! whole-PDU link, as `levels.rs`'s, that writes down every request it is sent. Nothing here
//! sets an environment variable: a test build's `~/.vagcan` is a temporary directory of its
//! own (`vag_cli_core::datadir`), so the tests may share one process.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use vag_cli_diag::anomaly::QUIET_RUN;
use vag_cli_diag::car::{self, Reach};
use vag_data_labels::obd::{PIDS, did_for_pid};
use vag_uds_transport::{AsyncIsoTpTransport, CanId, TransportError};

const ENGINE: u16 = 0x7E0;
/// A unit off the emissions block.
const ESC: u16 = 0x713;
/// Road speed, the identifier the moving-car check reads off the engine (SAE J1979 PID 0D).
const SPEED: u16 = 0xF40D;

#[derive(Default)]
struct Script {
	/// What each unit answers, per identifier; any other identifier it refuses.
	records: BTreeMap<(u16, u16), Vec<u8>>,
	/// A unit that answers its first so many requests and then nothing at all.
	quiet_after: Option<(u16, usize)>,
	/// A link that carries a unit's first so many requests and refuses the rest itself, as
	/// the dash board over BLE refuses past `MAX_DISTINCT_IDENTIFIERS` of one unit — which
	/// reaches the client as `TransportError::Protocol` (`vag_cli_core::bus`).
	refused_after: Option<(u16, usize)>,
	/// Every request, in order: the unit's request id and the PDU.
	asked: Vec<(u16, Vec<u8>)>,
}

#[derive(Clone, Default)]
struct Car {
	script: Arc<Mutex<Script>>,
}

impl Car {
	fn answers(self, unit: u16, did: u16, bytes: &[u8]) -> Car {
		self.script.lock().unwrap().records.insert((unit, did), bytes.to_vec());
		self
	}

	fn quiet_after(self, unit: u16, requests: usize) -> Car {
		self.script.lock().unwrap().quiet_after = Some((unit, requests));
		self
	}

	fn refused_by_the_link_after(self, unit: u16, requests: usize) -> Car {
		self.script.lock().unwrap().refused_after = Some((unit, requests));
		self
	}

	/// A car whose engine reports this road speed, and whose ESC answers the first two
	/// standard identifiers the list asks.
	fn at(kmh: u8) -> Car {
		Car::default().answers(ENGINE, SPEED, &[kmh]).with_esc()
	}

	fn with_esc(self) -> Car {
		self
			.answers(ESC, did_for_pid(PIDS[0].pid), &[0x57])
			.answers(ESC, did_for_pid(PIDS[1].pid), &[0x10])
	}

	fn asked(&self) -> Vec<(u16, Vec<u8>)> {
		self.script.lock().unwrap().asked.clone()
	}

	/// The identifiers asked of one unit, in order.
	fn asked_of(&self, unit: u16) -> Vec<u16> {
		self
			.asked()
			.into_iter()
			.filter(|(to, _)| *to == unit)
			.map(|(_, pdu)| u16::from_be_bytes([pdu[1], pdu[2]]))
			.collect()
	}
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
		self.car.script.lock().unwrap().asked.push((self.request, pdu.to_vec()));
		self.pdu = Some(pdu.to_vec());
		Ok(())
	}

	async fn recv(&mut self, _timeout: Duration) -> Result<Vec<u8>, TransportError> {
		let Some(pdu) = self.pdu.take() else {
			return Err(TransportError::Timeout);
		};
		let script = self.car.script.lock().unwrap();
		if let Some((unit, after)) = script.refused_after {
			let so_far = script.asked.iter().filter(|(to, _)| *to == unit).count();
			if unit == self.request && so_far > after {
				return Err(TransportError::Protocol(
					"the dash board refused it: too many different identifiers asked of one unit".into(),
				));
			}
		}
		// A unit that is not there says nothing at all; one gone quiet says nothing more.
		if !script.records.keys().any(|(unit, _)| *unit == self.request) {
			return Err(TransportError::Timeout);
		}
		if let Some((unit, after)) = script.quiet_after {
			let so_far = script.asked.iter().filter(|(to, _)| *to == unit).count();
			if unit == self.request && so_far > after {
				return Err(TransportError::Timeout);
			}
		}
		match pdu.as_slice() {
			[0x22, hi, lo] => {
				let did = u16::from_be_bytes([*hi, *lo]);
				Ok(match script.records.get(&(self.request, did)) {
					Some(record) => [&[0x62, *hi, *lo][..], record].concat(),
					None => vec![0x7F, 0x22, 0x31],
				})
			}
			[sid, ..] => Ok(vec![0x7F, *sid, 0x11]),
			[] => Err(TransportError::Timeout),
		}
	}
}

/// Run `sensors` on `unit_text` against `car`, and say which link it asked to be opened on.
async fn sensors(car: &Car, unit_text: &str) -> (anyhow::Result<()>, Vec<Reach>) {
	let reaches = Arc::new(Mutex::new(Vec::new()));
	let open = {
		let reaches = reaches.clone();
		let car = car.clone();
		async move |reach: Reach| {
			reaches.lock().unwrap().push(reach);
			Ok(car)
		}
	};
	let result = car::sensors(open, unit_text).await;
	let reaches = reaches.lock().unwrap().clone();
	(result, reaches)
}

fn speed_read() -> (u16, Vec<u8>) {
	(ENGINE, vec![0x22, 0xF4, 0x0D])
}

#[tokio::test]
async fn off_the_emissions_block_it_is_refused_on_a_moving_car() {
	let car = Car::at(30);
	let (result, reaches) = sensors(&car, "713").await;
	let refused = vag_cli_core::report::render(&result.expect_err("refused at 30 km/h"), 0, vag_cli_core::report::Colour::Never);
	assert!(refused.starts_with("error: the car is moving at 30 km/h\n"), "{refused}");
	assert!(refused.contains("help: park the car"), "{refused}");
	assert!(!refused.contains("session"), "sensors changes no session: {refused}");
	assert_eq!(reaches, [Reach::CableOnly]);
	assert_eq!(car.asked(), [speed_read()], "the speed and nothing else: the unit is never asked");
}

#[tokio::test]
async fn off_the_emissions_block_no_speed_answer_counts_as_moving() {
	// No engine on the bus at all, and an engine that refuses the speed: neither is 0 km/h.
	let absent = Car::default().with_esc();
	let refusing = Car::default().answers(ENGINE, 0xF405, &[0x50]).with_esc();
	for car in [absent, refusing] {
		let (result, reaches) = sensors(&car, "713").await;
		let refused = format!("{:#}", result.expect_err("refused with no speed"));
		assert!(refused.contains("cannot tell whether the car is moving"), "{refused}");
		assert_eq!(reaches, [Reach::CableOnly]);
		assert!(car.asked_of(ESC).is_empty(), "the unit is never asked: {:?}", car.asked());
	}
}

#[tokio::test]
async fn off_the_emissions_block_a_parked_car_is_read_through_a_cable_from_the_fixed_list_only() {
	let car = Car::at(0);
	let (result, reaches) = sensors(&car, "713").await;
	result.expect("read at 0 km/h");
	// A cable only: the dispatcher refuses the dash board for this (`main.rs`).
	assert_eq!(reaches, [Reach::CableOnly]);
	// The speed first, then the list, each identifier once and in order: never a range.
	let asked = car.asked();
	assert_eq!(asked[0], speed_read());
	let list: Vec<u16> = PIDS.iter().map(|p| did_for_pid(p.pid)).collect();
	assert_eq!(car.asked_of(ESC), list);
	assert_eq!(asked.len(), 1 + PIDS.len());
	// The allowlist: every request this read made is a `22` of one identifier.
	for (unit, pdu) in &asked {
		assert!(pdu.len() == 3 && pdu[0] == 0x22, "{unit:03X}: {pdu:02X?}");
	}
}

#[tokio::test]
async fn a_unit_that_falls_silent_under_the_read_stops_it() {
	// The ESC answers its first two identifiers, then nothing: the read stops after the
	// run of silences the monitor counts as a unit gone quiet, not at the end of the list.
	let car = Car::at(0).quiet_after(ESC, 2);
	let (result, _) = sensors(&car, "713").await;
	let stopped = format!("{:#}", result.expect_err("stopped"));
	assert!(stopped.contains("the read was stopped"), "{stopped}");
	assert_eq!(car.asked_of(ESC).len(), 2 + QUIET_RUN, "{:?}", car.asked_of(ESC));
	assert!(2 + QUIET_RUN < PIDS.len(), "the list is longer than the stop, or this proves nothing");

	// The engine too, where nothing else guards the read.
	let car = Car::default().answers(ENGINE, did_for_pid(PIDS[0].pid), &[0x20]).quiet_after(ENGINE, 1);
	let (result, _) = sensors(&car, "01").await;
	let stopped = format!("{:#}", result.expect_err("stopped"));
	assert!(stopped.contains("the read was stopped"), "{stopped}");
	assert_eq!(car.asked_of(ENGINE).len(), 1 + QUIET_RUN);
}

#[tokio::test]
async fn inside_the_emissions_block_it_is_not_a_sweep() {
	// Plain `sensors` is the engine: J1979 legislates these identifiers on it, and every
	// generic scan tool reads them off a moving car. No speed check of its own, any link.
	for unit_text in ["01", "7E0", "7E1"] {
		let car = Car::at(30).answers(0x7E1, SPEED, &[0x9C, 0x02]);
		let (result, reaches) = sensors(&car, unit_text).await;
		result.unwrap_or_else(|e| panic!("{unit_text}: {e:#}"));
		assert_eq!(reaches, [Reach::AnyLink], "{unit_text}");
		let list: Vec<u16> = PIDS.iter().map(|p| did_for_pid(p.pid)).collect();
		let request = if unit_text == "7E1" { 0x7E1 } else { ENGINE };
		assert_eq!(car.asked_of(request), list, "{unit_text}: the list, and no speed read before it");
		assert_eq!(car.asked().len(), PIDS.len(), "{unit_text}: no other unit asked");
	}
}

#[tokio::test]
async fn a_unit_that_does_not_parse_opens_nothing() {
	let car = Car::at(0);
	let (result, reaches) = sensors(&car, "ZZZ").await;
	assert!(format!("{:#}", result.expect_err("refused")).starts_with("--unit: "));
	assert!(reaches.is_empty());
	assert!(car.asked().is_empty());
}

#[tokio::test]
async fn the_dash_board_refusing_past_its_cap_is_not_the_unit_going_quiet() {
	// Over BLE the board remembers what an earlier command asked of a unit: `info` leaves
	// seven of the engine's identifiers, and a `sensors` inside that memory takes the
	// distinct set past its cap. From there the board refuses each one itself. That is the
	// link's no, not the engine falling silent: the read finishes, with what it got.
	let car = Car::at(0)
		.answers(ENGINE, did_for_pid(PIDS[0].pid), &[0x20])
		.answers(ENGINE, did_for_pid(PIDS[1].pid), &[0x50])
		.refused_by_the_link_after(ENGINE, 2);
	let (result, reaches) = sensors(&car, "01").await;
	result.unwrap_or_else(|e| panic!("a board refusal ended the read: {e:#}"));
	assert_eq!(reaches, [Reach::AnyLink]);
	assert_eq!(car.asked_of(ENGINE).len(), PIDS.len(), "every identifier asked, none halted");
}

#[tokio::test]
async fn units_identify_still_stops_on_real_silence_and_not_on_the_links_refusal() {
	// The same classification is `units --identify <unit>`'s: a unit that answers, then
	// says nothing, ends the read after the run of silences ...
	let car = Car::at(0).answers(ESC, 0xF100, &[0x01]).answers(ESC, 0xF101, &[0x02]).quiet_after(ESC, 2);
	let open = {
		let car = car.clone();
		async move || Ok(car)
	};
	let stopped = format!("{:#}", car::identification(open, "713", false).await.expect_err("stopped"));
	assert!(stopped.contains("the read was stopped"), "{stopped}");
	assert_eq!(car.asked_of(ESC).len(), 2 + QUIET_RUN);

	// ... and a link that refuses the rest itself does not.
	let car = Car::at(0)
		.answers(ESC, 0xF100, &[0x01])
		.answers(ESC, 0xF101, &[0x02])
		.refused_by_the_link_after(ESC, 2);
	let open = {
		let car = car.clone();
		async move || Ok(car)
	};
	car::identification(open, "713", false)
		.await
		.unwrap_or_else(|e| panic!("a link refusal ended the read: {e:#}"));
	assert!(car.asked_of(ESC).len() >= 0x100, "the whole block was asked: {}", car.asked_of(ESC).len());
}
