//! Dynamic session driver: bring-up → advance past the `0x39` auth-stall →
//! `f3` TesterPresent → (if positive) VIN read.
//!
//! Unlike [`crate::probe`]'s verbatim replays, this driver **tracks the cable's
//! per-channel plaintext counter (block off14) at runtime and derives every
//! outbound counter from it** — it never hardcodes a value (the counter is a
//! free-running/session field; see [`crate::link::paired_off14`]).
//!
//! ## What it does (owner's hardware experiment)
//! After the plaintext [`BRINGUP`], the cable is observed *stuck* in auth: it
//! repeatedly pushes the `0x39`-channel `b7` status block (a free-running off14).
//! To advance, the host must send the `0x39` auth-completion `b8` (the captured
//! [`AUTH39_BLOCK`], off14 restamped to the cable's current counter epoch). The
//! cable should then stop repeating the `0x39` block and move on (in the capture
//! it advanced to channel `0x38`, then to the `f3` engine channel). The driver
//! then sends an `f3` TesterPresent and looks for a decoded `7E` positive reply;
//! if it gets one, it immediately reads the VIN (`22 F1 90`).
//!
//! Read-only UDS throughout (TesterPresent is a keepalive no-op; RDBI reads the
//! VIN). Everything the cable sends is captured in the report so a stall is
//! diagnosable.

use std::collections::HashMap;
use std::time::Duration;

use crate::error::HexError;
use crate::frame::{self, Frame, MARKER_CABLE, MARKER_HOST, OP_DIAG_REQ, OP_DIAG_RESP};
use crate::link::{IsoTpReassembler, KS_F3, decrypt_block, encode_f3_request, paired_off14};
use crate::probe::BRINGUP;
use crate::usb::Backend;

/// The captured `0x39` auth-completion `b8` block (ciphertext), from
/// `research/reading-ecus.pcapng` seq 41. off14 (index 14) is the plaintext
/// counter — restamped per session; off15 (index 15 = `0xcd`) is the channel's
/// constant trailer (every `0x39` frame in the capture carries `0xcd`). The rest
/// (off0..13) is the fixed encrypted auth-completion payload, replayed verbatim.
pub const AUTH39_BLOCK: [u8; 16] = [
    0x39, 0xc7, 0x0a, 0x5d, 0xe7, 0x72, 0xcf, 0xa5, 0x6e, 0xfb, 0x41, 0xc6, 0x4c, 0xab, 0x38, 0xcd,
];

/// Channel id (block off0) of the `0x39` auth channel.
const CHAN_AUTH: u8 = 0x39;
/// Channel id (block off0) of the `f3` engine channel.
const CHAN_F3: u8 = 0xF3;
/// off14 of [`AUTH39_BLOCK`] — the capture's value, used only as a seed when the
/// cable has not pushed a `0x39` counter to derive from.
const AUTH39_SEED_OFF14: u8 = AUTH39_BLOCK[14];

/// One inter-step drain window (read acks between sends).
const STEP_READ: Duration = Duration::from_millis(120);

/// Result of the dynamic session drive.
#[derive(Debug, Default)]
pub struct DriveReport {
    /// off14 values the cable pushed on the `0x39` channel before we answered
    /// (the free-running counter we derived our auth off14 from).
    pub observed_auth_off14: Vec<u8>,
    /// The off14 we stamped on the auth-completion `b8` we sent.
    pub sent_auth_off14: u8,
    /// Did the cable advance past auth — i.e. did it send a `b7` on any channel
    /// other than `0x39` after we sent the auth-completion?
    pub advanced: bool,
    /// The off14 we stamped on the `f3` TesterPresent `b8` we sent.
    pub sent_tp_off14: u8,
    /// Did an `f3` `b7` decode to a UDS TesterPresent positive (`7E`)?
    pub tp_positive: bool,
    /// The VIN, if a `62 F1 90` response reassembled after a positive TP.
    pub vin: Option<String>,
    /// Every `f3` `b7` block decoded with [`KS_F3`] (off0..15), for inspection.
    pub f3_decoded_blocks: Vec<[u8; 16]>,
    /// Every frame the cable sent across the whole drive, in arrival order.
    pub received: Vec<Frame>,
    /// Human-readable step log (what we sent, what we saw) for diagnostics.
    pub log: Vec<String>,
}

/// Read the block off0 (channel) and off14 (counter) out of a diagnostic frame.
fn diag_chan_and_off14(f: &Frame) -> Option<(u8, u8)> {
    if (f.opcode != OP_DIAG_RESP && f.opcode != OP_DIAG_REQ) || f.data.len() < 16 {
        return None;
    }
    Some((f.data[0], f.data[14]))
}

/// Drain cable frames for up to `window`, appending to `out`, and update
/// `last_off14` (channel → most-recent off14) from every diagnostic frame seen.
async fn collect<B: Backend>(
    backend: &mut B,
    buf: &mut Vec<u8>,
    out: &mut Vec<Frame>,
    last_off14: &mut HashMap<u8, u8>,
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
            Err(_) => return Ok(()),
            Ok(Ok(0)) => tokio::time::sleep(Duration::from_millis(2)).await,
            Ok(Ok(n)) => {
                buf.extend_from_slice(&scratch[..n]);
                while let Some((f, consumed)) = frame::take_frame(buf, MARKER_CABLE) {
                    buf.drain(..consumed);
                    if let Some((chan, off14)) = diag_chan_and_off14(&f) {
                        last_off14.insert(chan, off14);
                    }
                    out.push(f);
                }
            }
            Ok(Err(e)) => return Err(e),
        }
    }
}

/// Send one diagnostic `b8` block, framed.
async fn send_b8<B: Backend>(backend: &mut B, block: &[u8; 16]) -> Result<(), HexError> {
    backend
        .write(&frame::frame_encode(MARKER_HOST, OP_DIAG_REQ, block))
        .await
}

/// Drive the dynamic session: bring-up → auth-completion → TesterPresent → VIN.
///
/// `listen` bounds each post-send observation window. Read-only UDS. See the
/// module docs for the full choreography and the open question (does [`KS_F3`],
/// recovered from capture frames *after* 15 `b6` re-auths, decode a session we
/// bootstrap with only the first `b6`?) that the live `7E` result answers.
pub async fn drive_session<B: Backend>(
    backend: &mut B,
    listen: Duration,
) -> Result<DriveReport, HexError> {
    let mut buf: Vec<u8> = Vec::new();
    let mut received: Vec<Frame> = Vec::new();
    let mut last_off14: HashMap<u8, u8> = HashMap::new();
    let mut report = DriveReport::default();

    // 1) Plaintext bring-up (verbatim). Drain acks + any cable pushes; this is
    //    where the cable starts repeating its 0x39 auth block.
    for &(opcode, payload) in BRINGUP {
        backend
            .write(&frame::frame_encode(MARKER_HOST, opcode, payload))
            .await?;
        collect(backend, &mut buf, &mut received, &mut last_off14, STEP_READ).await?;
    }
    // Listen a little longer for the cable's 0x39 push stream to establish its
    // current counter epoch.
    collect(backend, &mut buf, &mut received, &mut last_off14, listen).await?;

    // 2) Derive the auth-completion off14 from the cable's current 0x39 counter
    //    (paired_off14 = flip bit0), falling back to the captured seed if the
    //    cable has not pushed a 0x39 block yet.
    let auth_off14 = match last_off14.get(&CHAN_AUTH) {
        Some(&cnt) => {
            report.observed_auth_off14.push(cnt);
            paired_off14(cnt)
        }
        None => AUTH39_SEED_OFF14,
    };
    report.sent_auth_off14 = auth_off14;
    report.log.push(format!(
        "auth-completion: observed 0x39 counter {:?}, sending b8 off14={:#04x}",
        last_off14.get(&CHAN_AUTH),
        auth_off14
    ));

    let mut auth_block = AUTH39_BLOCK;
    auth_block[14] = auth_off14; // off15 stays 0xcd (channel constant)
    let recv_before = received.len();
    send_b8(backend, &auth_block).await?;
    collect(backend, &mut buf, &mut received, &mut last_off14, listen).await?;

    // 3) Advanced? The cable advances if it emits a b7 on any channel != 0x39
    //    after our auth-completion (in the capture it moved to 0x38, then f3).
    report.advanced = received[recv_before..].iter().any(|f| {
        matches!(diag_chan_and_off14(f), Some((chan, _)) if chan != CHAN_AUTH)
    });
    report.log.push(format!(
        "post-auth: {} new frame(s); advanced past 0x39 = {}",
        received.len() - recv_before,
        report.advanced
    ));

    // 4) f3 TesterPresent. Derive off14 from any f3 counter we've seen, else the
    //    capture's TP seed (0x00).
    let tp_off14 = match last_off14.get(&CHAN_F3) {
        Some(&cnt) => paired_off14(cnt),
        None => 0x00,
    };
    report.sent_tp_off14 = tp_off14;
    let tp_block = encode_f3_request(&[0x3E, 0x00], tp_off14).expect("TP PDU fits a single frame");
    let recv_before = received.len();
    send_b8(backend, &tp_block).await?;
    collect(backend, &mut buf, &mut received, &mut last_off14, listen).await?;

    // 5) Decode the f3 responses; detect a TesterPresent positive (off7 == 0x7E).
    for f in &received[recv_before..] {
        if !is_f3_response(f) {
            continue;
        }
        let block: [u8; 16] = f.data[..16].try_into().unwrap();
        let dec = decrypt_block(&block, &KS_F3);
        report.f3_decoded_blocks.push(dec);
        if dec[7] == 0x7E {
            report.tp_positive = true;
        }
    }
    report.log.push(format!(
        "TesterPresent: sent f3 b8 off14={tp_off14:#04x}; positive 7E = {}",
        report.tp_positive
    ));

    // 6) If TP came back positive, read the VIN on the same channel.
    if report.tp_positive {
        let vin_off14 = match last_off14.get(&CHAN_F3) {
            Some(&cnt) => paired_off14(cnt),
            None => 0x01,
        };
        let vin_block =
            encode_f3_request(&[0x22, 0xF1, 0x90], vin_off14).expect("VIN PDU fits a single frame");
        let recv_before = received.len();
        send_b8(backend, &vin_block).await?;
        collect(backend, &mut buf, &mut received, &mut last_off14, listen).await?;

        let mut reasm = IsoTpReassembler::new();
        for f in &received[recv_before..] {
            if !is_f3_response(f) {
                continue;
            }
            let block: [u8; 16] = f.data[..16].try_into().unwrap();
            let dec = decrypt_block(&block, &KS_F3);
            report.f3_decoded_blocks.push(dec);
            if let Some(pdu) = reasm.push_block(&dec)
                && pdu.len() >= 3
                && pdu[0] == 0x62
                && pdu[1] == 0xF1
                && pdu[2] == 0x90
            {
                report.vin = Some(String::from_utf8_lossy(&pdu[3..]).trim().to_string());
                break;
            }
        }
        report.log.push(format!("VIN read: sent f3 b8 off14={vin_off14:#04x}; vin = {:?}", report.vin));
    }

    report.received = received;
    Ok(report)
}

/// Is this an `f3` engine-channel response block? (off0=f3, off2=44, off3=dd.)
fn is_f3_response(f: &Frame) -> bool {
    f.opcode == OP_DIAG_RESP
        && f.data.len() >= 16
        && f.data[0] == 0xF3
        && f.data[2] == 0x44
        && f.data[3] == 0xDD
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::frame_encode;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// Reactive mock: records writes; when the host writes a diagnostic `b8`
    /// whose block channel (off0) matches a key in `react`, enqueues the scripted
    /// inbound bytes for that channel. This lets a test assert the driver observes
    /// the cable's counter *then* emits a correctly-derived off14.
    struct DriveMock {
        inbox: VecDeque<u8>,
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
        react: HashMap<u8, Vec<u8>>,
    }

    impl Backend for DriveMock {
        async fn write(&mut self, bytes: &[u8]) -> Result<(), HexError> {
            self.writes.lock().unwrap().push(bytes.to_vec());
            // Frame = [S][len][opcode][data..][xor]; block starts at index 3.
            if bytes.len() >= 4 && bytes[2] == OP_DIAG_REQ && bytes.len() >= 3 + 16 {
                let chan = bytes[3];
                if let Some(reply) = self.react.get(&chan) {
                    self.inbox.extend(reply.iter().copied());
                }
            }
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

    /// A `0x39` cable push block with a given off14 (ciphertext replayed from the
    /// capture's stuck block, off14 restamped).
    fn auth39_push(off14: u8) -> Vec<u8> {
        let mut blk = [
            0x39, 0x38, 0x82, 0x5d, 0xf7, 0x7d, 0xf0, 0x75, 0x6e, 0xeb, 0x41, 0xc5, 0x4d, 0x2b,
            off14, 0xcd,
        ];
        blk[14] = off14;
        frame_encode(MARKER_CABLE, OP_DIAG_RESP, &blk)
    }

    /// A `0x38` cable push block — proves the cable advanced off the 0x39 channel.
    fn chan38_push() -> Vec<u8> {
        let blk = [
            0x38, 0x38, 0x82, 0x5d, 0xf7, 0x7d, 0xf0, 0x75, 0x6e, 0xeb, 0x41, 0xc5, 0x4d, 0x2b,
            0x3c, 0xcd,
        ];
        frame_encode(MARKER_CABLE, OP_DIAG_RESP, &blk)
    }

    /// An f3 response block enciphered from a plaintext block (cipher = plain ^
    /// KS_F3), with the f3 response header stamped so `is_f3_response` matches.
    fn f3_resp(plain: [u8; 16]) -> Vec<u8> {
        let mut c = [0u8; 16];
        for i in 0..16 {
            c[i] = plain[i] ^ KS_F3[i];
        }
        c[0] = 0xF3;
        c[2] = 0x44;
        c[3] = 0xDD;
        frame_encode(MARKER_CABLE, OP_DIAG_RESP, &c)
    }

    #[tokio::test]
    async fn derives_auth_off14_from_observed_counter_and_reaches_vin() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        // Cable pushes its 0x39 stuck block twice (counter 0x50, then 0x51) before
        // we answer. Our auth off14 should be paired_off14(0x51) = 0x50.
        let mut inbox: VecDeque<u8> = VecDeque::new();
        inbox.extend(auth39_push(0x50));
        inbox.extend(auth39_push(0x51));

        // Build the f3 TesterPresent positive response (off6..= 05 7E 00 ..).
        let mut tp_plain = [0u8; 16];
        tp_plain[6] = 0x05;
        tp_plain[7] = 0x7E;
        // VIN response 62 F1 90 + 17 ASCII over FF + 2 CF.
        let vin = b"WVWZZZ1KZ6W123456";
        let mut pdu = vec![0x62u8, 0xF1, 0x90];
        pdu.extend_from_slice(vin);
        let mut ff = [0u8; 16];
        ff[6] = 0x10;
        ff[7] = pdu.len() as u8;
        for (i, &b) in pdu[..6].iter().enumerate() {
            ff[8 + i] = b;
        }
        let mut cf1 = [0u8; 16];
        cf1[6] = 0x21;
        cf1[7..14].copy_from_slice(&pdu[6..13]);
        let mut cf2 = [0u8; 16];
        cf2[6] = 0x22;
        cf2[7..14].copy_from_slice(&pdu[13..20]);

        let mut react: HashMap<u8, Vec<u8>> = HashMap::new();
        // On the auth-completion b8 (chan 0x39): cable advances to chan 0x38.
        react.insert(0x39, chan38_push());
        // On the f3 b8: first send TP positive, then (on the VIN b8) the multiframe.
        // Both f3 sends key on chan 0xf3; queue TP then the VIN frames in order.
        let mut f3_reply = f3_resp(tp_plain);
        f3_reply.extend(f3_resp(ff));
        f3_reply.extend(f3_resp(cf1));
        f3_reply.extend(f3_resp(cf2));
        react.insert(0xF3, f3_reply);

        let mut backend = DriveMock {
            inbox,
            writes: writes.clone(),
            react,
        };

        let report = drive_session(&mut backend, Duration::from_millis(60))
            .await
            .expect("drive runs");

        // Derived the auth off14 dynamically from the cable's counter (0x51 ^ 1).
        assert_eq!(report.observed_auth_off14, vec![0x51]);
        assert_eq!(report.sent_auth_off14, 0x50);
        // The recorded auth-completion b8 carries that off14 at block index 14.
        let auth_b8 = writes
            .lock()
            .unwrap()
            .iter()
            .find(|w| w.len() >= 4 && w[2] == OP_DIAG_REQ && w[3] == 0x39)
            .cloned()
            .expect("sent a 0x39 auth b8");
        assert_eq!(auth_b8[3 + 14], 0x50, "auth b8 off14 restamped");
        assert_eq!(auth_b8[3 + 15], 0xcd, "0x39 trailer constant preserved");

        assert!(report.advanced, "cable advanced off 0x39");
        assert!(report.tp_positive, "f3 TesterPresent positive 7E decoded");
        assert_eq!(report.vin.as_deref(), Some("WVWZZZ1KZ6W123456"));
    }

    #[tokio::test]
    async fn falls_back_to_seed_when_no_counter_observed() {
        // Cable pushes nothing before we answer → use the captured seed off14.
        let writes = Arc::new(Mutex::new(Vec::new()));
        let backend_writes = writes.clone();
        let mut backend = DriveMock {
            inbox: VecDeque::new(),
            writes: backend_writes,
            react: HashMap::new(),
        };
        let report = drive_session(&mut backend, Duration::from_millis(30))
            .await
            .expect("drive runs");
        assert!(report.observed_auth_off14.is_empty());
        assert_eq!(report.sent_auth_off14, AUTH39_SEED_OFF14);
        assert!(!report.advanced);
        assert!(!report.tp_positive);
        assert!(report.vin.is_none());
    }
}
