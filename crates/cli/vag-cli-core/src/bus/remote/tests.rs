//! The remote bus against a scripted board over an in-memory pipe: no radio.
//!
//! Identifiers and bytes are synthetic; nothing here is a fact about any car. Real time,
//! on a multi-threaded runtime, for the reason the cable bus's tests give.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use vag_uds_client::guard::{MAX_SUBSCRIPTIONS, MIN_PERIOD_MS};
use vag_uds_client::{AsyncUdsClient, UdsError};
use vag_uds_transport::link::{self, Answer, MemoryPipe, Message, Outcome, Piece, Pipe, Reading, Reassembler, Request, Subscribe, pipe_pair};
use vag_uds_transport::{CanId, TransportError};

use crate::bus::{Bus, Class, ExchangeError, Miss, Sample, Subscription, Unit};

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
	(Bus::start_remote(host, PEER), board)
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
async fn a_dropped_connection_fails_what_is_out_ends_every_subscription_and_every_later_command() {
	let (bus, mut board) = start();
	let mut sub = bus.subscribe(Class::Foreground, ENGINE, 0x2029, Duration::from_millis(100), None);
	subscribed(board.next().await);
	let asking = bus.clone();
	let out = tokio::spawn(async move { asking.exchange(Class::Foreground, ENGINE, vec![0x19, 0x02, 0x08]).await });
	requested(board.next().await);
	drop(board);

	let dropped = |result: &Result<_, ExchangeError>| matches!(result, Err(ExchangeError::Link(TransportError::Io(why))) if why == "the BLE connection to vagcan-dash dropped");
	let failed = out.await.unwrap();
	assert!(dropped(&failed), "{failed:?}");
	assert!(ended(&mut sub).await);

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
