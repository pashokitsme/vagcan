//! Connection actor: ONE tokio task owns the [`Backend`] byte pipe and
//! multiplexes N concurrent requests over the single serial link.
//!
//! This is the locked architecture from `todo/GOAL.md` (NOT `Arc<Mutex<dev>>`):
//! callers hold cheap-clone [`CableHandle`]s and get *concurrency* (their tasks
//! overlap while awaiting), while the actor serializes actual wire traffic and
//! owns all link state — the outbound framing, the reply matching, and the
//! per-request timeouts. Requests ride a bounded `mpsc` into the actor; each
//! carries a `oneshot` the actor resolves with the decoded reply [`Frame`].
//!
//! ## Reply matching (plaintext path)
//! The cable answers OUT frames in order, so the actor matches replies to
//! requests by **strict FIFO ordering**: the next complete `M` frame cut from
//! the read stream answers the oldest in-flight request. The actor owns any
//! future seq counter. Known hazard, accepted for now: if a request times out
//! and its reply arrives *late* while a newer request is already in flight,
//! strict ordering will mis-attribute that late frame to the newer request
//! (nothing in the plaintext frame disambiguates reliably — `OP_DIAG_REQ`
//! 0xB8 is answered by `OP_DIAG_RESP` 0xB7, so opcode echo is not universal).
//! The actor does drop stale *buffered* bytes whenever a request starts on an
//! otherwise idle link, which covers the common late-reply case.
//!
//! ## What this module does NOT do yet
//! Only the PLAINTEXT `S`/`M` frame path (init handshake, identify, keepalive)
//! is implemented. The encrypted DIAGNOSTIC path is gated on the per-channel
//! link keystream + session key (see `research/vag-hex-framing.md`).

use std::collections::VecDeque;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::error::HexError;
use crate::frame::{self, Frame, MARKER_CABLE, MARKER_HOST};
use crate::usb::Backend;

/// Depth of the bounded request queue between handles and the actor.
const REQUEST_QUEUE: usize = 16;
/// Bytes pulled from the backend per read.
const READ_CHUNK: usize = 4096;
/// Backoff between empty reads (device timed out with 0 bytes) so a quiet
/// device does not busy-spin the actor loop.
const EMPTY_READ_BACKOFF: Duration = Duration::from_millis(2);

/// One in-flight ask from a handle to the actor. The actor owns framing: the
/// handle sends `{opcode, payload}`, never raw wire bytes.
struct Request {
    opcode: u8,
    payload: Vec<u8>,
    timeout: Duration,
    reply: oneshot::Sender<Result<Frame, HexError>>,
}

/// A request already written to the wire, awaiting its reply frame.
struct Pending {
    reply: oneshot::Sender<Result<Frame, HexError>>,
    deadline: Instant,
}

/// The connection actor: owns the [`Backend`] and all link state. Create with
/// [`CableActor::new`] (for tests / custom spawning) or just call [`spawn`].
pub struct CableActor<B: Backend> {
    backend: B,
    rx: mpsc::Receiver<Request>,
    /// Read-side accumulator; complete `M` frames are cut out via
    /// [`frame::take_frame`].
    rx_buf: Vec<u8>,
    /// Requests on the wire, oldest first (strict FIFO reply matching).
    pending: VecDeque<Pending>,
}

/// Cheap-clone handle to a running [`CableActor`]. Dropping every handle closes
/// the request channel, which makes the actor exit and drop the backend.
#[derive(Clone, Debug)]
pub struct CableHandle {
    tx: mpsc::Sender<Request>,
}

/// Spawn the connection actor onto the current tokio runtime and hand back the
/// first [`CableHandle`].
pub fn spawn<B: Backend + 'static>(backend: B) -> CableHandle {
    let (actor, handle) = CableActor::new(backend);
    tokio::spawn(actor.run());
    handle
}

impl CableHandle {
    /// Send one plaintext host frame (`S`, `opcode`, `payload`) and await the
    /// cable's reply frame, waiting at most `timeout` once the request is on
    /// the wire. Safe to call from many tasks concurrently — the actor
    /// serializes the wire traffic and matches replies in order.
    pub async fn request(
        &self,
        opcode: u8,
        payload: &[u8],
        timeout: Duration,
    ) -> Result<Frame, HexError> {
        let (reply, reply_rx) = oneshot::channel();
        self.tx
            .send(Request {
                opcode,
                payload: payload.to_vec(),
                timeout,
                reply,
            })
            .await
            .map_err(|_| HexError::Io("cable actor gone".into()))?;
        reply_rx
            .await
            .map_err(|_| HexError::Io("cable actor dropped the request".into()))?
    }
}

// TODO(link-cipher): `impl vag_transport::AsyncIsoTpTransport for CableHandle`
// lands here once the per-channel link keystream + session key are ported —
// that is the encrypted OP_DIAG_REQ/OP_DIAG_RESP UDS path. This module is the
// plaintext frame path only.

impl<B: Backend> CableActor<B> {
    /// Build the actor and its first handle without spawning — callers that
    /// need the `JoinHandle` (tests, supervisors) run `actor.run()` themselves.
    pub fn new(backend: B) -> (Self, CableHandle) {
        let (tx, rx) = mpsc::channel(REQUEST_QUEUE);
        (
            Self {
                backend,
                rx,
                rx_buf: Vec::new(),
                pending: VecDeque::new(),
            },
            CableHandle { tx },
        )
    }

    /// The actor task body. Runs until every [`CableHandle`] is dropped *and*
    /// no request is in flight.
    pub async fn run(mut self) {
        let mut scratch = [0u8; READ_CHUNK];
        loop {
            if self.pending.is_empty() {
                // Idle link: only new requests can wake us. Waiting here (rather
                // than also reading) lets the mpsc close cleanly shut us down,
                // and lets us discard any stale buffered bytes before the next
                // request so a late reply can't be mis-attributed.
                match self.rx.recv().await {
                    Some(req) => {
                        self.rx_buf.clear();
                        self.start_request(req).await;
                    }
                    None => return, // every handle dropped, nothing pending
                }
                continue;
            }

            // A request is on the wire. Race three things: its timeout, a fresh
            // request arriving (pipeline it), and reply bytes from the backend.
            let deadline = self.pending.front().expect("pending non-empty").deadline;
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(deadline) => {
                    self.fail_oldest(HexError::Timeout);
                }
                maybe_req = self.rx.recv() => {
                    match maybe_req {
                        Some(req) => self.start_request(req).await,
                        None => {
                            // Senders gone, but drain the in-flight replies
                            // before exiting so their handles still resolve.
                            self.drain_pending().await;
                            return;
                        }
                    }
                }
                res = self.backend.read(&mut scratch) => {
                    match res {
                        Ok(0) => {
                            // Device idle: brief backoff so a silent link can't
                            // busy-spin the loop before the timeout fires.
                            tokio::time::sleep(EMPTY_READ_BACKOFF).await;
                        }
                        Ok(n) => {
                            self.rx_buf.extend_from_slice(&scratch[..n]);
                            self.dispatch_replies();
                        }
                        Err(e) => self.fail_oldest(HexError::Io(format!("backend read: {e}"))),
                    }
                }
            }
        }
    }

    /// Frame and write one request; on write failure resolve it immediately,
    /// otherwise enqueue it as pending with its deadline.
    async fn start_request(&mut self, req: Request) {
        let wire = frame::frame_encode(MARKER_HOST, req.opcode, &req.payload);
        if let Err(e) = self.backend.write(&wire).await {
            let _ = req.reply.send(Err(e));
            return;
        }
        let deadline = Instant::now() + req.timeout;
        self.pending.push_back(Pending {
            reply: req.reply,
            deadline,
        });
    }

    /// Cut every complete cable reply frame currently buffered and hand each to
    /// the oldest waiting request (strict FIFO). Stops when the buffer holds no
    /// more complete frames or no request is waiting.
    fn dispatch_replies(&mut self) {
        while !self.pending.is_empty() {
            let Some((frame, consumed)) = frame::take_frame(&self.rx_buf, MARKER_CABLE) else {
                break;
            };
            self.rx_buf.drain(..consumed);
            let pending = self.pending.pop_front().expect("checked non-empty");
            let _ = pending.reply.send(Ok(frame));
        }
    }

    /// Fail the oldest in-flight request (timeout / read error).
    fn fail_oldest(&mut self, err: HexError) {
        if let Some(pending) = self.pending.pop_front() {
            let _ = pending.reply.send(Err(err));
        }
    }

    /// Senders are gone: resolve every still-pending request, respecting each
    /// remaining deadline, then let the task exit.
    async fn drain_pending(&mut self) {
        let mut scratch = [0u8; READ_CHUNK];
        while !self.pending.is_empty() {
            let deadline = self.pending.front().expect("pending non-empty").deadline;
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(deadline) => self.fail_oldest(HexError::Timeout),
                res = self.backend.read(&mut scratch) => match res {
                    Ok(0) => tokio::time::sleep(EMPTY_READ_BACKOFF).await,
                    Ok(n) => {
                        self.rx_buf.extend_from_slice(&scratch[..n]);
                        self.dispatch_replies();
                    }
                    Err(e) => self.fail_oldest(HexError::Io(format!("backend read: {e}"))),
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{OP_IDENTIFY, OP_PROBE, frame_encode};
    use std::sync::{Arc, Mutex};

    /// Scripted in-memory backend. Every write is recorded; a write that
    /// matches a scripted request enqueues its reply bytes onto the read
    /// stream. `read` yields at most `max_read` bytes per call and pends
    /// (cancellation-safely) while the stream is empty.
    struct MockBackend {
        written: Arc<Mutex<Vec<Vec<u8>>>>,
        /// `(expected wire frame, reply bytes to enqueue)`.
        replies: Vec<(Vec<u8>, Vec<u8>)>,
        inbox: VecDeque<u8>,
        max_read: usize,
        fail_writes: bool,
    }

    impl MockBackend {
        fn new(replies: Vec<(Vec<u8>, Vec<u8>)>) -> (Self, Arc<Mutex<Vec<Vec<u8>>>>) {
            let written = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    written: written.clone(),
                    replies,
                    inbox: VecDeque::new(),
                    max_read: usize::MAX,
                    fail_writes: false,
                },
                written,
            )
        }
    }

    impl Backend for MockBackend {
        async fn write(&mut self, bytes: &[u8]) -> Result<(), HexError> {
            if self.fail_writes {
                return Err(HexError::Io("mock write failure".into()));
            }
            self.written.lock().unwrap().push(bytes.to_vec());
            if let Some(pos) = self.replies.iter().position(|(req, _)| req == bytes) {
                let (_, reply) = self.replies.remove(pos);
                self.inbox.extend(reply);
            }
            Ok(())
        }

        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, HexError> {
            if self.inbox.is_empty() {
                // Nothing scripted to arrive: pend forever. The actor drops
                // this future on new-request arrival or timeout; dropping it
                // loses nothing.
                std::future::pending::<()>().await;
            }
            let n = buf.len().min(self.inbox.len()).min(self.max_read);
            for slot in buf.iter_mut().take(n) {
                *slot = self.inbox.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    const T: Duration = Duration::from_secs(1);

    #[tokio::test]
    async fn identify_request_writes_frame_and_resolves_reply() {
        // The brief's vector: identify OUT is `53 04 04 <xor>` (xor = 0x53).
        let identify_wire = frame_encode(MARKER_HOST, OP_IDENTIFY, &[]);
        assert_eq!(identify_wire, vec![0x53, 0x04, 0x04, 0x53]);
        let reply_wire = frame_encode(MARKER_CABLE, OP_IDENTIFY, b"ROSSTECH");
        let (backend, written) = MockBackend::new(vec![(identify_wire.clone(), reply_wire)]);
        let handle = spawn(backend);

        let frame = handle.request(OP_IDENTIFY, &[], T).await.unwrap();

        assert_eq!(frame.marker, MARKER_CABLE);
        assert_eq!(frame.opcode, OP_IDENTIFY);
        assert_eq!(frame.data, b"ROSSTECH".to_vec());
        assert_eq!(&*written.lock().unwrap(), &[identify_wire]);
    }

    #[tokio::test]
    async fn concurrent_requests_multiplex_over_one_link() {
        let probe_wire = frame_encode(MARKER_HOST, OP_PROBE, &[]);
        let probe_reply = frame_encode(MARKER_CABLE, OP_PROBE, &[0x01, 0x60, 0x44]);
        let ident_wire = frame_encode(MARKER_HOST, OP_IDENTIFY, &[]);
        let ident_reply = frame_encode(MARKER_CABLE, OP_IDENTIFY, b"ROSSTECH");
        let (backend, _) =
            MockBackend::new(vec![(probe_wire, probe_reply), (ident_wire, ident_reply)]);
        let handle = spawn(backend);
        let handle2 = handle.clone();

        let (a, b) = tokio::join!(
            handle.request(OP_PROBE, &[], T),
            handle2.request(OP_IDENTIFY, &[], T),
        );

        let a = a.unwrap();
        let b = b.unwrap();
        assert_eq!((a.opcode, a.data), (OP_PROBE, vec![0x01, 0x60, 0x44]));
        assert_eq!((b.opcode, b.data), (OP_IDENTIFY, b"ROSSTECH".to_vec()));
    }

    #[tokio::test(start_paused = true)]
    async fn request_times_out_when_cable_stays_silent() {
        let (backend, _) = MockBackend::new(vec![]); // never replies
        let handle = spawn(backend);
        let err = handle
            .request(OP_PROBE, &[], Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(matches!(err, HexError::Timeout), "got {err:?}");
    }

    #[tokio::test]
    async fn reply_split_across_reads_is_reassembled() {
        let wire = frame_encode(MARKER_HOST, 0x09, &[0xAA, 0xBB]);
        let reply = frame_encode(MARKER_CABLE, 0x09, &[1, 2, 3, 4, 5, 6, 7, 8, 9]);
        let (mut backend, _) = MockBackend::new(vec![(wire, reply)]);
        backend.max_read = 3; // frame arrives over several short reads
        let handle = spawn(backend);

        let frame = handle.request(0x09, &[0xAA, 0xBB], T).await.unwrap();
        assert_eq!(frame.data, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[tokio::test]
    async fn reply_after_line_noise_is_still_matched() {
        let wire = frame_encode(MARKER_HOST, OP_PROBE, &[]);
        let mut noisy_reply = vec![0x00, 0xFF, 0x4D, 0x02]; // junk + false marker
        noisy_reply.extend_from_slice(&frame_encode(MARKER_CABLE, OP_PROBE, &[0x42]));
        let (backend, _) = MockBackend::new(vec![(wire, noisy_reply)]);
        let handle = spawn(backend);

        let frame = handle.request(OP_PROBE, &[], T).await.unwrap();
        assert_eq!(frame.data, vec![0x42]);
    }

    #[tokio::test]
    async fn write_failure_fails_that_request() {
        let (mut backend, _) = MockBackend::new(vec![]);
        backend.fail_writes = true;
        let handle = spawn(backend);
        let err = handle.request(OP_PROBE, &[], T).await.unwrap_err();
        assert!(matches!(err, HexError::Io(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn actor_exits_when_all_handles_drop() {
        let (backend, _) = MockBackend::new(vec![]);
        let (actor, handle) = CableActor::new(backend);
        let join = tokio::spawn(actor.run());
        drop(handle);
        tokio::time::timeout(T, join)
            .await
            .expect("actor exits once every handle is dropped")
            .unwrap();
    }

    #[tokio::test]
    async fn request_errors_when_actor_is_gone() {
        let (backend, _) = MockBackend::new(vec![]);
        let (actor, handle) = CableActor::new(backend);
        drop(actor); // never spawned
        let err = handle.request(OP_PROBE, &[], T).await.unwrap_err();
        assert!(matches!(err, HexError::Io(_)), "got {err:?}");
    }
}
