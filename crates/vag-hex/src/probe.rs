//! Live protocol probe: replay the plaintext bring-up burst and watch what the
//! cable pushes back.
//!
//! Purpose (hardware experiment): the *new* Ross-Tech build establishes the AES
//! link session key by **RSA-OAEP key transport** — after the plaintext setup
//! burst completes, the cable *unconditionally pushes* a ~131-byte frame
//! (`[b0][b0][len]` + 128-byte RSA-OAEP-SHA256-wrapped key), which the app
//! decrypts with an embedded RSA-1024 private key (see
//! `research/vag-hex-framing.md` "Session-key derivation" + `auth-mechanism-notes.md`).
//! The *old* VMProtect build used a different (`b6`/`b7`-derived) scheme and does
//! **not** push such a frame.
//!
//! This probe drives the bring-up (values replayed from the capture — the
//! transport layer is stable across builds since the cable is the same hardware)
//! and collects every frame the cable sends afterwards, flagging any wrapped-key
//! candidate. It tells us empirically **which protocol this cable speaks** before
//! we commit to the RSA-OAEP decode path. It does NOT decrypt anything and drives
//! no diagnostic/car traffic — it is read-only observation of the open sequence.

use std::time::Duration;

use crate::error::HexError;
use crate::frame::{self, Frame, MARKER_CABLE, MARKER_HOST};
use crate::usb::Backend;

/// One inter-frame drain window (read acks between bring-up sends).
const STEP_READ: Duration = Duration::from_millis(120);
/// A wrapped-key frame carries a 128-byte RSA-1024 ciphertext (+3-byte header),
/// so any inbound frame at least this large is a candidate.
const WRAPPED_KEY_MIN_DATA: usize = 100;

/// The plaintext bring-up sequence, replayed verbatim from the captured session
/// (`research/reading-ecus.pcapng`, seq 0..36). `(opcode, payload)` — the actor
/// framing prepends the `S` marker + length + xor. The `0x09` keyed exchange and
/// `0xb6` challenge are replayed as captured; per the RE, the cable's key push is
/// unconditional and does not depend on a *valid* auth response, so replayed
/// values are sufficient to reach the push.
pub const BRINGUP: &[(u8, &[u8])] = &[
    (0x02, &[]),                                     // probe
    (0x09, &[0x01, 0x1d, 0xa3, 0xfd, 0x47, 0xfd, 0xc5, 0x6a, 0x15]), // keyed exchange
    (0x04, &[]),                                     // identify → "ROSSTECH"
    (0x82, &[]),                                     // status
    (0x0d, &[]),                                     // status/mode
    (0xb0, &[]),                                     // setup burst ↓
    (0xb1, &[]),
    (0xb2, &[0x00, 0xb8, 0x05]),
    (0xb3, &[0x00, 0xff, 0xe0, 0xff, 0x00]),
    (0xb3, &[0x01, 0xff, 0xe0, 0x00, 0x00]),
    (0xb4, &[0x00, 0x3f, 0xe0, 0x00, 0x00]),
    (0xb4, &[0x01, 0x43, 0xe0, 0x00, 0x00]),
    (0xb4, &[0x02, 0x60, 0x00, 0x00, 0x00]),
    (0xb4, &[0x03, 0x00, 0x00, 0x00, 0x00]),
    (0xb4, &[0x04, 0x00, 0x00, 0x00, 0x00]),
    (0xb4, &[0x05, 0xef, 0x40, 0x00, 0x00]),
    (0xb5, &[0x00, 0x64]),
    (0xb5, &[0x01, 0x60]),
    (0xb6, &[
        0x02, 0x1b, 0x53, 0xb8, 0x21, 0x61, 0xd1, 0x45, 0xbd, 0xc7, 0xbf, 0xb7,
        0x55, 0x3e, 0xbe, 0x30, 0xd2, 0x11, 0x66, 0xfd, 0x54, 0xc0, 0xb4, 0xa5, 0x60,
    ]), // challenge (sprng nonce, replayed)
];

/// Result of a bring-up probe.
#[derive(Debug, Default)]
pub struct ProbeReport {
    /// Every frame the cable sent, in arrival order.
    pub received: Vec<Frame>,
    /// The first inbound frame large enough to carry an RSA-1024 wrapped key
    /// (≥100 data bytes) — the new-build key-transport signature. `None` means
    /// the cable did not push a wrapped key (likely the old `b6`/`b7` scheme).
    pub wrapped_key: Option<Frame>,
    /// Every raw byte read from the cable (before `S`/`M` framing), so a silent
    /// cable is distinguishable from one sending bytes we can't frame.
    pub raw_bytes: Vec<u8>,
}

impl ProbeReport {
    /// Does the collected traffic look like the new RSA-OAEP key-transport?
    #[must_use]
    pub fn looks_new_protocol(&self) -> bool {
        self.wrapped_key.is_some()
    }
}

/// A frame is a wrapped-key candidate if it is big enough to hold the 128-byte
/// RSA ciphertext. (The inner `[b0][b0][len]` header + `0xF0` nibble is checked
/// by the app's dispatcher, but on the wire we key off the size, which is the
/// unambiguous discriminator vs the 16-byte `b7`/`b8` blocks and small acks.)
#[must_use]
fn is_wrapped_key(f: &Frame) -> bool {
    f.data.len() >= WRAPPED_KEY_MIN_DATA
}

/// Read available bytes into `buf` for up to `window`, cutting complete cable
/// frames into `out`. Returns when the window elapses (a read timing out is
/// normal — the cable is quiet between pushes).
async fn collect_for<B: Backend>(
    backend: &mut B,
    buf: &mut Vec<u8>,
    out: &mut Vec<Frame>,
    raw: &mut Vec<u8>,
    window: Duration,
) -> Result<(), HexError> {
    let deadline = tokio::time::Instant::now() + window;
    let mut scratch = [0u8; 4096];
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        match tokio::time::timeout(remaining, backend.read(&mut scratch)).await {
            Err(_) => return Ok(()), // window elapsed
            Ok(Ok(0)) => tokio::time::sleep(Duration::from_millis(2)).await,
            Ok(Ok(n)) => {
                raw.extend_from_slice(&scratch[..n]);
                buf.extend_from_slice(&scratch[..n]);
                while let Some((f, consumed)) = frame::take_frame(buf, MARKER_CABLE) {
                    buf.drain(..consumed);
                    out.push(f);
                }
            }
            Ok(Err(e)) => return Err(e),
        }
    }
}

/// Drive the plaintext bring-up and collect what the cable pushes.
///
/// Sends every [`BRINGUP`] frame (draining acks between), then listens for
/// `listen` for any cable-pushed frame — notably the RSA-OAEP wrapped-key frame
/// the new build expects. Read-only: no diagnostic/car traffic, no decryption.
pub async fn probe_open<B: Backend>(
    backend: &mut B,
    listen: Duration,
) -> Result<ProbeReport, HexError> {
    let mut buf: Vec<u8> = Vec::new();
    let mut received: Vec<Frame> = Vec::new();
    let mut raw: Vec<u8> = Vec::new();

    for &(opcode, payload) in BRINGUP {
        let wire = frame::frame_encode(MARKER_HOST, opcode, payload);
        backend.write(&wire).await?;
        // Drain the ack / any immediate reply before the next send.
        collect_for(backend, &mut buf, &mut received, &mut raw, STEP_READ).await?;
    }

    // The cable pushes the wrapped key after its setup completes — listen.
    collect_for(backend, &mut buf, &mut received, &mut raw, listen).await?;

    let wrapped_key = received.iter().find(|f| is_wrapped_key(f)).cloned();
    Ok(ProbeReport {
        received,
        wrapped_key,
        raw_bytes: raw,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::frame_encode;
    use std::collections::VecDeque;

    /// Backend that replays a fixed inbound byte script regardless of writes.
    struct ScriptBackend {
        inbox: VecDeque<u8>,
    }
    impl ScriptBackend {
        fn new(inbound: Vec<u8>) -> Self {
            Self {
                inbox: inbound.into(),
            }
        }
    }
    impl Backend for ScriptBackend {
        async fn write(&mut self, _bytes: &[u8]) -> Result<(), HexError> {
            Ok(())
        }
        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, HexError> {
            if self.inbox.is_empty() {
                std::future::pending::<()>().await;
            }
            let n = buf.len().min(self.inbox.len());
            for slot in buf.iter_mut().take(n) {
                *slot = self.inbox.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    #[tokio::test]
    async fn detects_pushed_wrapped_key_frame() {
        // Cable pushes acks then a 131-byte 0xF0 frame (3 hdr + 128 ciphertext).
        let mut inbound = frame_encode(MARKER_CABLE, 0xFE, &[]);
        let blob: Vec<u8> = (0..131u32).map(|i| (i & 0xff) as u8).collect();
        inbound.extend(frame_encode(MARKER_CABLE, 0xF3, &blob));
        let mut backend = ScriptBackend::new(inbound);

        let report = probe_open(&mut backend, Duration::from_millis(200))
            .await
            .expect("probe runs");

        assert!(report.looks_new_protocol(), "should flag the wrapped key");
        let wk = report.wrapped_key.expect("wrapped key present");
        assert_eq!(wk.data.len(), 131);
        assert_eq!(wk.opcode, 0xF3);
    }

    #[tokio::test]
    async fn old_scheme_pushes_no_wrapped_key() {
        // Old build: only small b7/b9 frames, nothing ≥100 bytes.
        let mut inbound = frame_encode(MARKER_CABLE, 0xB7, &[0x39; 16]);
        inbound.extend(frame_encode(MARKER_CABLE, 0xB9, &[0x40]));
        let mut backend = ScriptBackend::new(inbound);

        let report = probe_open(&mut backend, Duration::from_millis(200))
            .await
            .expect("probe runs");

        assert!(!report.looks_new_protocol(), "no wrapped key in old scheme");
        assert!(report.received.len() >= 2, "still collected the small frames");
    }
}
