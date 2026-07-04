//! Cable wire framing — the flat "S/M" frame recovered from live USB capture.
//!
//! Ground truth: `research/vag-hex-framing.md` (two USBPcap captures, parsed by
//! `research/clb-crack/usbpcap.py`). This SUPERSEDES the earlier static-binary
//! guess of a 3-nested-layer format (CB / line / USB-chunk) — on the wire there
//! is exactly one frame:
//!
//! ```text
//! [marker] [len] [opcode] [data...] [xor]
//! ```
//! - `marker` = `0x53 'S'` host→cable (FTDI OUT ep 0x02), `0x4D 'M'` cable→host
//!   (IN ep 0x81).
//! - `len` = TOTAL frame length incl. marker+len+opcode+data+xor (single byte).
//! - `opcode` = 1-byte cable opcode; the reply echoes the same opcode.
//! - `xor` = XOR of every preceding byte (marker..last data), init 0.
//!
//! Confirmed on 3407/3409 captured frames. There is NO `[0x01][len][idx][cnt]`
//! USB sub-layer; the frame is a raw byte stream spanning USB transfers, so a
//! reader must resync on the marker and cut by `len` — see [`take_frame`].
//!
//! The DIAGNOSTIC channel (opcodes [`OP_DIAG_REQ`]/[`OP_DIAG_RESP`]) carries the
//! UDS PDU inside a 16-byte block that is enciphered with a per-channel XOR
//! keystream (recovered in research: `research/clb-crack/link_cipher.py`, and
//! the "Link cipher" section of the framing doc). That cipher + the inner
//! ISO-TP/UDS block layout are not yet ported here, so the top-level
//! [`encode`]/[`decode`] seams stay gated — see their `TODO(capture)`.

use crate::error::HexError;

/// Frame marker for host→cable (FTDI OUT) frames: ASCII `'S'`.
pub const MARKER_HOST: u8 = 0x53;
/// Frame marker for cable→host (FTDI IN) frames: ASCII `'M'`.
pub const MARKER_CABLE: u8 = 0x4D;

/// Smallest valid frame: marker+len+opcode+xor, empty data (`len == 4`).
pub const MIN_FRAME_LEN: usize = 4;

// --- Known plaintext opcodes (payload[0]); see the framing doc's vocab table ---
/// Probe / ping (short).
pub const OP_PROBE: u8 = 0x02;
/// Identify / version query — reply carries ASCII "ROSSTECH" + version bytes.
pub const OP_IDENTIFY: u8 = 0x04;
/// Poll / keepalive ping (the TesterPresent analogue on the plaintext wire).
pub const OP_KEEPALIVE: u8 = 0xA0;
/// Diagnostic request transport (wraps an enciphered UDS request block).
pub const OP_DIAG_REQ: u8 = 0xB8;
/// Diagnostic response transport (wraps an enciphered UDS response block).
pub const OP_DIAG_RESP: u8 = 0xB7;
/// Generic ACK for OUT setup/transport frames.
pub const OP_ACK: u8 = 0xFE;

/// XOR-fold checksum: init 0, XOR every byte in order.
pub fn xor_cksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |acc, &b| acc ^ b)
}

/// A parsed cable frame: its direction marker, opcode, and opcode data (the
/// bytes after the opcode, before the checksum).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// `0x53` host→cable or `0x4D` cable→host.
    pub marker: u8,
    /// 1-byte cable opcode (`payload[0]`).
    pub opcode: u8,
    /// Opcode payload bytes (may be empty).
    pub data: Vec<u8>,
}

/// Build one on-wire frame: `[marker][len][opcode][data..][xor]`.
///
/// `len` is a single wire byte = `data.len() + 4`; this panics in debug builds
/// if that exceeds 255 (real frames are far smaller — the largest opcode block
/// is ~40 bytes).
pub fn frame_encode(marker: u8, opcode: u8, data: &[u8]) -> Vec<u8> {
    let total = data.len() + 4; // marker + len + opcode + data + xor
    debug_assert!(total <= u8::MAX as usize, "frame len {total} overflows u8");
    let mut f = Vec::with_capacity(total);
    f.push(marker);
    f.push(total as u8);
    f.push(opcode);
    f.extend_from_slice(data);
    let x = xor_cksum(&f);
    f.push(x);
    f
}

/// Parse exactly one complete frame: validate marker, `len == bytes.len()`, and
/// the trailing XOR checksum; return the [`Frame`].
///
/// Use this when you already have a single framed buffer. To cut frames out of
/// a growing byte stream, use [`take_frame`].
pub fn frame_decode(bytes: &[u8]) -> Result<Frame, HexError> {
    if bytes.len() < MIN_FRAME_LEN {
        return Err(HexError::Framing(format!(
            "frame too short: {} bytes (need >= {MIN_FRAME_LEN})",
            bytes.len()
        )));
    }
    let marker = bytes[0];
    if marker != MARKER_HOST && marker != MARKER_CABLE {
        return Err(HexError::Framing(format!(
            "bad frame marker {marker:#04x} (want 0x53 'S' or 0x4D 'M')"
        )));
    }
    let len = bytes[1] as usize;
    if len != bytes.len() {
        return Err(HexError::Framing(format!(
            "frame len mismatch: header says {len}, got {} bytes",
            bytes.len()
        )));
    }
    let (body, tail) = bytes.split_at(bytes.len() - 1);
    let want = tail[0];
    let got = xor_cksum(body);
    if got != want {
        return Err(HexError::Framing(format!(
            "frame checksum mismatch: computed {got:#04x}, frame says {want:#04x}"
        )));
    }
    Ok(Frame {
        marker,
        opcode: bytes[2],
        data: bytes[3..bytes.len() - 1].to_vec(),
    })
}

/// Cut the next complete `marker` frame out of the front of a byte-stream `buf`.
///
/// Resyncs by skipping bytes until a `marker` that begins a well-formed,
/// checksum-valid frame. Returns `Some((frame, consumed))` where `consumed` is
/// the number of bytes to drop from the front of `buf` (skipped resync bytes +
/// the frame), or `None` if no complete valid frame has arrived yet (the caller
/// should read more bytes and retry). A `marker` byte whose framing/checksum
/// does not validate is treated as stream noise and skipped, not an error.
pub fn take_frame(buf: &[u8], marker: u8) -> Option<(Frame, usize)> {
    let mut i = 0;
    while i < buf.len() {
        if buf[i] != marker {
            i += 1;
            continue;
        }
        let rest = &buf[i..];
        if rest.len() < 2 {
            return None; // need the len byte before we can size the frame
        }
        let len = rest[1] as usize;
        if len < MIN_FRAME_LEN {
            i += 1; // impossible length — false marker, resync
            continue;
        }
        if rest.len() < len {
            return None; // frame started but hasn't fully arrived
        }
        match frame_decode(&rest[..len]) {
            Ok(frame) => return Some((frame, i + len)),
            Err(_) => {
                i += 1; // marker didn't begin a valid frame — skip it
                continue;
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Top-level UDS<->cable seams consumed by `transport.rs` (still gated).
// ---------------------------------------------------------------------------

// TODO(capture): the diagnostic UDS path is understood but not yet ported.
// A UDS PDU rides inside an OP_DIAG_REQ (0xB8) frame as a 16-byte block that is
// XOR-enciphered with a PER-CHANNEL keystream (research/clb-crack/link_cipher.py),
// wrapping an inner ISO-TP layout (off6 = ISO-TP PCI, off7 = UDS SID, off8..13 =
// data+padding, off14 = counter). Two things are not yet production-ready in
// Rust: (1) the 16-key keystream SCHEDULE is un-reversed, so keystreams are
// recovered empirically per channel rather than derived; (2) the inner ISO-TP
// segmentation across multiframe blocks. Until both land, these seams stay gated.

/// Encode a UDS PDU into an on-wire cable frame.
///
/// Gated: the diagnostic transport enciphers the PDU with a per-channel
/// keystream whose schedule is not yet reversed (see the `TODO(capture)` above
/// and `research/vag-hex-framing.md`). Returns [`HexError::Unspecified`] rather
/// than emit a frame under an unresolved key.
pub fn encode(_pdu: &[u8]) -> Result<Vec<u8>, HexError> {
    Err(HexError::Unspecified(
        "diagnostic encode: per-channel link keystream + ISO-TP layout not yet ported",
    ))
}

/// Decode one cable frame from the byte stream back into its UDS PDU.
///
/// Returns the PDU and bytes consumed. Gated on the same link-cipher/ISO-TP
/// work as [`encode`]. (Plaintext-frame parsing is available now via
/// [`frame_decode`]/[`take_frame`]; this seam is specifically the encrypted
/// UDS transport.)
pub fn decode(_bytes: &[u8]) -> Result<(Vec<u8>, usize), HexError> {
    Err(HexError::Unspecified(
        "diagnostic decode: per-channel link keystream + ISO-TP layout not yet ported",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real frames lifted verbatim from init-only.pcapng (usbpcap.py `frames`),
    // used as ground-truth vectors.

    #[test]
    fn xor_cksum_empty_is_zero() {
        assert_eq!(xor_cksum(&[]), 0);
    }

    #[test]
    fn xor_cksum_folds_all_bytes() {
        let bytes = [0x53u8, 0x04, 0x02];
        assert_eq!(xor_cksum(&bytes), 0x53 ^ 0x04 ^ 0x02);
    }

    #[test]
    fn encode_matches_real_probe_frame() {
        // Host OUT probe: `53 04 02 55` (marker S, opcode 0x02, empty data).
        let f = frame_encode(MARKER_HOST, OP_PROBE, &[]);
        assert_eq!(f, vec![0x53, 0x04, 0x02, 0x55]);
    }

    #[test]
    fn encode_matches_real_multibyte_frame() {
        // Host OUT: `53 0d 09 01f807d25de7355cb1 19` (opcode 0x09, 9 data bytes).
        let data = [0x01u8, 0xf8, 0x07, 0xd2, 0x5d, 0xe7, 0x35, 0x5c, 0xb1];
        let f = frame_encode(MARKER_HOST, 0x09, &data);
        assert_eq!(
            f,
            vec![0x53, 0x0d, 0x09, 0x01, 0xf8, 0x07, 0xd2, 0x5d, 0xe7, 0x35, 0x5c, 0xb1, 0x19]
        );
    }

    #[test]
    fn decode_real_cable_in_frame() {
        // Cable IN: `4d 07 02 016044 6d` (marker M, opcode 0x02, data 01 60 44).
        let raw = [0x4d, 0x07, 0x02, 0x01, 0x60, 0x44, 0x6d];
        let frame = frame_decode(&raw).expect("valid real frame");
        assert_eq!(frame.marker, MARKER_CABLE);
        assert_eq!(frame.opcode, 0x02);
        assert_eq!(frame.data, vec![0x01, 0x60, 0x44]);
    }

    #[test]
    fn encode_decode_round_trip() {
        let data = b"the quick brown fox";
        let wire = frame_encode(MARKER_HOST, OP_IDENTIFY, data);
        let frame = frame_decode(&wire).expect("round trips");
        assert_eq!(frame.marker, MARKER_HOST);
        assert_eq!(frame.opcode, OP_IDENTIFY);
        assert_eq!(frame.data, data);
    }

    #[test]
    fn decode_rejects_short_frame() {
        assert!(matches!(
            frame_decode(&[0x53, 0x03, 0x02]),
            Err(HexError::Framing(_))
        ));
    }

    #[test]
    fn decode_rejects_bad_marker() {
        // Same bytes as the probe frame but marker flipped to 0x99.
        assert!(matches!(
            frame_decode(&[0x99, 0x04, 0x02, 0x99 ^ 0x04 ^ 0x02]),
            Err(HexError::Framing(_))
        ));
    }

    #[test]
    fn decode_rejects_len_mismatch() {
        // len byte claims 5 but only 4 bytes present.
        assert!(matches!(
            frame_decode(&[0x53, 0x05, 0x02, 0x55]),
            Err(HexError::Framing(_))
        ));
    }

    #[test]
    fn decode_rejects_bad_checksum() {
        let mut wire = frame_encode(MARKER_HOST, OP_PROBE, &[]);
        let last = wire.len() - 1;
        wire[last] ^= 0xFF;
        assert!(matches!(frame_decode(&wire), Err(HexError::Framing(_))));
    }

    #[test]
    fn take_frame_cuts_one_frame_after_noise() {
        // Leading garbage, then a valid host frame, then trailing bytes.
        let good = frame_encode(MARKER_HOST, OP_PROBE, &[]);
        let mut buf = vec![0x00, 0xFF, 0x12]; // noise, no 0x53
        let frame_start = buf.len();
        buf.extend_from_slice(&good);
        buf.extend_from_slice(&[0xAA, 0xBB]); // trailing
        let (frame, consumed) = take_frame(&buf, MARKER_HOST).expect("finds the frame");
        assert_eq!(frame.opcode, OP_PROBE);
        assert_eq!(consumed, frame_start + good.len());
    }

    #[test]
    fn take_frame_returns_none_on_partial() {
        // Marker + len present, but the frame hasn't fully arrived.
        let partial = [0x53, 0x0d, 0x09, 0x01, 0xf8]; // len=13, only 5 bytes
        assert!(take_frame(&partial, MARKER_HOST).is_none());
    }

    #[test]
    fn take_frame_skips_false_marker_in_noise() {
        // A stray 0x53 whose following bytes don't form a valid frame, then a
        // real frame further along.
        let good = frame_encode(MARKER_HOST, 0x09, &[0x01, 0x02, 0x03]);
        let mut buf = vec![0x53, 0x04, 0x00, 0x00]; // 0x53 w/ bad checksum -> not a frame
        buf.extend_from_slice(&good);
        let (frame, _consumed) = take_frame(&buf, MARKER_HOST).expect("skips false marker");
        assert_eq!(frame.opcode, 0x09);
        assert_eq!(frame.data, vec![0x01, 0x02, 0x03]);
    }

    #[test]
    fn take_frame_none_when_marker_absent() {
        assert!(take_frame(&[0x00, 0x11, 0x22], MARKER_HOST).is_none());
    }

    #[test]
    fn diagnostic_seams_are_gated() {
        assert!(matches!(encode(b"22 F1 90"), Err(HexError::Unspecified(_))));
        assert!(matches!(decode(&[0x53, 0x04]), Err(HexError::Unspecified(_))));
    }
}
