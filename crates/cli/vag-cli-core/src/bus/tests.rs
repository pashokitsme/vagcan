//! The shell against a scripted car: no CAN, no adapter.
//!
//! Identifiers and bytes are synthetic; nothing here is a fact about any car. Real time,
//! on a multi-threaded runtime: the task runs on a blocking-pool thread, where a paused
//! clock would advance under it while it works.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use vag_uds_client::AsyncUdsClient;
use vag_uds_transport::{AsyncIsoTpTransport, CanId, TransportError};

use super::*;

const ENGINE: Unit = Unit {
	request: 0x7E0,
	response: 0x7E8,
};
const ABSENT: Unit = Unit {
	request: 0x7E5,
	response: 0x7ED,
};

/// What the scripted car was asked, and how it is to answer.
#[derive(Default)]
struct Script {
	records: BTreeMap<(u16, u16), Vec<u8>>,
	/// `7F 22 78` this many times before the real answer to the next request.
	pending: usize,
	/// Every request: the unit's request id, the PDU, and the deadline it was given.
	asked: Vec<(u16, Vec<u8>, Duration)>,
}

/// A link that carries whole PDUs, with its script shared so a test can read it.
struct Car {
	script: Arc<Mutex<Script>>,
	dropped: Arc<AtomicBool>,
}

impl Drop for Car {
	fn drop(&mut self) {
		self.dropped.store(true, Ordering::SeqCst);
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
		self.pdu = Some(pdu.to_vec());
		Ok(())
	}

	async fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
		let mut script = self.car.script.lock().unwrap();
		let Some(pdu) = self.pdu.clone() else {
			return Err(TransportError::Timeout);
		};
		if script.pending > 0 {
			script.pending -= 1;
			return Ok(vec![0x7F, pdu[0], 0x78]);
		}
		script.asked.push((self.request, pdu.clone(), timeout));
		self.pdu = None;
		if !script.records.keys().any(|(request, _)| *request == self.request) {
			return Err(TransportError::Timeout);
		}
		if pdu[0] != 0x22 {
			return Ok(vec![0x7F, pdu[0], 0x11]);
		}
		let mut answer = vec![0x62];
		for did in pdu[1..].chunks(2).map(|b| u16::from_be_bytes([b[0], b[1]])) {
			if let Some(record) = script.records.get(&(self.request, did)) {
				answer.extend_from_slice(&did.to_be_bytes());
				answer.extend_from_slice(record);
			}
		}
		match answer.len() {
			1 => Ok(vec![0x7F, 0x22, 0x31]),
			_ => Ok(answer),
		}
	}
}

fn car(records: &[(u16, u16, &[u8])]) -> (Car, Arc<Mutex<Script>>, Arc<AtomicBool>) {
	let script = Arc::new(Mutex::new(Script {
		records: records.iter().map(|(u, d, r)| ((*u, *d), r.to_vec())).collect(),
		..Script::default()
	}));
	let dropped = Arc::new(AtomicBool::new(false));
	(
		Car {
			script: script.clone(),
			dropped: dropped.clone(),
		},
		script,
		dropped,
	)
}

/// How many requests so far asked for `did`, alone or among others.
fn asked_for(script: &Mutex<Script>, did: u16) -> usize {
	let wanted = did.to_be_bytes();
	script
		.lock()
		.unwrap()
		.asked
		.iter()
		.filter(|(_, pdu, _)| pdu[0] == 0x22 && pdu[1..].chunks(2).any(|b| b == wanted))
		.count()
}

async fn sample(sub: &mut Subscription) -> Sample {
	tokio::time::timeout(Duration::from_secs(2), sub.next())
		.await
		.expect("a sample within two seconds")
		.expect("the bus is running")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_subscription_stops_its_requests() {
	let (link, script, _) = car(&[(0x7E0, 0x1000, &[1, 2])]);
	let bus = Bus::start(link, Budget::default());
	let mut sub = bus.subscribe(Class::Foreground, ENGINE, 0x1000, Duration::from_millis(20), None);
	for _ in 0..3 {
		let got = sample(&mut sub).await;
		assert_eq!(got.value, Ok(vec![1, 2]));
		assert_eq!((got.unit, got.did), (ENGINE, 0x1000));
	}
	drop(sub);
	// One request may already have been out when the unsubscribe arrived.
	tokio::time::sleep(Duration::from_millis(60)).await;
	let settled = asked_for(&script, 0x1000);
	tokio::time::sleep(Duration::from_millis(200)).await;
	assert_eq!(asked_for(&script, 0x1000), settled, "nobody wants it, so nobody asks for it");
}

#[tokio::test(flavor = "multi_thread")]
async fn two_subscribers_share_one_read() {
	let (link, script, _) = car(&[(0x7E0, 0x1000, &[7])]);
	let bus = Bus::start(link, Budget::default());
	let mut a = bus.subscribe(Class::Foreground, ENGINE, 0x1000, Duration::from_millis(30), None);
	let mut b = bus.subscribe(Class::Background, ENGINE, 0x1000, Duration::from_millis(30), None);
	let mut seen_a = Vec::new();
	let mut seen_b = Vec::new();
	for _ in 0..5 {
		seen_a.push(sample(&mut a).await.at.ms);
		seen_b.push(sample(&mut b).await.at.ms);
	}
	assert_eq!(seen_a, seen_b, "every reading reached both, stamped alike");
	let asked = asked_for(&script, 0x1000);
	assert!(
		asked <= 6,
		"one read per reading, not one per subscriber: {asked} requests for 5 readings"
	);
}

#[tokio::test(flavor = "multi_thread")]
async fn identifiers_of_one_unit_go_out_together() {
	let (link, script, _) = car(&[(0x7E0, 0x1000, &[1]), (0x7E0, 0x1001, &[2]), (0x7E0, 0x1002, &[3])]);
	let bus = Bus::start(link, Budget::default());
	let mut subs: Vec<Subscription> = (0x1000..=0x1002)
		.map(|did| bus.subscribe(Class::Foreground, ENGINE, did, Duration::from_millis(40), Some(1)))
		.collect();
	let mut got = BTreeMap::new();
	while got.len() < 3 {
		let (_, s) = tokio::time::timeout(Duration::from_secs(2), next_of(&mut subs)).await.unwrap().unwrap();
		got.insert(s.did, s.value.unwrap());
	}
	assert_eq!(got, BTreeMap::from([(0x1000, vec![1]), (0x1001, vec![2]), (0x1002, vec![3])]));
	let first = script.lock().unwrap().asked[0].1.clone();
	assert_eq!(first, [0x22, 0x10, 0x00, 0x10, 0x01, 0x10, 0x02], "one `22 d1 d2 d3`");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_one_shot_read_answers_once_or_says_why_not() {
	let (link, _, _) = car(&[(0x7E0, 0xF190, b"VIN")]);
	let bus = Bus::start(link, Budget::default());
	let (data, _) = bus.read_once(Class::Foreground, ENGINE, 0xF190).await.unwrap();
	assert_eq!(data, b"VIN");
	assert_eq!(bus.read_once(Class::Foreground, ENGINE, 0xF187).await, Err(Miss::Refused(0x31)));
	assert_eq!(bus.read_once(Class::Foreground, ABSENT, 0xF190).await, Err(Miss::NoAnswer));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_uds_client_runs_through_the_bus_unchanged() {
	use vag_uds_can::UnitLink as _;
	let (link, script, _) = car(&[(0x7E0, 0xF190, b"TMBJJ7NE1J0000000"), (0x7E0, 0xF187, b"PART")]);
	let bus = Bus::start(link, Budget::default());
	let mut uds = AsyncUdsClient::new(bus.clone().to_unit(CanId::Standard(0x7E0), CanId::Standard(0x7E8)));
	assert_eq!(uds.read_data_by_identifier(0xF187).await.unwrap(), b"PART");
	// A refusal reaches the client as the negative answer, decoded where it always was.
	assert!(matches!(
		uds.read_data_by_identifier(0x1234).await,
		Err(vag_uds_client::UdsError::NegativeResponse { sid: 0x22, nrc: 0x31 })
	));
	let identity = vag_uds_client::identity::read_identity(&mut uds).await;
	assert_eq!(identity.vin.as_deref(), Some("TMBJJ7NE1J0000000"));

	// The caller's own deadline, not the scheduler's, for its own exchange.
	let _ = uds.read_data_by_identifier_within(0xF187, Duration::from_millis(300)).await;
	let last = script.lock().unwrap().asked.last().unwrap().2;
	assert_eq!(last, Duration::from_millis(300));
	let bus = Bus::release(uds.into_transport());

	// Silence is a timeout to the client, as it was on the wire.
	let mut absent = AsyncUdsClient::new(bus.to_unit(CanId::Standard(0x7E5), CanId::Standard(0x7ED)));
	assert!(matches!(
		absent.read_data_by_identifier_within(0xF190, Duration::from_millis(50)).await,
		Err(vag_uds_client::UdsError::Transport(TransportError::Timeout))
	));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scheduled_read_waits_the_scheduler_deadline() {
	let (link, script, _) = car(&[(0x7E0, 0x1000, &[1])]);
	let bus = Bus::start(link, Budget::default());
	let mut sub = bus.subscribe(Class::Foreground, ENGINE, 0x1000, Duration::from_millis(50), None);
	sample(&mut sub).await;
	assert_eq!(script.lock().unwrap().asked[0].2, READ_DEADLINE);
}

#[tokio::test(flavor = "multi_thread")]
async fn response_pending_is_waited_out_by_the_bus() {
	let (link, script, _) = car(&[(0x7E0, 0xF190, b"VIN")]);
	script.lock().unwrap().pending = 3;
	let bus = Bus::start(link, Budget::default());
	let (data, _) = bus.read_once(Class::Foreground, ENGINE, 0xF190).await.unwrap();
	assert_eq!(data, b"VIN", "three `78`s, then the answer, and the planner saw only the answer");
	let asked = &script.lock().unwrap().asked;
	assert_eq!(asked.len(), 1, "asked once, not once per `78`");
	assert_eq!(asked[0].2, PENDING_WAIT, "the last wait was the pending one");
}

#[tokio::test(flavor = "multi_thread")]
async fn arrival_times_only_move_forward() {
	let (link, _, _) = car(&[(0x7E0, 0x1000, &[1]), (0x7E1, 0x2000, &[2])]);
	let bus = Bus::start(link, Budget::default());
	let gearbox = Unit {
		request: 0x7E1,
		response: 0x7E9,
	};
	let mut subs = vec![
		bus.subscribe(Class::Timing, ENGINE, 0x1000, Duration::from_millis(10), None),
		bus.subscribe(Class::Foreground, gearbox, 0x2000, Duration::from_millis(15), None),
	];
	let mut last = At { ms: 0, secs: 0.0 };
	for _ in 0..30 {
		let (_, s) = tokio::time::timeout(Duration::from_secs(2), next_of(&mut subs)).await.unwrap().unwrap();
		assert!(s.at.secs >= last.secs && s.at.ms >= last.ms, "{:?} after {last:?}", s.at);
		let ms = s.at.secs * 1000.0;
		assert!(
			ms + 1e-6 >= s.at.ms as f64 && ms < s.at.ms as f64 + 1.0,
			"one arrival, two spellings: {:?}",
			s.at
		);
		last = s.at;
	}
	assert!(last.secs <= bus.secs());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_unit_that_does_not_answer_is_a_miss_for_its_subscriber() {
	let (link, _, _) = car(&[(0x7E0, 0x1000, &[1])]);
	let bus = Bus::start(link, Budget::default());
	let mut sub = bus.subscribe(Class::Foreground, ABSENT, 0x1000, Duration::from_millis(20), None);
	assert_eq!(sample(&mut sub).await.value, Err(Miss::NoAnswer));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_service_is_refused_before_it_is_queued() {
	let (link, script, _) = car(&[(0x7E0, 0x1000, &[1])]);
	let bus = Bus::start(link, Budget::default());
	let refused = bus.exchange(Class::Foreground, ENGINE, vec![0x2E, 0xF1, 0x90, 0x00]).await;
	assert!(matches!(refused, Err(ExchangeError::Forbidden(_))), "{refused:?}");
	assert!(script.lock().unwrap().asked.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn when_every_handle_is_gone_the_task_ends_and_lets_go_of_the_link() {
	let (link, _, dropped) = car(&[(0x7E0, 0x1000, &[1])]);
	let bus = Bus::start(link, Budget::default());
	let mut sub = bus.subscribe(Class::Foreground, ENGINE, 0x1000, Duration::from_millis(20), None);
	sample(&mut sub).await;
	drop(bus);
	sample(&mut sub).await;
	assert!(!dropped.load(Ordering::SeqCst), "a live subscription keeps the bus");
	drop(sub);
	for _ in 0..100 {
		if dropped.load(Ordering::SeqCst) {
			return;
		}
		tokio::time::sleep(Duration::from_millis(10)).await;
	}
	panic!("the link was never dropped");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shutdown_closes_every_consumer() {
	let (link, _, dropped) = car(&[(0x7E0, 0x1000, &[1])]);
	let bus = Bus::start(link, Budget::default());
	let mut sub = bus.subscribe(Class::Foreground, ENGINE, 0x1000, Duration::from_millis(20), None);
	sample(&mut sub).await;
	bus.shutdown();
	// Whatever was already on its way still arrives; then the stream ends.
	while tokio::time::timeout(Duration::from_secs(2), sub.next()).await.unwrap().is_some() {}
	assert!(dropped.load(Ordering::SeqCst));
	assert_eq!(bus.read_once(Class::Foreground, ENGINE, 0x1000).await, Err(Miss::BusError));
}
