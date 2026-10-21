//! The adapter's side of slcan (Lawicel ASCII): what a USB-CAN adapter on slcan firmware
//! says to the host, with the car behind it.
//!
//! Commands are CR-terminated lines. `C`, `Sn`, `Mn`, `O` and `L` set the channel up and
//! are acknowledged with a CR; `V` and `F` are answered as Lawicel has them; a `t` frame on
//! an open channel goes to the [`Bus`] and is acknowledged `z`. What the bus sends comes
//! back as `t` lines. The client's codec (`vag_uds_can::slcan`) is the other end, and the
//! sim answers it as a CANable would — which acks are sent does not matter to it, since it
//! skips every line that is not a frame.

use std::time::Duration;

use crate::bus::{Bus, Frame};

/// Lawicel's error reply: a BEL, with no CR.
const ERROR: &[u8] = b"\x07";

/// `S6`: 500 kbit/s, the rate of VW's diagnostic CAN (ISO 15765-4).
const DIAGNOSTIC_RATE: u8 = 6;

/// What the adapter knows of the channel.
pub struct Adapter {
	pub bus: Bus,
	line: Vec<u8>,
	open: bool,
	listen_only: bool,
	rate: Option<u8>,
	pub log: Vec<Note>,
}

/// Channel events, for the sim's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
	Opened {
		rate: Option<u8>,
		listen_only: bool,
	},
	Closed,
	/// A line the adapter did not take.
	Refused(String),
}

impl Adapter {
	pub fn new(bus: Bus) -> Adapter {
		Adapter {
			bus,
			line: Vec::new(),
			open: false,
			listen_only: false,
			rate: None,
			log: Vec::new(),
		}
	}

	/// Whether the channel is open.
	pub fn is_open(&self) -> bool {
		self.open
	}

	/// Bytes from the host at `now`; whatever the adapter says back is appended to `out`.
	pub fn feed(&mut self, now: Duration, bytes: &[u8], out: &mut Vec<u8>) {
		for &byte in bytes {
			match byte {
				b'\r' | b'\n' => {
					let line = std::mem::take(&mut self.line);
					if !line.is_empty() {
						self.command(now, &line, out);
					}
				}
				_ => self.line.push(byte),
			}
		}
		// A line that never ends is not a command: bounded, so a stream of garbage cannot
		// grow it without end.
		if self.line.len() > 64 {
			self.log.push(Note::Refused(String::from_utf8_lossy(&self.line).into_owned()));
			self.line.clear();
		}
	}

	fn command(&mut self, now: Duration, line: &[u8], out: &mut Vec<u8>) {
		let text = String::from_utf8_lossy(line).into_owned();
		match line {
			[b'C'] => {
				if self.open {
					self.log.push(Note::Closed);
				}
				self.open = false;
				out.push(b'\r');
			}
			[b'S', digit @ b'0'..=b'8'] if !self.open => {
				self.rate = Some(digit - b'0');
				out.push(b'\r');
			}
			[b'M', mode @ (b'0' | b'1')] if !self.open => {
				self.listen_only = *mode == b'1';
				out.push(b'\r');
			}
			[open @ (b'O' | b'L')] if !self.open => {
				self.listen_only |= *open == b'L';
				self.open = true;
				self.bus.reset(now);
				self.log.push(Note::Opened {
					rate: self.rate,
					listen_only: self.listen_only,
				});
				out.push(b'\r');
			}
			// Lawicel `Vhhss`: made-up hardware and software versions.
			[b'V'] => out.extend_from_slice(b"V0101\r"),
			[b'N'] => out.extend_from_slice(b"NSIM0\r"),
			// No error flags: a bus with nobody but the sim on it never overruns.
			[b'F'] => out.extend_from_slice(b"F00\r"),
			[b't', ..] => match self.frame(line) {
				Some((id, data)) if self.open && !self.listen_only => {
					// A unit at another rate hears noise, not frames: said once, answered never.
					if self.rate == Some(DIAGNOSTIC_RATE) {
						let mut frames = Vec::new();
						self.bus.receive(now, id, &data, &mut frames);
						write_frames(&frames, out);
					}
					out.extend_from_slice(b"z\r");
				}
				_ => {
					self.log.push(Note::Refused(text));
					out.extend_from_slice(ERROR);
				}
			},
			// 29-bit frames: there is no unit on them.
			[b'T', ..] if self.open && !self.listen_only => out.extend_from_slice(b"Z\r"),
			_ => {
				self.log.push(Note::Refused(text));
				out.extend_from_slice(ERROR);
			}
		}
	}

	/// `tiiildd…`: an 11-bit id, a length, the bytes.
	fn frame(&self, line: &[u8]) -> Option<(u16, Vec<u8>)> {
		let text = std::str::from_utf8(line).ok()?;
		let id = u16::from_str_radix(text.get(1..4)?, 16).ok()?;
		let len = usize::from_str_radix(text.get(4..5)?, 16).ok().filter(|n| *n <= 8)?;
		let data = (0..len)
			.map(|i| u8::from_str_radix(text.get(5 + 2 * i..7 + 2 * i)?, 16).ok())
			.collect::<Option<Vec<u8>>>()?;
		(text.len() == 5 + 2 * len && id <= 0x7FF).then_some((id, data))
	}

	/// What the bus has due at `now`, as slcan lines.
	pub fn poll(&mut self, now: Duration, out: &mut Vec<u8>) {
		if !self.open {
			return;
		}
		let mut frames = Vec::new();
		self.bus.poll(now, &mut frames);
		write_frames(&frames, out);
	}

	/// When [`Self::poll`] next has something to do.
	pub fn next_due(&self) -> Option<Duration> {
		self.open.then(|| self.bus.next_due()).flatten()
	}
}

fn write_frames(frames: &[Frame], out: &mut Vec<u8>) {
	use std::io::Write as _;
	for frame in frames {
		let _ = write!(out, "t{:03X}8", frame.id);
		for byte in frame.data {
			let _ = write!(out, "{byte:02X}");
		}
		out.push(b'\r');
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::profile::Profile;

	fn adapter() -> Adapter {
		let car = Profile::parse("[[unit]]\nrequest = \"7E0\"\n[unit.did]\nF40D = \"00\"\n")
			.unwrap()
			.resolve()
			.unwrap();
		Adapter::new(Bus::new(car))
	}

	fn say(adapter: &mut Adapter, now: u64, text: &str) -> String {
		let mut out = Vec::new();
		adapter.feed(Duration::from_millis(now), text.as_bytes(), &mut out);
		adapter.poll(Duration::from_millis(now), &mut out);
		String::from_utf8(out).unwrap()
	}

	#[test]
	fn the_clients_open_sequence_then_a_read_and_its_answer() {
		let mut a = adapter();
		// What `SlcanBackend::open_channel_mode` writes, in one go.
		assert_eq!(say(&mut a, 0, "C\rS6\rM0\rO\r"), "\r\r\r\r");
		assert_eq!(
			say(&mut a, 1, "t7E080322F40D00000000\r"),
			"z\rt7E880462F40D00000000\r",
			"the ack, then the answer as a frame"
		);
		assert_eq!(say(&mut a, 2, "C\r"), "\r");
		assert_eq!(
			a.log,
			vec![
				Note::Opened {
					rate: Some(6),
					listen_only: false
				},
				Note::Closed
			]
		);
	}

	#[test]
	fn the_codec_of_the_client_decodes_what_the_adapter_writes() {
		let mut a = adapter();
		say(&mut a, 0, "C\rS6\rM0\rO\r");
		let reply = say(
			&mut a,
			1,
			&vag_uds_can::slcan::encode_frame(0x7E0, &[0x03, 0x22, 0xF4, 0x0D, 0, 0, 0, 0]).unwrap(),
		);
		let line = reply.split('\r').nth(1).unwrap();
		assert_eq!(
			vag_uds_can::slcan::decode_frame(line).unwrap(),
			(0x7E8, vec![0x04, 0x62, 0xF4, 0x0D, 0x00, 0, 0, 0])
		);
	}

	#[test]
	fn a_frame_on_a_closed_or_listen_only_channel_is_refused() {
		let mut a = adapter();
		assert_eq!(say(&mut a, 0, "t7E080322F40D00000000\r"), "\x07");
		say(&mut a, 0, "C\rS6\rM1\rO\r");
		assert_eq!(say(&mut a, 1, "t7E080322F40D00000000\r"), "\x07");
	}

	#[test]
	fn at_another_bitrate_nothing_answers() {
		let mut a = adapter();
		say(&mut a, 0, "C\rS4\rM0\rO\r");
		assert_eq!(say(&mut a, 1, "t7E080322F40D00000000\r"), "z\r");
	}

	#[test]
	fn version_and_status_are_answered_and_garbage_is_refused() {
		let mut a = adapter();
		assert_eq!(say(&mut a, 0, "V\rF\r"), "V0101\rF00\r");
		assert_eq!(say(&mut a, 0, "t7E0Z\r"), "\x07");
		assert_eq!(say(&mut a, 0, "hello\r"), "\x07");
	}

	#[test]
	fn the_version_reply_is_what_the_clients_probe_takes_for_slcan() {
		// `answers_version` writes `V\r` and waits for `Vhhss`.
		struct Port(Adapter, Vec<u8>);
		impl std::io::Read for Port {
			fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
				let n = self.1.len().min(buf.len());
				buf[..n].copy_from_slice(&self.1[..n]);
				self.1.drain(..n);
				Ok(n)
			}
		}
		impl std::io::Write for Port {
			fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
				let mut out = Vec::new();
				self.0.feed(Duration::ZERO, buf, &mut out);
				self.1.extend(out);
				Ok(buf.len())
			}
			fn flush(&mut self) -> std::io::Result<()> {
				Ok(())
			}
		}
		let mut port = Port(adapter(), Vec::new());
		assert!(vag_uds_can::slcan::answers_version(&mut port, Duration::from_millis(50)));
	}
}
