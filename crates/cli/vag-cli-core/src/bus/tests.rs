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
	/// Answers left over from earlier requests, handed out before the real one.
	stale: std::collections::VecDeque<Vec<u8>>,
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
		if let Some(stale) = script.stale.pop_front() {
			return Ok(stale);
		}
		if script.pending > 0 {
			script.pending -= 1;
			return Ok(vec![0x7F, pdu[0], 0x78]);
		}
		script.asked.push((self.request, pdu.clone(), timeout));
		self.pdu = None;
		if !script.records.keys().any(|(request, _)| *request == self.request) {
			return Err(TransportError::Timeout);
		}
		// A unit that honours the suppress bit says nothing to `3E 80`; `10 82` it refuses.
		if pdu == [0x3E, 0x80] {
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

#[test]
fn an_answer_is_only_taken_for_the_request_it_answers() {
	use super::task::answers;
	// Read: the positive SID and the first echoed identifier.
	assert!(answers(&[0x22, 0xF1, 0x90], &[0x62, 0xF1, 0x90, b'V']));
	assert!(answers(&[0x22, 0xF1, 0x90, 0xF1, 0x87], &[0x62, 0xF1, 0x90, b'V', 0xF1, 0x87, b'P']));
	assert!(
		!answers(&[0x22, 0xF1, 0x90], &[0x62, 0xF1, 0x87, b'P']),
		"another identifier's late answer"
	);
	assert!(!answers(&[0x22, 0xF1, 0x90], &[0x62]), "no echo at all");
	assert!(!answers(&[0x22, 0xF1, 0x90], &[0x59, 0x02, 0xFF]), "another service's answer");
	// Negative: `7F <this SID> nrc`, pending included.
	assert!(answers(&[0x22, 0xF1, 0x90], &[0x7F, 0x22, 0x31]));
	assert!(answers(&[0x22, 0xF1, 0x90], &[0x7F, 0x22, 0x78]));
	assert!(!answers(&[0x22, 0xF1, 0x90], &[0x7F, 0x19, 0x31]));
	assert!(!answers(&[0x22, 0xF1, 0x90], &[0x7F, 0x22]), "a negative answer carries its NRC");
	// Sub-functions are echoed, without the suppress-positive-response bit.
	assert!(answers(&[0x19, 0x02, 0xFF], &[0x59, 0x02, 0xFF]));
	assert!(!answers(&[0x19, 0x02, 0xFF], &[0x59, 0x0A]));
	assert!(answers(&[0x10, 0x03], &[0x50, 0x03, 0x00, 0x32, 0x01, 0xF4]));
	assert!(!answers(&[0x10, 0x03], &[0x50, 0x01]));
	assert!(answers(&[0x3E, 0x00], &[0x7E, 0x00]));
	assert!(answers(&[0x3E, 0x80], &[0x7E, 0x00]));
	assert!(!answers(&[0x3E, 0x00], &[0x7E]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_late_answer_to_an_earlier_request_is_not_taken_for_this_one() {
	let (link, script, _) = car(&[(0x7E0, 0xF190, b"VIN")]);
	script.lock().unwrap().stale.extend([
		vec![0x62, 0xF1, 0x87, b'P'],
		vec![0x7F, 0x19, 0x31],
		vec![0x7F, 0x19, 0x78],
		vec![0x59, 0x02, 0xFF],
	]);
	let bus = Bus::start(link, Budget::default());
	let (data, _) = bus.read_once(Class::Foreground, ENGINE, 0xF190).await.unwrap();
	assert_eq!(data, b"VIN", "four stale answers skipped, the right one taken");
	let asked = &script.lock().unwrap().asked;
	assert_eq!(asked.len(), 1);
	assert!(
		asked[0].2 < READ_DEADLINE,
		"the wait after a discard is what is left, not a fresh deadline"
	);
}

/// A CAN bus with frames already waiting on it before a request goes out, and a unit that
/// answers each read of `1000` with the next count.
struct LateCan {
	waiting: std::collections::VecDeque<(u32, Vec<u8>)>,
	count: u8,
}

impl vag_uds_can::CanBackend for LateCan {
	async fn send_frame(&mut self, id: u32, data: &[u8]) -> Result<(), vag_uds_can::CanError> {
		if id == 0x7E0 && data[..4] == [0x03, 0x22, 0x10, 0x00] {
			self.count += 1;
			self.waiting.push_back((0x7E8, vec![0x04, 0x62, 0x10, 0x00, self.count, 0, 0, 0]));
		}
		Ok(())
	}

	async fn recv_frame(&mut self, timeout: Duration) -> Result<(u32, Vec<u8>), vag_uds_can::CanError> {
		match self.waiting.pop_front() {
			Some(frame) => Ok(frame),
			None => {
				tokio::time::sleep(timeout.min(Duration::from_millis(5))).await;
				Err(vag_uds_can::CanError::Timeout)
			}
		}
	}
}

/// An answer that arrived after an identical request stopped waiting for it echoes the
/// same identifier, so matching cannot tell it from the real one. It is already on the
/// link when the next request goes out, and is discarded before the send, as the board
/// does: otherwise a repeating read runs one answer behind for the rest of the run.
#[tokio::test(flavor = "multi_thread")]
async fn a_late_answer_already_waiting_on_the_cable_is_discarded_before_the_next_send() {
	let late = (0x7E8, vec![0x04, 0x62, 0x10, 0x00, 0x99, 0, 0, 0]);
	let link = LateCan {
		waiting: [late].into(),
		count: 0,
	};
	let bus = Bus::start(link, Budget::default());
	let (data, _) = bus.read_once(Class::Foreground, ENGINE, 0x1000).await.unwrap();
	assert_eq!(data, [1], "this request's answer, not the late one for the last");
	let (data, _) = bus.read_once(Class::Foreground, ENGINE, 0x1000).await.unwrap();
	assert_eq!(data, [2]);
}

/// A link whose exchange never ends: a backend that ignores its own deadline.
struct Stuck;

impl vag_uds_can::UnitLink for Stuck {
	type Channel = Stuck;

	fn to_unit(self, _request: CanId, _response: CanId) -> Stuck {
		self
	}

	fn release(channel: Stuck) -> Stuck {
		channel
	}
}

impl AsyncIsoTpTransport for Stuck {
	async fn send(&mut self, _pdu: &[u8]) -> Result<(), TransportError> {
		Ok(())
	}

	async fn recv(&mut self, _timeout: Duration) -> Result<Vec<u8>, TransportError> {
		std::future::pending().await
	}
}

/// A one-shot read has a deadline of its own, queue and answer together: whatever holds
/// the task up, its caller gets a miss rather than waiting for ever.
#[tokio::test(flavor = "multi_thread")]
async fn a_one_shot_read_the_task_never_gets_to_is_a_miss_after_its_deadline() {
	let bus = Bus::start(Stuck, Budget::default());
	let asked = std::time::Instant::now();
	let (once, all) = tokio::time::timeout(Duration::from_secs(10), async {
		tokio::join!(
			bus.read_once(Class::Foreground, ENGINE, 0xF190),
			bus.read_all(Class::Foreground, &[(ENGINE, 0x0133), (ABSENT, 0x0146)])
		)
	})
	.await
	.expect("the reads come back rather than wait for ever");
	assert_eq!(once, Err(Miss::NoAnswer));
	assert_eq!(all, vec![Err(Miss::NoAnswer), Err(Miss::NoAnswer)]);
	assert!(asked.elapsed() >= ONCE_DEADLINE, "not before the deadline: {:?}", asked.elapsed());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_subscription_does_not_keep_the_others_waiting() {
	let (link, _, _) = car(&[(0x7E0, 0x1000, &[1]), (0x7E1, 0x2000, &[2])]);
	let bus = Bus::start(link, Budget::default());
	let gearbox = Unit {
		request: 0x7E1,
		response: 0x7E9,
	};
	let mut subs = vec![
		bus.subscribe(Class::Foreground, ENGINE, 0x1000, Duration::from_millis(10), None),
		bus.subscribe(Class::Foreground, gearbox, 0x2000, Duration::from_millis(10), None),
	];
	// Both queues fill while nobody reads them.
	tokio::time::sleep(Duration::from_millis(300)).await;
	let mut cursor = 0;
	let mut order = Vec::new();
	for _ in 0..10 {
		let (i, _) = next_of(&mut subs, &mut cursor).await.unwrap();
		order.push(i);
	}
	assert_eq!(order, [0, 1, 0, 1, 0, 1, 0, 1, 0, 1], "turn and turn about, across calls");
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
		let (_, s) = tokio::time::timeout(Duration::from_secs(2), next_of(&mut subs, &mut 0))
			.await
			.unwrap()
			.unwrap();
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
async fn one_shot_reads_asked_together_share_a_request() {
	let (link, script, _) = car(&[(0x7E0, 0x0133, &[101]), (0x7E0, 0x0146, &[55])]);
	let bus = Bus::start(link, Budget::default());
	let got = bus
		.read_all(Class::Foreground, &[(ENGINE, 0x0133), (ENGINE, 0x0146), (ABSENT, 0x0133)])
		.await;
	let values: Vec<Option<Vec<u8>>> = got.into_iter().map(|r| r.ok().map(|(data, _)| data)).collect();
	assert_eq!(values, vec![Some(vec![101]), Some(vec![55]), None], "in the order asked");
	let engine: Vec<Vec<u8>> = script
		.lock()
		.unwrap()
		.asked
		.iter()
		.filter(|(request, _, _)| *request == 0x7E0)
		.map(|(_, pdu, _)| pdu.clone())
		.collect();
	assert_eq!(engine, vec![vec![0x22, 0x01, 0x33, 0x01, 0x46]], "one request for the engine's two");
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
		let (_, s) = tokio::time::timeout(Duration::from_secs(2), next_of(&mut subs, &mut 0))
			.await
			.unwrap()
			.unwrap();
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
async fn a_request_that_suppressed_its_answer_is_a_success_after_a_short_wait() {
	use vag_uds_can::UnitLink as _;
	let (link, script, _) = car(&[(0x7E0, 0xF190, b"VIN")]);
	let bus = Bus::start(link, Budget::default());
	let (answer, _) = bus
		.exchange(Class::Foreground, ENGINE, vec![0x3E, 0x80])
		.await
		.expect("no answer was the answer asked for");
	assert!(answer.is_empty(), "{answer:?}");
	let waited = script.lock().unwrap().asked.last().unwrap().2;
	assert_eq!(waited, SUPPRESSED_WAIT, "the refusal is waited for, not the caller's whole deadline");

	// A refusal still comes back as the refusal.
	let (refused, _) = bus.exchange(Class::Foreground, ENGINE, vec![0x10, 0x82]).await.unwrap();
	assert_eq!(refused, [0x7F, 0x10, 0x11]);
	// And the unit is not taken for absent.
	assert_eq!(bus.read_once(Class::Foreground, ENGINE, 0xF190).await.unwrap().0, b"VIN");

	// Through the link seam it is the silence a CAN link gives a client, only sooner.
	let mut channel = bus.clone().to_unit(CanId::Standard(0x7E0), CanId::Standard(0x7E8));
	channel.send(&[0x3E, 0x80]).await.unwrap();
	assert!(matches!(channel.recv(Duration::from_secs(2)).await, Err(TransportError::Timeout)));
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
