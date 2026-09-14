//! `benchecu` — a CANable answering diagnostic requests as a control unit, on a
//! bench pair. Never on a car.
//!
//! ```text
//! benchecu --bench --device /dev/cu.usbmodem1101 --unit 7E0 --speed-kmh 42 --seconds 60
//! ```
//!
//! The dash board backs off a unit that does not answer, so on a bench with no
//! car its scheduler never runs at the rate it would on one. This puts the
//! CANable on the pair as that unit: it answers `22 F40D` with a fixed road
//! speed, refuses the rest, and counts what it was asked each second — so a
//! subscription's rate over BLE is measured at the bus end, by the fixture.
//!
//! It transmits, so it guards itself (`CLAUDE.md` Safety): nothing starts
//! without `--bench`; it listens 2 s before its first answer; and a frame on
//! any id but the units it plays or `7DF` — another node, which a bench pair
//! has none of and a car always has — stops it, then or at any time after.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, Instant};
use vag_uds_can::{CanBackend, CanError, SlcanBackend, SlcanBitrate, SlcanMode};
use vag_uds_client::address::UnitAddress;

/// The adapter's serial baud, as `vag_cli_core::device::ADAPTER_BAUD`.
const ADAPTER_BAUD: u32 = 115_200;
/// ISO 15765-2 leaves unused frame bytes to the sender; these are padded.
const PAD: u8 = 0xAA;
/// ISO 15765-4's functional request id. Tolerated by the guard, not answered:
/// the board addresses units physically.
const FUNCTIONAL: u32 = 0x7DF;
/// How long to listen for other nodes before the first answer.
const LISTEN: Duration = Duration::from_secs(2);
/// ISO 15765-2 N_Bs and N_Cr: the wait for a flow control, and for the next
/// consecutive frame.
const N_TIMEOUT: Duration = Duration::from_millis(1000);
/// FC.WAIT frames accepted in a row before a response is abandoned.
const MAX_FC_WAIT: u8 = 10;
/// The largest PDU a 12-bit first-frame length carries (ISO 15765-2).
const MAX_PDU: usize = 4095;
/// Vehicle speed, one byte in km/h: OBD-II PID `0D` (SAE J1979), mirrored at
/// `F400 + PID`. A protocol identifier, not one car's.
const DID_SPEED: u16 = 0xF40D;

/// ISO 14229-1 negative response codes.
const NRC_SERVICE_NOT_SUPPORTED: u8 = 0x11;
const NRC_SUBFUNCTION_NOT_SUPPORTED: u8 = 0x12;
const NRC_INCORRECT_LENGTH: u8 = 0x13;
const NRC_RESPONSE_TOO_LONG: u8 = 0x14;
const NRC_REQUEST_OUT_OF_RANGE: u8 = 0x31;

const REFUSAL: &str = "benchecu transmits answers as a control unit; bench pair only";
const CAR: &str = "traffic from other nodes — this looks like a car, not a bench";

const USAGE: &str = "\
benchecu — answers diagnostic requests as a control unit on a bench pair; never on a car

For measuring the dash board's rates over BLE without a car: the board backs off
a unit that does not answer, so the CANable plays one that does.

Expects a bench pair: the board and a CANable on one terminated CAN bus at
500 kbit/s, and nothing else on it. No car.

usage:
  benchecu --bench --device <serial path> --unit <request id> [--unit ...]
           [--speed-kmh <0..255>] [--seconds <N>]

  --bench       required; says this is a bench pair
  --device      the CANable's serial port, e.g. /dev/cu.usbmodem1101
  --unit        request id to answer, hex; repeatable (7E0 answers on 7E8, 714 on 77E)
  --speed-kmh   what F40D reads, default 0
  --seconds     stop after N seconds of answering; otherwise Ctrl-C

answers:
  22 F40D ...   62 F40D <speed> for each F40D asked; nothing served: 7F 22 31
  3E 00         7E 00
  3E 80         no answer
  10, 19, rest  7F <sid> 11

It listens 2 s before answering. A frame on any id but the --unit ids and 7DF
is another node: it stops, then or at any time after.

prints: once a second the requests seen per unit and identifier (7E0 F40D 10/s),
and a total at exit.";

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Invocation {
	Help,
	/// No `--bench`: nothing is opened.
	Refused,
	Run(Config),
}

#[derive(Debug, PartialEq, Eq)]
struct Config {
	device: String,
	units: Vec<UnitAddress>,
	speed_kmh: u8,
	seconds: Option<u64>,
}

fn parse(args: &[String]) -> Result<Invocation, String> {
	if args.iter().any(|a| a == "--help" || a == "-h") {
		return Ok(Invocation::Help);
	}
	if !args.iter().any(|a| a == "--bench") {
		return Ok(Invocation::Refused);
	}
	let (mut device, mut units, mut speed_kmh, mut seconds) = (None, Vec::new(), 0u8, None);
	let mut rest = args.iter();
	while let Some(flag) = rest.next() {
		let mut value = || rest.next().ok_or_else(|| format!("{flag} wants a value"));
		match flag.as_str() {
			"--bench" => {}
			"--device" => device = Some(value()?.clone()),
			"--unit" => {
				let text = value()?;
				let id = u16::from_str_radix(text.trim_start_matches("0x"), 16).map_err(|_| format!("{text:?} is not a hex request id like 7E0"))?;
				// The response id by the same rule the client addresses units with.
				let unit = UnitAddress::from_request(id).ok_or_else(|| format!("{id:03X} is in neither diagnostic block (700-7BF or 7E0-7E7)"))?;
				if !units.contains(&unit) {
					units.push(unit);
				}
			}
			"--speed-kmh" => speed_kmh = value()?.parse().map_err(|_| "--speed-kmh is a number 0..255".to_string())?,
			"--seconds" => seconds = Some(value()?.parse().map_err(|_| "--seconds is a whole number".to_string())?),
			other => return Err(format!("unknown argument {other}")),
		}
	}
	let device = device.ok_or("--device <serial path> is required")?;
	if units.is_empty() {
		return Err("at least one --unit <request id> is required".to_string());
	}
	Ok(Invocation::Run(Config {
		device,
		units,
		speed_kmh,
		seconds,
	}))
}

// ---- The car guard ---------------------------------------------------------

/// Whether a frame's id means another node is on the bus: anything but the
/// request ids played and the functional id. The board sends on nothing else,
/// and this adapter does not hear its own frames.
fn is_foreign(id: u32, requests: &[u16]) -> bool {
	id != FUNCTIONAL && !requests.iter().any(|&r| u32::from(r) == id)
}

/// Why serving stopped, when it is not a clean end.
#[derive(Debug)]
enum Stop {
	Foreign(u32),
	Can(CanError),
}

/// Listen for `wait` without answering; any foreign frame refuses.
async fn listen<B: CanBackend>(backend: &mut B, requests: &[u16], wait: Duration) -> Result<(), Stop> {
	let deadline = Instant::now() + wait;
	loop {
		let remaining = deadline.saturating_duration_since(Instant::now());
		if remaining.is_zero() {
			return Ok(());
		}
		match backend.recv_frame(remaining).await {
			Ok((id, _)) if is_foreign(id, requests) => return Err(Stop::Foreign(id)),
			Ok(_) | Err(CanError::Timeout) => {}
			Err(e) => return Err(Stop::Can(e)),
		}
	}
}

// ---- ISO 15765-2 -----------------------------------------------------------

/// `bytes` (at most eight) as one padded frame.
fn padded(bytes: &[u8]) -> [u8; 8] {
	let mut frame = [PAD; 8];
	frame[..bytes.len()].copy_from_slice(bytes);
	frame
}

/// What one frame on a unit's request id comes to.
#[derive(Debug, PartialEq, Eq)]
enum Received {
	/// Part of a request, or a frame the unit ignores.
	Nothing,
	/// Send this flow control on the response id.
	FlowControl([u8; 8]),
	/// A whole request PDU.
	Request(Vec<u8>),
}

/// A multi-frame request being collected.
struct Partial {
	length: usize,
	data: Vec<u8>,
	next_sn: u8,
	last: Instant,
}

/// Request reassembly for one unit.
#[derive(Default)]
struct Reassembler {
	partial: Option<Partial>,
}

impl Reassembler {
	fn push(&mut self, frame: &[u8], now: Instant) -> Received {
		// N_Cr: a request whose next frame is this late is abandoned.
		if self.partial.as_ref().is_some_and(|p| now.duration_since(p.last) > N_TIMEOUT) {
			self.partial = None;
		}
		let Some(&pci) = frame.first() else {
			return Received::Nothing;
		};
		match pci >> 4 {
			// Single frame. A new request abandons one in progress.
			0x0 => {
				self.partial = None;
				let length = usize::from(pci & 0x0F);
				match frame.get(1..1 + length) {
					Some(pdu) if length > 0 => Received::Request(pdu.to_vec()),
					_ => Received::Nothing,
				}
			}
			// First frame.
			0x1 => {
				self.partial = None;
				if frame.len() < 8 {
					return Received::Nothing;
				}
				let length = usize::from(u16::from_be_bytes([pci & 0x0F, frame[1]]));
				if length == 0 {
					// The escape for a length past 4095: more than this unit takes.
					return Received::FlowControl(padded(&[0x32, 0x00, 0x00]));
				}
				if length <= 7 {
					// Would have fit a single frame: not a valid first frame.
					return Received::Nothing;
				}
				self.partial = Some(Partial {
					length,
					data: frame[2..8].to_vec(),
					next_sn: 1,
					last: now,
				});
				// Clear to send, no block limit, no gap.
				Received::FlowControl(padded(&[0x30, 0x00, 0x00]))
			}
			// Consecutive frame.
			0x2 => {
				let Some(partial) = self.partial.as_mut() else {
					return Received::Nothing;
				};
				if pci & 0x0F != partial.next_sn {
					self.partial = None;
					return Received::Nothing;
				}
				let take = (partial.length - partial.data.len()).min(7).min(frame.len() - 1);
				partial.data.extend_from_slice(&frame[1..1 + take]);
				partial.next_sn = (partial.next_sn + 1) & 0x0F;
				partial.last = now;
				if partial.data.len() < partial.length {
					return Received::Nothing;
				}
				self.partial.take().map_or(Received::Nothing, |p| Received::Request(p.data))
			}
			_ => Received::Nothing,
		}
	}
}

/// How a response goes out.
#[derive(Debug, PartialEq, Eq)]
enum Segments {
	Single([u8; 8]),
	Multi { first: [u8; 8], consecutive: Vec<[u8; 8]> },
}

/// Cut a response of at most [`MAX_PDU`] bytes into frames.
fn segment(pdu: &[u8]) -> Segments {
	if pdu.len() <= 7 {
		let mut frame = [PAD; 8];
		frame[0] = pdu.len() as u8;
		frame[1..=pdu.len()].copy_from_slice(pdu);
		return Segments::Single(frame);
	}
	let length = pdu.len() as u16;
	let mut first = [PAD; 8];
	first[0] = 0x10 | (length >> 8) as u8;
	first[1] = length as u8;
	first[2..].copy_from_slice(&pdu[..6]);
	let consecutive = pdu[6..]
		.chunks(7)
		.enumerate()
		.map(|(i, chunk)| {
			let mut frame = [PAD; 8];
			frame[0] = 0x20 | ((i + 1) & 0x0F) as u8;
			frame[1..=chunk.len()].copy_from_slice(chunk);
			frame
		})
		.collect();
	Segments::Multi { first, consecutive }
}

/// STmin byte → minimum gap between consecutive frames (ISO 15765-2 §9.6.5.4),
/// the same table `vag_uds_can::isotp` sends by.
fn stmin_gap(stmin: u8) -> Duration {
	match stmin {
		0x00..=0x7F => Duration::from_millis(u64::from(stmin)),
		0xF1..=0xF9 => Duration::from_micros(u64::from(stmin - 0xF0) * 100),
		// Reserved: the standard's answer is the longest gap.
		_ => Duration::from_millis(0x7F),
	}
}

#[derive(Debug, PartialEq, Eq)]
enum Transmit {
	Sent,
	/// The requester's flow control did not let it through.
	Abandoned(&'static str),
}

/// Wait for the requester's flow control on `from`: `(block size, STmin)` on
/// clear-to-send. Frames on other played ids meanwhile are dropped — one
/// conversation at a time.
async fn flow_control<B: CanBackend>(backend: &mut B, from: u32, requests: &[u16]) -> Result<Result<(u8, u8), &'static str>, Stop> {
	let mut waits = 0u8;
	let mut deadline = Instant::now() + N_TIMEOUT;
	loop {
		let remaining = deadline.saturating_duration_since(Instant::now());
		if remaining.is_zero() {
			return Ok(Err("no flow control"));
		}
		let (id, data) = match backend.recv_frame(remaining).await {
			Ok(frame) => frame,
			Err(CanError::Timeout) => return Ok(Err("no flow control")),
			Err(e) => return Err(Stop::Can(e)),
		};
		if is_foreign(id, requests) {
			return Err(Stop::Foreign(id));
		}
		if id != from || data.first().map(|pci| pci >> 4) != Some(0x3) {
			continue;
		}
		match data[0] & 0x0F {
			0x0 => return Ok(Ok((data.get(1).copied().unwrap_or(0), data.get(2).copied().unwrap_or(0)))),
			0x1 if waits < MAX_FC_WAIT => {
				waits += 1;
				deadline = Instant::now() + N_TIMEOUT;
			}
			0x1 => return Ok(Err("too many flow control waits")),
			0x2 => return Ok(Err("the requester's buffer overflowed")),
			_ => return Ok(Err("invalid flow status")),
		}
	}
}

/// Send one response from `unit`, honouring block size and STmin.
async fn transmit<B: CanBackend>(backend: &mut B, unit: UnitAddress, pdu: &[u8], requests: &[u16]) -> Result<Transmit, Stop> {
	let response = u32::from(unit.response);
	match segment(pdu) {
		Segments::Single(frame) => backend.send_frame(response, &frame).await.map_err(Stop::Can)?,
		Segments::Multi { first, consecutive } => {
			backend.send_frame(response, &first).await.map_err(Stop::Can)?;
			let mut rest = consecutive.as_slice();
			while !rest.is_empty() {
				let (block_size, stmin) = match flow_control(backend, u32::from(unit.request), requests).await? {
					Ok(clear) => clear,
					Err(why) => return Ok(Transmit::Abandoned(why)),
				};
				let block = match block_size {
					0 => rest.len(),
					n => usize::from(n).min(rest.len()),
				};
				let (now, later) = rest.split_at(block);
				for (i, frame) in now.iter().enumerate() {
					if i > 0 && stmin > 0 {
						tokio::time::sleep(stmin_gap(stmin)).await;
					}
					backend.send_frame(response, frame).await.map_err(Stop::Can)?;
				}
				rest = later;
			}
		}
	}
	Ok(Transmit::Sent)
}

// ---- UDS -------------------------------------------------------------------

fn negative(sid: u8, nrc: u8) -> Vec<u8> {
	vec![0x7F, sid, nrc]
}

/// The unit's answer to one request PDU; `None` for no answer.
fn answer(pdu: &[u8], speed_kmh: u8) -> Option<Vec<u8>> {
	let (&sid, rest) = pdu.split_first()?;
	match sid {
		0x22 => {
			if rest.is_empty() || !rest.len().is_multiple_of(2) {
				return Some(negative(sid, NRC_INCORRECT_LENGTH));
			}
			// Only what is served, in request order.
			let mut positive = vec![sid + 0x40];
			for did in rest.as_chunks::<2>().0.iter().map(|&p| u16::from_be_bytes(p)) {
				if did == DID_SPEED {
					positive.extend_from_slice(&did.to_be_bytes());
					positive.push(speed_kmh);
				}
			}
			Some(match positive.len() {
				1 => negative(sid, NRC_REQUEST_OUT_OF_RANGE),
				n if n > MAX_PDU => negative(sid, NRC_RESPONSE_TOO_LONG),
				_ => positive,
			})
		}
		0x3E => match rest {
			[0x00] => Some(vec![0x7E, 0x00]),
			// The suppress-positive-response bit.
			[0x80] => None,
			[_] => Some(negative(sid, NRC_SUBFUNCTION_NOT_SUPPORTED)),
			_ => Some(negative(sid, NRC_INCORRECT_LENGTH)),
		},
		// Sessions (10), fault memory (19) and every other service.
		_ => Some(negative(sid, NRC_SERVICE_NOT_SUPPORTED)),
	}
}

// ---- Counting --------------------------------------------------------------

/// What one request asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Asked {
	Did(u16),
	Service(u8),
}

impl fmt::Display for Asked {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Asked::Did(did) => write!(f, "{did:04X}"),
			Asked::Service(sid) => write!(f, "sid {sid:02X}"),
		}
	}
}

/// Each identifier a `22` names, or the service of anything else.
fn asked(pdu: &[u8]) -> Vec<Asked> {
	match pdu {
		[0x22, dids @ ..] if !dids.is_empty() && dids.len().is_multiple_of(2) => {
			dids.as_chunks::<2>().0.iter().map(|&p| Asked::Did(u16::from_be_bytes(p))).collect()
		}
		[sid, ..] => vec![Asked::Service(*sid)],
		[] => Vec::new(),
	}
}

#[derive(Default)]
struct Counts {
	window: BTreeMap<(u16, Asked), u32>,
	total: BTreeMap<(u16, Asked), u32>,
	/// When answering began; `None` while still listening.
	since: Option<Instant>,
}

impl Counts {
	fn saw(&mut self, unit: u16, pdu: &[u8]) {
		for key in asked(pdu).into_iter().map(|a| (unit, a)) {
			*self.window.entry(key).or_default() += 1;
			*self.total.entry(key).or_default() += 1;
		}
	}

	/// The second just ended, and a fresh window.
	fn window_line(&mut self) -> String {
		let line = if self.window.is_empty() {
			"no requests".to_string()
		} else {
			self
				.window
				.iter()
				.map(|((unit, a), n)| format!("{unit:03X} {a} {n}/s"))
				.collect::<Vec<_>>()
				.join("  ")
		};
		self.window.clear();
		line
	}

	fn total_lines(&self) -> Vec<String> {
		let Some(since) = self.since else {
			return vec!["total: never answered".to_string()];
		};
		let over = since.elapsed().as_secs_f64();
		let mut lines = vec![format!("total over {over:.1} s:")];
		if self.total.is_empty() {
			lines.push("  no requests".to_string());
		}
		for ((unit, a), n) in &self.total {
			lines.push(format!("  {unit:03X} {a} {n} ({:.1}/s)", f64::from(*n) / over.max(f64::EPSILON)));
		}
		lines
	}
}

// ---- Serving ---------------------------------------------------------------

/// Answer as `config.units` until `config.seconds` pass, printing once a second.
async fn serve<B: CanBackend>(backend: &mut B, config: &Config, counts: &mut Counts) -> Result<(), Stop> {
	let requests: Vec<u16> = config.units.iter().map(|u| u.request).collect();
	let mut reassemblers: Vec<Reassembler> = config.units.iter().map(|_| Reassembler::default()).collect();
	let started = Instant::now();
	counts.since = Some(started);
	let until = config.seconds.map(|s| started + Duration::from_secs(s));
	let mut second = 1u64;
	loop {
		let now = Instant::now();
		if until.is_some_and(|u| now >= u) {
			return Ok(());
		}
		let tick = started + Duration::from_secs(second);
		if now >= tick {
			println!("{second:>5} s  {}", counts.window_line());
			second += 1;
			continue;
		}
		let wait = until.map_or(tick, |u| u.min(tick)).saturating_duration_since(now);
		let (id, data) = match backend.recv_frame(wait).await {
			Ok(frame) => frame,
			Err(CanError::Timeout) => continue,
			Err(e) => return Err(Stop::Can(e)),
		};
		if is_foreign(id, &requests) {
			return Err(Stop::Foreign(id));
		}
		// `7DF` has no unit here: tolerated, not answered.
		let Some(at) = config.units.iter().position(|u| u32::from(u.request) == id) else {
			continue;
		};
		let unit = config.units[at];
		match reassemblers[at].push(&data, Instant::now()) {
			Received::Nothing => {}
			Received::FlowControl(frame) => backend.send_frame(u32::from(unit.response), &frame).await.map_err(Stop::Can)?,
			Received::Request(pdu) => {
				counts.saw(unit.request, &pdu);
				if let Some(reply) = answer(&pdu, config.speed_kmh)
					&& let Transmit::Abandoned(why) = transmit(backend, unit, &reply, &requests).await?
				{
					println!("{:03X} response abandoned: {why}", unit.request);
				}
			}
		}
	}
}

#[tokio::main]
async fn main() -> Result<()> {
	let args: Vec<String> = std::env::args().skip(1).collect();
	let config = match parse(&args) {
		Ok(Invocation::Help) => {
			println!("{USAGE}");
			return Ok(());
		}
		Ok(Invocation::Refused) => bail!("{REFUSAL}"),
		Ok(Invocation::Run(config)) => config,
		Err(e) => bail!("{e}\n\n{USAGE}"),
	};
	let requests: Vec<u16> = config.units.iter().map(|u| u.request).collect();
	let playing: Vec<String> = config.units.iter().map(|u| format!("{:03X}→{:03X}", u.request, u.response)).collect();

	let mut backend = SlcanBackend::open_mode(&config.device, ADAPTER_BAUD, SlcanBitrate::Rate500k, SlcanMode::Normal)
		.await
		.with_context(|| format!("opening {}", config.device))?;
	println!("benchecu on {} — bench pair only, never on a car", config.device);
	println!("listening {} s for other nodes before answering", LISTEN.as_secs());

	let mut counts = Counts::default();
	let run = async {
		listen(&mut backend, &requests, LISTEN).await?;
		println!("answering as {}, F40D = {} km/h", playing.join(" "), config.speed_kmh);
		serve(&mut backend, &config, &mut counts).await
	};
	let result = tokio::select! {
		result = run => result,
		_ = tokio::signal::ctrl_c() => Ok(()),
	};
	// Off the bus first: on a car, the adapter stops acknowledging at once.
	backend.close_channel().await.ok();
	for line in counts.total_lines() {
		println!("{line}");
	}
	match result {
		Ok(()) => Ok(()),
		Err(Stop::Foreign(id)) => bail!("{CAR} (a frame on {id:03X})"),
		Err(Stop::Can(e)) => bail!("the adapter: {e}"),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::collections::VecDeque;

	/// Frames in from a script; frames out recorded. An empty script waits out
	/// the timeout, as a quiet bus does.
	#[derive(Default)]
	struct Fake {
		incoming: VecDeque<(u32, Vec<u8>)>,
		sent: Vec<(u32, Vec<u8>)>,
	}

	impl CanBackend for Fake {
		async fn send_frame(&mut self, id: u32, data: &[u8]) -> Result<(), CanError> {
			self.sent.push((id, data.to_vec()));
			Ok(())
		}

		async fn recv_frame(&mut self, timeout: Duration) -> Result<(u32, Vec<u8>), CanError> {
			match self.incoming.pop_front() {
				Some(frame) => Ok(frame),
				None => {
					tokio::time::sleep(timeout).await;
					Err(CanError::Timeout)
				}
			}
		}
	}

	fn args(text: &str) -> Vec<String> {
		text.split_whitespace().map(str::to_string).collect()
	}

	fn engine() -> UnitAddress {
		UnitAddress::from_request(0x7E0).unwrap()
	}

	#[test]
	fn nothing_runs_without_bench() {
		assert_eq!(parse(&args("--device /dev/x --unit 7E0")), Ok(Invocation::Refused));
		assert_eq!(parse(&args("--help")), Ok(Invocation::Help));
		let Ok(Invocation::Run(config)) = parse(&args(
			"--bench --device /dev/x --unit 7E0 --unit 714 --unit 7E0 --speed-kmh 42 --seconds 5",
		)) else {
			panic!("a full command line runs");
		};
		assert_eq!(
			config.units.iter().map(|u| (u.request, u.response)).collect::<Vec<_>>(),
			vec![(0x7E0, 0x7E8), (0x714, 0x77E)]
		);
		assert_eq!((config.speed_kmh, config.seconds), (42, Some(5)));
		assert!(parse(&args("--bench --device /dev/x --unit 123")).is_err());
		assert!(parse(&args("--bench --unit 7E0")).is_err());
		assert!(parse(&args("--bench --device /dev/x")).is_err());
		assert!(parse(&args("--bench --device /dev/x --unit 7E0 --speed-kmh 256")).is_err());
	}

	#[test]
	fn the_guard_passes_played_ids_and_functional_and_nothing_else() {
		let requests = [0x7E0, 0x714];
		let first_foreign = |ids: &[u32]| ids.iter().copied().find(|&id| is_foreign(id, &requests));
		assert_eq!(first_foreign(&[0x7E0, 0x7DF, 0x714, 0x7E0]), None);
		// The engine answering: another node.
		assert_eq!(first_foreign(&[0x7E0, 0x7E8, 0x7E0]), Some(0x7E8));
		// Everyday car traffic.
		assert_eq!(first_foreign(&[0x7DF, 0x280]), Some(0x280));
		// A played id as an extended frame is not that id.
		assert_eq!(
			first_foreign(&[0x7E0 | vag_uds_can::CAN_EFF_FLAG]),
			Some(0x7E0 | vag_uds_can::CAN_EFF_FLAG)
		);
	}

	#[tokio::test]
	async fn listening_refuses_on_a_foreign_frame() {
		let mut bus = Fake::default();
		bus.incoming.extend([(0x7E0, vec![0x03, 0x22, 0xF4, 0x0D]), (0x1A0, vec![0; 8])]);
		assert!(matches!(
			listen(&mut bus, &[0x7E0], Duration::from_millis(50)).await,
			Err(Stop::Foreign(0x1A0))
		));
		assert!(bus.sent.is_empty(), "nothing is answered while listening");

		let mut quiet = Fake::default();
		quiet.incoming.push_back((0x7E0, vec![0x02, 0x3E, 0x80]));
		assert!(listen(&mut quiet, &[0x7E0], Duration::from_millis(50)).await.is_ok());
	}

	#[test]
	fn a_single_frame_request_is_whole() {
		let mut r = Reassembler::default();
		let now = Instant::now();
		assert_eq!(
			r.push(&[0x03, 0x22, 0xF4, 0x0D, PAD, PAD, PAD, PAD], now),
			Received::Request(vec![0x22, 0xF4, 0x0D])
		);
		assert_eq!(r.push(&[0x00, PAD, PAD, PAD, PAD, PAD, PAD, PAD], now), Received::Nothing);
		assert_eq!(r.push(&[0x05, 0x22, 0xF4], now), Received::Nothing, "shorter than its length");
	}

	#[test]
	fn a_multi_frame_request_is_reassembled_after_flow_control() {
		let pdu = [0x22, 0xF1, 0x87, 0xF4, 0x0D, 0xF1, 0x90, 0xF4, 0x0D, 0xF1, 0x87, 0xF4, 0x0D, 0xF4, 0x0D];
		let mut r = Reassembler::default();
		let now = Instant::now();
		assert_eq!(
			r.push(&[0x10, 0x0F, 0x22, 0xF1, 0x87, 0xF4, 0x0D, 0xF1], now),
			Received::FlowControl([0x30, 0x00, 0x00, PAD, PAD, PAD, PAD, PAD])
		);
		assert_eq!(r.push(&[0x21, 0x90, 0xF4, 0x0D, 0xF1, 0x87, 0xF4, 0x0D], now), Received::Nothing);
		assert_eq!(r.push(&[0x22, 0xF4, 0x0D, PAD, PAD, PAD, PAD, PAD], now), Received::Request(pdu.to_vec()));
	}

	#[test]
	fn a_request_out_of_sequence_or_late_is_dropped() {
		let now = Instant::now();
		let mut r = Reassembler::default();
		r.push(&[0x10, 0x0A, 1, 2, 3, 4, 5, 6], now);
		assert_eq!(
			r.push(&[0x22, 7, 8, 9, 10, PAD, PAD, PAD], now),
			Received::Nothing,
			"wrong sequence number"
		);
		assert_eq!(
			r.push(&[0x21, 7, 8, 9, 10, PAD, PAD, PAD], now),
			Received::Nothing,
			"and the request is gone"
		);

		r.push(&[0x10, 0x0A, 1, 2, 3, 4, 5, 6], now);
		assert_eq!(
			r.push(&[0x21, 7, 8, 9, 10, PAD, PAD, PAD], now + N_TIMEOUT + Duration::from_millis(1)),
			Received::Nothing
		);
	}

	#[test]
	fn a_first_frame_past_4095_bytes_gets_overflow() {
		let mut r = Reassembler::default();
		assert_eq!(
			r.push(&[0x10, 0x00, 0x00, 0x00, 0x10, 0x00, 0x22, 0xF4], Instant::now()),
			Received::FlowControl(padded(&[0x32, 0x00, 0x00]))
		);
	}

	#[test]
	fn a_response_is_segmented_and_padded() {
		assert_eq!(
			segment(&[0x62, 0xF4, 0x0D, 0x2A]),
			Segments::Single([0x04, 0x62, 0xF4, 0x0D, 0x2A, PAD, PAD, PAD])
		);
		let pdu: Vec<u8> = (0..22).collect();
		let Segments::Multi { first, consecutive } = segment(&pdu) else {
			panic!("22 bytes are multi-frame");
		};
		assert_eq!(first, [0x10, 22, 0, 1, 2, 3, 4, 5]);
		assert_eq!(
			consecutive,
			vec![
				[0x21, 6, 7, 8, 9, 10, 11, 12],
				[0x22, 13, 14, 15, 16, 17, 18, 19],
				[0x23, 20, 21, PAD, PAD, PAD, PAD, PAD]
			]
		);
	}

	#[test]
	fn stmin_follows_the_standard_table() {
		assert_eq!(stmin_gap(0x0A), Duration::from_millis(10));
		assert_eq!(stmin_gap(0xF3), Duration::from_micros(300));
		assert_eq!(stmin_gap(0x80), Duration::from_millis(127));
	}

	#[tokio::test]
	async fn consecutive_frames_wait_for_flow_control_per_block() {
		let pdu: Vec<u8> = (0..22).collect();
		let mut bus = Fake::default();
		// Block of two, 1 ms apart; then a wait; then the rest.
		bus.incoming.extend([
			(0x7E0, padded(&[0x30, 0x02, 0x01]).to_vec()),
			(0x7E0, padded(&[0x31, 0x00, 0x00]).to_vec()),
			(0x7E0, padded(&[0x30, 0x00, 0x00]).to_vec()),
		]);
		assert_eq!(transmit(&mut bus, engine(), &pdu, &[0x7E0]).await.unwrap(), Transmit::Sent);
		let pcis: Vec<(u32, u8)> = bus.sent.iter().map(|(id, f)| (*id, f[0])).collect();
		assert_eq!(pcis, vec![(0x7E8, 0x10), (0x7E8, 0x21), (0x7E8, 0x22), (0x7E8, 0x23)]);
		assert!(bus.incoming.is_empty(), "a flow control was read before each block");

		let mut refused = Fake::default();
		refused.incoming.push_back((0x7E0, padded(&[0x32, 0x00, 0x00]).to_vec()));
		assert!(matches!(
			transmit(&mut refused, engine(), &pdu, &[0x7E0]).await.unwrap(),
			Transmit::Abandoned(_)
		));
		assert_eq!(refused.sent.len(), 1, "only the first frame");

		let mut car = Fake::default();
		car.incoming.push_back((0x7E8, padded(&[0x30, 0x00, 0x00]).to_vec()));
		assert!(matches!(transmit(&mut car, engine(), &pdu, &[0x7E0]).await, Err(Stop::Foreign(0x7E8))));
	}

	#[test]
	fn the_answer_table() {
		assert_eq!(answer(&[0x22, 0xF1, 0x87, 0xF4, 0x0D], 42), Some(vec![0x62, 0xF4, 0x0D, 42]));
		assert_eq!(
			answer(&[0x22, 0xF4, 0x0D, 0xF1, 0x87, 0xF4, 0x0D], 7),
			Some(vec![0x62, 0xF4, 0x0D, 7, 0xF4, 0x0D, 7])
		);
		assert_eq!(answer(&[0x22, 0xF1, 0x87], 42), Some(vec![0x7F, 0x22, 0x31]));
		assert_eq!(answer(&[0x22, 0xF4], 42), Some(vec![0x7F, 0x22, 0x13]));
		assert_eq!(answer(&[0x3E, 0x00], 0), Some(vec![0x7E, 0x00]));
		assert_eq!(answer(&[0x3E, 0x80], 0), None);
		assert_eq!(answer(&[0x10, 0x03], 0), Some(vec![0x7F, 0x10, 0x11]));
		assert_eq!(answer(&[0x19, 0x02, 0xFF], 0), Some(vec![0x7F, 0x19, 0x11]));
		assert_eq!(answer(&[0x2E, 0xF1, 0x90, 0x00], 0), Some(vec![0x7F, 0x2E, 0x11]));
		assert_eq!(answer(&[], 0), None);
	}

	#[test]
	fn requests_are_counted_per_identifier() {
		let mut counts = Counts::default();
		counts.saw(0x7E0, &[0x22, 0xF1, 0x87, 0xF4, 0x0D]);
		counts.saw(0x7E0, &[0x22, 0xF4, 0x0D]);
		counts.saw(0x7E0, &[0x3E, 0x80]);
		assert_eq!(counts.window_line(), "7E0 F187 1/s  7E0 F40D 2/s  7E0 sid 3E 1/s");
		assert_eq!(counts.window_line(), "no requests");
		assert_eq!(counts.total[&(0x7E0, Asked::Did(0xF40D))], 2);
	}

	#[tokio::test]
	async fn the_loop_answers_a_request_and_stops_on_another_node() {
		let config = Config {
			device: String::new(),
			units: vec![engine()],
			speed_kmh: 42,
			seconds: None,
		};
		let mut bus = Fake::default();
		bus.incoming.extend([
			(0x7E0, padded(&[0x03, 0x22, 0xF4, 0x0D]).to_vec()),
			(0x7DF, padded(&[0x02, 0x01, 0x0D]).to_vec()),
			(0x7E8, padded(&[0x04, 0x62, 0xF4, 0x0D, 0x00]).to_vec()),
		]);
		let mut counts = Counts::default();
		assert!(matches!(serve(&mut bus, &config, &mut counts).await, Err(Stop::Foreign(0x7E8))));
		assert_eq!(
			bus.sent,
			vec![(0x7E8, vec![0x04, 0x62, 0xF4, 0x0D, 42, PAD, PAD, PAD])],
			"7DF is not answered"
		);
		assert_eq!(counts.total[&(0x7E0, Asked::Did(0xF40D))], 1);

		let timed = Config { seconds: Some(0), ..config };
		let mut quiet = Fake::default();
		assert!(serve(&mut quiet, &timed, &mut Counts::default()).await.is_ok(), "--seconds ends the run");
	}
}
