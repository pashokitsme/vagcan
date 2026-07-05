//! Link cipher DECODE for the `0xb8`/`0xb7` diagnostic channel.
//!
//! The diagnostic UDS PDUs ride inside a 16-byte block carried by the `0xb8`
//! (host→cable request) and `0xb7` (cable→host response) transport frames. That
//! block is enciphered with a **position-dependent XOR keystream** that is fixed
//! per logical channel: `plain[i] = cipher[i] ^ KS_channel[i]`, `i = 0..15`. It
//! is provably byte-local (NOT a diffusing block cipher) — see the "Link cipher"
//! section of `research/vag-hex-framing.md` and the reference
//! `research/clb-crack/link_cipher.py`.
//!
//! **Scope boundary (`research/SCOPE-BOUNDARY.md`): DECODE ONLY.** The per-channel
//! keystream is `KS = AES(IV_row)` under a *runtime session key* that is
//! established by the `0xb6` and is
//! deliberately **not** derived here. This module therefore **recovers a
//! keystream from known-plaintext** (the ISO-TP/UDS structure of a captured
//! session) and XOR-decodes with it. It does not, and must not, synthesise the
//! AES session key or touch the auth handshake.
//!
//! # Inner 16-byte block layout (HIGH confidence for off 6..=13)
//! ```text
//! off 0..=5  addressing/header (off1 = echoed SID, off4 = direction bit)
//! off 6      ISO-TP PCI  (0x0N single-frame, 0x1N first-frame, 0x2N consecutive)
//! off 7      UDS SID     (request) / SID|0x40 (positive resp) / 0x7F (negative)
//! off 8..=13 UDS data bytes, then ISO-TP padding (req pad 0x00, resp pad 0x55/0xFF)
//! off 14     per-frame transport counter
//! off 15     trailer / checksum-like
//! ```

/// The enciphered diagnostic block is exactly 16 bytes.
pub const BLOCK_LEN: usize = 16;

/// Block offset of the ISO-TP PCI byte.
pub const OFF_PCI: usize = 6;
/// Block offset of the UDS service id (SID) byte.
pub const OFF_SID: usize = 7;

/// XOR-decode a 16-byte diagnostic block with a channel keystream.
///
/// `plain[i] = cipher[i] ^ keystream[i]`. This is the whole cipher — a pure,
/// byte-local XOR — so encode is the identical operation. Keystream bytes that
/// were never recovered (e.g. the addressing header or counter) simply yield
/// garbage at those offsets; decode only ever reads the UDS region (off 6..=13).
#[must_use]
pub fn decrypt_block(cipher: &[u8; BLOCK_LEN], keystream: &[u8; BLOCK_LEN]) -> [u8; BLOCK_LEN] {
    let mut out = [0u8; BLOCK_LEN];
    for (o, (c, k)) in out.iter_mut().zip(cipher.iter().zip(keystream.iter())) {
        *o = c ^ k;
    }
    out
}

/// Recover a channel keystream from a known-plaintext crib.
///
/// For every offset `i` where `plaintext[i]` is `Some(p)`, the keystream byte is
/// `cipher[i] ^ p`; offsets whose plaintext is unknown (`None`) are left `0`.
/// This is the textbook known-plaintext recovery: a request↔response pair (or a
/// single request whose ISO-TP structure is known) exposes enough plaintext to
/// peel the keystream over the UDS-bearing region. Since the request and the
/// response of one channel share the same keystream, a keystream recovered from
/// a request decodes that channel's responses too.
///
/// Build the crib by hand, or with [`iso_tp_single_frame_crib`] for the common
/// case of a single-frame UDS request padded with `0x00`.
#[must_use]
pub fn recover_keystream(
    cipher: &[u8; BLOCK_LEN],
    plaintext: &[Option<u8>; BLOCK_LEN],
) -> [u8; BLOCK_LEN] {
    let mut ks = [0u8; BLOCK_LEN];
    for (i, slot) in ks.iter_mut().enumerate() {
        if let Some(p) = plaintext[i] {
            *slot = cipher[i] ^ p;
        }
    }
    ks
}

/// Build a known-plaintext crib for a single-frame UDS request.
///
/// The block carries `PCI` at [`OFF_PCI`], `SID` at [`OFF_SID`], then the UDS
/// data bytes, then `0x00` ISO-TP padding out to off 13. The `pci` low nibble is
/// the PDU length (SID + data), so the data bytes at `off 8..(7 + pdu_len)` are
/// left unknown (their values are request-specific) while the trailing padding
/// is known-`0x00`. Header (off 0..=5), counter (off 14) and trailer (off 15) are
/// addressing/framing, not needed to read UDS, so they stay unknown.
///
/// This mirrors `recover_channel_ks` in `research/clb-crack/link_cipher.py`.
#[must_use]
pub fn iso_tp_single_frame_crib(pci: u8, sid: u8) -> [Option<u8>; BLOCK_LEN] {
    let mut crib = [None; BLOCK_LEN];
    crib[OFF_PCI] = Some(pci);
    crib[OFF_SID] = Some(sid);
    // PDU occupies off7..(7 + pdu_len); everything after it up to off13 is 0x00 pad.
    let pdu_len = (pci & 0x0F) as usize;
    let pad_start = OFF_SID + pdu_len; // first padding offset after the PDU
    for slot in crib.iter_mut().take(14).skip(pad_start) {
        *slot = Some(0x00);
    }
    crib
}

/// The inner UDS view decoded out of one diagnostic block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdsSlice {
    /// The full XOR-decoded 16-byte block (all offsets, for inspection).
    pub block: [u8; BLOCK_LEN],
    /// ISO-TP PCI byte (block off 6).
    pub pci: u8,
    /// The inner UDS PDU bytes (SID + data), extracted for a single frame.
    pub uds: Vec<u8>,
}

/// Decode one `0xb8`/`0xb7` diagnostic frame payload into its inner UDS PDU.
///
/// `frame_payload` is the 16-byte enciphered block that follows the transport
/// opcode (i.e. [`crate::frame::Frame::data`] of a [`crate::frame::OP_DIAG_REQ`]
/// / [`crate::frame::OP_DIAG_RESP`] frame). Returns [`None`] if the payload is
/// shorter than one block, or if the block is not an ISO-TP **single frame**
/// (PCI high nibble `0`).
///
/// Multiframe blocks (first-frame `0x1N`, consecutive `0x2N`) carry a PDU that
/// spans several blocks; reassembling them needs cross-frame state and is not
/// done by this per-frame call — decode such blocks with [`decrypt_block`] and
/// reassemble at the transport layer. (For single-frame UDS — TesterPresent,
/// short RDBI reads — this yields the PDU directly.)
#[must_use]
pub fn decode_diag_frame(frame_payload: &[u8], keystream: &[u8; BLOCK_LEN]) -> Option<UdsSlice> {
    let cipher: &[u8; BLOCK_LEN] = frame_payload.get(..BLOCK_LEN)?.try_into().ok()?;
    let block = decrypt_block(cipher, keystream);
    let pci = block[OFF_PCI];

    // ISO-TP single frame: high nibble 0, low nibble = PDU length (SID + data).
    if pci & 0xF0 != 0 {
        return None;
    }
    let pdu_len = (pci & 0x0F) as usize;
    let end = OFF_SID + pdu_len;
    if end > BLOCK_LEN {
        return None; // malformed: PDU length runs past the block
    }
    Some(UdsSlice {
        block,
        pci,
        uds: block[OFF_SID..end].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Fixtures: 16-byte b8/b7 blocks lifted verbatim from real captures ----
    // Primary "f3" channel (TesterPresent + a short RDBI poll), from the
    // `research/vag-hex-framing.md` "Link cipher" decrypted-proof section
    // (originally reading-ecus.pcapng). do NOT commit the pcapng — these are the
    // small block vectors it authorises.

    /// f3 TesterPresent request → off6..13 = `02 3E 00 00 00 00 00 00`.
    const F3_TESTER_PRESENT_REQ: [u8; 16] = hex16("f38344dd7c5f009799f6da7c9c3a00fc");
    /// f3 ReadDataByIdentifier request → off6..13 = `03 22 74 58 00 00 00 00`.
    const F3_RDBI_REQ: [u8; 16] = hex16("f39f44dd7c5f018bedaeda7c9c3afbfd");
    /// f3 TesterPresent positive response → off6.. = `05 7E 00 ..`.
    const F3_TESTER_PRESENT_RESP: [u8; 16] = hex16("f38244dd6c5f07d799f68f8363c500fc");

    // Gearbox SW-version channel (b3..eb0d..55), multiframe RDBI response.
    /// The channel's modal single-frame RDBI request (crib source).
    const SW_VERSION_REQ: [u8; 16] = hex16("b331eb0d335589ad90f89e94b35a3c6d");
    /// A response block whose decoded data region carries `10 03` = "1003".
    const SW_VERSION_RESP: [u8; 16] = hex16("b330eb0d23559ba1f15c5584b0503f6d");

    /// The keystream over the UDS region for the f3 channel, from the framing doc.
    const KS_F3_6_13: [u8; 8] = [0x02, 0xA9, 0x99, 0xF6, 0xDA, 0x7C, 0x9C, 0x3A];

    /// Const hex → [u8;16] so fixtures read as the on-wire hex string.
    const fn hex16(s: &str) -> [u8; 16] {
        let b = s.as_bytes();
        assert!(b.len() == 32, "hex16 needs 32 nibbles");
        let mut out = [0u8; 16];
        let mut i = 0;
        while i < 16 {
            out[i] = (nibble(b[i * 2]) << 4) | nibble(b[i * 2 + 1]);
            i += 1;
        }
        out
    }
    const fn nibble(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("bad hex nibble"),
        }
    }

    /// The f3 known-plaintext: the whole UDS region of TesterPresent is known
    /// (PCI `02`, SID `3E`, data `00`, then `00` padding).
    fn f3_tester_present_crib() -> [Option<u8>; 16] {
        let mut c = [None; 16];
        for (i, p) in [0x02u8, 0x3E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
            .into_iter()
            .enumerate()
        {
            c[6 + i] = Some(p);
        }
        c
    }

    #[test]
    fn decrypt_block_is_pure_xor() {
        let ks = [0xAAu8; 16];
        let cipher = [0x55u8; 16];
        assert_eq!(decrypt_block(&cipher, &ks), [0xFFu8; 16]);
        // Involutive: decrypt(decrypt(x)) == x.
        let once = decrypt_block(&F3_RDBI_REQ, &ks);
        assert_eq!(decrypt_block(&once, &ks), F3_RDBI_REQ);
    }

    #[test]
    fn recover_keystream_matches_framing_doc_vector() {
        // KS[6..13] = 02 A9 99 F6 DA 7C 9C 3A, from TesterPresent plaintext.
        let ks = recover_keystream(&F3_TESTER_PRESENT_REQ, &f3_tester_present_crib());
        assert_eq!(&ks[6..14], &KS_F3_6_13);
    }

    #[test]
    fn decode_tester_present_request() {
        let ks = recover_keystream(&F3_TESTER_PRESENT_REQ, &f3_tester_present_crib());
        let uds = decode_diag_frame(&F3_TESTER_PRESENT_REQ, &ks).expect("single frame");
        assert_eq!(uds.pci, 0x02);
        assert_eq!(uds.uds, vec![0x3E, 0x00]); // UDS TesterPresent
    }

    #[test]
    fn decode_rdbi_request_with_the_same_channel_keystream() {
        // Keystream recovered from TesterPresent decodes the channel's RDBI too.
        let ks = recover_keystream(&F3_TESTER_PRESENT_REQ, &f3_tester_present_crib());
        let uds = decode_diag_frame(&F3_RDBI_REQ, &ks).expect("single frame");
        assert_eq!(uds.pci, 0x03);
        assert_eq!(uds.uds, vec![0x22, 0x74, 0x58]); // UDS ReadDataByIdentifier
    }

    #[test]
    fn decode_tester_present_positive_response() {
        // The shared channel keystream decodes the response direction as well;
        // off7..9 carry the UDS positive response 7E 00.
        let ks = recover_keystream(&F3_TESTER_PRESENT_REQ, &f3_tester_present_crib());
        let block = decrypt_block(&F3_TESTER_PRESENT_RESP, &ks);
        assert_eq!(&block[7..9], &[0x7E, 0x00]); // UDS TesterPresent positive response
    }

    #[test]
    fn iso_tp_crib_marks_pci_sid_and_padding() {
        let crib = iso_tp_single_frame_crib(0x03, 0x22);
        assert_eq!(crib[6], Some(0x03));
        assert_eq!(crib[7], Some(0x22));
        assert_eq!(crib[8], None); // DID byte — request-specific, unknown
        assert_eq!(crib[9], None); // DID byte — unknown
        assert_eq!(crib[10], Some(0x00)); // padding after the 3-byte PDU
        assert_eq!(crib[13], Some(0x00));
    }

    #[test]
    fn decode_sw_version_multiframe_block_carries_1003() {
        // Recover the channel keystream from the modal RDBI request's ISO-TP
        // structure (PCI/SID + 0x00 padding tail), then decode a response block.
        let ks = recover_keystream(&SW_VERSION_REQ, &iso_tp_single_frame_crib(0x03, 0x22));
        let block = decrypt_block(&SW_VERSION_RESP, &ks);
        // The response is an ISO-TP first frame (PCI high nibble != 0), so
        // decode_diag_frame declines it — reassembly is a transport concern.
        assert!(decode_diag_frame(&SW_VERSION_RESP, &ks).is_none());
        // Its data region nonetheless decodes to bytes containing 10 03 = "1003".
        assert!(
            block.windows(2).any(|w| w == [0x10, 0x03]),
            "decoded block {block:02x?} should contain SW-version 10 03"
        );
        assert_eq!(&block[11..13], &[0x10, 0x03]);
    }

    #[test]
    fn decode_rejects_short_payload() {
        let ks = [0u8; 16];
        assert!(decode_diag_frame(&[0x00, 0x01, 0x02], &ks).is_none());
    }

    #[test]
    fn decode_declines_multiframe_pci() {
        // A block whose PCI decodes to a first-frame (0x1N) is not a single frame.
        let mut ks = [0u8; 16];
        ks[OFF_PCI] = 0x00; // so decoded PCI == cipher PCI
        let mut cipher = [0u8; 16];
        cipher[OFF_PCI] = 0x11;
        assert!(decode_diag_frame(&cipher, &ks).is_none());
    }
}
