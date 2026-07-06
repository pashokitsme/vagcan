//! Session-replay diagnostic reader: replay a recorded host→cable frame
//! sequence to bring the cable's session up to the engine ECU's `f3` channel,
//! then issue a UDS read (VIN) on that channel.
//!
//! ## Why replay the WHOLE sequence
//! The cable is a session-oriented transport: a diagnostic channel to a given
//! ECU only becomes active after the correct ordered sequence of setup frames
//! has been exchanged from a fresh power-on. The engine `f3` channel is the
//! *fifteenth* `b6` epoch in the capture (`research/vag-hex-framing.md`), so
//! partial-sequence experiments never reached it — its keystream ([`KS_F3`]) is
//! locked to the session state the full ordered replay reconstructs. This module
//! therefore re-sends the complete recorded OUT-frame sequence in order (the IN
//! frames are the recorded responses, used to detect where a live replay diverges
//! from the recording) up to the `f3`-channel frame, then issues its own UDS
//! request on that channel.
//!
//! ## Seam
//! The replay loop runs over a [`FrameTransport`] (send one frame / receive the
//! next) so the `--dry-run` path and the unit tests drive it with no hardware —
//! mirroring the backend seam in [`crate::probe`] / [`crate::drive`]. The live
//! path wraps a [`Backend`] in [`CableTransport`]; tests use an in-memory mock.
//!
//! Read-only UDS throughout: the default read is ReadDataByIdentifier `F1 90`
//! (VIN); nothing here writes to the vehicle.

use std::time::Duration;

use crate::error::HexError;
use crate::frame::{self, MARKER_CABLE, MARKER_HOST, OP_DIAG_REQ, OP_DIAG_RESP};
use crate::link::{
    IsoTpReassembler, KS_F3, decode_diag_frame, decrypt_block, encode_f3_request, paired_off14,
};
use crate::usb::Backend;

/// Channel id (block off0) of the `f3` engine channel.
const CHAN_F3: u8 = 0xF3;

/// Direction of a recorded frame, in the wire sense.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// Host→cable (`0x53 'S'`): what the replay re-sends.
    Out,
    /// Cable→host (`0x4D 'M'`): the recorded expected response.
    In,
}

/// One recorded frame from the replay stream.
///
/// `payload` is the frame opcode byte followed by its data (i.e. the bytes after
/// the `S`/`M` marker+len, before the trailing XOR) — exactly what
/// [`FrameTransport::send`] re-frames and writes for an OUT frame, and what
/// [`FrameTransport::recv`] returns for a cable IN frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayFrame {
    /// Position in the recorded stream (0-based; the `--target-index` key).
    pub idx: usize,
    /// Wire direction.
    pub dir: Dir,
    /// Opcode byte + data (no marker/len/xor).
    pub payload: Vec<u8>,
}

/// A recorded IN frame that did not match what the live cable actually sent —
/// the exact point where a live replay diverged from the recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence {
    /// Stream index of the IN frame that mismatched.
    pub idx: usize,
    /// The bytes the recording expected (opcode + data).
    pub expected: Vec<u8>,
    /// The bytes the live cable actually sent (opcode + data).
    pub observed: Vec<u8>,
}

/// Outcome of a live replay drive.
#[derive(Debug, Default)]
pub struct ReplayReport {
    /// The index the replay was asked to reach.
    pub target_index: usize,
    /// OUT frames re-sent to the cable.
    pub sent_out: usize,
    /// Set if a recorded IN frame did not match the live cable (replay stopped).
    pub divergence: Option<Divergence>,
    /// Reached `target_index` with no divergence.
    pub reached_target: bool,
    /// The UDS PDU we sent on the `f3` channel once the target was reached.
    pub read_pdu: Vec<u8>,
    /// The block off14 counter stamped on that `f3` read (derived at runtime).
    pub sent_read_off14: u8,
    /// The decoded UDS response PDU, if the cable answered.
    pub response_pdu: Option<Vec<u8>>,
    /// The ASCII VIN, if the response was a `62 F1 90` ReadDataByIdentifier.
    pub vin: Option<String>,
    /// Human-readable step log.
    pub log: Vec<String>,
}

/// A bidirectional frame channel the replay loop drives.
///
/// `send` takes a recorded OUT payload (opcode + data) and is responsible for the
/// `S` marker + length + XOR framing; `recv` returns the next cable frame's
/// payload (opcode + data), or [`HexError::Timeout`] if none arrives. Declared
/// with `-> impl Future + Send` to mirror [`Backend`]; impls may use plain
/// `async fn` bodies.
pub trait FrameTransport {
    /// Frame and send one recorded OUT payload (opcode + data).
    fn send(&mut self, payload: &[u8]) -> impl Future<Output = Result<(), HexError>> + Send;
    /// Receive the next cable frame; returns its payload (opcode + data).
    fn recv(&mut self) -> impl Future<Output = Result<Vec<u8>, HexError>> + Send;
}

/// A [`FrameTransport`] over the live cable [`Backend`].
pub struct CableTransport<'a, B: Backend> {
    backend: &'a mut B,
    buf: Vec<u8>,
    recv_window: Duration,
}

impl<'a, B: Backend> CableTransport<'a, B> {
    /// Wrap a backend; `recv_window` bounds how long a single [`recv`](FrameTransport::recv)
    /// waits for a complete cable frame before returning [`HexError::Timeout`].
    pub fn new(backend: &'a mut B, recv_window: Duration) -> Self {
        Self {
            backend,
            buf: Vec::new(),
            recv_window,
        }
    }
}

impl<B: Backend> FrameTransport for CableTransport<'_, B> {
    async fn send(&mut self, payload: &[u8]) -> Result<(), HexError> {
        let opcode = *payload
            .first()
            .ok_or_else(|| HexError::Framing("empty OUT payload has no opcode".into()))?;
        let wire = frame::frame_encode(MARKER_HOST, opcode, &payload[1..]);
        self.backend.write(&wire).await
    }

    async fn recv(&mut self) -> Result<Vec<u8>, HexError> {
        let deadline = tokio::time::Instant::now() + self.recv_window;
        let mut scratch = [0u8; 4096];
        loop {
            // Cut a complete cable frame out of the buffer if one is present.
            if let Some((f, consumed)) = frame::take_frame(&self.buf, MARKER_CABLE) {
                self.buf.drain(..consumed);
                let mut payload = Vec::with_capacity(1 + f.data.len());
                payload.push(f.opcode);
                payload.extend_from_slice(&f.data);
                return Ok(payload);
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(HexError::Timeout);
            }
            match tokio::time::timeout(remaining, self.backend.read(&mut scratch)).await {
                Err(_) => return Err(HexError::Timeout),
                Ok(Ok(0)) => tokio::time::sleep(Duration::from_millis(2)).await,
                Ok(Ok(n)) => self.buf.extend_from_slice(&scratch[..n]),
                Ok(Err(e)) => return Err(e),
            }
        }
    }
}

/// The block off14 counter of a diagnostic frame on the `f3` channel, if this
/// payload is one. `payload[0]` = opcode (`b8`/`b7`), `payload[1..17]` = the
/// 16-byte block, so block off0 = `payload[1]` and block off14 = `payload[15]`.
fn f3_off14(payload: &[u8]) -> Option<u8> {
    if payload.len() >= 17
        && matches!(payload[0], OP_DIAG_REQ | OP_DIAG_RESP)
        && payload[1] == CHAN_F3
    {
        Some(payload[15])
    } else {
        None
    }
}

/// Is this payload an `f3`-channel diagnostic frame whose block decodes under
/// [`KS_F3`] to a sane single-frame UDS PDU (TesterPresent `3E` or
/// ReadDataByIdentifier `22 xx xx`)? Used to VERIFY the `f3`-channel index from
/// the data rather than trust a hardcoded position.
fn is_f3_uds_frame(payload: &[u8]) -> bool {
    if payload.len() < 17 || !matches!(payload[0], OP_DIAG_REQ | OP_DIAG_RESP) || payload[1] != CHAN_F3
    {
        return false;
    }
    match decode_diag_frame(&payload[1..], &KS_F3) {
        Some(u) => matches!(
            u.uds.as_slice(),
            [0x3E, ..] | [0x22, _, _, ..]
        ),
        None => false,
    }
}

/// Find the stream index at which the engine `f3` channel becomes active — the
/// first frame whose block decodes under [`KS_F3`] to sane single-frame UDS.
#[must_use]
pub fn f3_channel_index(stream: &[ReplayFrame]) -> Option<usize> {
    stream
        .iter()
        .find(|fr| is_f3_uds_frame(&fr.payload))
        .map(|fr| fr.idx)
}

// --------------------------------------------------------------------------
// Stream (JSONL) parsing
// --------------------------------------------------------------------------

/// Parse the JSONL replay stream (one `{"idx":N,"dir":"out"|"in","payload":"<hex>"}`
/// per line) into an ordered list of [`ReplayFrame`]. Blank lines are skipped.
pub fn parse_stream(text: &str) -> Result<Vec<ReplayFrame>, HexError> {
    let mut out = Vec::new();
    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fr = parse_line(line)
            .map_err(|e| HexError::Framing(format!("stream line {}: {e}", lineno + 1)))?;
        out.push(fr);
    }
    Ok(out)
}

/// Serialize one frame back to its JSONL line (round-trips [`parse_stream`]).
#[must_use]
pub fn to_jsonl_line(fr: &ReplayFrame) -> String {
    let dir = match fr.dir {
        Dir::Out => "out",
        Dir::In => "in",
    };
    let hex: String = fr.payload.iter().map(|b| format!("{b:02x}")).collect();
    format!("{{\"idx\":{},\"dir\":\"{dir}\",\"payload\":\"{hex}\"}}", fr.idx)
}

fn parse_line(line: &str) -> Result<ReplayFrame, String> {
    let idx = json_uint(line, "idx")?;
    let dir = match json_string(line, "dir")?.as_str() {
        "out" => Dir::Out,
        "in" => Dir::In,
        other => return Err(format!("bad dir {other:?} (want \"out\"/\"in\")")),
    };
    let payload = parse_hex(&json_string(line, "payload")?).map_err(|e| e.to_string())?;
    Ok(ReplayFrame { idx, dir, payload })
}

/// Extract an unsigned integer value for `"key":` from a flat JSON object line.
fn json_uint(line: &str, key: &str) -> Result<usize, String> {
    let pat = format!("\"{key}\":");
    let start = line.find(&pat).ok_or_else(|| format!("missing key {key:?}"))? + pat.len();
    let rest = line[start..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end]
        .parse::<usize>()
        .map_err(|_| format!("bad integer for {key:?}: {:?}", &rest[..end]))
}

/// Extract a string value for `"key":` from a flat JSON object line.
fn json_string(line: &str, key: &str) -> Result<String, String> {
    let pat = format!("\"{key}\":");
    let start = line.find(&pat).ok_or_else(|| format!("missing key {key:?}"))? + pat.len();
    let rest = line[start..].trim_start();
    let rest = rest
        .strip_prefix('"')
        .ok_or_else(|| format!("value for {key:?} is not a string"))?;
    let end = rest.find('"').ok_or_else(|| format!("unterminated string for {key:?}"))?;
    Ok(rest[..end].to_string())
}

/// Parse a hex string (whitespace ignored) into bytes.
pub fn parse_hex(s: &str) -> Result<Vec<u8>, HexError> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.len() % 2 != 0 {
        return Err(HexError::Framing(format!("odd-length hex string {s:?}")));
    }
    (0..cleaned.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&cleaned[i..i + 2], 16)
                .map_err(|_| HexError::Framing(format!("bad hex byte in {s:?}")))
        })
        .collect()
}

// --------------------------------------------------------------------------
// Dry-run planning (no hardware)
// --------------------------------------------------------------------------

/// What a `--dry-run` would do: it exercises the encode/decode path without
/// opening the cable, so CI and the unit tests can validate the plan.
#[derive(Debug, Clone)]
pub struct DryRunPlan {
    /// The resolved target index.
    pub target_index: usize,
    /// OUT frames that would be re-sent up to and including the target.
    pub out_up_to_target: usize,
    /// IN frames that would be compared up to and including the target.
    pub in_up_to_target: usize,
    /// The UDS read PDU that would be sent on the `f3` channel.
    pub read_pdu: Vec<u8>,
    /// The block off14 counter that would be stamped (derived from the last
    /// `f3` frame at/before the target).
    pub read_off14: u8,
    /// The encoded 16-byte `f3` request block.
    pub encoded_read: [u8; 16],
    /// The PDU recovered by decoding `encoded_read` — must equal `read_pdu`.
    pub decoded_read_pdu: Vec<u8>,
    /// Did `encode`→`decode` round-trip the read PDU?
    pub round_trip_ok: bool,
}

/// Plan (but do not execute) a replay drive: validate the target exists, derive
/// the read counter, and confirm the read PDU encodes and decodes back.
pub fn plan_dry_run(
    stream: &[ReplayFrame],
    target_index: usize,
    read_pdu: &[u8],
) -> Result<DryRunPlan, HexError> {
    if !stream.iter().any(|f| f.idx == target_index) {
        return Err(HexError::Framing(format!(
            "target index {target_index} is not present in the stream"
        )));
    }
    let mut out_up = 0usize;
    let mut in_up = 0usize;
    let mut last_f3: Option<u8> = None;
    for fr in stream.iter().filter(|f| f.idx <= target_index) {
        match fr.dir {
            Dir::Out => out_up += 1,
            Dir::In => in_up += 1,
        }
        if let Some(off14) = f3_off14(&fr.payload) {
            last_f3 = Some(off14);
        }
    }
    let read_off14 = last_f3.map_or(0x00, paired_off14);
    let encoded_read = encode_f3_request(read_pdu, read_off14)
        .ok_or_else(|| HexError::Framing("read PDU does not fit an f3 single frame".into()))?;
    let decoded = decode_diag_frame(&encoded_read, &KS_F3)
        .ok_or_else(|| HexError::Framing("encoded read PDU did not decode as a single frame".into()))?;
    let round_trip_ok = decoded.uds == read_pdu;
    Ok(DryRunPlan {
        target_index,
        out_up_to_target: out_up,
        in_up_to_target: in_up,
        read_pdu: read_pdu.to_vec(),
        read_off14,
        encoded_read,
        decoded_read_pdu: decoded.uds,
        round_trip_ok,
    })
}

// --------------------------------------------------------------------------
// Live replay drive
// --------------------------------------------------------------------------

/// Replay the recorded stream up to `target_index`, then read `read_pdu` on the
/// `f3` engine channel.
///
/// Re-sends each OUT frame in order; after each recorded IN frame it receives the
/// live cable's frame and compares it byte-for-byte. On the FIRST mismatch it
/// records a [`Divergence`] (the exact idx where the live session left the
/// recording) and stops. On reaching `target_index` with no divergence it encodes
/// `read_pdu` with [`encode_f3_request`], stamping the next counter derived from
/// the last `f3` frame's off14 via [`paired_off14`] (never hardcoded), sends it,
/// and decodes the response with [`decrypt_block`] + [`IsoTpReassembler`]. A VIN
/// read (`22 F1 90`) yields the ASCII VIN.
pub async fn replay_drive<T: FrameTransport>(
    transport: &mut T,
    stream: &[ReplayFrame],
    target_index: usize,
    read_pdu: &[u8],
) -> Result<ReplayReport, HexError> {
    let mut report = ReplayReport {
        target_index,
        read_pdu: read_pdu.to_vec(),
        ..Default::default()
    };
    let mut last_f3_off14: Option<u8> = None;

    // 1) Replay the ordered sequence up to and including the target.
    for fr in stream.iter().filter(|f| f.idx <= target_index) {
        match fr.dir {
            Dir::Out => {
                transport.send(&fr.payload).await?;
                report.sent_out += 1;
            }
            Dir::In => {
                let observed = transport.recv().await?;
                if observed != fr.payload {
                    report.log.push(format!(
                        "DIVERGENCE at idx {}: live cable frame did not match the recording",
                        fr.idx
                    ));
                    report.divergence = Some(Divergence {
                        idx: fr.idx,
                        expected: fr.payload.clone(),
                        observed,
                    });
                    return Ok(report);
                }
            }
        }
        if let Some(off14) = f3_off14(&fr.payload) {
            last_f3_off14 = Some(off14);
        }
    }
    report.reached_target = true;
    report.log.push(format!(
        "reached target idx {target_index} with no divergence ({} OUT frames re-sent)",
        report.sent_out
    ));

    // 2) Encode + send our own read on the f3 channel, counter continuing the
    //    sequence from the last f3 frame we saw.
    let read_off14 = last_f3_off14.map_or(0x00, paired_off14);
    report.sent_read_off14 = read_off14;
    let block = encode_f3_request(read_pdu, read_off14)
        .ok_or_else(|| HexError::Framing("read PDU does not fit an f3 single frame".into()))?;
    let mut read_payload = Vec::with_capacity(1 + block.len());
    read_payload.push(OP_DIAG_REQ);
    read_payload.extend_from_slice(&block);
    transport.send(&read_payload).await?;
    report.log.push(format!(
        "sent f3 read {} with off14={read_off14:#04x}",
        read_pdu.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
    ));

    // 3) Collect + decode the response (single- or multi-frame ISO-TP).
    let mut reasm = IsoTpReassembler::new();
    for _ in 0..64 {
        match transport.recv().await {
            Ok(p) => {
                if p.len() >= 17 && p[0] == OP_DIAG_RESP && p[1] == CHAN_F3 {
                    let cipher: [u8; 16] = p[1..17].try_into().expect("17-byte payload");
                    let dec = decrypt_block(&cipher, &KS_F3);
                    if let Some(pdu) = reasm.push_block(&dec) {
                        if pdu.len() >= 3 && pdu[0] == 0x62 && pdu[1] == 0xF1 && pdu[2] == 0x90 {
                            report.vin =
                                Some(String::from_utf8_lossy(&pdu[3..]).trim().to_string());
                        }
                        report.response_pdu = Some(pdu);
                        break;
                    }
                }
            }
            Err(HexError::Timeout) => break, // cable went quiet — stop collecting
            Err(e) => return Err(e),
        }
    }
    report.log.push(format!(
        "f3 response: pdu={:02x?}, vin={:?}",
        report.response_pdu, report.vin
    ));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// In-memory transport: `recv` pops the next scripted frame (Timeout when
    /// empty); `send` records what the replay wrote.
    struct MockTransport {
        recvs: VecDeque<Vec<u8>>,
        sent: Vec<Vec<u8>>,
    }
    impl MockTransport {
        fn new(recvs: Vec<Vec<u8>>) -> Self {
            Self {
                recvs: recvs.into(),
                sent: Vec::new(),
            }
        }
    }
    impl FrameTransport for MockTransport {
        async fn send(&mut self, payload: &[u8]) -> Result<(), HexError> {
            self.sent.push(payload.to_vec());
            Ok(())
        }
        async fn recv(&mut self) -> Result<Vec<u8>, HexError> {
            self.recvs.pop_front().ok_or(HexError::Timeout)
        }
    }

    /// An `f3` request payload (opcode `b8` + 16-byte block) for a UDS PDU.
    fn f3_out(pdu: &[u8], off14: u8) -> Vec<u8> {
        let block = encode_f3_request(pdu, off14).expect("fits single frame");
        let mut p = vec![OP_DIAG_REQ];
        p.extend_from_slice(&block);
        p
    }

    /// An `f3` response payload from a plaintext block: cipher = plain ^ KS_F3,
    /// with block off0 forced to 0xF3 so the response filter matches.
    fn f3_resp(plain: [u8; 16]) -> Vec<u8> {
        let mut c = [0u8; 16];
        for i in 0..16 {
            c[i] = plain[i] ^ KS_F3[i];
        }
        c[0] = CHAN_F3;
        let mut p = vec![OP_DIAG_RESP];
        p.extend_from_slice(&c);
        p
    }

    /// The VIN response (62 F1 90 + 17 ASCII) as f3 FF + 2 CF payloads.
    fn vin_response_frames(vin: &str) -> Vec<Vec<u8>> {
        let mut pdu = vec![0x62u8, 0xF1, 0x90];
        pdu.extend_from_slice(vin.as_bytes());
        assert_eq!(pdu.len(), 20);
        let mut ff = [0u8; 16];
        ff[6] = 0x10;
        ff[7] = pdu.len() as u8;
        ff[8..14].copy_from_slice(&pdu[..6]);
        let mut cf1 = [0u8; 16];
        cf1[6] = 0x21;
        cf1[7..14].copy_from_slice(&pdu[6..13]);
        let mut cf2 = [0u8; 16];
        cf2[6] = 0x22;
        cf2[7..14].copy_from_slice(&pdu[13..20]);
        vec![f3_resp(ff), f3_resp(cf1), f3_resp(cf2)]
    }

    #[test]
    fn stream_jsonl_round_trips() {
        let frames = vec![
            ReplayFrame { idx: 0, dir: Dir::Out, payload: vec![0x02] },
            ReplayFrame { idx: 1, dir: Dir::In, payload: vec![0xFE] },
            ReplayFrame { idx: 2, dir: Dir::Out, payload: f3_out(&[0x22, 0x74, 0x58], 0xFB) },
        ];
        let jsonl: String = frames
            .iter()
            .map(to_jsonl_line)
            .collect::<Vec<_>>()
            .join("\n");
        let parsed = parse_stream(&jsonl).expect("parses");
        assert_eq!(parsed, frames);
    }

    #[test]
    fn parse_stream_skips_blank_lines() {
        let text = "\n{\"idx\":0,\"dir\":\"out\",\"payload\":\"02\"}\n\n";
        let parsed = parse_stream(text).expect("parses");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].payload, vec![0x02]);
    }

    #[test]
    fn read_pdu_encodes_and_decodes_with_ks_f3() {
        // The default read (VIN DID F1 90) must round-trip through the f3 codec.
        let pdu = [0x22u8, 0xF1, 0x90];
        let block = encode_f3_request(&pdu, 0x40).expect("fits");
        let dec = decode_diag_frame(&block, &KS_F3).expect("single frame");
        assert_eq!(dec.uds, pdu);
    }

    #[test]
    fn f3_channel_index_found_from_data() {
        let stream = vec![
            ReplayFrame { idx: 0, dir: Dir::Out, payload: vec![0x02] },
            ReplayFrame { idx: 1, dir: Dir::In, payload: vec![0xFE] },
            // A real f3 RDBI request decodes to 22 74 58 under KS_F3.
            ReplayFrame { idx: 2, dir: Dir::Out, payload: f3_out(&[0x22, 0x74, 0x58], 0xFB) },
        ];
        assert_eq!(f3_channel_index(&stream), Some(2));
    }

    #[tokio::test]
    async fn divergence_is_flagged_at_the_exact_index() {
        // idx1 is a recorded IN [0xFE]; the mock returns a differing frame.
        let stream = vec![
            ReplayFrame { idx: 0, dir: Dir::Out, payload: vec![0x02] },
            ReplayFrame { idx: 1, dir: Dir::In, payload: vec![0xFE] },
            ReplayFrame { idx: 2, dir: Dir::Out, payload: f3_out(&[0x22, 0x74, 0x58], 0xFB) },
        ];
        let mut transport = MockTransport::new(vec![vec![0xFF, 0x20]]); // != recorded 0xFE
        let report = replay_drive(&mut transport, &stream, 2, &[0x22, 0xF1, 0x90])
            .await
            .expect("runs");
        let d = report.divergence.expect("divergence flagged");
        assert_eq!(d.idx, 1);
        assert_eq!(d.expected, vec![0xFE]);
        assert_eq!(d.observed, vec![0xFF, 0x20]);
        assert!(!report.reached_target);
    }

    #[test]
    fn dry_run_reports_target_and_round_trips() {
        let stream = vec![
            ReplayFrame { idx: 0, dir: Dir::Out, payload: vec![0x02] },
            ReplayFrame { idx: 1, dir: Dir::In, payload: vec![0xFE] },
            ReplayFrame { idx: 2, dir: Dir::Out, payload: f3_out(&[0x22, 0x74, 0x58], 0xFB) },
        ];
        let plan = plan_dry_run(&stream, 2, &[0x22, 0xF1, 0x90]).expect("plans");
        assert_eq!(plan.target_index, 2);
        assert_eq!(plan.out_up_to_target, 2);
        assert_eq!(plan.in_up_to_target, 1);
        assert!(plan.round_trip_ok);
        assert_eq!(plan.decoded_read_pdu, vec![0x22, 0xF1, 0x90]);
        // Counter continues from the last f3 frame (off14 0xFB → paired 0xFA).
        assert_eq!(plan.read_off14, 0xFA);
    }

    #[test]
    fn dry_run_rejects_missing_target() {
        let stream = vec![ReplayFrame { idx: 0, dir: Dir::Out, payload: vec![0x02] }];
        assert!(matches!(
            plan_dry_run(&stream, 99, &[0x22, 0xF1, 0x90]),
            Err(HexError::Framing(_))
        ));
    }

    #[tokio::test]
    async fn full_replay_reaches_vin() {
        // Stream: bring-up probe → ack → f3 request (target). Counter off14=0xFB
        // on the target f3 frame, so our read stamps paired_off14(0xFB)=0xFA.
        let stream = vec![
            ReplayFrame { idx: 0, dir: Dir::Out, payload: vec![0x02] },
            ReplayFrame { idx: 1, dir: Dir::In, payload: vec![0xFE] },
            ReplayFrame { idx: 2, dir: Dir::Out, payload: f3_out(&[0x22, 0x74, 0x58], 0xFB) },
        ];
        // recv queue: the recorded IN ack, then the VIN multiframe response.
        let mut recvs = vec![vec![0xFEu8]];
        recvs.extend(vin_response_frames("XW8AD4NE9JH008917"));
        let mut transport = MockTransport::new(recvs);

        let report = replay_drive(&mut transport, &stream, 2, &[0x22, 0xF1, 0x90])
            .await
            .expect("runs");

        assert!(report.reached_target);
        assert!(report.divergence.is_none());
        assert_eq!(report.sent_out, 2);
        assert_eq!(report.sent_read_off14, 0xFA);
        assert_eq!(report.vin.as_deref(), Some("XW8AD4NE9JH008917"));
        // The last thing sent is our f3 read block (opcode b8 + block off0 f3).
        let last = transport.sent.last().expect("sent a read");
        assert_eq!(last[0], OP_DIAG_REQ);
        assert_eq!(last[1], CHAN_F3);
        assert_eq!(last[15], 0xFA); // block off14 counter
    }
}
