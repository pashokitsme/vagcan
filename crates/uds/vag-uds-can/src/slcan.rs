//! slcan (LAWICEL) ASCII framing over a serial byte stream.
//!
//! The codec and the stream-generic [`SlcanBackend`] always build; only the
//! real-serial-port constructor needs the `slcan` feature (tokio-serial).

use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::time::Instant;

use vag_uds_transport::link::{self, HelloReply, Message};

use crate::CanError;
use crate::backend::{CAN_EFF_FLAG, CAN_EFF_MASK, CAN_SFF_MASK, CanBackend};
use crate::board::{HelloScan, Scanned};

/// CAN bitrate presets (`Sn` slcan setup command). VAG diagnostic CAN is 500k.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlcanBitrate {
	Rate10k = 0,
	Rate20k = 1,
	Rate50k = 2,
	Rate100k = 3,
	Rate125k = 4,
	Rate250k = 5,
	Rate500k = 6,
	Rate800k = 7,
	Rate1m = 8,
}

/// How the adapter drives the bus once the channel opens (`Mn` command).
///
/// [`Silent`](SlcanMode::Silent) is listen-only: the controller receives but
/// never writes a bit — not even an acknowledge — so it cannot disturb a bus
/// another tester is using. That is the mode for sniffing a live VCDS session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SlcanMode {
	/// Normal: receives, acknowledges, and may transmit.
	#[default]
	Normal = 0,
	/// Listen-only ("silent"): receives only, never drives the bus.
	Silent = 1,
}

/// Encode one classic CAN frame as an slcan ASCII line (with trailing CR).
pub fn encode_frame(id: u32, data: &[u8]) -> Result<String, CanError> {
	if data.len() > 8 {
		return Err(CanError::Unsupported("classic CAN frame data must be <= 8 bytes"));
	}
	use std::fmt::Write;
	let mut line = if id & CAN_EFF_FLAG != 0 {
		format!("T{:08X}{:X}", id & CAN_EFF_MASK, data.len())
	} else {
		format!("t{:03X}{:X}", id & CAN_SFF_MASK, data.len())
	};
	// Into the line directly rather than a fresh `String` per byte — this is on
	// the send path of every frame, and a sweep sends thousands.
	for b in data {
		let _ = write!(line, "{b:02X}");
	}
	line.push('\r');
	Ok(line)
}

fn hex_field(s: &str, range: std::ops::Range<usize>) -> Result<u32, CanError> {
	let field = s
		.get(range)
		.ok_or_else(|| CanError::MalformedFrame(format!("slcan line too short: {s:?}")))?;
	u32::from_str_radix(field, 16).map_err(|_| CanError::MalformedFrame(format!("bad hex {field:?} in {s:?}")))
}

/// Decode one slcan ASCII line (`t...`/`T...`) into `(raw_id, data)`.
///
/// Trailing bytes after the data field (e.g. adapter timestamps) are ignored.
pub fn decode_frame(line: &str) -> Result<(u32, Vec<u8>), CanError> {
	let line = line.trim_end_matches(['\r', '\n']);
	let (id, dlc_pos) = match line.as_bytes().first() {
		Some(b't') => (hex_field(line, 1..4)?, 4),
		Some(b'T') => (hex_field(line, 1..9)? | CAN_EFF_FLAG, 9),
		_ => return Err(CanError::MalformedFrame(format!("not an slcan frame: {line:?}"))),
	};
	let dlc = hex_field(line, dlc_pos..dlc_pos + 1)? as usize;
	if dlc > 8 {
		return Err(CanError::MalformedFrame(format!("dlc {dlc} > 8 in {line:?}")));
	}
	let mut data = Vec::with_capacity(dlc);
	for i in 0..dlc {
		let at = dlc_pos + 1 + i * 2;
		data.push(hex_field(line, at..at + 2)? as u8);
	}
	Ok((id, data))
}

/// slcan backend over any async byte stream (serial port, `tokio::io::duplex`
/// in tests). Skips non-frame lines (command acks, status) on receive.
pub struct SlcanBackend<S> {
	stream: S,
	buf: Vec<u8>,
	/// Frames read while waiting for a command's reply, with when each was
	/// read, handed out by [`CanBackend::recv_frame`] before anything new — see
	/// [`Self::status_flags`].
	pending: std::collections::VecDeque<KeptFrame>,
}

/// A frame kept through a command's wait: when it was read, and what it decoded to.
type KeptFrame = (std::time::Instant, Result<(u32, Vec<u8>), CanError>);

impl<S: AsyncRead + AsyncWrite + Unpin + Send> SlcanBackend<S> {
	pub fn new(stream: S) -> Self {
		SlcanBackend {
			stream,
			buf: Vec::new(),
			pending: std::collections::VecDeque::new(),
		}
	}

	/// Send the channel-open sequence in [`SlcanMode::Normal`] — see
	/// [`Self::open_channel_mode`].
	pub async fn open_channel(&mut self, bitrate: SlcanBitrate) -> Result<(), CanError> {
		self.open_channel_mode(bitrate, SlcanMode::Normal).await
	}

	/// Send the channel-open sequence: close, set bitrate, set the bus mode,
	/// open. Fire-and-forget: many adapters NAK a redundant `C` (and the
	/// CANable2 firmware acks nothing at all), so acks are not checked.
	///
	/// **The mode command must precede `O`.** The controller only accepts mode
	/// configuration while it is in init state; after the channel opens the
	/// registers are locked and a later `M` is silently ignored.
	///
	/// The mode is sent explicitly on *every* open, including
	/// [`SlcanMode::Normal`]. The adapter keeps its silent flag across a
	/// close/open cycle, so a normal open that omitted `M0` would inherit
	/// listen-only from an earlier sniffing run and transmit nothing.
	pub async fn open_channel_mode(&mut self, bitrate: SlcanBitrate, mode: SlcanMode) -> Result<(), CanError> {
		let cmd = format!("C\rS{}\rM{}\rO\r", bitrate as u8, mode as u8);
		self.write_all(cmd.as_bytes()).await
	}

	/// Ask the adapter for its Lawicel status flags (`F`): `Some(bits)` from an
	/// `Fxx` reply within `wait`, `None` when none came.
	///
	/// **`None` is "not known", never "no flags".** The CANable's firmware has
	/// no `F` and answers nothing; the vag-dash board's `slcan` image answers,
	/// and counts frames its ring could not hold into bit 3 (data overrun) with
	/// bit 0 (receive queue full). The bits clear on read.
	///
	/// Safe on an open channel: frames that arrive while the reply is awaited
	/// are kept, in order, and [`CanBackend::recv_frame`] returns them before
	/// reading anything new — asking must not cost the capture a frame. They are
	/// handed out after the wait, so a caller that timestamps frames must take
	/// their time from [`Self::recv_frame_arrived`], not from the clock.
	pub async fn status_flags(&mut self, wait: Duration) -> Result<Option<u8>, CanError> {
		self.write_all(b"F\r").await?;
		let deadline = Instant::now() + wait;
		loop {
			let line = match self.read_line(deadline).await {
				Ok(line) => line,
				Err(CanError::Timeout) => return Ok(None),
				Err(e) => return Err(e),
			};
			let text = String::from_utf8_lossy(&line);
			let text = text.trim_matches(|c: char| c == '\u{7}' || c.is_whitespace());
			match text.as_bytes() {
				[b't' | b'T', ..] => self.pending.push_back((std::time::Instant::now(), decode_frame(text))),
				[b'F', hi, lo] if hi.is_ascii_hexdigit() && lo.is_ascii_hexdigit() => {
					return Ok(u8::from_str_radix(&text[1..], 16).ok());
				}
				// Acks and other replies are not the answer, and not bus traffic.
				_ => continue,
			}
		}
	}

	/// [`CanBackend::recv_frame`], with when the frame was read off the port.
	///
	/// For a frame read now that is now; for one kept while [`Self::status_flags`]
	/// waited it is when it was kept, up to that whole wait earlier. A capture
	/// stamped at hand-out would squeeze those frames into one instant.
	pub async fn recv_frame_arrived(&mut self, timeout: Duration) -> Result<(std::time::Instant, u32, Vec<u8>), CanError> {
		if let Some((arrived, frame)) = self.pending.pop_front() {
			return frame.map(|(id, data)| (arrived, id, data));
		}
		let deadline = Instant::now() + timeout;
		loop {
			let line = self.read_line(deadline).await?;
			// Strip stray BEL (error ack) bytes; they are not CR-terminated.
			let text = String::from_utf8_lossy(&line);
			let text = text.trim_matches(|c: char| c == '\u{7}' || c.is_whitespace());
			match text.as_bytes().first() {
				Some(b't' | b'T') => return decode_frame(text).map(|(id, data)| (std::time::Instant::now(), id, data)),
				// Command acks ('z', 'Z', version/status replies) and empty
				// lines are not bus traffic — skip them.
				_ => continue,
			}
		}
	}

	/// Send the channel-close command.
	pub async fn close_channel(&mut self) -> Result<(), CanError> {
		self.write_all(b"C\r").await
	}

	async fn write_all(&mut self, bytes: &[u8]) -> Result<(), CanError> {
		self.stream.write_all(bytes).await.map_err(|e| CanError::Io(e.to_string()))?;
		self.stream.flush().await.map_err(|e| CanError::Io(e.to_string()))
	}

	/// Next CR-terminated line (without the CR), reading more bytes as needed.
	async fn read_line(&mut self, deadline: Instant) -> Result<Vec<u8>, CanError> {
		loop {
			if let Some(pos) = self.buf.iter().position(|&b| b == b'\r') {
				let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
				line.pop(); // drop the CR
				return Ok(line);
			}
			let remaining = deadline.saturating_duration_since(Instant::now());
			if remaining.is_zero() {
				return Err(CanError::Timeout);
			}
			let mut chunk = [0u8; 256];
			let n = tokio::time::timeout(remaining, self.stream.read(&mut chunk))
				.await
				.map_err(|_| CanError::Timeout)?
				.map_err(|e| CanError::Io(e.to_string()))?;
			if n == 0 {
				return Err(CanError::Disconnected);
			}
			self.buf.extend_from_slice(&chunk[..n]);
		}
	}
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> CanBackend for SlcanBackend<S> {
	async fn send_frame(&mut self, id: u32, data: &[u8]) -> Result<(), CanError> {
		let line = encode_frame(id, data)?;
		self.write_all(line.as_bytes()).await
	}

	async fn recv_frame(&mut self, timeout: Duration) -> Result<(u32, Vec<u8>), CanError> {
		self.recv_frame_arrived(timeout).await.map(|(_, id, data)| (id, data))
	}
}

/// A serial device that could be a CAN adapter.
#[cfg(feature = "slcan")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterInfo {
	/// Device path to open (e.g. `/dev/cu.usbmodem…`).
	pub path: String,
	/// Human description: product string, or the USB ids when there is none.
	pub description: String,
	/// True when the USB ids match an adapter we know speaks slcan.
	pub known: bool,
	/// True for the vag-dash board's own USB ([`BOARD_USB`]).
	pub board: bool,
}

/// Espressif's USB-Serial-JTAG, the ESP32-C3's own USB, which the vag-dash
/// board enumerates under whatever image it runs.
#[cfg(feature = "slcan")]
pub const BOARD_USB: (u16, u16) = (0x303a, 0x1001);

/// USB ids of adapters known to run slcan firmware. Used only to *rank*
/// candidates — an unknown device is still offered, since plenty of adapters
/// speak slcan under other ids.
#[cfg(feature = "slcan")]
const KNOWN_ADAPTERS: &[(u16, u16, &str)] = &[
	(0x16d0, 0x117e, "CANable 2.0 (slcan)"),
	(0x16d0, 0x117f, "CANable (slcan)"),
	(0xad50, 0x60c4, "CANable (slcan, older)"),
	// Not here: the vag-dash board, `BOARD_USB`. Its ids are Espressif's
	// USB-Serial-JTAG, which every ESP32-C3 and -S3 enumerates under whatever
	// it runs, and only the board's `dash` and `slcan` images read a car. Ids alone
	// would mark every ESP32 on the desk a CAN adapter; `vag_cli_core::device` asks
	// each such port Hello and then `V` ([`probe_board`]) and only a reply makes it one.
];

/// List serial devices that plausibly are CAN adapters, known ones first.
///
/// Bluetooth and console ports are filtered out: they are serial devices, but
/// offering them as CAN adapters only invites picking one by mistake.
#[cfg(feature = "slcan")]
pub fn list_adapters() -> Result<Vec<AdapterInfo>, CanError> {
	use tokio_serial::{SerialPortType, available_ports};

	let ports = available_ports().map_err(|e| CanError::Io(e.to_string()))?;
	let mut out = Vec::new();
	for port in ports {
		// macOS exposes both `tty.*` (blocking, for incoming calls) and `cu.*`
		// (call-out) nodes for one device; `cu.*` is the one to open.
		if port.port_name.contains("/tty.") {
			continue;
		}
		// USB only. Bluetooth serial ports are also "serial ports" — a paired
		// pair of headphones shows up as one — and offering those as CAN
		// adapters just invites picking the wrong device. Filtering by port
		// TYPE catches them all; filtering by name does not, since they are
		// named after the peripheral.
		let SerialPortType::UsbPort(usb) = &port.port_type else {
			continue;
		};
		out.push(classify_usb(port.port_name, usb.vid, usb.pid, usb.product.clone()));
	}
	// Known adapters first, then alphabetically, so the default pick is stable.
	out.sort_by(|a, b| b.known.cmp(&a.known).then_with(|| a.path.cmp(&b.path)));
	Ok(out)
}

/// One USB serial port as the listing presents it.
#[cfg(feature = "slcan")]
pub fn classify_usb(path: String, vid: u16, pid: u16, product: Option<String>) -> AdapterInfo {
	let known = KNOWN_ADAPTERS.iter().find(|(v, p, _)| *v == vid && *p == pid);
	let description = match known {
		Some((_, _, name)) => (*name).to_string(),
		None => product.unwrap_or_else(|| format!("USB {vid:04x}:{pid:04x}")),
	};
	AdapterInfo {
		path,
		description,
		known: known.is_some(),
		board: (vid, pid) == BOARD_USB,
	}
}

/// Whether the port answers slcan's version query (`V`) with a well-formed
/// version line within `wait`.
///
/// Sends `V\r` and nothing else — no close, no open, nothing that reaches a
/// bus — then reads until `wait` runs out, looking for a whole line that is
/// Lawicel's `Vhhss` (`V` and four hex digits: hardware and software version).
/// Lines before it are skipped: an slcan image with a channel left open queues
/// bus frames ahead of the reply. The `dash` display image ignores the query
/// and prints frames and log lines, none of which is such a line.
///
/// A read that times out is not an error here, only "nothing yet"; any other
/// read error ends the probe as "no answer".
pub fn answers_version<P: std::io::Read + std::io::Write>(port: &mut P, wait: Duration) -> bool {
	let deadline = std::time::Instant::now() + wait;
	if port.write_all(b"V\r").and_then(|()| port.flush()).is_err() {
		return false;
	}
	let mut pending: Vec<u8> = Vec::new();
	let mut chunk = [0u8; 256];
	while std::time::Instant::now() < deadline {
		match port.read(&mut chunk) {
			Ok(0) => return false,
			Ok(n) => pending.extend_from_slice(&chunk[..n]),
			Err(e)
				if matches!(
					e.kind(),
					std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
				) =>
			{
				continue;
			}
			Err(_) => return false,
		}
		while let Some(end) = pending.iter().position(|&b| b == b'\r' || b == b'\n') {
			let line: Vec<u8> = pending.drain(..=end).collect();
			// A BEL is an error reply with no CR of its own; it may prefix a line.
			let line = line.trim_ascii();
			let line = &line[line.iter().take_while(|&&b| b == 0x07).count()..];
			if is_version_reply(line) {
				return true;
			}
		}
	}
	false
}

/// `V` followed by exactly four hex digits.
fn is_version_reply(line: &[u8]) -> bool {
	matches!(line, [b'V', digits @ ..] if digits.len() == 4 && digits.iter().all(u8::is_ascii_hexdigit))
}

/// What a port made of a framed Hello ([`answers_hello`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelloAnswer {
	/// The board's HelloReply: an image that speaks the link — `dash`.
	Reply(HelloReply),
	/// Framed bytes came back — a message, or a frame that did not reassemble — but no
	/// reply, even to a second Hello. Something on the port speaks the link.
	Framed,
	/// Nothing framed at all.
	Silent,
}

/// Whether the port answers a framed Hello ([`vag_uds_transport::link`]) within `wait`.
///
/// Sends the Hello and reads until `wait` runs out or the reply arrives. The display
/// image's text and an slcan image's frames are text to the link and skipped. The first
/// time framed bytes come back without the reply — stale readings for an earlier host, a
/// half frame — the Hello is sent once more, within the same wait, with the scanner reset.
///
/// A read that times out is only "nothing yet"; any other read error ends the probe with
/// what was seen so far.
pub fn answers_hello<P: std::io::Read + std::io::Write>(port: &mut P, wait: Duration) -> HelloAnswer {
	let deadline = std::time::Instant::now() + wait;
	let Ok(hello) = link::encode(&Message::Hello) else {
		return HelloAnswer::Silent;
	};
	let say_hello = |port: &mut P| port.write_all(&hello).and_then(|()| port.flush()).is_ok();
	if !say_hello(port) {
		return HelloAnswer::Silent;
	}
	let mut scan = HelloScan::default();
	let (mut framed, mut asked_again) = (false, false);
	let mut chunk = [0u8; 256];
	while std::time::Instant::now() < deadline {
		let n = match port.read(&mut chunk) {
			Ok(0) => break,
			Ok(n) => n,
			Err(e) if timed_out(&e) => continue,
			Err(_) => break,
		};
		match scan.push(&chunk[..n]) {
			Scanned::Reply { reply, .. } => return HelloAnswer::Reply(reply),
			Scanned::Nothing { framed: true, .. } => {
				framed = true;
				if !asked_again {
					asked_again = true;
					scan.reset();
					if !say_hello(port) {
						break;
					}
				}
			}
			Scanned::Nothing { .. } => {}
		}
	}
	if framed { HelloAnswer::Framed } else { HelloAnswer::Silent }
}

/// A read error that only means nothing arrived yet.
fn timed_out(e: &std::io::Error) -> bool {
	matches!(
		e.kind(),
		std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
	)
}

/// What a vag-dash board said when asked which image it runs ([`ask_board`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardAnswer {
	/// The `dash` image: it answered Hello, and reads the car through the link with the
	/// panel still running. `version` is what it named, or `unknown` when framed bytes
	/// came back but no reply.
	Dash { version: String },
	/// A well-formed `V` reply to slcan's version query: the `slcan` image, an adapter.
	Slcan,
	/// Neither: an older display image, or anything else.
	Silent,
	/// The port would not open to ask, with the reason — usually another
	/// program holding it.
	Unopened(String),
}

/// Ask an open board port which image it runs: Hello first ([`answers_hello`]), and slcan's
/// `V` ([`answers_version`]) only when nothing framed came back.
///
/// **A `V` never goes to a board that speaks the link.** It is an slcan command line,
/// and the `dash` image takes one as the switch into its adapter mode, blanking the
/// panel. So framed bytes without a reply still make the answer `Dash`.
///
/// The `V` is sent after a CR of its own: an slcan image took the Hello's bytes as the
/// start of a line, and would read `V` as the end of that line rather than a command.
pub fn ask_board<P: std::io::Read + std::io::Write>(port: &mut P, wait: Duration) -> BoardAnswer {
	match answers_hello(port, wait) {
		HelloAnswer::Reply(reply) => BoardAnswer::Dash { version: reply.version },
		HelloAnswer::Framed => BoardAnswer::Dash { version: "unknown".into() },
		HelloAnswer::Silent => {
			if port.write_all(b"\r").and_then(|()| port.flush()).is_err() {
				return BoardAnswer::Silent;
			}
			match answers_version(port, wait) {
				true => BoardAnswer::Slcan,
				false => BoardAnswer::Silent,
			}
		}
	}
}

/// How long a board gets to answer each question, Hello and then `V`. Either image
/// replies within a USB round trip, milliseconds; this is generous for that and short
/// enough not to be noticed in front of a command.
#[cfg(feature = "slcan")]
pub const BOARD_PROBE_WAIT: Duration = Duration::from_millis(300);

/// Open the board's port and ask it which image it runs — see [`ask_board`]. Only ever
/// call this on a [`BOARD_USB`] port: asking an unknown device a question is writing
/// bytes into somebody else's console.
#[cfg(feature = "slcan")]
pub fn probe_board(path: &str, baud: u32, wait: Duration) -> BoardAnswer {
	match tokio_serial::new(path, baud).timeout(Duration::from_millis(50)).open() {
		Ok(mut port) => {
			// Whatever the image printed before the question is not an answer to it.
			let _ = port.clear(tokio_serial::ClearBuffer::Input);
			ask_board(&mut port, wait)
		}
		Err(e) => BoardAnswer::Unopened(e.to_string()),
	}
}

/// An slcan backend over a real serial port — the concrete type callers name
/// without having to depend on `tokio-serial` themselves.
#[cfg(feature = "slcan")]
pub type SerialSlcan = SlcanBackend<tokio_serial::SerialStream>;

/// Open a real slcan serial adapter and open its CAN channel.
#[cfg(feature = "slcan")]
impl SlcanBackend<tokio_serial::SerialStream> {
	pub async fn open(path: &str, baud: u32, bitrate: SlcanBitrate) -> Result<Self, CanError> {
		SlcanBackend::open_mode(path, baud, bitrate, SlcanMode::Normal).await
	}

	/// Open a real slcan serial adapter and open its CAN channel in `mode`.
	/// [`SlcanMode::Silent`] is the safe way onto a bus somebody else is using.
	pub async fn open_mode(path: &str, baud: u32, bitrate: SlcanBitrate, mode: SlcanMode) -> Result<Self, CanError> {
		use tokio_serial::SerialPortBuilderExt;
		let stream = tokio_serial::new(path, baud)
			.open_native_async()
			.map_err(|e| CanError::Io(e.to_string()))?;
		let mut backend = SlcanBackend::new(stream);
		backend.open_channel_mode(bitrate, mode).await?;
		Ok(backend)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::CAN_EFF_FLAG;

	#[test]
	fn encodes_standard_frame() {
		let line = encode_frame(0x7E0, &[0x02, 0x10, 0x03, 0, 0, 0, 0, 0]).unwrap();
		assert_eq!(line, "t7E080210030000000000\r");
	}

	#[test]
	fn encodes_extended_frame() {
		let line = encode_frame(0x18DA_10F1 | CAN_EFF_FLAG, &[0x3E, 0x00]).unwrap();
		assert_eq!(line, "T18DA10F123E00\r");
	}

	#[test]
	fn encode_rejects_oversized_data() {
		let err = encode_frame(0x7E0, &[0u8; 9]).unwrap_err();
		assert!(matches!(err, CanError::Unsupported(_)), "got {err:?}");
	}

	#[test]
	fn decodes_standard_frame() {
		let (id, data) = decode_frame("t7E88025003AAAAAAAAAA\r").unwrap();
		assert_eq!(id, 0x7E8);
		assert_eq!(data, vec![0x02, 0x50, 0x03, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA]);
	}

	#[test]
	fn decodes_extended_frame() {
		let (id, data) = decode_frame("T18DAF11027E00").unwrap();
		assert_eq!(id, 0x18DA_F110 | CAN_EFF_FLAG);
		assert_eq!(data, vec![0x7E, 0x00]);
	}

	#[test]
	fn decode_tolerates_trailing_timestamp() {
		// CANtact-style adapters append a 4-hex-digit timestamp.
		let (id, data) = decode_frame("t7E82500312AB").unwrap();
		assert_eq!(id, 0x7E8);
		assert_eq!(data, vec![0x50, 0x03]);
	}

	#[test]
	fn decode_rejects_garbage() {
		for bad in ["", "x123", "t7E", "t7E09", "t7E01ZZ"] {
			assert!(decode_frame(bad).is_err(), "accepted {bad:?}");
		}
	}

	/// A port that answers only what it was scripted to, and only once `V\r`
	/// has been written to it. A timeout is what a real port with a timeout
	/// returns when nothing arrives.
	struct FakePort {
		before: Vec<u8>,
		after_v: Vec<u8>,
		written: Vec<u8>,
	}

	impl FakePort {
		fn new(before: &[u8], after_v: &[u8]) -> Self {
			FakePort {
				before: before.to_vec(),
				after_v: after_v.to_vec(),
				written: Vec::new(),
			}
		}
	}

	impl std::io::Read for FakePort {
		fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
			let asked = self.written.windows(2).any(|w| w == b"V\r");
			let source = if !self.before.is_empty() {
				&mut self.before
			} else if asked && !self.after_v.is_empty() {
				&mut self.after_v
			} else {
				return Err(std::io::ErrorKind::TimedOut.into());
			};
			// Seven bytes at a time, so a line split across reads is exercised.
			let n = source.len().min(buf.len()).min(7);
			buf[..n].copy_from_slice(&source[..n]);
			source.drain(..n);
			Ok(n)
		}
	}

	impl std::io::Write for FakePort {
		fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
			self.written.extend_from_slice(buf);
			Ok(buf.len())
		}
		fn flush(&mut self) -> std::io::Result<()> {
			Ok(())
		}
	}

	const WAIT: Duration = Duration::from_millis(30);

	#[test]
	fn the_slcan_image_answers_the_version_query() {
		let mut port = FakePort::new(b"", b"V0101\r");
		assert!(answers_version(&mut port, WAIT));
		assert_eq!(port.written, b"V\r", "the probe sends the version query and nothing else");
	}

	#[test]
	fn bus_frames_ahead_of_the_reply_do_not_hide_it() {
		// An slcan image with its channel left open queues frames before the reply.
		let mut port = FakePort::new(b"T17F0001080102030405060708\r", b"t7E8025003\rV0101\r");
		assert!(answers_version(&mut port, WAIT));
	}

	#[test]
	fn the_display_image_does_not_answer() {
		// `dash` prints frames and log lines and ignores slcan commands.
		let mut port = FakePort::new(
			b"plan: 2 units, 9 channels\r\nFRAME 256 64 AAAA\r\ncan: 7E0 timeout\r\n",
			b"FRAME 256 64 AAAA\r\n",
		);
		assert!(!answers_version(&mut port, WAIT));
	}

	#[test]
	fn silence_is_not_an_answer() {
		let mut port = FakePort::new(b"", b"");
		assert!(!answers_version(&mut port, WAIT));
	}

	#[test]
	fn a_version_that_is_not_a_whole_well_formed_line_is_not_an_answer() {
		for reply in [&b"V01\r"[..], b"note: V0101 somewhere\r", b"V0101", b"\x07", b"VZZZZ\r"] {
			let mut port = FakePort::new(b"", reply);
			assert!(!answers_version(&mut port, WAIT), "accepted {:?}", String::from_utf8_lossy(reply));
		}
	}

	/// A port that answers by rule: each write holding a rule's trigger makes that rule's
	/// reply readable. `before` is readable from the start. Reads come seven bytes at a
	/// time, and time out when there is nothing.
	struct ScriptedPort {
		readable: Vec<u8>,
		rules: Vec<(Vec<u8>, Vec<u8>)>,
		written: Vec<u8>,
	}

	impl ScriptedPort {
		fn new(before: &[u8]) -> Self {
			ScriptedPort {
				readable: before.to_vec(),
				rules: Vec::new(),
				written: Vec::new(),
			}
		}

		fn on(mut self, trigger: &[u8], reply: &[u8]) -> Self {
			self.rules.push((trigger.to_vec(), reply.to_vec()));
			self
		}

		fn sent_v(&self) -> bool {
			self.written.windows(2).any(|w| w == b"V\r")
		}
	}

	impl std::io::Read for ScriptedPort {
		fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
			if self.readable.is_empty() {
				return Err(std::io::ErrorKind::TimedOut.into());
			}
			let n = self.readable.len().min(buf.len()).min(7);
			buf[..n].copy_from_slice(&self.readable[..n]);
			self.readable.drain(..n);
			Ok(n)
		}
	}

	impl std::io::Write for ScriptedPort {
		fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
			self.written.extend_from_slice(buf);
			for (trigger, reply) in &self.rules {
				if buf.windows(trigger.len()).any(|w| w == trigger.as_slice()) {
					self.readable.extend_from_slice(reply);
				}
			}
			Ok(buf.len())
		}
		fn flush(&mut self) -> std::io::Result<()> {
			Ok(())
		}
	}

	fn hello() -> Vec<u8> {
		link::encode(&Message::Hello).unwrap()
	}

	fn hello_reply(version: &str) -> Vec<u8> {
		link::encode(&Message::HelloReply(HelloReply {
			image: "dash".into(),
			version: version.into(),
		}))
		.unwrap()
	}

	const DISPLAY_TEXT: &[u8] = b"plan: 2 units, 9 channels\r\nFRAME 256 64 AAAA\r\ncan: 7E0 timeout\r\n";

	#[test]
	fn a_board_that_answers_hello_is_the_dash_image_and_is_never_sent_v() {
		for before in [&b""[..], DISPLAY_TEXT] {
			let mut port = ScriptedPort::new(before).on(&hello(), &hello_reply("0.1.0")).on(b"V\r", b"V0101\r");
			assert_eq!(ask_board(&mut port, WAIT), BoardAnswer::Dash { version: "0.1.0".into() });
			assert_eq!(port.written, hello(), "Hello and nothing else");
		}
	}

	#[test]
	fn a_board_that_does_not_answer_hello_but_answers_v_is_the_slcan_image() {
		// With a channel left open, frames arrive ahead of everything.
		let mut port = ScriptedPort::new(b"t7E8025003\r").on(b"V\r", b"T17F0001080102030405060708\rV0101\r");
		assert_eq!(ask_board(&mut port, WAIT), BoardAnswer::Slcan);
		assert_eq!(
			port.written,
			[hello(), b"\r".to_vec(), b"V\r".to_vec()].concat(),
			"V after Hello, on a line of its own"
		);
	}

	#[test]
	fn a_board_that_answers_neither_is_silent() {
		for before in [&b""[..], DISPLAY_TEXT] {
			// An older display image: text before, and text after the questions.
			let mut port = ScriptedPort::new(before).on(b"V\r", b"FRAME 256 64 AAAA\r\n");
			assert_eq!(ask_board(&mut port, WAIT), BoardAnswer::Silent);
			assert!(port.sent_v());
		}
	}

	#[test]
	fn framed_bytes_without_a_reply_are_asked_again_and_never_answered_with_v() {
		let stale = link::encode(&Message::Answer(link::Answer {
			seq: 3,
			outcome: link::Outcome::NoAnswer,
		}))
		.unwrap();
		for (before, answer) in [(stale.clone(), Vec::new()), (Vec::new(), b"\x00\x7A\x02\x00".to_vec())] {
			let mut port = ScriptedPort::new(&before).on(&hello(), &answer).on(b"V\r", b"V0101\r");
			assert_eq!(ask_board(&mut port, WAIT), BoardAnswer::Dash { version: "unknown".into() });
			assert!(!port.sent_v(), "V would switch the dash image into its adapter mode");
			assert_eq!(port.written, [hello(), hello()].concat(), "one Hello more, within the wait");
		}
	}

	#[test]
	fn the_second_hello_finds_the_reply_a_broken_frame_hid() {
		// The first Hello is answered with a frame nobody sends; the second with the reply.
		let mut port = ScriptedPort::new(b"\x00\x7A\x02\x00").on(&hello(), &hello_reply("0.2.0"));
		match answers_hello(&mut port, WAIT) {
			HelloAnswer::Reply(reply) => assert_eq!(reply.version, "0.2.0"),
			other => panic!("{other:?}"),
		}
	}

	#[cfg(feature = "slcan")]
	#[test]
	fn the_board_is_not_a_known_adapter_by_its_ids_alone() {
		// Every ESP32-C3 and -S3 enumerates as 303a:1001. The ids say "a board
		// that might be an adapter"; only an answer to `V` says it is one.
		let board = classify_usb("/dev/cu.usbmodem1101".into(), 0x303a, 0x1001, Some("USB JTAG/serial debug unit".into()));
		assert!(board.board);
		assert!(!board.known, "{board:?}");
		let canable = classify_usb("/dev/cu.usbmodem206E37A148451".into(), 0x16d0, 0x117e, None);
		assert!(canable.known && !canable.board, "{canable:?}");
	}

	#[tokio::test]
	async fn backend_writes_frame_as_ascii_line() {
		let (client, mut adapter) = tokio::io::duplex(256);
		let mut backend = SlcanBackend::new(client);
		backend.send_frame(0x7E0, &[0x02, 0x10, 0x03]).await.unwrap();

		let mut got = vec![0u8; 64];
		let n = adapter.read(&mut got).await.unwrap();
		assert_eq!(&got[..n], b"t7E03021003\r");
	}

	#[tokio::test]
	async fn backend_parses_incoming_frame_and_skips_acks() {
		let (client, mut adapter) = tokio::io::duplex(256);
		let mut backend = SlcanBackend::new(client);
		// tx-ack 'z', a bare CR, a BEL error byte, then the actual frame.
		adapter.write_all(b"z\r\r\x07t7E825003\r").await.unwrap();

		let (id, data) = backend.recv_frame(Duration::from_millis(200)).await.unwrap();
		assert_eq!(id, 0x7E8);
		assert_eq!(data, vec![0x50, 0x03]);
	}

	#[tokio::test]
	async fn backend_recv_times_out() {
		let (client, _adapter) = tokio::io::duplex(256);
		let mut backend = SlcanBackend::new(client);
		let err = backend.recv_frame(Duration::from_millis(10)).await.unwrap_err();
		assert!(matches!(err, CanError::Timeout), "got {err:?}");
	}

	#[tokio::test]
	async fn backend_recv_reports_disconnect() {
		let (client, adapter) = tokio::io::duplex(256);
		drop(adapter);
		let mut backend = SlcanBackend::new(client);
		let err = backend.recv_frame(Duration::from_millis(50)).await.unwrap_err();
		assert!(matches!(err, CanError::Disconnected), "got {err:?}");
	}

	#[tokio::test]
	async fn status_flags_are_read_from_the_f_reply() {
		let (client, mut adapter) = tokio::io::duplex(256);
		let mut backend = SlcanBackend::new(client);
		adapter.write_all(b"F08\r").await.unwrap();
		assert_eq!(backend.status_flags(Duration::from_millis(200)).await.unwrap(), Some(0x08));
		let mut got = vec![0u8; 16];
		let n = adapter.read(&mut got).await.unwrap();
		assert_eq!(&got[..n], b"F\r");
	}

	#[tokio::test]
	async fn frames_that_arrive_ahead_of_the_f_reply_are_not_lost() {
		// The channel is open while the question is asked; the bus does not wait.
		let (client, mut adapter) = tokio::io::duplex(256);
		let mut backend = SlcanBackend::new(client);
		adapter.write_all(b"t7E825003\rT17F00010101\rF00\rt7E9101\r").await.unwrap();
		assert_eq!(backend.status_flags(Duration::from_millis(200)).await.unwrap(), Some(0));
		let wait = Duration::from_millis(50);
		assert_eq!(backend.recv_frame(wait).await.unwrap(), (0x7E8, vec![0x50, 0x03]));
		assert_eq!(backend.recv_frame(wait).await.unwrap(), (0x17F0_0010 | CAN_EFF_FLAG, vec![0x01]));
		assert_eq!(backend.recv_frame(wait).await.unwrap(), (0x7E9, vec![0x01]));
	}

	#[tokio::test]
	async fn a_frame_kept_through_the_f_wait_carries_when_it_was_read() {
		let (client, mut adapter) = tokio::io::duplex(256);
		let mut backend = SlcanBackend::new(client);
		adapter.write_all(b"t7E825003\r").await.unwrap();
		let asked = std::time::Instant::now();
		// No `F` reply: the whole wait passes with the frame already read.
		assert_eq!(backend.status_flags(Duration::from_millis(100)).await.unwrap(), None);
		let answered = std::time::Instant::now();
		let (arrived, id, data) = backend.recv_frame_arrived(Duration::ZERO).await.unwrap();
		assert_eq!((id, data), (0x7E8, vec![0x50, 0x03]));
		assert!(arrived >= asked && answered - arrived >= Duration::from_millis(80), "stamped at hand-out");
	}

	#[tokio::test]
	async fn an_adapter_without_f_reports_nothing_rather_than_no_flags() {
		// The CANable's firmware has no `F`: silence, or a BEL, is "not known".
		for reply in [&b""[..], b"\x07", b"z\r"] {
			let (client, mut adapter) = tokio::io::duplex(256);
			let mut backend = SlcanBackend::new(client);
			adapter.write_all(reply).await.unwrap();
			assert_eq!(backend.status_flags(Duration::from_millis(30)).await.unwrap(), None, "{reply:?}");
		}
	}

	#[tokio::test]
	async fn open_channel_sends_close_bitrate_mode_open() {
		// The default open is explicitly NORMAL mode: `M0` must be sent, or a
		// channel left silent by an earlier sniff would stay listen-only.
		let (client, mut adapter) = tokio::io::duplex(256);
		let mut backend = SlcanBackend::new(client);
		backend.open_channel(SlcanBitrate::Rate500k).await.unwrap();

		let mut got = vec![0u8; 64];
		let n = adapter.read(&mut got).await.unwrap();
		assert_eq!(&got[..n], b"C\rS6\rM0\rO\r");
	}

	#[tokio::test]
	async fn silent_mode_is_requested_before_the_channel_opens() {
		// The controller latches its bus mode at open: `M1` after `O` would be
		// ignored and we would ACK on a bus we promised only to listen to.
		let (client, mut adapter) = tokio::io::duplex(256);
		let mut backend = SlcanBackend::new(client);
		backend.open_channel_mode(SlcanBitrate::Rate500k, SlcanMode::Silent).await.unwrap();

		let mut got = vec![0u8; 64];
		let n = adapter.read(&mut got).await.unwrap();
		assert_eq!(&got[..n], b"C\rS6\rM1\rO\r");

		let text = std::str::from_utf8(&got[..n]).unwrap();
		assert!(
			text.find("M1").unwrap() < text.find('O').unwrap(),
			"mode must be set while the controller is still in init state: {text:?}"
		);
	}
}
