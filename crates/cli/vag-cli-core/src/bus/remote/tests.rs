//! The remote bus against a scripted board over an in-memory pipe: no radio.
//!
//! Identifiers and bytes are synthetic; nothing here is a fact about any car. Real time,
//! on a multi-threaded runtime, for the reason the cable bus's tests give.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::time::Duration;

use vag_uds_client::guard::{MAX_SUBSCRIPTIONS, MIN_PERIOD_MS};
use vag_uds_client::{AsyncUdsClient, UdsError};
use vag_uds_transport::link::{
	self, Answer, HelloReply, MemoryPipe, Message, Outcome, Piece, Pipe, Priority, Reading, Reassembler, Request, Subscribe, pipe_pair,
};
use vag_uds_transport::{CanId, TransportError};

use crate::bus::{Bus, Carrier, Class, ExchangeError, Miss, Sample, Subscription, Unit, next_of};

const ENGINE: Unit = Unit {
	request: 0x7E0,
	response: 0x7E8,
};
const PEER: &str = "vagcan-dash";
/// How long a test waits for something it expects before calling the code stuck.
const PATIENCE: Duration = Duration::from_secs(2);

/// The board's end of the pipe.
struct Board {
	pipe: MemoryPipe,
	reassembler: Reassembler,
	heard: VecDeque<Message>,
}

impl Board {
	/// The next message from the host, or `None` once it has let go of the pipe.
	async fn recv(&mut self) -> Option<Message> {
		loop {
			if let Some(message) = self.heard.pop_front() {
				return Some(message);
			}
			let chunk = self.pipe.read().await?;
			for piece in self.reassembler.push(&chunk) {
				match piece {
					Piece::Message(message) => self.heard.push_back(message),
					other => panic!("the host sent {other:?}"),
				}
			}
		}
	}

	async fn next(&mut self) -> Message {
		tokio::time::timeout(PATIENCE, self.recv())
			.await
			.expect("the host sent something")
			.expect("the pipe is open")
	}

	/// Nothing more from the host for a while.
	async fn quiet(&mut self) {
		if let Ok(message) = tokio::time::timeout(Duration::from_millis(150), self.recv()).await {
			panic!("the host sent {message:?}");
		}
	}

	async fn send(&mut self, message: Message) {
		self.pipe.write(&link::encode(&message).unwrap()).await.unwrap();
	}
}

fn start() -> (Bus, Board) {
	start_chunked(244)
}

/// A bus over a pipe whose writes are cut at `chunk` bytes, as notifications are.
fn start_chunked(chunk: usize) -> (Bus, Board) {
	let (host, board) = pipe_pair(chunk);
	let board = Board {
		pipe: board,
		reassembler: Reassembler::new(),
		heard: VecDeque::new(),
	};
	(Bus::start_remote(host, PEER, Carrier::Ble), board)
}

fn subscribed(message: Message) -> Subscribe {
	match message {
		Message::Subscribe(s) => s,
		other => panic!("expected a subscribe, got {other:?}"),
	}
}

fn requested(message: Message) -> Request {
	match message {
		Message::Request(r) => r,
		other => panic!("expected a request, got {other:?}"),
	}
}

fn reading(sub: u16, at_ms: u32, outcome: Outcome) -> Message {
	Message::Reading(Reading { sub, at_ms, outcome })
}

async fn sample(sub: &mut Subscription) -> Sample {
	tokio::time::timeout(PATIENCE, sub.next())
		.await
		.expect("a sample in time")
		.expect("the subscription is live")
}

async fn ended(sub: &mut Subscription) -> bool {
	tokio::time::timeout(PATIENCE, sub.next()).await.expect("an end in time").is_none()
}

/// A board answering every request from `records` as the unit would, one at a time,
/// until the host lets go. It refuses `F40D` itself, the way its guard refuses on a
/// moving car.
async fn serve(mut board: Board, records: BTreeMap<u16, Vec<u8>>) {
	while let Some(message) = board.recv().await {
		let Message::Request(request) = message else { continue };
		let outcome = match request.pdu.as_slice() {
			[0x22, 0xF4, 0x0D] => Outcome::Refused("the car is moving".into()),
			[0x22, dids @ ..] => {
				let mut answer = vec![0x62];
				for did in dids.chunks(2) {
					if let Some(record) = records.get(&u16::from_be_bytes([did[0], did[1]])) {
						answer.extend_from_slice(did);
						answer.extend_from_slice(record);
					}
				}
				match answer.len() {
					1 => Outcome::Pdu(vec![0x7F, 0x22, 0x31]),
					_ => Outcome::Pdu(answer),
				}
			}
			[sid, ..] => Outcome::Pdu(vec![0x7F, *sid, 0x11]),
			[] => unreachable!("the link refuses an empty PDU"),
		};
		board.send(Message::Answer(Answer { seq: request.seq, outcome })).await;
	}
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subscription_is_polled_by_the_board_and_keeps_the_boards_timing() {
	let (bus, mut board) = start();
	let mut sub = bus.subscribe(Class::Timing, ENGINE, 0xF40D, Duration::from_millis(10), Some(1));
	let asked = subscribed(board.next().await);
	assert_eq!((asked.request_id, asked.response_id, asked.did), (0x7E0, 0x7E8, 0xF40D));
	assert_eq!(asked.period_ms, MIN_PERIOD_MS, "a period under the guard's floor is raised to it");

	for (at_ms, kmh) in [(90_000, 0), (90_020, 3), (90_041, 7)] {
		board.send(reading(asked.sub, at_ms, Outcome::Pdu(vec![0x62, 0xF4, 0x0D, kmh]))).await;
	}
	let got = [sample(&mut sub).await, sample(&mut sub).await, sample(&mut sub).await];
	let values: Vec<_> = got.iter().map(|s| s.value.clone()).collect();
	assert_eq!(values, vec![Ok(vec![0]), Ok(vec![3]), Ok(vec![7])], "the echo is taken off");
	assert!(got.iter().all(|s| (s.unit, s.did) == (ENGINE, 0xF40D)));
	let gaps = [got[1].at.secs - got[0].at.secs, got[2].at.secs - got[0].at.secs];
	assert!(
		(gaps[0] - 0.020).abs() < 1e-9 && (gaps[1] - 0.041).abs() < 1e-9,
		"the board's gaps, not the radio's: {gaps:?}"
	);
	assert!(got[0].at.secs <= bus.secs(), "the first reading is stamped on arrival");
	assert!(got.iter().all(|s| s.at.ms as f64 <= s.at.secs * 1000.0 + 1e-6), "one time, two spellings");
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_a_subscription_unsubscribes_it_on_the_board() {
	let (bus, mut board) = start();
	let sub = bus.subscribe(Class::Foreground, ENGINE, 0x2029, Duration::from_millis(100), None);
	let first = subscribed(board.next().await).sub;
	drop(sub);
	assert_eq!(board.next().await, Message::Unsubscribe { sub: first });

	let mut again = bus.subscribe(Class::Foreground, ENGINE, 0x2029, Duration::from_millis(100), None);
	let second = subscribed(board.next().await).sub;
	assert_ne!(first, second, "an id just freed is not handed out again");
	// A reading still on its way for the old id reaches nobody.
	board.send(reading(first, 1, Outcome::Pdu(vec![0x62, 0x20, 0x29, 1]))).await;
	board.send(reading(second, 2, Outcome::Pdu(vec![0x62, 0x20, 0x29, 2]))).await;
	assert_eq!(sample(&mut again).await.value, Ok(vec![2]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reading_that_is_no_record_is_a_miss_and_a_refusal_ends_the_subscription() {
	let (bus, mut board) = start();
	let mut sub = bus.subscribe(Class::Foreground, ENGINE, 0x2029, Duration::from_millis(100), None);
	let id = subscribed(board.next().await).sub;
	for outcome in [
		Outcome::NoAnswer,
		Outcome::Pdu(vec![0x7F, 0x22, 0x31]),
		Outcome::Pdu(vec![0x62, 0x20, 0x2A, 1]),
		Outcome::BusError("bus off".into()),
	] {
		board.send(reading(id, 10, outcome)).await;
	}
	let mut misses = Vec::new();
	for _ in 0..4 {
		misses.push(sample(&mut sub).await.value);
	}
	assert_eq!(
		misses,
		vec![Err(Miss::NoAnswer), Err(Miss::Refused(0x31)), Err(Miss::Malformed), Err(Miss::BusError)],
		"silence, the unit's refusal, another identifier's echo, a bus error"
	);

	board.send(reading(id, 20, Outcome::Refused("8 evenly spaced identifiers".into()))).await;
	assert_eq!(sample(&mut sub).await.value, Err(Miss::BusError));
	assert!(ended(&mut sub).await, "ended on the board, so ended here");
	drop(sub);
	board.quiet().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_one_shot_read_is_a_request_of_that_one_identifier() {
	let (bus, mut board) = start();
	let asking = bus.clone();
	let read = tokio::spawn(async move { asking.read_once(Class::Foreground, ENGINE, 0xF190).await });
	let asked = requested(board.next().await);
	assert_eq!(
		(asked.request_id, asked.response_id, asked.pdu.as_slice()),
		(0x7E0, 0x7E8, &[0x22, 0xF1, 0x90][..])
	);
	board
		.send(Message::Answer(Answer {
			seq: asked.seq,
			outcome: Outcome::Pdu(b"\x62\xF1\x90VIN".to_vec()),
		}))
		.await;
	assert_eq!(read.await.unwrap().unwrap().0, b"VIN");
}

/// Put `[10 03]` out, let the board answer it with `outcome` — after one stale answer
/// for another request — and return what the exchange came back with.
async fn exchanged(bus: &Bus, board: &mut Board, outcome: Outcome) -> Result<Vec<u8>, ExchangeError> {
	let asking = bus.clone();
	let out = tokio::spawn(async move { asking.exchange(Class::Foreground, ENGINE, vec![0x10, 0x03]).await });
	let asked = requested(board.next().await);
	assert_eq!(asked.pdu, [0x10, 0x03]);
	let stale = Answer {
		seq: asked.seq.wrapping_add(1),
		outcome: Outcome::Pdu(vec![0x50, 0x01]),
	};
	board.send(Message::Answer(stale)).await;
	board.send(Message::Answer(Answer { seq: asked.seq, outcome })).await;
	out.await.unwrap().map(|(pdu, _)| pdu)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_exchange_comes_back_as_the_board_answered_it() {
	let (bus, mut board) = start();
	let answered = exchanged(&bus, &mut board, Outcome::Pdu(vec![0x7F, 0x10, 0x22])).await;
	assert_eq!(answered.unwrap(), [0x7F, 0x10, 0x22], "a negative answer is the client's to decode");
	let positive = exchanged(&bus, &mut board, Outcome::Pdu(vec![0x50, 0x03, 0x00, 0x32, 0x01, 0xF4])).await;
	assert_eq!(positive.unwrap(), [0x50, 0x03, 0x00, 0x32, 0x01, 0xF4]);
	// `50 01` answers `10 01`, not the `10 03` that was asked.
	let another = exchanged(&bus, &mut board, Outcome::Pdu(vec![0x50, 0x01])).await;
	assert!(
		matches!(&another, Err(ExchangeError::Link(TransportError::Protocol(why))) if why.contains("do not answer")),
		"{another:?}"
	);
	let silent = exchanged(&bus, &mut board, Outcome::NoAnswer).await;
	assert!(matches!(silent, Err(ExchangeError::NoAnswer)), "{silent:?}");
	let refused = exchanged(&bus, &mut board, Outcome::Refused("the car is moving".into())).await;
	assert!(
		matches!(&refused, Err(ExchangeError::Refused(why)) if why == "the car is moving"),
		"{refused:?}"
	);
	let failed = exchanged(&bus, &mut board, Outcome::BusError("bus off".into())).await;
	assert!(
		matches!(&failed, Err(ExchangeError::Link(TransportError::Io(why))) if why.contains("bus off")),
		"{failed:?}"
	);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_service_is_refused_before_it_reaches_the_board() {
	let (bus, mut board) = start();
	let refused = bus.exchange(Class::Foreground, ENGINE, vec![0x2E, 0xF1, 0x90, 0x00]).await;
	assert!(matches!(refused, Err(ExchangeError::Forbidden(_))), "{refused:?}");
	board.quiet().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_uds_client_runs_through_the_remote_bus_unchanged() {
	use vag_uds_can::UnitLink as _;
	let (bus, board) = start();
	let records = BTreeMap::from([(0xF190, b"TMBJJ7NE1J0000000".to_vec()), (0xF187, b"PART".to_vec())]);
	tokio::spawn(serve(board, records));

	let mut uds = AsyncUdsClient::new(bus.clone().to_unit(CanId::Standard(0x7E0), CanId::Standard(0x7E8)));
	assert_eq!(uds.read_data_by_identifier(0xF187).await.unwrap(), b"PART");
	assert!(matches!(
		uds.read_data_by_identifier(0x1234).await,
		Err(UdsError::NegativeResponse { sid: 0x22, nrc: 0x31 })
	));
	let identity = vag_uds_client::identity::read_identity(&mut uds).await;
	assert_eq!(identity.vin.as_deref(), Some("TMBJJ7NE1J0000000"));

	// The board's refusal reaches the client in the board's words.
	match uds.read_data_by_identifier(0xF40D).await {
		Err(UdsError::Transport(TransportError::Protocol(why))) => assert!(why.contains("the car is moving"), "{why}"),
		other => panic!("expected the board's refusal, got {other:?}"),
	}
}

#[tokio::test(flavor = "multi_thread")]
async fn text_from_the_board_and_readings_cut_into_small_notifications_are_told_apart() {
	let (bus, mut board) = start_chunked(20);
	let mut sub = bus.subscribe(Class::Foreground, ENGINE, 0x1001, Duration::from_millis(100), None);
	let id = subscribed(board.next().await).sub;
	let record: Vec<u8> = (0..60).collect();
	let mut pdu = vec![0x62, 0x10, 0x01];
	pdu.extend_from_slice(&record);

	// Before the MTU is agreed the board notifies 20 bytes at a time; after, 244.
	board.pipe.write(b"state page=1/2 brightness=77 unsaved=0 gen=3").await.unwrap();
	board.send(reading(id, 100, Outcome::Pdu(pdu.clone()))).await;
	board.pipe.write(b"state page=2/2 brightness=77 unsaved=0 gen=3").await.unwrap();
	board.pipe.set_chunk(244);
	board.send(reading(id, 120, Outcome::Pdu(pdu))).await;
	board.pipe.write(b"state page=1/2 brightness=77 unsaved=0 gen=4").await.unwrap();

	assert_eq!(sample(&mut sub).await.value, Ok(record.clone()));
	assert_eq!(sample(&mut sub).await.value, Ok(record));
}

#[tokio::test(flavor = "multi_thread")]
async fn over_the_cable_a_reply_to_hello_and_the_images_text_are_ignored_and_a_break_says_usb() {
	const ON_USB: &str = "the dash board on /dev/cu.usbmodem1101";
	let (host, board) = pipe_pair(64);
	let mut board = Board {
		pipe: board,
		reassembler: Reassembler::new(),
		heard: VecDeque::new(),
	};
	// What the handshake can leave behind it: a reply to a Hello sent again, and a log line.
	let reply = HelloReply {
		image: "dash".into(),
		version: "0.1.0".into(),
	};
	board.send(Message::HelloReply(reply)).await;
	board.pipe.write(b"can: 7E0 timeout\r\n").await.unwrap();
	let bus = Bus::start_remote(host, ON_USB, Carrier::Usb);

	let mut sub = bus.subscribe(Class::Foreground, ENGINE, 0x2029, Duration::from_millis(100), None);
	let id = subscribed(board.next().await).sub;
	board.pipe.write(b"FRAME 256 64 AAAA\r\n").await.unwrap();
	board.send(reading(id, 10, Outcome::Pdu(vec![0x62, 0x20, 0x29, 7]))).await;
	assert_eq!(sample(&mut sub).await.value, Ok(vec![7]));

	drop(board);
	assert_eq!(sample(&mut sub).await.value, Err(Miss::BusError));
	assert_eq!(
		bus.closed().as_deref(),
		Some("the USB connection to the dash board on /dev/cu.usbmodem1101 dropped")
	);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_connection_fails_what_is_out_ends_every_subscription_and_every_later_command() {
	let (bus, mut board) = start();
	let mut sub = bus.subscribe(Class::Foreground, ENGINE, 0x2029, Duration::from_millis(100), None);
	subscribed(board.next().await);
	let asking = bus.clone();
	let out = tokio::spawn(async move { asking.exchange(Class::Foreground, ENGINE, vec![0x19, 0x02, 0x08]).await });
	requested(board.next().await);
	assert_eq!(bus.closed(), None, "nothing to say while the link is up");
	drop(board);

	let dropped = |result: &Result<_, ExchangeError>| matches!(result, Err(ExchangeError::Link(TransportError::Io(why))) if why == "the BLE connection to vagcan-dash dropped");
	let failed = out.await.unwrap();
	assert!(dropped(&failed), "{failed:?}");
	assert_eq!(
		sample(&mut sub).await.value,
		Err(Miss::BusError),
		"a last miss, so the stream's end reads as a failure"
	);
	assert!(ended(&mut sub).await);
	assert_eq!(bus.closed().as_deref(), Some("the BLE connection to vagcan-dash dropped"));

	assert_eq!(bus.read_once(Class::Foreground, ENGINE, 0xF190).await, Err(Miss::BusError));
	let later = bus.exchange(Class::Foreground, ENGINE, vec![0x22, 0xF1, 0x90]).await;
	assert!(dropped(&later), "{later:?}");
	let mut late = bus.subscribe(Class::Foreground, ENGINE, 0x2029, Duration::from_millis(100), None);
	assert!(ended(&mut late).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subscription_past_the_boards_limit_is_refused_here() {
	let (bus, mut board) = start();
	let mut subs: Vec<Subscription> = (0..MAX_SUBSCRIPTIONS as u16)
		.map(|i| bus.subscribe(Class::Foreground, ENGINE, 0x2000 + i, Duration::from_millis(500), None))
		.collect();
	for _ in 0..MAX_SUBSCRIPTIONS {
		subscribed(board.next().await);
	}

	let mut over = bus.subscribe(Class::Foreground, ENGINE, 0x3000, Duration::from_millis(500), None);
	assert_eq!(sample(&mut over).await.value, Err(Miss::BusError));
	assert!(ended(&mut over).await);
	board.quiet().await;

	// An unsubscribe frees a slot.
	subs.pop();
	assert!(matches!(board.next().await, Message::Unsubscribe { .. }));
	let _again = bus.subscribe(Class::Foreground, ENGINE, 0x3000, Duration::from_millis(500), None);
	assert_eq!(subscribed(board.next().await).did, 0x3000);
}

/// `measure`'s speed channel goes out marked timing, for the board to rank ahead of the
/// host's other work; every other class goes out normal.
#[tokio::test(flavor = "multi_thread")]
async fn only_a_timing_subscription_is_sent_as_timing() {
	let (bus, mut board) = start();
	let _speed = bus.subscribe(Class::Timing, ENGINE, 0xF40D, Duration::from_millis(20), None);
	let asked = subscribed(board.next().await);
	assert_eq!((asked.did, asked.priority), (0xF40D, Priority::Timing));
	let mut others = Vec::new();
	for (class, did) in [(Class::Foreground, 0x2029), (Class::Remote, 0x202A), (Class::Background, 0x206E)] {
		others.push(bus.subscribe(class, ENGINE, did, Duration::from_millis(100), None));
		let asked = subscribed(board.next().await);
		assert_eq!((asked.did, asked.priority), (did, Priority::Normal), "{class:?}");
	}
}

/// The board's guard holds one timing subscription per connection; the second is refused
/// here rather than sent to be refused. Dropping the first, or the board refusing it,
/// frees the slot.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_timing_subscription_is_refused_here_until_the_first_ends() {
	let (bus, mut board) = start();
	let mut speed = bus.subscribe(Class::Timing, ENGINE, 0xF40D, Duration::from_millis(20), None);
	let first = subscribed(board.next().await);
	assert_eq!(first.priority, Priority::Timing);

	let mut second = bus.subscribe(Class::Timing, ENGINE, 0xF40C, Duration::from_millis(20), None);
	assert_eq!(sample(&mut second).await.value, Err(Miss::BusError));
	assert!(ended(&mut second).await);
	board.quiet().await;
	board.send(reading(first.sub, 10, Outcome::Pdu(vec![0x62, 0xF4, 0x0D, 5]))).await;
	assert_eq!(sample(&mut speed).await.value, Ok(vec![5]), "the first still reads");

	// A normal subscription is not held to the timing cap.
	let _normal = bus.subscribe(Class::Foreground, ENGINE, 0x2029, Duration::from_millis(100), None);
	assert_eq!(subscribed(board.next().await).priority, Priority::Normal);

	drop(speed);
	assert_eq!(board.next().await, Message::Unsubscribe { sub: first.sub });
	let mut again = bus.subscribe(Class::Timing, ENGINE, 0xF40C, Duration::from_millis(20), None);
	let again_id = subscribed(board.next().await);
	assert_eq!((again_id.did, again_id.priority), (0xF40C, Priority::Timing));

	// Refused by the board, it ends here too, and the slot is free again.
	board
		.send(reading(again_id.sub, 20, Outcome::Refused("more than 1 timing subscription".into())))
		.await;
	assert_eq!(sample(&mut again).await.value, Err(Miss::BusError));
	assert!(ended(&mut again).await);
	let _third = bus.subscribe(Class::Timing, ENGINE, 0xF40D, Duration::from_millis(20), None);
	assert_eq!(subscribed(board.next().await).priority, Priority::Timing);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_an_answer_fixes_the_board_clock_not_a_refusal_stamped_with_the_boards_now() {
	let (bus, mut board) = start();
	let refused = bus.subscribe(Class::Foreground, ENGINE, 0x2029, Duration::from_millis(100), None);
	let refused_id = subscribed(board.next().await).sub;
	let mut speed = bus.subscribe(Class::Timing, ENGINE, 0xF40D, Duration::from_millis(20), None);
	let speed_id = subscribed(board.next().await).sub;
	// Refused at the board's own now, an hour past the answers that follow.
	board
		.send(reading(refused_id, 3_600_000, Outcome::Refused("the unit is locked".into())))
		.await;
	for (at_ms, kmh) in [(5_000, 1), (5_020, 2)] {
		board.send(reading(speed_id, at_ms, Outcome::Pdu(vec![0x62, 0xF4, 0x0D, kmh]))).await;
	}
	let first = sample(&mut speed).await;
	let second = sample(&mut speed).await;
	assert!(
		(second.at.secs - first.at.secs - 0.020).abs() < 1e-9,
		"the board's gap between answers: {:?} then {:?}",
		first.at,
		second.at
	);
	assert!(
		first.at.secs > 0.0 && first.at.secs <= bus.secs(),
		"fixed at the first answer's arrival: {:?}",
		first.at
	);
	drop(refused);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_frame_that_does_not_reassemble_breaks_the_link_rather_than_being_read_past() {
	let (bus, mut board) = start();
	let mut sub = bus.subscribe(Class::Foreground, ENGINE, 0x2029, Duration::from_millis(100), None);
	subscribed(board.next().await);
	// An unknown type where a frame starts: what a lost chunk looks like from here.
	board.pipe.write(&[0x00, 0x7A, 0x02, 0x00, 1, 2]).await.unwrap();
	assert_eq!(sample(&mut sub).await.value, Err(Miss::BusError));
	assert!(ended(&mut sub).await);
	let why = bus.closed().expect("a reason");
	assert!(why.contains("vagcan-dash") && why.contains("lost data"), "{why}");
}

/// Readings the eager board answers each subscribe with, at once.
const READINGS_EACH: u16 = 4;
/// Unread notifications btleplug's channel holds before it loses the oldest.
const NOTIFICATION_SLOTS: usize = 16;

/// The eager board's side: what the host has not read yet, and what it lost.
#[derive(Default)]
struct Lane {
	unread: VecDeque<Vec<u8>>,
	reader: Option<Waker>,
	/// Chunks lost because [`NOTIFICATION_SLOTS`] were already unread.
	lost: usize,
	/// How many chunks the host had left unread at each of its writes.
	unread_at_writes: Vec<usize>,
	host: Reassembler,
}

/// A board that answers a subscribe the moment it is written, into a lane shaped like
/// btleplug's notification channel: it holds [`NOTIFICATION_SLOTS`] unread chunks and
/// loses the oldest past that.
struct Eager(Arc<Mutex<Lane>>);

impl Pipe for Eager {
	async fn write(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
		let mut lane = self.0.lock().unwrap();
		let unread = lane.unread.len();
		lane.unread_at_writes.push(unread);
		let pieces = lane.host.push(bytes);
		for piece in pieces {
			let Piece::Message(Message::Subscribe(s)) = piece else { continue };
			let [hi, lo] = s.did.to_be_bytes();
			for i in 0..READINGS_EACH {
				let answer = Outcome::Pdu(vec![0x62, hi, lo, i as u8]);
				let frame = link::encode(&reading(s.sub, 1_000 + u32::from(i) * 20, answer)).unwrap();
				if lane.unread.len() == NOTIFICATION_SLOTS {
					lane.unread.pop_front();
					lane.lost += 1;
				}
				lane.unread.push_back(frame);
			}
		}
		if let Some(reader) = lane.reader.take() {
			reader.wake();
		}
		Ok(())
	}

	async fn read(&mut self) -> Option<Vec<u8>> {
		std::future::poll_fn(|cx| {
			let mut lane = self.0.lock().unwrap();
			match lane.unread.pop_front() {
				Some(chunk) => Poll::Ready(Some(chunk)),
				None => {
					lane.reader = Some(cx.waker().clone());
					Poll::Pending
				}
			}
		})
		.await
	}
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_that_suppressed_its_answer_and_was_not_refused_is_a_success_with_no_data() {
	let (bus, mut board) = start();
	for (pdu, suppressed) in [(vec![0x3E, 0x80], true), (vec![0x3E, 0x00], false)] {
		let asking = bus.clone();
		let sent = pdu.clone();
		let out = tokio::spawn(async move { asking.exchange(Class::Foreground, ENGINE, sent).await });
		let asked = requested(board.next().await);
		assert_eq!(asked.pdu, pdu);
		board
			.send(Message::Answer(Answer {
				seq: asked.seq,
				outcome: Outcome::NoAnswer,
			}))
			.await;
		match (out.await.unwrap(), suppressed) {
			(Ok((answer, _)), true) => assert!(answer.is_empty(), "{answer:?}"),
			(Err(ExchangeError::NoAnswer), false) => {}
			(other, _) => panic!("{pdu:02X?}: {other:?}"),
		}
	}
}

#[tokio::test(flavor = "multi_thread")]
async fn readings_streaming_in_while_subscribes_are_written_are_taken_in_between_the_writes() {
	let lane = Arc::new(Mutex::new(Lane::default()));
	let bus = Bus::start_remote(Eager(lane.clone()), PEER, Carrier::Ble);
	let mut subs: Vec<Subscription> = (0..30u16)
		.map(|i| bus.subscribe(Class::Foreground, ENGINE, 0x2000 + i, Duration::from_millis(100), None))
		.collect();
	let mut cursor = 0;
	for _ in 0..30 * READINGS_EACH {
		let (_, got) = tokio::time::timeout(PATIENCE, next_of(&mut subs, &mut cursor))
			.await
			.expect("every reading arrives")
			.expect("the bus is running");
		assert!(got.value.is_ok(), "{got:?}");
	}
	let lane = lane.lock().unwrap();
	assert_eq!(lane.lost, 0, "no reading lost to a full notification channel");
	assert!(
		lane.unread_at_writes.iter().all(|&unread| unread == 0),
		"nothing left unread when the next write went out: {:?}",
		lane.unread_at_writes
	);
}

/// A bus over the cable parts with a Hello, which closes its session on the board, so its
/// subscriptions stop polling the car when the host exits with the cable still in.
#[tokio::test(flavor = "multi_thread")]
async fn a_bus_over_the_cable_says_hello_when_it_ends() {
	let (host, board) = pipe_pair(64);
	let mut board = Board {
		pipe: board,
		reassembler: Reassembler::new(),
		heard: VecDeque::new(),
	};
	let bus = Bus::start_remote(host, PEER, Carrier::Usb);
	let sub = bus.subscribe(Class::Foreground, ENGINE, 0x2000, Duration::from_millis(100), None);
	assert!(matches!(board.next().await, Message::Subscribe(_)));
	drop(sub);
	drop(bus);
	// The unsubscribe may not go out first: the Hello closes the whole session anyway.
	let mut parting = board.next().await;
	if matches!(parting, Message::Unsubscribe { .. }) {
		parting = board.next().await;
	}
	assert_eq!(parting, Message::Hello);
	let closed = tokio::time::timeout(PATIENCE, board.recv()).await.expect("the pipe closes");
	assert_eq!(closed, None);

	// A shutdown asked for parts the same way.
	let (host, board) = pipe_pair(64);
	let mut board = Board {
		pipe: board,
		reassembler: Reassembler::new(),
		heard: VecDeque::new(),
	};
	let bus = Bus::start_remote(host, PEER, Carrier::Usb);
	bus.shutdown();
	assert_eq!(board.next().await, Message::Hello);
}

/// The board's clock is a `u32` of milliseconds, and it wraps after 49.7 days: the gap
/// across the wrap is still the board's gap, not every later reading at 0 s.
#[tokio::test(flavor = "multi_thread")]
async fn a_board_clock_that_wraps_keeps_the_boards_gaps() {
	let (bus, mut board) = start();
	let mut speed = bus.subscribe(Class::Timing, ENGINE, 0xF40D, Duration::from_millis(20), None);
	let id = subscribed(board.next().await).sub;
	for (at_ms, kmh) in [(u32::MAX - 9, 1), (10, 2), (30, 3)] {
		board.send(reading(id, at_ms, Outcome::Pdu(vec![0x62, 0xF4, 0x0D, kmh]))).await;
	}
	let got = [sample(&mut speed).await, sample(&mut speed).await, sample(&mut speed).await];
	let gaps = [got[1].at.secs - got[0].at.secs, got[2].at.secs - got[1].at.secs];
	assert!(
		(gaps[0] - 0.020).abs() < 1e-9 && (gaps[1] - 0.020).abs() < 1e-9,
		"the board's gaps across its clock's wrap: {:?}",
		got.iter().map(|s| s.at).collect::<Vec<_>>()
	);
}

/// The parting Hello over the cable is written while the process may be exiting, when
/// the runtime's timer has already shut down and polling one panics. A runtime with no
/// timer at all stands in for that: the parting must not need one.
#[test]
fn parting_over_the_cable_needs_no_timer() {
	let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
	let (host, board) = pipe_pair(64);
	let mut board = Board {
		pipe: board,
		reassembler: Reassembler::new(),
		heard: VecDeque::new(),
	};
	runtime.block_on(async move {
		let (commands, inbox) = tokio::sync::mpsc::unbounded_channel::<crate::bus::Command>();
		drop(commands);
		let closed = Arc::new(std::sync::OnceLock::new());
		super::run(host, PEER.to_string(), Carrier::Usb, inbox, tokio::time::Instant::now(), closed).await;
	});
	assert_eq!(runtime.block_on(board.recv()), Some(Message::Hello), "the Hello still went out");
}

/// Over BLE the disconnect closes the board's session, and nothing more is sent.
#[tokio::test(flavor = "multi_thread")]
async fn a_bus_over_ble_ends_without_a_word() {
	let (bus, mut board) = start();
	drop(bus);
	let closed = tokio::time::timeout(PATIENCE, board.recv()).await.expect("the pipe closes");
	assert_eq!(closed, None);
}
