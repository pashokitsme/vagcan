//! Decryption of Ross-Tech VCDS compiled `.clb` label files.
//!
//! `.clb` files are a per-record-encrypted container: each record's plaintext
//! is a line of the SAME textual format `label::parse_label` already
//! understands. Encryption is TEA (Tiny Encryption Algorithm), 32 rounds, in
//! CBC mode, little-endian 32-bit words. Every record uses its own IV, derived
//! from a file-constant `w7` (0..=255) and the record's index (`w15`). `w7` is
//! not stored in the file; it is recovered by brute force, scoring each
//! candidate's decrypted output for printability.

const DELTA: u32 = 0x9E37_79B9;
/// Initial `sum` for decryption: `DELTA.wrapping_mul(32)`.
const SUM0: u32 = 0xC6EF_3720;
const KEY_CLB: [u32; 4] = [0xfa7e_14d0, 0x249b_910e, 0x2fdd_6ffc, 0x1583_4a78];

/// Decrypt one 8-byte TEA block (32 rounds), before the CBC xor with the
/// previous ciphertext block / IV.
fn tea_decrypt_block(block: [u8; 8], key: &[u32; 4]) -> [u8; 8] {
    let mut v0 = u32::from_le_bytes(block[0..4].try_into().unwrap());
    let mut v1 = u32::from_le_bytes(block[4..8].try_into().unwrap());
    let mut s = SUM0;
    for _ in 0..32 {
        v1 = v1.wrapping_sub(
            (v0 << 4)
                .wrapping_add(key[2])
                ^ v0.wrapping_add(s)
                ^ (v0 >> 5).wrapping_add(key[3]),
        );
        v0 = v0.wrapping_sub(
            (v1 << 4)
                .wrapping_add(key[0])
                ^ v1.wrapping_add(s)
                ^ (v1 >> 5).wrapping_add(key[1]),
        );
        s = s.wrapping_sub(DELTA);
    }
    let mut out = [0u8; 8];
    out[0..4].copy_from_slice(&v0.to_le_bytes());
    out[4..8].copy_from_slice(&v1.to_le_bytes());
    out
}

/// TEA-CBC decrypt: `cipher` is processed in 8-byte blocks (any trailing
/// partial block, which should not occur for well-formed records, is
/// ignored). `P_i = TEA_dec(C_i) XOR C_{i-1}`, with `C_{-1} = iv`.
fn tea_cbc_decrypt(cipher: &[u8], key: &[u32; 4], iv: [u8; 8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(cipher.len());
    let mut prev = iv;
    for block in cipher.chunks_exact(8) {
        let block: [u8; 8] = block.try_into().unwrap();
        let dec = tea_decrypt_block(block, key);
        let mut plain = [0u8; 8];
        for i in 0..8 {
            plain[i] = dec[i] ^ prev[i];
        }
        out.extend_from_slice(&plain);
        prev = block;
    }
    out
}

/// Per-record IV: a function of the file-constant `w7` and the record's
/// index `w15`. All arithmetic is u32 wrapping.
fn clb_iv(w7: u32, w15: u32) -> [u8; 8] {
    let w15_1 = w15.wrapping_add(1);
    let w15_2 = w15.wrapping_add(2);
    let w15_3 = w15.wrapping_add(3);
    let w15_4 = w15.wrapping_add(4);
    let w7_1 = w7.wrapping_add(1);
    let w7_3 = w7.wrapping_add(3);

    let a = (w7.wrapping_add(2)).wrapping_mul(w15_1).wrapping_mul(w15_3);
    let mut w24 = w7_1.wrapping_mul(w15_2).wrapping_add(a);
    let r = w7 % w15_1;
    let w8 = if r != 0 { r } else { w15 % w7_1 };
    let mut w23 = w7_3.wrapping_mul(w7_1).wrapping_mul(w15_2).wrapping_add(w8);
    if w24 < 0xffff {
        w24 = (w24 << 16).wrapping_add(w15_4.wrapping_mul(w7_3).wrapping_mul(w7_1).wrapping_mul(w15_2));
    }
    if w23 < 0xffff {
        w23 = (w23 << 16).wrapping_add(w15_1.wrapping_mul(w15_2).wrapping_mul(w15_3));
    }
    let mut out = [0u8; 8];
    out[0..4].copy_from_slice(&w24.to_le_bytes());
    out[4..8].copy_from_slice(&w23.to_le_bytes());
    out
}

/// One record parsed out of the `.clb` container, before decryption.
enum RawRecord {
    /// An empty line (`00 0a` marker with no payload).
    Blank,
    /// An encrypted record: `cipher` is the ciphertext (a multiple of 8
    /// bytes), `len` is the plaintext length in bytes, `index` is the
    /// record's position among non-blank records (used as `w15`).
    Data {
        cipher: Vec<u8>,
        len: u16,
        index: u32,
    },
}

/// Walk the `.clb` container format, splitting it into records without
/// decrypting them.
fn parse_container(data: &[u8]) -> Vec<RawRecord> {
    let mut pos = 0;
    let mut index = 0u32;
    let mut records = Vec::new();
    while pos + 2 <= data.len() {
        if data[pos] == 0x00 && data[pos + 1] == 0x0a {
            records.push(RawRecord::Blank);
            pos += 2;
            continue;
        }
        let len = ((data[pos] as usize) << 8) | (data[pos + 1] as usize);
        if len == 0 {
            break;
        }
        let clen = len.div_ceil(8) * 8;
        if pos + 2 + clen > data.len() {
            break; // truncated container; stop rather than panic
        }
        let cipher = data[pos + 2..pos + 2 + clen].to_vec();
        pos += 2 + clen;
        if pos + 2 <= data.len() && data[pos] == 0x00 && data[pos + 1] == 0x0a {
            pos += 2;
        }
        records.push(RawRecord::Data {
            cipher,
            len: len as u16,
            index,
        });
        index += 1;
    }
    records
}

/// Count bytes that look like printable label text (tab/LF/CR or printable
/// ASCII), used to score a candidate `w7` during brute force.
fn score_printable(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .filter(|&&b| b == 9 || b == 10 || b == 13 || (32..=126).contains(&b))
        .count() as u32
}

/// Recover the file-constant `w7` (0..=255, not stored in the file) by
/// brute force: for each candidate, decrypt every record's first block with
/// `clb_iv(w7, index)` and score the printability of its leading `min(8, len)`
/// plaintext bytes. Ties resolve to the lowest `w7` (candidates are tried in
/// ascending order and only strictly-greater scores replace the winner).
fn recover_w7(records: &[RawRecord]) -> u32 {
    let mut best_w7 = 0u32;
    let mut best_score: i64 = -1;
    for w7 in 0..=255u32 {
        let mut score = 0u32;
        for rec in records {
            let RawRecord::Data { cipher, len, index } = rec else {
                continue;
            };
            if cipher.len() < 8 {
                continue;
            }
            let iv = clb_iv(w7, *index);
            let first_block: [u8; 8] = cipher[0..8].try_into().unwrap();
            let dec = tea_decrypt_block(first_block, &KEY_CLB);
            let mut plain = [0u8; 8];
            for i in 0..8 {
                plain[i] = dec[i] ^ iv[i];
            }
            let take = (*len as usize).min(8);
            score += score_printable(&plain[..take]);
        }
        if i64::from(score) > best_score {
            best_score = i64::from(score);
            best_w7 = w7;
        }
    }
    best_w7
}

/// Decrypt a `.clb` file's bytes into the decoded label text (the same
/// format `label::parse_label` accepts). Latin-1 decoding is applied
/// byte-for-byte, matching `label::parse_label`'s own decoding.
pub fn decrypt_clb(data: &[u8]) -> String {
    let records = parse_container(data);
    let w7 = recover_w7(&records);
    let mut lines = Vec::with_capacity(records.len());
    for rec in &records {
        match rec {
            RawRecord::Blank => lines.push(String::new()),
            RawRecord::Data { cipher, len, index } => {
                let iv = clb_iv(w7, *index);
                let plain = tea_cbc_decrypt(cipher, &KEY_CLB, iv);
                let take = (*len as usize).min(plain.len());
                let text: String = plain[..take].iter().map(|&b| b as char).collect();
                lines.push(text);
            }
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::label::{parse_label, Record};

    /// Synthetic 80-byte `.clb` fixture (TEA-CBC-encrypted with `KEY_CLB`,
    /// `w7 = 7`) — no proprietary data, produced solely to exercise this
    /// decoder end to end.
    const FIXTURE_HEX: &str = "002738e02cf98f11742ee0b6f41102c2e55c4890aa526e2753a9263c7947f8b656f3467dc8f892f6c03a000a00202d7dc10402a81d837c41c4b66f69b6b50479e421595f5f5c20f4d6edd2d07b99000a";

    fn hex_decode(s: &str) -> Vec<u8> {
        assert_eq!(s.len() % 2, 0);
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn decrypts_fixture_to_expected_lines() {
        let data = hex_decode(FIXTURE_HEX);
        let text = decrypt_clb(&data);
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines, vec!["001,1,Engine Speed,,Range: 0..8000 /min", "001,2,Coolant,,Range: -48..143 C"]);
    }

    #[test]
    fn recovers_w7_seven() {
        let data = hex_decode(FIXTURE_HEX);
        let records = parse_container(&data);
        assert_eq!(recover_w7(&records), 7);
    }

    #[test]
    fn decrypted_fixture_parses_into_measurements() {
        let data = hex_decode(FIXTURE_HEX);
        let text = decrypt_clb(&data);
        let lf = parse_label("fixture.clb", text.as_bytes());
        let measurements: Vec<_> = lf
            .records
            .iter()
            .filter_map(|r| match r {
                Record::Measurement(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(measurements.len(), 2);
        assert_eq!(measurements[0].block, 1);
        assert_eq!(measurements[0].field, 1);
        assert_eq!(measurements[0].name, "Engine Speed");
        assert_eq!(measurements[1].block, 1);
        assert_eq!(measurements[1].field, 2);
        assert_eq!(measurements[1].name, "Coolant");
    }

    #[test]
    fn tea_cbc_roundtrip_is_reversible_with_matching_encrypt() {
        // Sanity check independent of the fixture: TEA is a Feistel-style
        // cipher, so encrypting with the "reverse" schedule (s starting at 0,
        // running forward, applying the inverse of the decrypt round) and
        // then decrypting must recover the original 8-byte block, using the
        // fixture's own key.
        fn tea_encrypt_block(block: [u8; 8], key: &[u32; 4]) -> [u8; 8] {
            let mut v0 = u32::from_le_bytes(block[0..4].try_into().unwrap());
            let mut v1 = u32::from_le_bytes(block[4..8].try_into().unwrap());
            let mut s = 0u32;
            for _ in 0..32 {
                s = s.wrapping_add(DELTA);
                v0 = v0.wrapping_add(
                    (v1 << 4).wrapping_add(key[0]) ^ v1.wrapping_add(s) ^ (v1 >> 5).wrapping_add(key[1]),
                );
                v1 = v1.wrapping_add(
                    (v0 << 4).wrapping_add(key[2]) ^ v0.wrapping_add(s) ^ (v0 >> 5).wrapping_add(key[3]),
                );
            }
            let mut out = [0u8; 8];
            out[0..4].copy_from_slice(&v0.to_le_bytes());
            out[4..8].copy_from_slice(&v1.to_le_bytes());
            out
        }

        let plain: [u8; 8] = *b"ABCDEFGH";
        let key = KEY_CLB;
        let cipher = tea_encrypt_block(plain, &key);
        let decrypted = tea_decrypt_block(cipher, &key);
        assert_eq!(decrypted, plain);
    }

    #[test]
    fn clb_iv_matches_hand_derived_vector_for_w7_seven() {
        // Hand-derived per the spec formula (w7=7, w15=0):
        //   a = (7+2)*(0+1)*(0+3) = 27
        //   w24 = (7+1)*(0+2) + 27 = 43; 43 < 0xffff so
        //         w24 = (43<<16) + (0+4)*(7+3)*(7+1)*(0+2) = 2_818_048 + 640 = 2_818_688 (0x2B0280)
        //   r = 7 % 1 = 0  =>  w8 = 0 % 8 = 0
        //   w23 = (7+3)*(7+1)*(0+2) + 0 = 160; 160 < 0xffff so
        //         w23 = (160<<16) + (0+1)*(0+2)*(0+3) = 10_485_760 + 6 = 10_485_766 (0xA00006)
        assert_eq!(clb_iv(7, 0), [0x80, 0x02, 0x2B, 0x00, 0x06, 0x00, 0xA0, 0x00]);
    }
}
