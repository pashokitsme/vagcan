//! Cable serial envelope: wrap/unwrap a UDS PDU for the cable's own wire format.
//!
//! Recovered wire format (`research/vag-hex-framing.md`, clean-room static
//! recovery, cable's own on-wire framing, no VCDS involved) is **three nested
//! layers**, outermost last on the wire:
//!
//! - Layer A [`cb_encode`]/[`cb_decode`]/[`cb_reassemble`] — the "CB command
//!   frame": a 7-byte header (seq, reserved, cmd BE16, total_len BE16,
//!   frag_len) plus payload, fragmented at ≤ 0xDB (219) bytes. HIGH confidence
//!   except `seq` semantics (LOW — see the doc comment on [`cb_encode`]).
//! - Layer B [`line_wrap`]/[`line_unwrap`] — the line frame:
//!   `[0x04][frame_len][A bytes..][XOR checksum]`. HIGH confidence.
//! - Layer C [`usb_chunk`]/[`usb_dechunk`] — USB packetization:
//!   `[0x01][len][idx][cnt][slice..]` chunks of a Layer-B frame. HIGH
//!   confidence for the header shape; MED on the `0x01` marker and on when
//!   this mode (vs the HID "mode 7", no outer prefix) is selected.
//!
//! The top-level [`encode`]/[`decode`] entry points that `transport.rs`
//! consumes stay [`HexError::Unspecified`] because the CB **command id** that
//! wraps a UDS/ISO-TP PDU is not recoverable statically — see the
//! `TODO(capture)` on [`UDS_CB_CMD`]. The encrypted CB variant's cipher is
//! also unknown; see [`encode_encrypted`].

use crate::error::HexError;

/// XOR-fold checksum: init 0, XOR every byte in order. Used by Layer B.
pub fn xor_cksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |acc, &b| acc ^ b)
}

/// Max Layer-A payload bytes per fragment (`0xDB` = 219, the encoder's memcpy
/// length / `frag_len` cap — see the spec's "Fragmentation" section).
pub const CB_MAX_FRAG: usize = 0xDB;

/// A parsed Layer-A ("CB command") frame: one fragment's header + payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CbFrame {
    /// Per-frame counter byte (`cable+0x1a8`). LOW confidence on semantics —
    /// the spec only confirms it's re-read per fragment, not what drives it.
    pub seq: u8,
    /// 16-bit command id, big-endian on the wire.
    pub cmd: u16,
    /// Total payload length across all fragments of this command.
    pub total_len: u16,
    /// Payload bytes carried in *this* fragment (`<= CB_MAX_FRAG`).
    pub frag_len: u8,
    /// This fragment's payload bytes (length `frag_len`).
    pub payload: Vec<u8>,
}

/// Layer A: encode `payload` under `cmd` into one or more CB command frames,
/// fragmenting at [`CB_MAX_FRAG`] (219) bytes. Each returned `Vec<u8>` is one
/// on-the-wire Layer-A frame: 7-byte header + that fragment's payload slice.
///
/// `seq` is applied to every fragment produced by this call. The real cable
/// firmware re-reads its own counter (`cable+0x1a8`) fresh for *each*
/// fragment (spec: "Fragmentation" section) — so if that counter free-runs
/// between fragments, per-fragment `seq` could legitimately differ from what
/// this function produces. Semantics of that counter are LOW confidence and
/// unconfirmed by capture, so this function takes the simpler, spec-literal
/// shape (single `seq` in, applied uniformly) rather than guess a
/// fragment-to-fragment increment rule.
pub fn cb_encode(seq: u8, cmd: u16, payload: &[u8]) -> Vec<Vec<u8>> {
    let total_len = payload.len() as u16;
    if payload.is_empty() {
        // Zero-length commands (e.g. the cmd 0x20 "identify" query) still
        // send a single header-only frame.
        return vec![cb_header(seq, cmd, total_len, 0)];
    }
    payload
        .chunks(CB_MAX_FRAG)
        .map(|frag| {
            let mut frame = cb_header(seq, cmd, total_len, frag.len() as u8);
            frame.extend_from_slice(frag);
            frame
        })
        .collect()
}

fn cb_header(seq: u8, cmd: u16, total_len: u16, frag_len: u8) -> Vec<u8> {
    vec![
        seq,
        0x00,
        (cmd >> 8) as u8,
        cmd as u8,
        (total_len >> 8) as u8,
        total_len as u8,
        frag_len,
    ]
}

/// Layer A inverse: parse a single on-wire CB command frame (one Layer-A
/// frame: 7-byte header + `frag_len` payload bytes) into a [`CbFrame`].
///
/// Validates the header is internally consistent: enough bytes are present
/// for the declared `frag_len`, and `total_len >= frag_len` (a fragment can
/// never claim to carry more than the whole command). Does **not** validate
/// consistency *across* fragments of the same command — see
/// [`cb_reassemble`] for that.
pub fn cb_decode(frame: &[u8]) -> Result<CbFrame, HexError> {
    if frame.len() < 7 {
        return Err(HexError::Framing(format!(
            "CB frame too short: {} bytes (need >= 7 for header)",
            frame.len()
        )));
    }
    let seq = frame[0];
    let reserved = frame[1];
    if reserved != 0x00 {
        return Err(HexError::Framing(format!(
            "CB frame reserved byte not 0x00: {reserved:#04x}"
        )));
    }
    let cmd = u16::from_be_bytes([frame[2], frame[3]]);
    let total_len = u16::from_be_bytes([frame[4], frame[5]]);
    let frag_len = frame[6];
    let payload = &frame[7..];
    if payload.len() != frag_len as usize {
        return Err(HexError::Framing(format!(
            "CB frame frag_len mismatch: header says {}, {} bytes present",
            frag_len,
            payload.len()
        )));
    }
    if total_len < frag_len as u16 {
        return Err(HexError::Framing(format!(
            "CB frame total_len ({total_len}) < frag_len ({frag_len})"
        )));
    }
    Ok(CbFrame {
        seq,
        cmd,
        total_len,
        frag_len,
        payload: payload.to_vec(),
    })
}

/// Reassemble a run of [`CbFrame`] fragments (in wire order) belonging to the
/// same command back into `(seq, cmd, full_payload)`.
///
/// Validates every fragment shares the same `cmd` and `total_len`, and that
/// the concatenated payload length equals `total_len` exactly — a mismatch
/// means a fragment was dropped, duplicated, or misordered. `seq` of the
/// *first* fragment is returned (see [`cb_encode`]'s doc comment on why
/// per-fragment `seq` isn't reconciled here).
pub fn cb_reassemble(frames: &[CbFrame]) -> Result<(u8, u16, Vec<u8>), HexError> {
    let first = frames
        .first()
        .ok_or_else(|| HexError::Framing("no CB fragments to reassemble".into()))?;
    let cmd = first.cmd;
    let total_len = first.total_len;
    let mut payload = Vec::with_capacity(total_len as usize);
    for (i, f) in frames.iter().enumerate() {
        if f.cmd != cmd {
            return Err(HexError::Framing(format!(
                "CB fragment {i} cmd mismatch: expected {cmd:#06x}, got {:#06x}",
                f.cmd
            )));
        }
        if f.total_len != total_len {
            return Err(HexError::Framing(format!(
                "CB fragment {i} total_len mismatch: expected {total_len}, got {}",
                f.total_len
            )));
        }
        payload.extend_from_slice(&f.payload);
    }
    if payload.len() != total_len as usize {
        return Err(HexError::Framing(format!(
            "CB reassembly length mismatch: total_len says {}, got {} bytes from {} fragment(s)",
            total_len,
            payload.len(),
            frames.len()
        )));
    }
    Ok((first.seq, cmd, payload))
}

/// Layer B: wrap Layer-A bytes `a` into the on-wire line frame
/// `[0x04][frame_len][a..][XOR checksum]`, `frame_len = a.len() + 3`.
///
/// `frame_len` is a single on-wire byte, matching the firmware's `strb`
/// (truncating u8 store) — callers must keep `a.len() <= 252` (Layer-A frames
/// are at most `7 + CB_MAX_FRAG` = 226 bytes, so this always holds for
/// [`cb_encode`] output).
pub fn line_wrap(a: &[u8]) -> Vec<u8> {
    let frame_len = (a.len() + 3) as u8;
    let mut f = Vec::with_capacity(a.len() + 3);
    f.push(0x04);
    f.push(frame_len);
    f.extend_from_slice(a);
    let cksum = xor_cksum(&f);
    f.push(cksum);
    f
}

/// Layer B inverse: verify marker/length/checksum on a line frame and return
/// the enclosed Layer-A bytes.
pub fn line_unwrap(frame: &[u8]) -> Result<Vec<u8>, HexError> {
    if frame.len() < 3 {
        return Err(HexError::Framing(format!(
            "line frame too short: {} bytes (need >= 3)",
            frame.len()
        )));
    }
    if frame[0] != 0x04 {
        return Err(HexError::Framing(format!(
            "bad line frame marker: {:#04x} (want 0x04)",
            frame[0]
        )));
    }
    let frame_len = frame[1] as usize;
    if frame_len != frame.len() {
        return Err(HexError::Framing(format!(
            "line frame_len mismatch: header says {}, got {} bytes",
            frame_len,
            frame.len()
        )));
    }
    let (body, tail) = frame.split_at(frame.len() - 1);
    let want = tail[0];
    let got = xor_cksum(body);
    if got != want {
        return Err(HexError::Framing(format!(
            "line frame checksum mismatch: computed {got:#04x}, frame says {want:#04x}"
        )));
    }
    Ok(frame[2..frame.len() - 1].to_vec())
}

/// Layer C: split a Layer-B line frame into USB packets of at most
/// `maxpacket - 4` payload bytes, each prefixed
/// `[0x01][len][idx (1-based)][cnt]`.
///
/// Returns an empty `Vec` if `maxpacket <= 4` (no room for a payload byte) or
/// `frame` is empty — neither case arises for real [`line_wrap`] output
/// (always >= 3 bytes) driven over a sane USB `maxpacket`.
///
/// `idx`/`cnt` are single wire bytes: this panics in debug builds if the frame
/// would need > 255 packets (real frames are <= 226 bytes, so this never fires
/// on live traffic — the assert just refuses to silently emit wrapped indices).
pub fn usb_chunk(frame: &[u8], maxpacket: usize) -> Vec<Vec<u8>> {
    let chunk_size = maxpacket.saturating_sub(4);
    if chunk_size == 0 || frame.is_empty() {
        debug_assert!(chunk_size > 0, "usb_chunk: maxpacket {maxpacket} <= 4 leaves no payload room");
        return Vec::new();
    }
    let n = frame.len().div_ceil(chunk_size);
    debug_assert!(n <= u8::MAX as usize, "usb_chunk: {n} packets overflow u8 idx/cnt");
    let cnt = n as u8;
    frame
        .chunks(chunk_size)
        .enumerate()
        .map(|(i, slice)| {
            let mut p = Vec::with_capacity(4 + slice.len());
            p.push(0x01);
            p.push(slice.len() as u8);
            p.push((i + 1) as u8);
            p.push(cnt);
            p.extend_from_slice(slice);
            p
        })
        .collect()
}

/// Layer C inverse: reassemble USB packets (in wire order) back into the
/// Layer-B line frame. Validates the `0x01` marker, in-order 1-based `idx`,
/// a consistent `cnt` across packets, and that each packet's `len` matches
/// the payload bytes actually present.
pub fn usb_dechunk(chunks: &[Vec<u8>]) -> Result<Vec<u8>, HexError> {
    if chunks.is_empty() {
        return Err(HexError::Framing("no USB packets to dechunk".into()));
    }
    let cnt = chunks.len();
    let mut out = Vec::new();
    for (i, pkt) in chunks.iter().enumerate() {
        if pkt.len() < 4 {
            return Err(HexError::Framing(format!(
                "USB packet {i} too short: {} bytes (need >= 4)",
                pkt.len()
            )));
        }
        if pkt[0] != 0x01 {
            return Err(HexError::Framing(format!(
                "USB packet {i} bad marker: {:#04x} (want 0x01)",
                pkt[0]
            )));
        }
        let len = pkt[1] as usize;
        let idx = pkt[2] as usize;
        let pkt_cnt = pkt[3] as usize;
        if idx != i + 1 {
            return Err(HexError::Framing(format!(
                "USB packet out of order: expected idx {}, got {idx}",
                i + 1
            )));
        }
        if pkt_cnt != cnt {
            return Err(HexError::Framing(format!(
                "USB packet {i} cnt mismatch: packet says {pkt_cnt}, batch has {cnt}"
            )));
        }
        let data = &pkt[4..];
        if data.len() != len {
            return Err(HexError::Framing(format!(
                "USB packet {i} len mismatch: header says {len}, {} bytes present",
                data.len()
            )));
        }
        out.extend_from_slice(data);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Top-level entry points consumed by `transport.rs`.
// ---------------------------------------------------------------------------

// TODO(capture): the CB command id that carries a UDS/ISO-TP PDU is not
// recoverable statically. The spec's "Command vocabulary" lists 27 observed
// 16-bit ids from the 47 encoder call-sites, and the init handshake's commands
// (`0x3B` config, `0x20` identify, `0x21` mode-set), but nothing ties an id to
// "wraps an arbitrary UDS payload". Pinning it needs a live capture of a real
// diagnostic session (e.g. a VIN read `22 F1 90` round-trip) as known-plaintext.
// The `seq` rule (LOW confidence, see `cb_encode`) and mode 7/8 selection are
// blocked on the same capture. Until then the two seams below stay gated.

/// Encode a UDS PDU into an on-wire cable frame (Layer A + Layer B bytes).
///
/// Mechanically `cb_encode(seq, cmd, pdu)` fragments each through `line_wrap`
/// — both implemented and unit-tested. Gated on the unknown `cmd`/`seq` (see
/// the `TODO(capture)` above): returns [`HexError::Unspecified`] rather than
/// emit a frame under a guessed command id.
pub fn encode(_pdu: &[u8]) -> Result<Vec<u8>, HexError> {
    Err(HexError::Unspecified(
        "cable frame encode: CB cmd id for a UDS PDU needs capture",
    ))
}

/// Decode one cable frame from the byte stream back into its UDS PDU.
///
/// Returns the PDU and the number of raw bytes consumed, so a caller can drive
/// this over a growing read buffer. Mechanically `line_unwrap` -> `cb_decode`
/// (-> `cb_reassemble` across fragments) — all implemented and unit-tested.
/// Gated on the same `cmd` mapping as [`encode`]: without knowing which `cmd`
/// marks "this frame is a UDS PDU", a byte stream can't be told apart from
/// other CB traffic (init acks, etc).
pub fn decode(_bytes: &[u8]) -> Result<(Vec<u8>, usize), HexError> {
    Err(HexError::Unspecified(
        "cable frame decode: CB cmd id for a UDS PDU needs capture",
    ))
}

/// Encrypted CB variant (flag `cable+0x5cd0` set): stub.
///
/// The spec classifies this as an unknown 128-bit block cipher with 16
/// rotating keys (key schedule table `0x140171d30`, round function
/// `0x14007afd0`/`0x14007b108`) — round function not reversed, out of the
/// clean-room interop scope. Do not attempt to guess or approximate it.
pub fn encode_encrypted(_pdu: &[u8]) -> Result<Vec<u8>, HexError> {
    Err(HexError::Unspecified(
        "encrypted CB variant: cipher is unknown, not reversed (out of scope)",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- xor_cksum ----------------------------------------------------------

    #[test]
    fn xor_cksum_empty_is_zero() {
        assert_eq!(xor_cksum(&[]), 0);
    }

    #[test]
    fn xor_cksum_folds_all_bytes() {
        // 0x04 ^ 0x09 ^ 0x01 ^ 0x02 ^ 0x03 = hand-computed below.
        let bytes = [0x04u8, 0x09, 0x01, 0x02, 0x03];
        let expect = 0x04 ^ 0x09 ^ 0x01 ^ 0x02 ^ 0x03;
        assert_eq!(xor_cksum(&bytes), expect);
    }

    // -- cb_encode (Layer A) --------------------------------------------------

    #[test]
    fn cb_encode_empty_payload_is_single_header_only_frame() {
        let frames = cb_encode(0x01, 0x20, &[]);
        assert_eq!(frames.len(), 1);
        assert_eq!(
            frames[0],
            vec![0x01, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00],
            "seq=0x01 reserved=0 cmd=0x0020 total_len=0 frag_len=0"
        );
    }

    #[test]
    fn cb_encode_exactly_219_bytes_is_single_fragment() {
        let payload = vec![0xAAu8; 219];
        let frames = cb_encode(0x02, 0x3B, &payload);
        assert_eq!(frames.len(), 1, "219 bytes fits in one CB_MAX_FRAG fragment");
        let f = &frames[0];
        assert_eq!(f.len(), 7 + 219);
        assert_eq!(&f[0..7], &[0x02, 0x00, 0x00, 0x3B, 0x00, 0xDB, 0xDB]);
        assert_eq!(&f[7..], payload.as_slice());
    }

    #[test]
    fn cb_encode_220_bytes_splits_into_two_fragments() {
        let payload: Vec<u8> = (0..220u32).map(|i| i as u8).collect();
        let frames = cb_encode(0x03, 0x1FB, &payload);
        assert_eq!(frames.len(), 2, "220 bytes needs a second, 1-byte fragment");

        let f0 = &frames[0];
        assert_eq!(f0.len(), 7 + 219);
        // seq=3 reserved=0 cmd=0x01FB total_len=220(0x00DC) frag_len=219(0xDB)
        assert_eq!(&f0[0..7], &[0x03, 0x00, 0x01, 0xFB, 0x00, 0xDC, 0xDB]);
        assert_eq!(&f0[7..], &payload[0..219]);

        let f1 = &frames[1];
        assert_eq!(f1.len(), 7 + 1);
        assert_eq!(&f1[0..7], &[0x03, 0x00, 0x01, 0xFB, 0x00, 0xDC, 0x01]);
        assert_eq!(&f1[7..], &payload[219..220]);
    }

    // -- cb_decode / cb_reassemble (Layer A inverse) -------------------------

    #[test]
    fn cb_decode_round_trips_a_single_fragment() {
        let payload = b"hello";
        let frames = cb_encode(0x07, 0x21, payload);
        assert_eq!(frames.len(), 1);
        let parsed = cb_decode(&frames[0]).expect("well-formed CB frame parses");
        assert_eq!(parsed.seq, 0x07);
        assert_eq!(parsed.cmd, 0x21);
        assert_eq!(parsed.total_len, payload.len() as u16);
        assert_eq!(parsed.frag_len, payload.len() as u8);
        assert_eq!(parsed.payload, payload);
    }

    #[test]
    fn cb_decode_rejects_short_frame() {
        let err = cb_decode(&[0x00; 6]).unwrap_err();
        assert!(matches!(err, HexError::Framing(_)));
    }

    #[test]
    fn cb_decode_rejects_frag_len_mismatch() {
        // Header claims frag_len=5 but only 2 payload bytes follow.
        let bad = vec![0x00, 0x00, 0x00, 0x20, 0x00, 0x05, 0x05, 0xAA, 0xBB];
        let err = cb_decode(&bad).unwrap_err();
        assert!(matches!(err, HexError::Framing(_)));
    }

    #[test]
    fn cb_decode_rejects_total_len_less_than_frag_len() {
        // total_len=1 but frag_len=2: impossible, a fragment can't exceed the total.
        let bad = vec![0x00, 0x00, 0x00, 0x20, 0x00, 0x01, 0x02, 0xAA, 0xBB];
        let err = cb_decode(&bad).unwrap_err();
        assert!(matches!(err, HexError::Framing(_)));
    }

    #[test]
    fn cb_reassemble_merges_two_fragments() {
        let payload: Vec<u8> = (0..220u32).map(|i| i as u8).collect();
        let frames = cb_encode(0x09, 0x1FB, &payload);
        let parsed: Vec<CbFrame> = frames.iter().map(|f| cb_decode(f).unwrap()).collect();
        let (seq, cmd, out) = cb_reassemble(&parsed).expect("consistent fragments reassemble");
        assert_eq!(seq, 0x09);
        assert_eq!(cmd, 0x1FB);
        assert_eq!(out, payload);
    }

    #[test]
    fn cb_reassemble_rejects_cmd_mismatch() {
        let mut f0 = cb_decode(&cb_encode(0x01, 0x20, b"ab")[0]).unwrap();
        let f1 = cb_decode(&cb_encode(0x01, 0x21, b"cd")[0]).unwrap();
        f0.total_len = f1.total_len; // isolate the cmd mismatch
        let err = cb_reassemble(&[f0, f1]).unwrap_err();
        assert!(matches!(err, HexError::Framing(_)));
    }

    #[test]
    fn cb_reassemble_rejects_empty_input() {
        let err = cb_reassemble(&[]).unwrap_err();
        assert!(matches!(err, HexError::Framing(_)));
    }

    // -- line_wrap / line_unwrap (Layer B) -----------------------------------

    #[test]
    fn line_wrap_hand_computed_vector() {
        // A-bytes = [0x01, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00] (empty-payload
        // cb_encode header from above). frame_len = 7 + 3 = 10.
        let a = [0x01u8, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00];
        let expect_cksum = 0x04u8 ^ 10 ^ 0x01 ^ 0x00 ^ 0x00 ^ 0x20 ^ 0x00 ^ 0x00 ^ 0x00;
        let wrapped = line_wrap(&a);
        assert_eq!(
            wrapped,
            vec![0x04, 10, 0x01, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, expect_cksum]
        );
    }

    #[test]
    fn line_wrap_unwrap_round_trip() {
        let a = b"the quick brown fox".to_vec();
        let wrapped = line_wrap(&a);
        let unwrapped = line_unwrap(&wrapped).expect("well-formed line frame unwraps");
        assert_eq!(unwrapped, a);
    }

    #[test]
    fn line_unwrap_rejects_bad_marker() {
        let mut wrapped = line_wrap(b"x");
        wrapped[0] = 0x05;
        let err = line_unwrap(&wrapped).unwrap_err();
        assert!(matches!(err, HexError::Framing(_)));
    }

    #[test]
    fn line_unwrap_rejects_bad_frame_len() {
        let mut wrapped = line_wrap(b"x");
        wrapped[1] = 0xFF;
        let err = line_unwrap(&wrapped).unwrap_err();
        assert!(matches!(err, HexError::Framing(_)));
    }

    #[test]
    fn line_unwrap_rejects_bad_checksum() {
        let mut wrapped = line_wrap(b"x");
        let last = wrapped.len() - 1;
        wrapped[last] ^= 0xFF;
        let err = line_unwrap(&wrapped).unwrap_err();
        assert!(matches!(err, HexError::Framing(_)));
    }

    #[test]
    fn line_unwrap_rejects_short_frame() {
        let err = line_unwrap(&[0x04, 0x03]).unwrap_err();
        assert!(matches!(err, HexError::Framing(_)));
    }

    // -- usb_chunk / usb_dechunk (Layer C) -----------------------------------

    #[test]
    fn usb_chunk_hand_computed_small_frame() {
        // 5-byte frame, maxpacket=8 -> chunk_size=4 -> 2 packets (4 + 1 bytes).
        let frame = [0xAAu8, 0xBB, 0xCC, 0xDD, 0xEE];
        let chunks = usb_chunk(&frame, 8);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], vec![0x01, 4, 1, 2, 0xAA, 0xBB, 0xCC, 0xDD]);
        assert_eq!(chunks[1], vec![0x01, 1, 2, 2, 0xEE]);
    }

    #[test]
    fn usb_chunk_dechunk_round_trip() {
        let frame = line_wrap(b"round trip payload through layer C chunking");
        let chunks = usb_chunk(&frame, 16);
        assert!(chunks.len() > 1, "test should exercise multi-packet chunking");
        let out = usb_dechunk(&chunks).expect("well-formed chunk run dechunks");
        assert_eq!(out, frame);
    }

    #[test]
    fn usb_dechunk_rejects_bad_marker() {
        let mut chunks = usb_chunk(&line_wrap(b"hi"), 8);
        chunks[0][0] = 0x02;
        let err = usb_dechunk(&chunks).unwrap_err();
        assert!(matches!(err, HexError::Framing(_)));
    }

    #[test]
    fn usb_dechunk_rejects_out_of_order_idx() {
        let mut chunks = usb_chunk(&line_wrap(b"hello world"), 8);
        assert!(chunks.len() >= 2);
        chunks.swap(0, 1);
        let err = usb_dechunk(&chunks).unwrap_err();
        assert!(matches!(err, HexError::Framing(_)));
    }

    #[test]
    fn usb_dechunk_rejects_empty_input() {
        let err = usb_dechunk(&[]).unwrap_err();
        assert!(matches!(err, HexError::Framing(_)));
    }

    // -- full round trip: cb_encode -> line_wrap -> usb_chunk ->
    //    usb_dechunk -> line_unwrap -> cb_decode == original ------------------

    #[test]
    fn full_round_trip_single_fragment() {
        let seq = 0x05u8;
        let cmd = 0x21u16;
        let payload = b"22 F1 90 style diagnostic payload".to_vec();

        let a_frames = cb_encode(seq, cmd, &payload);
        assert_eq!(a_frames.len(), 1);

        let line = line_wrap(&a_frames[0]);
        let usb_packets = usb_chunk(&line, 16);
        assert!(usb_packets.len() > 1);

        let recovered_line = usb_dechunk(&usb_packets).expect("dechunk succeeds");
        assert_eq!(recovered_line, line);

        let recovered_a = line_unwrap(&recovered_line).expect("line_unwrap succeeds");
        assert_eq!(recovered_a, a_frames[0]);

        let parsed = cb_decode(&recovered_a).expect("cb_decode succeeds");
        assert_eq!(parsed.seq, seq);
        assert_eq!(parsed.cmd, cmd);
        assert_eq!(parsed.payload, payload);
    }

    #[test]
    fn full_round_trip_multi_fragment() {
        let seq = 0x0Au8;
        let cmd = 0x1FBu16;
        let payload: Vec<u8> = (0..220u32).map(|i| (i * 7) as u8).collect();

        let a_frames = cb_encode(seq, cmd, &payload);
        assert_eq!(a_frames.len(), 2);

        let mut recovered_frames = Vec::new();
        for a in &a_frames {
            let line = line_wrap(a);
            let usb_packets = usb_chunk(&line, 32);
            let recovered_line = usb_dechunk(&usb_packets).unwrap();
            let recovered_a = line_unwrap(&recovered_line).unwrap();
            assert_eq!(&recovered_a, a);
            recovered_frames.push(cb_decode(&recovered_a).unwrap());
        }

        let (out_seq, out_cmd, out_payload) = cb_reassemble(&recovered_frames).unwrap();
        assert_eq!(out_seq, seq);
        assert_eq!(out_cmd, cmd);
        assert_eq!(out_payload, payload);
    }

    #[test]
    fn full_round_trip_empty_payload() {
        let seq = 0x00u8;
        let cmd = 0x20u16;
        let payload: Vec<u8> = Vec::new();

        let a_frames = cb_encode(seq, cmd, &payload);
        assert_eq!(a_frames.len(), 1);

        let line = line_wrap(&a_frames[0]);
        let usb_packets = usb_chunk(&line, 16);
        let recovered_line = usb_dechunk(&usb_packets).unwrap();
        let recovered_a = line_unwrap(&recovered_line).unwrap();
        let parsed = cb_decode(&recovered_a).unwrap();
        assert_eq!(parsed.seq, seq);
        assert_eq!(parsed.cmd, cmd);
        assert_eq!(parsed.payload, payload);
    }

    // -- top-level encode/decode stay gated ----------------------------------

    #[test]
    fn encode_is_unspecified_until_cmd_mapping_pinned() {
        assert!(matches!(encode(b"22 F1 90"), Err(HexError::Unspecified(_))));
    }

    #[test]
    fn decode_is_unspecified_until_cmd_mapping_pinned() {
        assert!(matches!(decode(&[0x04, 0x00]), Err(HexError::Unspecified(_))));
    }

    #[test]
    fn encrypted_variant_is_unspecified() {
        assert!(matches!(
            encode_encrypted(b"x"),
            Err(HexError::Unspecified(_))
        ));
    }
}
