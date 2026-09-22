//! The board's USB console: one byte stream, three protocols, two modes.
//!
//! The `dash` image's USB-Serial-JTAG port carries, from the host:
//!
//! - **framed link messages** ([`vag_uds_transport::link`]), which start with a NUL —
//!   `vagcan` reading the car through the board, the panel running beside it;
//! - **`dashsim`'s button lines**, `BTN S` and `BTN L`, ended by `\n`;
//! - **slcan (Lawicel) command lines**, CR-terminated ASCII — `vagcan --slcan`, which
//!   makes the board a plain CAN adapter.
//!
//! `todo/dash/14-one-bus-three-clients.md` §3 decided the board's two modes and that
//! the first bytes choose between them. [`Console`] is that choice, with no I/O and no
//! clock, so it is tested here and the firmware only acts on what it says:
//!
//! | mode | a framed message | `BTN S` / `BTN L` | an slcan line (`\r`) | anything else |
//! |---|---|---|---|---|
//! | [`Mode::Panel`] | [`Input::Message`] | [`Input::Press`] | an opening command, no link client: [`Input::EnterAdapter`] then [`Input::Slcan`]; a `C`: [`Input::Closed`]; an empty line: nothing; a link client active: [`Input::Ignored`] | [`Input::Ignored`] |
//! | [`Mode::Adapter`] | [`Input::Message`] | — | [`Input::Slcan`], the empty line included; after a `C`, [`Input::LeaveAdapter`] | [`Input::Slcan`] (the adapter answers `\x07`) |
//!
//! - **A framed message never switches.** It is routed in either mode; what the board
//!   does with one in adapter mode (answer a Hello, refuse the rest) is the shell's.
//! - **Only a CR-terminated line that opens or uses the adapter switches**: it starts
//!   with one of `S s O L M t T r R F E V v N Z` ([`enters_adapter`]) — a setting, an
//!   open, a frame, a status or version question. A line ended by `\n` is a terminal or
//!   `dashsim`, not slcan.
//! - **`C` and the empty line never switch.** The host's handshake before a Hello is
//!   `\rC\r`: the `\r` ends whatever half line an earlier program left, and the `C` ends
//!   an adapter session an earlier `--slcan` run left open. In panel mode there is no
//!   channel to close, so a `C` is answered `\r` ([`Input::Closed`]) and nothing else,
//!   and an empty line is nothing at all. A probe that got no answer to its Hello sends
//!   `\r` and then `V`: the `V` is what switches.
//! - **"A link client is active"** is the caller's to say: a framed session holding a
//!   subscription or a request ([`crate::remote::Session::is_active`]). While one is,
//!   slcan lines are not taken.
//! - **Leaving** is on `C`, decided here the moment the line is seen so the lines after
//!   it in the same chunk are judged in panel mode, and when the host is gone, which only
//!   the firmware can see ([`Console::disconnected`]): its start-of-frame packets stop,
//!   or it stops taking what the adapter writes. A dead host on a quiet bus is neither;
//!   that takes `C` — the next host's handshake sends one — or the cable.
//!
//! In adapter mode lines are cut exactly as the standalone `slcan` image cuts them —
//! the same [`LineParser`]: `\r` ends a line, `\n` is ignored, a line too long to be a
//! command comes back as the error byte alone. In panel mode `\n` ends a line too.

use alloc::vec::Vec;
use core::fmt;

use vag_uds_transport::link::{LinkError, Message, Piece, Reassembler};

/// The longest command line kept. A 29-bit frame with eight bytes is `T`, 8 hex of id,
/// 1 of length and 16 of data: 26 bytes.
pub const LINE_MAX: usize = 32;

/// Lawicel's error reply, and what an overflowed line is handed on as.
pub const BELL: u8 = 0x07;

/// The slcan commands that open or use the adapter: a CR-terminated line starting with
/// one of these, in panel mode, is a host that wants a CAN adapter. `C` is not among
/// them (module docs). `s`, Lawicel's raw bit-timing setting, is one the board's adapter
/// refuses, but a host that sends it wants an adapter, and the refusal is its answer.
/// (The `slcan` module of `vag-dash-fw` holds the commands themselves.)
const OPENING_COMMANDS: &[u8] = b"SsOLMtTrRFEVvNZ";

/// One command line, without its terminator. Fixed-size: the standalone `slcan` image
/// does not allocate in its steady state, and this is on its path for every frame.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CommandLine {
	bytes: [u8; LINE_MAX],
	len: u8,
}

impl CommandLine {
	const EMPTY: CommandLine = CommandLine {
		bytes: [0; LINE_MAX],
		len: 0,
	};

	/// A line of `bytes`, cut at [`LINE_MAX`].
	pub fn new(bytes: &[u8]) -> Self {
		let mut line = Self::EMPTY;
		let len = bytes.len().min(LINE_MAX);
		line.bytes[..len].copy_from_slice(&bytes[..len]);
		line.len = len as u8;
		line
	}

	pub fn as_bytes(&self) -> &[u8] {
		&self.bytes[..usize::from(self.len)]
	}

	fn push(&mut self, byte: u8) -> Result<(), ()> {
		let slot = self.bytes.get_mut(usize::from(self.len)).ok_or(())?;
		*slot = byte;
		self.len += 1;
		Ok(())
	}
}

impl Default for CommandLine {
	fn default() -> Self {
		Self::EMPTY
	}
}

impl fmt::Debug for CommandLine {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{:?}", self.as_bytes().escape_ascii())
	}
}

/// How a line ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
	Cr,
	Lf,
}

/// Bytes into command lines.
///
/// `\r` ends a line. `\n` ends one only when built with [`LineParser::lines`]; built with
/// [`LineParser::slcan`] it is ignored, so a terminal user's Enter (`\r\n`) is one
/// command. A line too long to be any command is discarded whole — a desynchronised
/// stream, not a command — and comes back as [`BELL`] alone, which no command matches.
#[derive(Debug, Default)]
pub struct LineParser {
	line: CommandLine,
	overflow: bool,
	newline_ends: bool,
}

impl LineParser {
	/// Lawicel's cut: `\r` only.
	pub fn slcan() -> Self {
		Self::default()
	}

	/// `\r` or `\n`.
	pub fn lines() -> Self {
		LineParser {
			newline_ends: true,
			..Self::default()
		}
	}

	/// One byte in; the line it ended, if it ended one.
	pub fn feed(&mut self, byte: u8) -> Option<(CommandLine, Ending)> {
		let ending = match byte {
			b'\r' => Ending::Cr,
			b'\n' if self.newline_ends => Ending::Lf,
			b'\n' => return None,
			_ => {
				if self.line.push(byte).is_err() {
					self.overflow = true;
				}
				return None;
			}
		};
		let done = if self.overflow { CommandLine::new(&[BELL]) } else { self.line };
		self.line = CommandLine::EMPTY;
		self.overflow = false;
		Some((done, ending))
	}

	/// Forget a line in progress.
	pub fn reset(&mut self) {
		self.line = CommandLine::EMPTY;
		self.overflow = false;
	}
}

/// Which of its two jobs the board is doing (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
	/// Mode 1: the panel, the planner, and framed link clients.
	Panel,
	/// Mode 2: a plain slcan adapter on the cable; the planner sends nothing.
	Adapter,
}

/// `dashsim`'s two gestures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
	Short,
	Long,
}

/// Why a line was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ignored {
	/// Not a button and not an slcan command ended by `\r`.
	NotACommand,
	/// An slcan command, while a framed link client holds a session.
	LinkActive,
}

/// What the console made of the bytes, in the order they came.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
	/// A framed link message, in either mode.
	Message(Message),
	/// A frame that did not reassemble; dropped, and the stream resynchronised.
	Malformed(LinkError),
	/// A button line.
	Press(Button),
	/// Switch to [`Mode::Adapter`] now: the planner stops sending. The line that chose
	/// it follows as [`Input::Slcan`].
	EnterAdapter,
	/// One slcan command line, for the adapter.
	Slcan(CommandLine),
	/// Back to [`Mode::Panel`]: the `C` that closed the adapter came just before.
	LeaveAdapter,
	/// A `C` in panel mode: answer Lawicel's `\r`, and nothing else.
	Closed,
	/// A line nobody takes.
	Ignored { line: CommandLine, why: Ignored },
}

/// The console's state: which mode, and the frame and line in progress.
#[derive(Debug)]
pub struct Console {
	mode: Mode,
	reassembler: Reassembler,
	parser: LineParser,
	/// When the last byte came, on the caller's clock ([`Console::push_at`]).
	last_byte_ms: u64,
}

/// How long a frame may stand unfinished with no byte arriving before it is given up.
///
/// A host killed half way through a frame never sends the rest, and without this the next
/// host's `\rC\r` and Hello would be read as that frame's body. A host sends a frame in
/// one go and USB hands it over within milliseconds, so a frame this quiet is no slow one.
pub const FRAME_GAP_MS: u64 = 200;

/// What a frame given up after [`FRAME_GAP_MS`] is reported as.
pub const UNFINISHED: LinkError = LinkError::Malformed("a frame the host never finished");

/// The longest framed message a host sends the board, in body bytes: a Request's `seq`,
/// request id and response id, then a PDU of at most [`crate::guard::MAX_REQUEST_BYTES`].
/// Longer frames are passed over as they arrive, never gathered — a flood of 4 KB
/// requests ran the board's heap out while they were (2026-09-22). A Subscribe (11) and
/// a Hello (0) are well inside it. The board's BLE link uses it too.
pub const MAX_HOST_BODY: usize = 5 + crate::guard::MAX_REQUEST_BYTES;

impl Default for Console {
	fn default() -> Self {
		Self::new()
	}
}

impl Console {
	/// Panel mode, nothing in progress: the board at boot.
	pub fn new() -> Self {
		Console {
			mode: Mode::Panel,
			reassembler: Reassembler::with_max_body(MAX_HOST_BODY),
			parser: LineParser::lines(),
			last_byte_ms: 0,
		}
	}

	pub fn mode(&self) -> Mode {
		self.mode
	}

	/// One chunk from the host. `link_active`: whether a framed link client holds a
	/// session on this carrier now.
	///
	/// A Request or a Subscribe routed within the chunk makes the link active for the rest
	/// of it: the session will hold it by the time anything is done with what follows, and
	/// the caller's word was taken before it came.
	///
	/// No time passes between chunks given this way; the firmware uses [`Console::push_at`].
	pub fn push(&mut self, chunk: &[u8], link_active: bool) -> Vec<Input> {
		self.push_at(chunk, link_active, self.last_byte_ms)
	}

	/// [`Console::push`], with the moment the chunk arrived in milliseconds on any
	/// monotonic clock: a frame in progress whose last byte is [`FRAME_GAP_MS`] old is
	/// given up first, and said as [`UNFINISHED`].
	pub fn push_at(&mut self, chunk: &[u8], mut link_active: bool, now_ms: u64) -> Vec<Input> {
		let mut out = Vec::new();
		if chunk.is_empty() {
			return out;
		}
		if self.reassembler.in_frame() && now_ms.saturating_sub(self.last_byte_ms) >= FRAME_GAP_MS {
			self.reassembler.reset();
			out.push(Input::Malformed(UNFINISHED));
		}
		self.last_byte_ms = now_ms;
		for piece in self.reassembler.push(chunk) {
			match piece {
				Piece::Message(message) => {
					link_active |= matches!(message, Message::Request(_) | Message::Subscribe(_));
					out.push(Input::Message(message));
				}
				Piece::Error(why) => out.push(Input::Malformed(why)),
				Piece::Text(bytes) => {
					for byte in bytes {
						if let Some((line, ending)) = self.parser.feed(byte) {
							self.line(line, ending, link_active, &mut out);
						}
					}
				}
			}
		}
		out
	}

	/// The caller was away: waiting on a queue of its own — back-pressure — not on the host.
	/// The gap a frame in progress is given up after ([`FRAME_GAP_MS`]) restarts from
	/// `now_ms`, so only the host's silence counts toward it.
	pub fn resume(&mut self, now_ms: u64) {
		self.last_byte_ms = self.last_byte_ms.max(now_ms);
	}

	/// The cable was pulled (or the host stopped the bus): what was in progress is
	/// forgotten, and adapter mode ends.
	pub fn disconnected(&mut self) -> Option<Input> {
		self.reassembler.reset();
		self.parser.reset();
		match self.mode {
			Mode::Adapter => {
				self.set(Mode::Panel);
				Some(Input::LeaveAdapter)
			}
			Mode::Panel => None,
		}
	}

	fn set(&mut self, mode: Mode) {
		self.mode = mode;
		self.parser.newline_ends = mode == Mode::Panel;
	}

	fn line(&mut self, line: CommandLine, ending: Ending, link_active: bool, out: &mut Vec<Input>) {
		let text = line.as_bytes();
		match self.mode {
			Mode::Adapter => {
				let close = closes(text);
				out.push(Input::Slcan(line));
				if close {
					self.set(Mode::Panel);
					out.push(Input::LeaveAdapter);
				}
			}
			Mode::Panel => match text.trim_ascii() {
				b"" => {}
				b"BTN S" => out.push(Input::Press(Button::Short)),
				b"BTN L" => out.push(Input::Press(Button::Long)),
				_ if ending == Ending::Cr && (closes(text) || enters_adapter(text)) => {
					if link_active {
						out.push(Input::Ignored {
							line,
							why: Ignored::LinkActive,
						});
					} else if closes(text) {
						out.push(Input::Closed);
					} else {
						self.set(Mode::Adapter);
						out.push(Input::EnterAdapter);
						out.push(Input::Slcan(line));
					}
				}
				_ => out.push(Input::Ignored {
					line,
					why: Ignored::NotACommand,
				}),
			},
		}
	}
}

/// Whether `line` starts with a command that opens or uses the adapter
/// ([`OPENING_COMMANDS`]).
pub fn enters_adapter(line: &[u8]) -> bool {
	line.first().is_some_and(|head| OPENING_COMMANDS.contains(head))
}

/// Whether `line` is Lawicel's close: `C` and nothing else. One rule for both sides of
/// adapter mode — this console leaves it on such a line, and the adapter (`vag-dash-fw`'s
/// `slcan`) closes its channel on it and refuses a `C` with arguments, closing nothing. Were
/// the two to differ, a `C1` would shut the channel and leave the board in adapter mode.
pub fn closes(line: &[u8]) -> bool {
	line == b"C"
}

#[cfg(test)]
mod tests {
	use super::*;
	use alloc::vec;
	use vag_uds_transport::link::{self, HelloReply, Request};

	fn slcan(text: &[u8]) -> Input {
		Input::Slcan(CommandLine::new(text))
	}

	fn request() -> Message {
		Message::Request(Request {
			seq: 1,
			request_id: 0x7E0,
			response_id: 0x7E8,
			// A PDU full of the bytes a line parser would take for terminators and commands.
			pdu: vec![0x22, b'\r', b'\n', b'V', b'\r', b'C', b'\r'],
		})
	}

	fn feed_bytewise(console: &mut Console, bytes: &[u8], link_active: bool) -> Vec<Input> {
		bytes.iter().flat_map(|b| console.push(core::slice::from_ref(b), link_active)).collect()
	}

	#[test]
	fn the_host_s_slcan_open_switches_to_adapter_mode_at_the_first_setting() {
		let mut console = Console::new();
		let inputs = console.push(b"C\rS6\rM0\rO\r", false);
		assert_eq!(inputs, vec![Input::Closed, Input::EnterAdapter, slcan(b"S6"), slcan(b"M0"), slcan(b"O")]);
		assert_eq!(console.mode(), Mode::Adapter);
	}

	#[test]
	fn a_version_query_switches_and_is_handed_to_the_adapter() {
		let mut console = Console::new();
		assert_eq!(console.push(b"V\r", false), vec![Input::EnterAdapter, slcan(b"V")]);
	}

	#[test]
	fn a_bare_close_in_panel_mode_is_answered_and_switches_nothing() {
		let mut console = Console::new();
		assert_eq!(console.push(b"C\r", false), vec![Input::Closed]);
		assert_eq!(console.mode(), Mode::Panel);
	}

	#[test]
	fn slcan_lines_are_not_taken_while_a_link_client_is_active() {
		let mut console = Console::new();
		for text in [&b"V"[..], b"S6", b"C", b"O"] {
			let mut bytes = text.to_vec();
			bytes.push(b'\r');
			assert_eq!(
				console.push(&bytes, true),
				vec![Input::Ignored {
					line: CommandLine::new(text),
					why: Ignored::LinkActive
				}]
			);
		}
		assert_eq!(console.mode(), Mode::Panel);
	}

	#[test]
	fn a_framed_message_never_switches_in_either_mode() {
		let mut console = Console::new();
		let frame = link::encode(&request()).unwrap();
		assert_eq!(console.push(&frame, false), vec![Input::Message(request())]);
		assert_eq!(console.mode(), Mode::Panel);
		console.push(b"O\r", false);
		assert_eq!(console.mode(), Mode::Adapter);
		assert_eq!(console.push(&frame, false), vec![Input::Message(request())]);
		assert_eq!(
			console.push(&link::encode(&Message::Hello).unwrap(), false),
			vec![Input::Message(Message::Hello)]
		);
		assert_eq!(console.mode(), Mode::Adapter);
	}

	#[test]
	fn dashsim_buttons_are_presses_however_the_line_ends() {
		let mut console = Console::new();
		assert_eq!(
			console.push(b"BTN S\nBTN L\nBTN S\r\n", false),
			vec![Input::Press(Button::Short), Input::Press(Button::Long), Input::Press(Button::Short)]
		);
		assert_eq!(console.mode(), Mode::Panel);
	}

	#[test]
	fn a_command_ended_by_a_newline_is_a_terminal_not_slcan() {
		let mut console = Console::new();
		assert_eq!(
			console.push(b"V\n", false),
			vec![Input::Ignored {
				line: CommandLine::new(b"V"),
				why: Ignored::NotACommand
			}]
		);
		assert_eq!(console.mode(), Mode::Panel);
	}

	#[test]
	fn text_that_is_no_command_is_ignored_and_empty_lines_are_not_even_that() {
		let mut console = Console::new();
		assert_eq!(
			console.push(b"\r\n\rhello\rB\r", false),
			vec![
				Input::Ignored {
					line: CommandLine::new(b"hello"),
					why: Ignored::NotACommand
				},
				Input::Ignored {
					line: CommandLine::new(b"B"),
					why: Ignored::NotACommand
				},
			]
		);
		assert_eq!(console.mode(), Mode::Panel);
	}

	#[test]
	fn in_adapter_mode_every_line_is_the_adapter_s_and_close_leaves_at_once() {
		let mut console = Console::new();
		console.push(b"S6\r", false);
		assert_eq!(
			console.push(b"t7E0322F190\r\rBTN S\rC\rBTN S\n", false),
			vec![
				slcan(b"t7E0322F190"),
				slcan(b""),
				slcan(b"BTN S"),
				slcan(b"C"),
				Input::LeaveAdapter,
				Input::Press(Button::Short)
			]
		);
		assert_eq!(console.mode(), Mode::Panel);
	}

	#[test]
	fn a_close_with_arguments_is_the_adapter_s_to_refuse_and_leaves_nothing() {
		let mut console = Console::new();
		console.push(b"S6\rO\r", false);
		// The adapter refuses `C1` and keeps its channel; the console keeps adapter mode with it.
		assert_eq!(console.push(b"C1\rCC\r", false), vec![slcan(b"C1"), slcan(b"CC")]);
		assert_eq!(console.mode(), Mode::Adapter);
		assert_eq!(console.push(b"C\r", false), vec![slcan(b"C"), Input::LeaveAdapter]);
		// In panel mode a `C` with arguments is no command at all.
		assert_eq!(
			console.push(b"C1\r", false),
			vec![Input::Ignored {
				line: CommandLine::new(b"C1"),
				why: Ignored::NotACommand
			}]
		);
		assert_eq!(console.mode(), Mode::Panel);
	}

	#[test]
	fn only_a_bare_c_closes() {
		assert!(closes(b"C"));
		for line in [&b"C1"[..], b"CC", b"C ", b"c", b"", b" C"] {
			assert!(!closes(line), "{:?}", line.escape_ascii().to_string());
		}
	}

	#[test]
	fn a_close_then_a_new_open_in_one_chunk_leaves_and_enters_again() {
		let mut console = Console::new();
		console.push(b"S6\rO\r", false);
		assert_eq!(
			console.push(b"C\rS6\rM0\rO\r", false),
			vec![
				slcan(b"C"),
				Input::LeaveAdapter,
				Input::EnterAdapter,
				slcan(b"S6"),
				slcan(b"M0"),
				slcan(b"O")
			]
		);
		assert_eq!(console.mode(), Mode::Adapter);
	}

	#[test]
	fn in_adapter_mode_lines_are_cut_as_the_standalone_image_cuts_them() {
		let mut console = Console::new();
		console.push(b"O\r", false);
		// `\n` is ignored, as a terminal's Enter sends `\r\n`: one command, not two.
		assert_eq!(console.push(b"F\r\nV\r\n", false), vec![slcan(b"F"), slcan(b"V")]);
		// A line too long to be any command comes back as the error byte alone.
		let mut long = vec![b't'; 40];
		long.push(b'\r');
		assert_eq!(console.push(&long, false), vec![slcan(&[BELL])]);
		assert_eq!(console.push(b"F\r", false), vec![slcan(b"F")], "the next line is clean");
	}

	#[test]
	fn fed_one_byte_at_a_time_the_console_says_the_same() {
		let mut stream = Vec::new();
		// An unsubscribe holds nothing, so it does not make the link active; its id is two
		// carriage returns, which must stay inside the frame.
		let unsubscribe = Message::Unsubscribe { sub: 0x0D0D };
		stream.extend_from_slice(b"BTN S\n");
		stream.extend(link::encode(&unsubscribe).unwrap());
		stream.extend_from_slice(b"C\rS6\rO\r");
		stream.extend(link::encode(&Message::Hello).unwrap());
		stream.extend_from_slice(b"t7E0322F190\rC\rBTN L\n");
		let whole = Console::new().push(&stream, false);
		let bytewise = feed_bytewise(&mut Console::new(), &stream, false);
		assert_eq!(whole, bytewise);
		assert_eq!(
			whole,
			vec![
				Input::Press(Button::Short),
				Input::Message(unsubscribe),
				Input::Closed,
				Input::EnterAdapter,
				slcan(b"S6"),
				slcan(b"O"),
				Input::Message(Message::Hello),
				slcan(b"t7E0322F190"),
				slcan(b"C"),
				Input::LeaveAdapter,
				Input::Press(Button::Long),
			]
		);
	}

	#[test]
	fn a_request_or_subscribe_makes_the_link_active_for_the_rest_of_its_chunk() {
		let subscribe = Message::Subscribe(vag_uds_transport::link::Subscribe {
			sub: 1,
			request_id: 0x7E0,
			response_id: 0x7E8,
			did: 0x2000,
			period_ms: 100,
			priority: vag_uds_transport::link::Priority::Normal,
		});
		for message in [request(), subscribe] {
			let mut bytes = link::encode(&message).unwrap();
			bytes.extend_from_slice(b"V\r");
			let mut console = Console::new();
			assert_eq!(
				console.push(&bytes, false),
				vec![
					Input::Message(message.clone()),
					Input::Ignored {
						line: CommandLine::new(b"V"),
						why: Ignored::LinkActive
					}
				]
			);
			assert_eq!(console.mode(), Mode::Panel);
		}
		// A Hello holds nothing: what follows it in the chunk may still switch.
		let mut bytes = link::encode(&Message::Hello).unwrap();
		bytes.extend_from_slice(b"V\r");
		assert_eq!(
			Console::new().push(&bytes, false),
			vec![Input::Message(Message::Hello), Input::EnterAdapter, slcan(b"V")]
		);
	}

	#[test]
	fn a_request_too_long_for_the_board_is_passed_over_and_the_next_message_heard() {
		let long = link::encode(&Message::Request(Request {
			seq: 1,
			request_id: 0x7E0,
			response_id: 0x7E8,
			pdu: vec![0x22; 4095],
		}))
		.unwrap();
		let hello = link::encode(&Message::Hello).unwrap();
		let mut console = Console::new();
		let mut heard = Vec::new();
		for chunk in [long.as_slice(), hello.as_slice()].concat().chunks(64) {
			heard.extend(console.push_at(chunk, false, 1_000));
		}
		assert_eq!(
			heard,
			vec![
				Input::Malformed(LinkError::OverCap {
					len: 4100,
					cap: MAX_HOST_BODY
				}),
				Input::Message(Message::Hello)
			]
		);
	}

	#[test]
	fn a_frame_left_half_way_is_given_up_after_a_quiet_gap_and_the_next_host_is_heard() {
		let frame = link::encode(&request()).unwrap();
		let mut next = b"\rC\r".to_vec();
		next.extend(link::encode(&Message::Hello).unwrap());

		let mut console = Console::new();
		assert!(console.push_at(&frame[..6], false, 1_000).is_empty());
		assert_eq!(
			console.push_at(&next, false, 1_000 + FRAME_GAP_MS + 50),
			vec![Input::Malformed(UNFINISHED), Input::Closed, Input::Message(Message::Hello)]
		);
		assert_eq!(console.mode(), Mode::Panel);

		// Within the gap the same bytes are the frame's body, and nothing of them is heard.
		let mut console = Console::new();
		console.push_at(&frame[..6], false, 1_000);
		assert_eq!(console.push_at(&next, false, 1_000 + FRAME_GAP_MS - 100), vec![]);

		// A frame that keeps arriving, however slowly byte by byte, is never given up.
		let mut console = Console::new();
		let mut heard = Vec::new();
		for (at, byte) in frame.iter().enumerate() {
			heard.extend(console.push_at(core::slice::from_ref(byte), false, 1_000 + at as u64 * (FRAME_GAP_MS - 1)));
		}
		assert_eq!(heard, vec![Input::Message(request())]);
	}

	#[test]
	fn the_board_s_own_wait_between_two_halves_of_a_frame_is_not_the_host_s_silence() {
		let frame = link::encode(&request()).unwrap();
		let mut console = Console::new();
		assert!(console.push_at(&frame[..6], false, 1_000).is_empty());
		// Handing on what the first half said waited 300 ms on a full queue; the host sent
		// the rest meanwhile, and it is read the moment the board looks again.
		console.resume(1_000 + 300);
		assert_eq!(console.push_at(&frame[6..], false, 1_000 + 301), vec![Input::Message(request())]);

		// The host's own silence past the gap, after a resume, still gives the frame up.
		let mut console = Console::new();
		console.push_at(&frame[..6], false, 1_000);
		console.resume(1_300);
		assert_eq!(console.push_at(&frame[6..], false, 1_300 + FRAME_GAP_MS)[0], Input::Malformed(UNFINISHED));
	}

	#[test]
	fn a_broken_frame_is_reported_and_the_console_carries_on() {
		let mut console = Console::new();
		assert_eq!(
			console.push(&[0x00, 0x7A, 0x02, 0x00, 1, 2], false),
			vec![Input::Malformed(LinkError::UnknownType(0x7A))]
		);
		assert_eq!(console.push(b"BTN S\n", false), vec![Input::Press(Button::Short)]);
	}

	#[test]
	fn a_disconnect_leaves_adapter_mode_and_forgets_what_was_in_progress() {
		let mut console = Console::new();
		assert_eq!(console.disconnected(), None, "nothing to leave");
		console.push(b"O\rt7E0", false);
		let reply = link::encode(&Message::HelloReply(HelloReply {
			image: "dash".into(),
			version: "0.1.0".into(),
		}))
		.unwrap();
		console.push(&reply[..3], false);
		assert_eq!(console.disconnected(), Some(Input::LeaveAdapter));
		assert_eq!(console.mode(), Mode::Panel);
		// Neither the half line nor the half frame survives into the next session.
		assert_eq!(console.push(b"BTN S\n", false), vec![Input::Press(Button::Short)]);
		assert_eq!(
			console.push(&link::encode(&Message::Hello).unwrap(), false),
			vec![Input::Message(Message::Hello)]
		);
	}

	#[test]
	fn the_opening_letters_are_settings_opens_frames_and_questions() {
		for head in b"SsOLMtTrRFEVvNZ" {
			assert!(enters_adapter(&[*head]), "{}", *head as char);
		}
		for head in b"CBXxWmUQ0 \x07" {
			assert!(!enters_adapter(&[*head]), "{}", *head as char);
		}
		assert!(!enters_adapter(b""));
	}

	/// What the host writes before its Hello: `\r` to end a half line, `C` to end an
	/// adapter session an earlier `--slcan` run left open.
	fn handshake() -> Vec<u8> {
		let mut bytes = b"\rC\r".to_vec();
		bytes.extend(link::encode(&Message::Hello).unwrap());
		bytes
	}

	#[test]
	fn the_host_handshake_never_enters_adapter_mode_in_panel_mode() {
		let mut console = Console::new();
		assert_eq!(console.push(&handshake(), false), vec![Input::Closed, Input::Message(Message::Hello)]);
		assert_eq!(console.mode(), Mode::Panel);
		// Byte by byte, and with a link client active, the same: nothing switches.
		let bytewise = feed_bytewise(&mut Console::new(), &handshake(), false);
		assert_eq!(bytewise, vec![Input::Closed, Input::Message(Message::Hello)]);
		let mut busy = Console::new();
		assert_eq!(
			busy.push(&handshake(), true),
			vec![
				Input::Ignored {
					line: CommandLine::new(b"C"),
					why: Ignored::LinkActive
				},
				Input::Message(Message::Hello)
			]
		);
		assert_eq!(busy.mode(), Mode::Panel);
	}

	#[test]
	fn the_host_handshake_ends_an_adapter_session_left_open() {
		let mut console = Console::new();
		console.push(b"S6\rM0\rO\r", false);
		assert_eq!(console.mode(), Mode::Adapter);
		assert_eq!(
			console.push(&handshake(), false),
			vec![slcan(b""), slcan(b"C"), Input::LeaveAdapter, Input::Message(Message::Hello)]
		);
		assert_eq!(console.mode(), Mode::Panel);
	}

	#[test]
	fn a_probe_that_got_no_hello_answer_switches_at_its_v_not_at_its_empty_line() {
		let mut console = Console::new();
		assert_eq!(console.push(b"\r", false), vec![]);
		assert_eq!(console.mode(), Mode::Panel);
		assert_eq!(console.push(b"V\r", false), vec![Input::EnterAdapter, slcan(b"V")]);
		// In adapter mode the empty line is the adapter's, which answers it; `V` follows.
		assert_eq!(console.push(b"\rV\r", false), vec![slcan(b""), slcan(b"V")]);
	}

	#[test]
	fn a_raw_bit_timing_setting_enters_adapter_mode_to_be_refused_there() {
		let mut console = Console::new();
		assert_eq!(console.push(b"s031C\r", false), vec![Input::EnterAdapter, slcan(b"s031C")]);
	}

	#[test]
	fn the_slcan_parser_ignores_newlines_and_the_line_parser_ends_on_them() {
		let mut slcan = LineParser::slcan();
		let lines: Vec<_> = b"S6\r\nab\ncd\r".iter().filter_map(|b| slcan.feed(*b)).collect();
		assert_eq!(
			lines,
			vec![(CommandLine::new(b"S6"), Ending::Cr), (CommandLine::new(b"abcd"), Ending::Cr)]
		);
		let mut text = LineParser::lines();
		let lines: Vec<_> = b"ab\ncd\r".iter().filter_map(|b| text.feed(*b)).collect();
		assert_eq!(lines, vec![(CommandLine::new(b"ab"), Ending::Lf), (CommandLine::new(b"cd"), Ending::Cr)]);
	}

	#[test]
	fn a_line_of_exactly_the_maximum_is_kept_and_one_more_byte_is_an_overflow() {
		let mut parser = LineParser::slcan();
		let exact = [b'T'; LINE_MAX];
		for b in exact {
			assert_eq!(parser.feed(b), None);
		}
		assert_eq!(parser.feed(b'\r'), Some((CommandLine::new(&exact), Ending::Cr)));
		for b in [b'T'; LINE_MAX + 1] {
			parser.feed(b);
		}
		assert_eq!(parser.feed(b'\r'), Some((CommandLine::new(&[BELL]), Ending::Cr)));
	}
}
