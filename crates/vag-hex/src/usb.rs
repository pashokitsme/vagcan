//! Byte-pipe backend: the raw read/write channel to the cable.
//!
//! The cable is an FTDI bridge reached via D2XX bulk (OUT ep `0x02`, IN ep
//! `0x81`; VID `0x0403`). Everything above this module depends only on the
//! async [`Backend`] trait, so the framer/actor stay backend-agnostic and a
//! future `nusb`/raw-libusb backend can slot in behind the same seam.
//!
//! ## Threading model (locked by `todo/GOAL.md`)
//! The FTDI D2XX handle is **blocking** and must never run on the tokio reactor
//! and never via `spawn_blocking` per call. Instead one dedicated `std::thread`
//! owns the handle for its whole life and services a command queue; the async
//! [`Backend`] methods hand it work over a `tokio::sync::mpsc` channel and await
//! the result on a `oneshot`. This is the "connection-actor's byte pipe": a
//! single OS thread per cable, bridged to async by channels.
//!
//! ## FTDI IN status strip
//! At the raw-USB level FTDI prepends a 2-byte modem/line status to every
//! 64-byte IN packet (`research/clb-crack/usbpcap.py::strip_ftdi_in`). The D2XX
//! *library* already removes it before `FT_Read` returns, so the native path
//! sets `status_prefixed = false`. The strip is still implemented and unit
//! tested here ([`strip_ftdi_status`]) because the future raw-bulk backend
//! reads the un-stripped stream and the worker will apply it (`status_prefixed
//! = true`). Applying it to D2XX output would corrupt the stream, so we do not.

use std::collections::VecDeque;

use tokio::sync::{mpsc, oneshot};

use crate::error::HexError;

#[cfg(feature = "d2xx")]
mod d2xx;

/// FTDI vendor id shared by every Ross-Tech HEX cable.
pub const FTDI_VID: u16 = 0x0403;

/// A bidirectional async byte channel to the cable.
///
/// Static dispatch — no `async_trait`, no `dyn`. The methods are declared as
/// `-> impl Future + Send` (rather than bare `async fn`) so a *generic* actor
/// (`CableActor<B: Backend>`) can be `tokio::spawn`ed on a multi-threaded
/// runtime: the trait-level `Send` bound on the returned futures is what makes
/// the actor's own future provably `Send`. Implementors may still write plain
/// `async fn` bodies (that is an allowed refinement) as long as the future is
/// `Send`.
///
/// ## Cancellation safety
/// `read` futures may be dropped mid-flight (the actor `select!`s over reads
/// and request arrivals, and wraps reads in `tokio::time::timeout`). A
/// cancelled `read` MUST NOT lose stream bytes: any bytes already pulled from
/// the device must be delivered by a subsequent `read` call.
pub trait Backend: Send {
    /// Write all `bytes` to the cable, resolving once handed to the wire.
    fn write(&mut self, bytes: &[u8]) -> impl Future<Output = Result<(), HexError>> + Send;
    /// Read up to `buf.len()` bytes; returns the count read (0 = nothing within
    /// the device read timeout — the caller retries).
    fn read(&mut self, buf: &mut [u8]) -> impl Future<Output = Result<usize, HexError>> + Send;
}

/// One enumerated FTDI device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CableInfo {
    /// FTDI serial string (e.g. `"RT000001"`); the [`D2xxBackend::open`] key.
    pub serial: String,
    /// FTDI product description (e.g. `"HEX-V2"`).
    pub description: String,
    /// USB vendor id (FTDI = `0x0403`).
    pub vid: u16,
    /// USB product id.
    pub pid: u16,
}

/// Enumerate the FTDI devices currently on the bus.
///
/// With the `d2xx` feature off (no native lib compiled) this returns
/// [`HexError::Unspecified`].
pub fn list_cables() -> Result<Vec<CableInfo>, HexError> {
    #[cfg(feature = "d2xx")]
    {
        d2xx::list_cables()
    }
    #[cfg(not(feature = "d2xx"))]
    {
        Err(HexError::Unspecified("d2xx feature disabled: cannot enumerate"))
    }
}

// --------------------------------------------------------------------------
// Dedicated-thread bridge
// --------------------------------------------------------------------------

/// Blocking device owned exclusively by the worker thread. The real
/// [`d2xx::D2xxDevice`] implements this; tests use an in-memory fake.
pub(crate) trait RawDevice: Send {
    /// Write bytes, returning the count accepted this call (may be partial).
    fn write(&mut self, bytes: &[u8]) -> Result<usize, HexError>;
    /// Blocking read (bounded by the device read timeout) into `buf`; returns
    /// the count read (0 = timed out with nothing).
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, HexError>;
}

/// How many bytes the worker pulls from the device per underlying read.
const READ_CHUNK: usize = 4096;
/// Command-queue depth between async callers and the worker thread.
const CMD_QUEUE: usize = 32;

enum Cmd {
    Write {
        bytes: Vec<u8>,
        ack: oneshot::Sender<Result<(), HexError>>,
    },
    Read {
        max: usize,
        reply: oneshot::Sender<Result<Vec<u8>, HexError>>,
    },
}

/// Body of the dedicated OS thread: owns `dev` for its whole life, drains the
/// command queue, and closes `dev` on drop when the queue is closed.
fn worker_loop(mut dev: Box<dyn RawDevice>, status_prefixed: bool, mut rx: mpsc::Receiver<Cmd>) {
    // Already-read, already-stripped bytes not yet handed to a caller.
    let mut pending: VecDeque<u8> = VecDeque::new();
    let mut scratch = vec![0u8; READ_CHUNK];

    while let Some(cmd) = rx.blocking_recv() {
        match cmd {
            Cmd::Write { bytes, ack } => {
                let _ = ack.send(write_all(dev.as_mut(), &bytes));
            }
            Cmd::Read { max, reply } => {
                let result = serve_read(dev.as_mut(), status_prefixed, &mut pending, &mut scratch, max);
                // Cancellation safety: if the caller's `Backend::read` future was
                // dropped (its `reply_rx` is gone), we must NOT lose the bytes we
                // just drained — put them back at the front of `pending` for the
                // next reader. The actor relies on this (it `select!`s over reads).
                if let Err(returned) = reply.send(result)
                    && let Ok(bytes) = returned
                {
                    for &b in bytes.iter().rev() {
                        pending.push_front(b);
                    }
                }
            }
        }
    }
    // Queue closed: all `D2xxBackend` handles dropped. Returning drops `dev`,
    // whose `Drop` closes the FTDI handle.
}

fn write_all(dev: &mut dyn RawDevice, bytes: &[u8]) -> Result<(), HexError> {
    let mut off = 0;
    while off < bytes.len() {
        let n = dev.write(&bytes[off..])?;
        if n == 0 {
            return Err(HexError::Io("device accepted 0 bytes on write".into()));
        }
        off += n;
    }
    Ok(())
}

fn serve_read(
    dev: &mut dyn RawDevice,
    status_prefixed: bool,
    pending: &mut VecDeque<u8>,
    scratch: &mut [u8],
    max: usize,
) -> Result<Vec<u8>, HexError> {
    if pending.is_empty() {
        let n = dev.read(scratch)?;
        if n > 0 {
            let payload = if status_prefixed {
                strip_ftdi_status(&scratch[..n])
            } else {
                scratch[..n].to_vec()
            };
            pending.extend(payload);
        }
    }
    let take = max.min(pending.len());
    Ok(pending.drain(..take).collect())
}

/// The physical HEX cable over FTDI D2XX.
///
/// Cheap to move (holds only the command sender). Dropping it closes the
/// channel, which lets the worker thread finish and close the FTDI handle.
pub struct D2xxBackend {
    cmd_tx: mpsc::Sender<Cmd>,
}

impl D2xxBackend {
    /// FTDI parameters this backend programs on open. Values are provisional
    /// defaults (the exact FTDI control setup VCDS uses is not in the capture
    /// yet — see `research/vag-hex-capture-guide.md`); adjust once captured.
    pub const BAUD_RATE: u32 = 115_200;
    /// Latency timer in ms — matches the captured working session (1 ms).
    pub const LATENCY_TIMER_MS: u8 = 1;
    /// Read/write timeout in ms programmed via `FT_SetTimeouts`.
    pub const TIMEOUT_MS: u32 = 1_000;

    /// Open the cable by FTDI `serial` (`None` = first device found), program
    /// the FTDI params, purge RX+TX, and spawn the worker thread.
    pub fn open(serial: Option<&str>) -> Result<Self, HexError> {
        #[cfg(feature = "d2xx")]
        {
            let dev = d2xx::open_device(serial)?;
            // D2XX's FT_Read already drops the 2-byte-per-packet status, so the
            // worker must NOT strip again (`status_prefixed = false`).
            Ok(Self::from_raw_device(Box::new(dev), false))
        }
        #[cfg(not(feature = "d2xx"))]
        {
            let _ = serial;
            Err(HexError::Unspecified("d2xx feature disabled: cannot open cable"))
        }
    }

    /// Spawn the dedicated worker thread around any [`RawDevice`]. The seam the
    /// real open and the in-memory tests share.
    pub(crate) fn from_raw_device(dev: Box<dyn RawDevice>, status_prefixed: bool) -> Self {
        let (cmd_tx, rx) = mpsc::channel(CMD_QUEUE);
        std::thread::Builder::new()
            .name("vag-hex-d2xx".into())
            .spawn(move || worker_loop(dev, status_prefixed, rx))
            .expect("spawn vag-hex-d2xx worker thread");
        Self { cmd_tx }
    }
}

impl Backend for D2xxBackend {
    async fn write(&mut self, bytes: &[u8]) -> Result<(), HexError> {
        let (ack, ack_rx) = oneshot::channel();
        self.cmd_tx
            .send(Cmd::Write {
                bytes: bytes.to_vec(),
                ack,
            })
            .await
            .map_err(|_| HexError::Io("d2xx worker thread gone".into()))?;
        ack_rx
            .await
            .map_err(|_| HexError::Io("d2xx worker dropped write ack".into()))?
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, HexError> {
        if buf.is_empty() {
            return Ok(0);
        }
        let (reply, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(Cmd::Read {
                max: buf.len(),
                reply,
            })
            .await
            .map_err(|_| HexError::Io("d2xx worker thread gone".into()))?;
        let data = reply_rx
            .await
            .map_err(|_| HexError::Io("d2xx worker dropped read reply".into()))??;
        buf[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }
}

/// Strip the 2-byte FTDI modem/line status that prefixes each 64-byte USB IN
/// packet, replicating `research/clb-crack/usbpcap.py::strip_ftdi_in`.
///
/// FTDI hardware inserts `[modem_status, line_status]` at the head of every
/// packet of up to 64 bytes; a bulk transfer reassembled across N packets
/// repeats the pair every 64 bytes. A trailing block shorter than 2 bytes is
/// dropped whole (it is status-only). Only for raw-bulk sources — the D2XX
/// library strips this itself.
pub(crate) fn strip_ftdi_status(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for block in data.chunks(64) {
        if block.len() >= 2 {
            out.extend_from_slice(&block[2..]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // ---- strip_ftdi_status ------------------------------------------------

    #[test]
    fn strip_drops_two_status_bytes_from_a_short_packet() {
        // One sub-64 packet: [0x01 0x60] status + payload.
        let data = [0x01, 0x60, b'M', 0x07, 0x02];
        assert_eq!(strip_ftdi_status(&data), vec![b'M', 0x07, 0x02]);
    }

    #[test]
    fn strip_removes_status_per_64_byte_block() {
        // Two full 64-byte packets: each contributes 62 payload bytes.
        let mut data = Vec::new();
        data.extend_from_slice(&[0x01, 0x60]);
        data.extend(std::iter::repeat_n(0xAA, 62));
        data.extend_from_slice(&[0x01, 0x60]);
        data.extend(std::iter::repeat_n(0xBB, 62));
        let out = strip_ftdi_status(&data);
        assert_eq!(out.len(), 124);
        assert!(out[..62].iter().all(|&b| b == 0xAA));
        assert!(out[62..].iter().all(|&b| b == 0xBB));
    }

    #[test]
    fn strip_handles_partial_final_block() {
        // 64-byte block then a 3-byte tail: 62 + 1 payload bytes.
        let mut data = vec![0x01, 0x60];
        data.extend(std::iter::repeat_n(0xAA, 62));
        data.extend_from_slice(&[0x01, 0x60, 0x99]);
        let out = strip_ftdi_status(&data);
        assert_eq!(out.len(), 63);
        assert_eq!(*out.last().unwrap(), 0x99);
    }

    #[test]
    fn strip_drops_status_only_and_empty_inputs() {
        assert!(strip_ftdi_status(&[]).is_empty());
        assert!(strip_ftdi_status(&[0x01, 0x60]).is_empty()); // status, no payload
        assert!(strip_ftdi_status(&[0x01]).is_empty()); // runt < 2 bytes dropped
    }

    // ---- in-memory fake device -------------------------------------------

    /// Loopback/scripted device. `to_read` is fed to reads (as if arriving from
    /// the wire); `written` records everything the worker writes.
    struct FakeDevice {
        to_read: VecDeque<u8>,
        written: Arc<Mutex<Vec<u8>>>,
    }

    impl RawDevice for FakeDevice {
        fn write(&mut self, bytes: &[u8]) -> Result<usize, HexError> {
            self.written.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, HexError> {
            let n = buf.len().min(self.to_read.len());
            for slot in buf.iter_mut().take(n) {
                *slot = self.to_read.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    fn fake(to_read: Vec<u8>) -> (Box<FakeDevice>, Arc<Mutex<Vec<u8>>>) {
        let written = Arc::new(Mutex::new(Vec::new()));
        let dev = Box::new(FakeDevice {
            to_read: to_read.into(),
            written: written.clone(),
        });
        (dev, written)
    }

    #[tokio::test]
    async fn bridge_write_reaches_the_device() {
        let (dev, written) = fake(vec![]);
        let mut backend = D2xxBackend::from_raw_device(dev, false);
        backend.write(&[0x53, 0x04, 0x02, 0x55]).await.unwrap();
        // Allow the worker to flush the write before asserting.
        for _ in 0..100 {
            if written.lock().unwrap().len() == 4 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        assert_eq!(&*written.lock().unwrap(), &[0x53, 0x04, 0x02, 0x55]);
    }

    #[tokio::test]
    async fn bridge_read_returns_device_bytes_without_strip() {
        let (dev, _) = fake(vec![b'M', 0x07, 0x02, 0x01, 0x60, 0x44]);
        let mut backend = D2xxBackend::from_raw_device(dev, false);
        let mut buf = [0u8; 16];
        let n = backend.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], &[b'M', 0x07, 0x02, 0x01, 0x60, 0x44]);
    }

    #[tokio::test]
    async fn bridge_read_strips_ftdi_status_when_prefixed() {
        // Raw-bulk source: 2 status bytes + real payload. status_prefixed=true.
        let (dev, _) = fake(vec![0x01, 0x60, b'M', 0x07, 0x02]);
        let mut backend = D2xxBackend::from_raw_device(dev, true);
        let mut buf = [0u8; 16];
        let n = backend.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], &[b'M', 0x07, 0x02]);
    }

    #[tokio::test]
    async fn bridge_read_honours_buffer_size_and_buffers_the_rest() {
        let (dev, _) = fake(vec![1, 2, 3, 4, 5]);
        let mut backend = D2xxBackend::from_raw_device(dev, false);
        let mut small = [0u8; 2];
        let n1 = backend.read(&mut small).await.unwrap();
        assert_eq!(&small[..n1], &[1, 2]);
        let mut rest = [0u8; 16];
        let n2 = backend.read(&mut rest).await.unwrap();
        assert_eq!(&rest[..n2], &[3, 4, 5]);
    }

    #[tokio::test]
    async fn worker_requeues_bytes_when_read_reply_receiver_dropped() {
        // A `Backend::read` future can be cancelled (actor `select!`/timeout)
        // after its Cmd::Read was queued. The worker must NOT lose the bytes it
        // drained for that orphaned read — they belong to the next read.
        let (dev, _) = fake(vec![1, 2, 3]);
        let mut backend = D2xxBackend::from_raw_device(dev, false);
        let (reply, reply_rx) = oneshot::channel();
        drop(reply_rx); // simulate the cancelled read future
        backend.cmd_tx.send(Cmd::Read { max: 16, reply }).await.unwrap();
        // The next (live) read must still see the full stream.
        let mut buf = [0u8; 8];
        let n = backend.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], &[1, 2, 3]);
    }

    #[tokio::test]
    async fn bridge_read_empty_device_returns_zero() {
        let (dev, _) = fake(vec![]);
        let mut backend = D2xxBackend::from_raw_device(dev, false);
        let mut buf = [0u8; 8];
        assert_eq!(backend.read(&mut buf).await.unwrap(), 0);
    }
}
