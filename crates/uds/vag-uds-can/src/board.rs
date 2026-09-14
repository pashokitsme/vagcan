//! The dash board over its USB cable: a [`Pipe`] for the framed link, and the Hello
//! that starts a session on it.
//!
//! The `dash` image's USB console carries framed link messages beside its text
//! (`vag_uds_client::console`), so the laptop reads the car through the board with the
//! panel still running — the same [`link`] BLE carries, over a serial port instead.
//!
//! [`StreamPipe`] is generic over the byte stream so it is tested over
//! `tokio::io::duplex`; [`SerialPipe`] is it over a real port.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::time::Instant;
use vag_uds_transport::TransportError;
use vag_uds_transport::link::{self, HelloReply, Message, Piece, Pipe, Reassembler};

use crate::CanError;

/// How long [`StreamPipe::handshake`] waits for the board's HelloReply. The board answers
/// within a USB round trip; the rest is room for a FIFO full of stale bytes to drain.
pub const HANDSHAKE_WAIT: Duration = Duration::from_secs(1);

/// A Hello unanswered this long is sent again: the first may have landed in a half
/// frame an earlier host left in the board's reassembler.
const HELLO_AGAIN: Duration = Duration::from_millis(300);

/// After a frame that did not reassemble, the soonest Hello goes out again — so a burst
/// of stale garbage costs one Hello, not one per chunk.
const HELLO_GAP: Duration = Duration::from_millis(50);

/// The most one read takes off the stream.
const READ_CHUNK: usize = 4096;

/// What ends an slcan session an earlier `--slcan` run left open, sent before the Hello.
///
/// `C` closes the adapter and leaves adapter mode; in panel mode it is answered and
/// switches nothing. The leading CR ends whatever half line the board's console holds,
/// which would otherwise make the `C` part of it.
const CLOSE_ADAPTER: &[u8] = b"\rC\r";

/// The link's [`Pipe`] over an async byte stream.
pub struct StreamPipe<S> {
	stream: S,
	buf: Vec<u8>,
	/// Bytes read past the HelloReply in the chunk that carried it, handed out by the
	/// next [`Pipe::read`] before anything new.
	pending: Vec<u8>,
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> StreamPipe<S> {
	pub fn new(stream: S) -> Self {
		StreamPipe {
			stream,
			buf: vec![0; READ_CHUNK],
			pending: Vec::new(),
		}
	}

	/// Start a session: end any adapter session, send Hello, and wait up to `wait` for
	/// the board's HelloReply.
	///
	/// Everything before the reply is read and dropped — the board's text, readings and
	/// answers for an earlier host, half frames — because the port's FIFO may hold any of
	/// it. A frame that does not reassemble, or a Hello unanswered for [`HELLO_AGAIN`],
	/// sends Hello again with the scanner reset. What followed the reply in its chunk is
	/// kept for the next [`Pipe::read`].
	///
	/// [`CanError::Timeout`] when no reply came: the board is not running the `dash`
	/// image, or is not listening.
	pub async fn handshake(&mut self, wait: Duration) -> Result<HelloReply, CanError> {
		let hello = link::encode(&Message::Hello).map_err(|e| CanError::Io(e.to_string()))?;
		self.write_raw(CLOSE_ADAPTER).await?;
		self.write_raw(&hello).await?;
		let deadline = Instant::now() + wait;
		let mut sent = Instant::now();
		let mut broken = false;
		let mut scan = HelloScan::default();
		loop {
			let again = sent + if broken { HELLO_GAP } else { HELLO_AGAIN };
			if Instant::now() >= again {
				scan.reset();
				self.write_raw(&hello).await?;
				sent = Instant::now();
				broken = false;
				continue;
			}
			if Instant::now() >= deadline {
				return Err(CanError::Timeout);
			}
			let read = tokio::time::timeout_at(deadline.min(again), self.stream.read(&mut self.buf)).await;
			let n = match read {
				Err(_elapsed) => continue,
				Ok(Ok(0)) => return Err(CanError::Disconnected),
				Ok(Ok(n)) => n,
				Ok(Err(e)) => return Err(CanError::Io(e.to_string())),
			};
			match scan.push(&self.buf[..n]) {
				Scanned::Reply { reply, used } => {
					self.pending = self.buf[used..n].to_vec();
					return Ok(reply);
				}
				Scanned::Nothing { broken: true, .. } => broken = true,
				Scanned::Nothing { .. } => {}
			}
		}
	}

	async fn write_raw(&mut self, bytes: &[u8]) -> Result<(), CanError> {
		self.stream.write_all(bytes).await.map_err(|e| CanError::Io(e.to_string()))?;
		self.stream.flush().await.map_err(|e| CanError::Io(e.to_string()))
	}
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> Pipe for StreamPipe<S> {
	async fn write(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
		self.write_raw(bytes).await.map_err(Into::into)
	}

	/// Cancel-safe: the kept bytes are handed out without waiting, and tokio's `read`
	/// takes nothing off the stream unless it returns.
	async fn read(&mut self) -> Option<Vec<u8>> {
		if !self.pending.is_empty() {
			return Some(std::mem::take(&mut self.pending));
		}
		match self.stream.read(&mut self.buf).await {
			Ok(0) | Err(_) => None,
			Ok(n) => Some(self.buf[..n].to_vec()),
		}
	}
}

/// What one chunk held, as far as a host waiting for a HelloReply cares.
#[derive(Debug)]
pub(crate) enum Scanned {
	/// The reply, and how many of the chunk's bytes it took to reach its end.
	Reply { reply: HelloReply, used: usize },
	/// No reply. `framed`: a message or a frame that did not reassemble was seen, so
	/// something on the port speaks the link. `broken`: one did not reassemble.
	Nothing { framed: bool, broken: bool },
}

/// Finds a HelloReply in whatever else the port holds.
#[derive(Debug, Default)]
pub(crate) struct HelloScan {
	reassembler: Reassembler,
}

impl HelloScan {
	/// Fed a byte at a time, for two reasons: the reply's end has to be known to keep
	/// what follows it, and a frame header that is not trusted then costs its own four
	/// bytes rather than the rest of the chunk — which may be the reply.
	pub(crate) fn push(&mut self, chunk: &[u8]) -> Scanned {
		let (mut framed, mut broken) = (false, false);
		for (at, byte) in chunk.iter().enumerate() {
			for piece in self.reassembler.push(std::slice::from_ref(byte)) {
				match piece {
					Piece::Message(Message::HelloReply(reply)) => return Scanned::Reply { reply, used: at + 1 },
					Piece::Message(_) => framed = true,
					Piece::Error(_) => (framed, broken) = (true, true),
					Piece::Text(_) => {}
				}
			}
		}
		Scanned::Nothing { framed, broken }
	}

	/// Forget a frame in progress: a half frame with a trusted length would swallow the
	/// reply to the next Hello.
	pub(crate) fn reset(&mut self) {
		self.reassembler.reset();
	}
}

/// The link's pipe over the board's serial port.
#[cfg(feature = "slcan")]
pub type SerialPipe = StreamPipe<tokio_serial::SerialStream>;

#[cfg(feature = "slcan")]
impl StreamPipe<tokio_serial::SerialStream> {
	/// Open the board's port at `baud` and start a session ([`StreamPipe::handshake`],
	/// [`HANDSHAKE_WAIT`]). The board's reply comes back with the pipe.
	pub async fn open_board(path: &str, baud: u32) -> Result<(Self, HelloReply), CanError> {
		use tokio_serial::{SerialPort as _, SerialPortBuilderExt as _};
		let stream = tokio_serial::new(path, baud)
			.open_native_async()
			.map_err(|e| CanError::Io(e.to_string()))?;
		// What the image printed before anyone asked is not an answer; the handshake
		// would skip it anyway, this only makes that shorter.
		let _ = stream.clear(tokio_serial::ClearBuffer::Input);
		let mut pipe = StreamPipe::new(stream);
		let reply = pipe.handshake(HANDSHAKE_WAIT).await?;
		Ok((pipe, reply))
	}
}

#[cfg(test)]
mod tests {
	use std::sync::{Arc, Mutex};

	use tokio::io::DuplexStream;
	use vag_uds_transport::link::{Outcome, Reading};

	use super::*;

	fn reply() -> HelloReply {
		HelloReply {
			image: "dash".into(),
			version: "0.1.0".into(),
		}
	}

	fn frame(message: &Message) -> Vec<u8> {
		link::encode(message).unwrap()
	}

	#[tokio::test]
	async fn a_whole_write_arrives() {
		let (host, mut board) = tokio::io::duplex(256);
		let mut pipe = StreamPipe::new(host);
		pipe.write(b"\x00\x07\x00\x00").await.unwrap();
		let mut got = [0u8; 4];
		board.read_exact(&mut got).await.unwrap();
		assert_eq!(&got, b"\x00\x07\x00\x00");
	}

	#[tokio::test]
	async fn a_large_write_arrives_whole() {
		// Past the duplex's buffer and past one read: the write waits for the reader.
		let (host, mut board) = tokio::io::duplex(64);
		let mut pipe = StreamPipe::new(host);
		let big: Vec<u8> = (0..5000u32).map(|i| i as u8).collect();
		let reader = tokio::spawn(async move {
			let mut got = vec![0u8; 5000];
			board.read_exact(&mut got).await.unwrap();
			got
		});
		pipe.write(&big).await.unwrap();
		assert_eq!(reader.await.unwrap(), big);
	}

	#[tokio::test]
	async fn chunks_come_back_as_they_were_read_and_the_end_is_none() {
		let (host, mut board) = tokio::io::duplex(256);
		let mut pipe = StreamPipe::new(host);
		board.write_all(b"state page=1/2").await.unwrap();
		assert_eq!(pipe.read().await.as_deref(), Some(&b"state page=1/2"[..]));
		board.write_all(b"more").await.unwrap();
		drop(board);
		assert_eq!(pipe.read().await.as_deref(), Some(&b"more"[..]));
		assert_eq!(pipe.read().await, None);
	}

	#[tokio::test]
	async fn a_read_given_up_on_loses_nothing() {
		let (host, mut board) = tokio::io::duplex(256);
		let mut pipe = StreamPipe::new(host);
		assert!(tokio::time::timeout(Duration::from_millis(20), pipe.read()).await.is_err());
		board.write_all(b"after").await.unwrap();
		assert_eq!(pipe.read().await.as_deref(), Some(&b"after"[..]));
	}

	/// A board on the other end of a duplex: `stale` is in its FIFO before anyone asks,
	/// and the `n`th Hello it hears is answered with `answer(n)`. Everything the host
	/// sent is kept.
	fn board(stale: &[u8], mut answer: impl FnMut(usize) -> Vec<u8> + Send + 'static) -> (StreamPipe<DuplexStream>, Arc<Mutex<Vec<u8>>>) {
		let (host, mut end) = tokio::io::duplex(8192);
		let heard = Arc::new(Mutex::new(Vec::new()));
		let kept = heard.clone();
		let stale = stale.to_vec();
		tokio::spawn(async move {
			end.write_all(&stale).await.unwrap();
			let mut reassembler = Reassembler::new();
			let mut hellos = 0;
			let mut chunk = [0u8; 256];
			loop {
				let n = match end.read(&mut chunk).await {
					Ok(0) | Err(_) => return,
					Ok(n) => n,
				};
				kept.lock().unwrap().extend_from_slice(&chunk[..n]);
				for piece in reassembler.push(&chunk[..n]) {
					if piece == Piece::Message(Message::Hello) {
						let bytes = answer(hellos);
						hellos += 1;
						if end.write_all(&bytes).await.is_err() {
							return;
						}
					}
				}
			}
		});
		(StreamPipe::new(host), heard)
	}

	fn hellos(heard: &Mutex<Vec<u8>>) -> usize {
		let hello = frame(&Message::Hello);
		heard.lock().unwrap().windows(hello.len()).filter(|w| *w == hello.as_slice()).count()
	}

	#[tokio::test]
	async fn the_handshake_closes_an_adapter_session_says_hello_and_keeps_what_followed_the_reply() {
		let reading = frame(&Message::Reading(Reading {
			sub: 1,
			at_ms: 5,
			outcome: Outcome::NoAnswer,
		}));
		let after = [b"state page=1/2".as_slice(), &reading].concat();
		let answered = after.clone();
		let (mut pipe, heard) = board(b"", move |_| [frame(&Message::HelloReply(reply())), answered.clone()].concat());
		assert_eq!(pipe.handshake(HANDSHAKE_WAIT).await.unwrap(), reply());
		let mut got = Vec::new();
		while got.len() < after.len() {
			got.extend(pipe.read().await.expect("the pipe is open"));
		}
		assert_eq!(got, after, "the bytes after the reply are the next read's");
		let heard = heard.lock().unwrap().clone();
		assert!(heard.starts_with(b"\rC\r\x00\x07\x00\x00"), "{heard:?}");
	}

	#[tokio::test]
	async fn stale_text_and_a_broken_frame_ahead_of_the_reply_are_read_past() {
		// A FIFO holding the display image's lines and a frame header nobody sends.
		let stale = b"FRAME 256 64 AAAA\r\n\x00\x7A\x02\x00\x01\x02can: 7E0 timeout\r\n";
		let (mut pipe, _) = board(stale, |_| frame(&Message::HelloReply(reply())));
		assert_eq!(pipe.handshake(HANDSHAKE_WAIT).await.unwrap(), reply());
	}

	#[tokio::test]
	async fn a_frame_that_does_not_reassemble_sends_hello_again() {
		// The first Hello is answered with a broken frame and nothing else. The wait is
		// under HELLO_AGAIN, so only the broken frame can have sent the second Hello.
		let (mut pipe, heard) = board(b"", |n| match n {
			0 => b"\x00\x7A\x02\x00".to_vec(),
			_ => frame(&Message::HelloReply(reply())),
		});
		assert_eq!(pipe.handshake(HELLO_AGAIN - Duration::from_millis(100)).await.unwrap(), reply());
		assert_eq!(hellos(&heard), 2);
	}

	#[tokio::test]
	async fn a_half_frame_that_swallows_the_reply_is_forgotten_when_hello_goes_again() {
		// A Reading header promising 4095 bytes: the first reply disappears into its body.
		let (mut pipe, heard) = board(b"\x00\x05\xFF\x0F", |_| frame(&Message::HelloReply(reply())));
		assert_eq!(pipe.handshake(HANDSHAKE_WAIT).await.unwrap(), reply());
		assert!(hellos(&heard) >= 2);
	}

	#[tokio::test]
	async fn a_board_that_never_answers_hello_times_out() {
		let (mut pipe, _) = board(b"plan: 2 units, 9 channels\r\n", |_| b"FRAME 1 2 AA\r\n".to_vec());
		let failed = pipe.handshake(Duration::from_millis(80)).await;
		assert!(matches!(failed, Err(CanError::Timeout)), "{failed:?}");
	}
}
