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

/// The captured TesterPresent request block (ciphertext, primary `f3` channel):
/// off6..13 XOR [`KS_F3`] = `02 3E 00 …` = UDS TesterPresent. Replayed verbatim to
/// prove the live session key equals the capture's (see `session_probe`).
pub const TP_B8_BLOCK: [u8; 16] = [
    0xf3, 0x83, 0x44, 0xdd, 0x7c, 0x5f, 0x00, 0x97, 0x99, 0xf6, 0xda, 0x7c, 0x9c, 0x3a, 0x00, 0xfc,
];

/// Recovered primary-channel keystream — re-exported from [`crate::link`], the
/// canonical location. (off1 + off6..13 recovered; rest 0.)
pub use crate::link::KS_F3;

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

/// Drive the bring-up, then replay the captured TesterPresent `b8` block and
/// collect the cable's `b7` responses.
///
/// If the live session key equals the capture's (the determinism we observed:
/// replaying the capture's `b6` reproduced its `b7`/`09` byte-for-byte), then a
/// `b7` response here decodes with [`KS_F3`] to a UDS TesterPresent positive
/// reply (`7E 00`) — proving the recovered keystreams work live end-to-end.
///
/// Sends the TP block several times (the cable tracks a per-frame counter at
/// off14; retrying tolerates a stale start value). Read-only diagnostic UDS
/// (TesterPresent is a no-op keepalive — no car state changes).
pub async fn session_probe<B: Backend>(
    backend: &mut B,
    listen: Duration,
) -> Result<ProbeReport, HexError> {
    let mut buf: Vec<u8> = Vec::new();
    let mut received: Vec<Frame> = Vec::new();
    let mut raw: Vec<u8> = Vec::new();

    for &(opcode, payload) in BRINGUP {
        backend
            .write(&frame::frame_encode(MARKER_HOST, opcode, payload))
            .await?;
        collect_for(backend, &mut buf, &mut received, &mut raw, STEP_READ).await?;
    }

    // Replay TesterPresent (opcode 0xB8 diagnostic request) a few times.
    let tp = frame::frame_encode(MARKER_HOST, crate::frame::OP_DIAG_REQ, &TP_B8_BLOCK);
    for _ in 0..4 {
        backend.write(&tp).await?;
        collect_for(backend, &mut buf, &mut received, &mut raw, Duration::from_millis(300)).await?;
    }
    collect_for(backend, &mut buf, &mut received, &mut raw, listen).await?;

    let wrapped_key = received.iter().find(|f| is_wrapped_key(f)).cloned();
    Ok(ProbeReport {
        received,
        wrapped_key,
        raw_bytes: raw,
    })
}

/// The post-auth choreography that follows [`BRINGUP`], replayed verbatim from
/// the capture (`research/reading-ecus.pcapng`, the OUT frames right after the
/// `0xb6` challenge — see `research/clb-crack/choreo.py`). Per the RE, the cable
/// will NOT accept an `0xf3` diagnostic `0xb8` until this runs: a `0x39`-channel
/// exchange, an `a0` poll, a `0x19 00` read, the `0x0b` indexed-block burst
/// (idx 00..07), and the keyed `0x09` challenge/response triplet. The `0x39` and
/// `0x09` payloads are the captured session's bytes; they apply live because the
/// cable is deterministic (same session key ⇒ same challenge ⇒ same response),
/// the property this whole replay path relies on. `(opcode, payload)`.
pub const POST_AUTH: &[(u8, &[u8])] = &[
    (0xB8, &[
        0x39, 0xc7, 0x0a, 0x5d, 0xe7, 0x72, 0xcf, 0xa5, 0x6e, 0xfb, 0x41, 0xc6, 0x4c, 0xab, 0x38,
        0xcd,
    ]),
    (0xA0, &[]),
    (0x19, &[0x00]),
    (0x0B, &[0x00, 0x00]),
    (0x0B, &[0x01, 0x00]),
    (0x0B, &[0x02, 0x00]),
    (0x0B, &[0x03, 0x00]),
    (0x0B, &[0x04, 0x00]),
    (0x0B, &[0x05, 0x00]),
    (0x0B, &[0x06, 0x00]),
    (0x0B, &[0x07, 0x00]),
    (0x09, &[0x05, 0x00, 0x83, 0x80, 0x41, 0xbe, 0xe4, 0x44, 0x71]),
    (0x09, &[0x02, 0x07, 0xe3, 0x2b, 0xde, 0xa5, 0x7b, 0x38, 0x64]),
    (0x09, &[0x03, 0xb1, 0x77, 0xb1, 0xf2, 0x02, 0x5c, 0x6d, 0xc0]),
    (0xA0, &[]),
];

/// Result of a VIN read attempt.
#[derive(Debug, Default)]
pub struct VinReport {
    /// The decoded VIN (17 chars) if the multiframe response reassembled and
    /// carried a `62 F1 90` ReadDataByIdentifier positive reply.
    pub vin: Option<String>,
    /// Every `f3` `b7` response block decoded to its inner ISO-TP bytes, for
    /// inspection when the VIN did not come through.
    pub decoded_blocks: Vec<[u8; 16]>,
    /// Every frame the cable sent during the read (all opcodes), in order.
    pub received: Vec<Frame>,
}

/// Drive a live VIN read on the `f3` (engine) channel.
///
/// Sequence: [`BRINGUP`] → [`POST_AUTH`] choreography → a TesterPresent (to
/// confirm/keep the diagnostic session) → the encoded `ReadDataByIdentifier
/// F1 90` request → collect `f3` `b7` responses and reassemble the ISO-TP
/// multiframe into `62 F1 90 <17 ASCII>`.
///
/// The request block is crafted with [`crate::link::encode_f3_request`] and the
/// counter/trailer are stamped by the recovered off15 rule ([`crate::link::
/// f3_trailer`]). Read-only UDS: RDBI F1 90 reads the VIN, changes no car state.
///
/// This is the owner's hardware experiment: whether THIS cable's firmware keys
/// the link (new RSA-OAEP build) and accepts the replayed choreography is
/// verifiable only live. The report surfaces every decoded block so a partial
/// or unexpected response is still diagnosable.
pub async fn vin_read<B: Backend>(
    backend: &mut B,
    listen: Duration,
) -> Result<VinReport, HexError> {
    use crate::link::{IsoTpReassembler, KS_F3, decode_diag_frame, decrypt_block, encode_f3_request};

    let mut buf: Vec<u8> = Vec::new();
    let mut received: Vec<Frame> = Vec::new();
    let mut raw: Vec<u8> = Vec::new();

    // 1) Bring-up + post-auth choreography (verbatim replay).
    for &(opcode, payload) in BRINGUP.iter().chain(POST_AUTH) {
        backend
            .write(&frame::frame_encode(MARKER_HOST, opcode, payload))
            .await?;
        collect_for(backend, &mut buf, &mut received, &mut raw, STEP_READ).await?;
    }

    // 2) TesterPresent on f3 to confirm the diagnostic session is live.
    backend
        .write(&frame::frame_encode(MARKER_HOST, frame::OP_DIAG_REQ, &TP_B8_BLOCK))
        .await?;
    collect_for(backend, &mut buf, &mut received, &mut raw, Duration::from_millis(300)).await?;

    // 3) The VIN request: ReadDataByIdentifier DID F1 90 on the f3 channel.
    //    off14 counter = 0x01 (the trailer is derived to match by f3_trailer).
    let vin_block = encode_f3_request(&[0x22, 0xF1, 0x90], 0x01)
        .expect("VIN PDU fits a single frame");
    backend
        .write(&frame::frame_encode(MARKER_HOST, frame::OP_DIAG_REQ, &vin_block))
        .await?;

    // 4) Collect + reassemble the multiframe f3 response.
    collect_for(backend, &mut buf, &mut received, &mut raw, listen).await?;

    let mut reasm = IsoTpReassembler::new();
    let mut decoded_blocks = Vec::new();
    let mut vin = None;
    for f in &received {
        if f.opcode != frame::OP_DIAG_RESP || f.data.len() < 16 {
            continue;
        }
        let block: [u8; 16] = f.data[..16].try_into().unwrap();
        // Only reassemble f3 responses (header off0=f3, off2=44, off3=dd).
        if !(block[0] == 0xF3 && block[2] == 0x44 && block[3] == 0xDD) {
            continue;
        }
        let dec = decrypt_block(&block, &KS_F3);
        decoded_blocks.push(dec);
        if let Some(pdu) = reasm.push_block(&dec) {
            // Positive RDBI reply: 62 F1 90 <17 ASCII>.
            if pdu.len() >= 3 && pdu[0] == 0x62 && pdu[1] == 0xF1 && pdu[2] == 0x90 {
                vin = Some(String::from_utf8_lossy(&pdu[3..]).trim().to_string());
                break;
            }
        }
        // Single-frame path (short reads) also flows through decode_diag_frame.
        let _ = decode_diag_frame(&block, &KS_F3);
    }

    Ok(VinReport {
        vin,
        decoded_blocks,
        received,
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

    #[tokio::test]
    async fn vin_read_reassembles_multiframe_response() {
        use crate::link::KS_F3;

        // Encipher a plaintext f3 response block: cipher = plain ^ KS_F3, then
        // stamp the f3 response header (off0/2/3) so vin_read's filter matches.
        fn enc_resp(plain: [u8; 16]) -> [u8; 16] {
            let mut c = [0u8; 16];
            for i in 0..16 {
                c[i] = plain[i] ^ KS_F3[i];
            }
            c[0] = 0xF3;
            c[2] = 0x44;
            c[3] = 0xDD;
            c
        }
        fn iso(pci: u8, data: &[u8], at: usize) -> [u8; 16] {
            let mut b = [0u8; 16];
            b[6] = pci;
            for (i, &d) in data.iter().enumerate() {
                b[at + i] = d;
            }
            b
        }

        // VIN response 62 F1 90 + 17 ASCII = 20 bytes over FF + 2 CF.
        let vin = b"WVWZZZ1KZ6W123456";
        let mut pdu = vec![0x62u8, 0xF1, 0x90];
        pdu.extend_from_slice(vin);

        let mut ff = iso(0x10, &[pdu.len() as u8], 7);
        for (i, &b) in pdu[..6].iter().enumerate() {
            ff[8 + i] = b;
        }
        let cf1 = iso(0x21, &pdu[6..13], 7);
        let cf2 = iso(0x22, &pdu[13..20], 7);

        let mut inbound = Vec::new();
        for blk in [ff, cf1, cf2] {
            inbound.extend(frame_encode(MARKER_CABLE, crate::frame::OP_DIAG_RESP, &enc_resp(blk)));
        }
        let mut backend = ScriptBackend::new(inbound);

        let report = vin_read(&mut backend, Duration::from_millis(200))
            .await
            .expect("vin_read runs");
        assert_eq!(report.vin.as_deref(), Some("WVWZZZ1KZ6W123456"));
    }
}
