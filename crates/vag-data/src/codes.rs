//! Decoder for Ross-Tech VCDS `Codes.dat` — the fault-text store.
//!
//! `Codes.dat` (and its translations, e.g. `Code-RUS.dat`) is a flat keyed
//! record store. Each record is one line:
//!
//! ```text
//! record := <8 ASCII digits: key> ' ' <u8 cipher_len> <u8 text_len> <cipher> "\r\n"
//! ```
//!
//! `cipher_len` is `text_len` rounded up to a multiple of 8, and the payload
//! is TEA-CBC under the same `KEY_ROD` the `.rod` files use ([`crate::rod`]).
//! What was missing until now was the per-record first-block IV, which is why
//! earlier passes lost the first eight characters of every text. It is
//! derived from the key's own decimal spelling and one file-wide byte — see
//! [`block0_iv`] and [`CodesDb::file_constant`].
//!
//! ## What the keys are, and what they are not
//!
//! Two disjoint bands, and telling them apart matters more than reading them:
//!
//! * **below 65 536** — a legacy 5-digit VAG fault-code space (KWP-era). Sparse.
//! * **90 000 and above** — the 24-bit ISO/SAE DTC, `system:2 | code:14 | failure_type:8`,
//!   read as one big-endian number. `B1168` with failure type `0xF2` is
//!   `0x9168F2` = 9 529 586.
//!
//! **A VW-internal fault number is neither.** The number a VAG control unit
//! puts in a UDS `0x19` response — 229 504, 7 680, 291 104 on the reference
//! car — is not a key here, and the two spaces overlap numerically, so a
//! lookup that ignores the distinction returns a wrong answer rather than no
//! answer. Nothing in this module accepts a VW fault number; callers must
//! supply an ISO DTC. See `research/codes-dat.md`.

use std::collections::BTreeMap;

use crate::rod::{KS, MT, OFF_ROD};
use crate::tea::tea_cbc_decrypt;

/// The code page a `Codes.dat` is written in.
///
/// The file stores single bytes, not Unicode, and the page belongs to the
/// translation rather than to the container — nothing in a record says which
/// one it is. The English `Codes.dat` is Windows-1252 and `Code-RUS.dat` is
/// Windows-1251.
///
/// Mapping a byte straight to a `char` — ISO 8859-1, the obvious thing —
/// silently produces wrong text either way: `0x96` occurs 191 times in the
/// English file, where it is an en dash and 8859-1 makes it U+0096, a C1
/// control; and 27 380 of the Russian file's 27 587 records turn into
/// mojibake. So the page is asked for, with the English one as the default
/// because that is the file this project reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodePage {
    /// Windows-1252 — the English `Codes.dat`.
    #[default]
    Windows1252,
    /// Windows-1251 — `Code-RUS.dat`.
    Windows1251,
}

impl CodePage {
    /// Decode one record's bytes.
    ///
    /// A byte the page leaves undefined becomes U+FFFD rather than something
    /// plausible: these files decrypt exactly, so an undefined byte means the
    /// text is not in this page and the reader should see that.
    pub fn decode(self, bytes: &[u8]) -> String {
        bytes.iter().map(|&b| self.char_of(b)).collect()
    }

    fn char_of(self, b: u8) -> char {
        if b < 0x80 {
            return b as char;
        }
        match self {
            // 1252 is 8859-1 except for the 0x80..0xA0 block.
            Self::Windows1252 => match b {
                0xA0..=0xFF => b as char,
                _ => CP1252_HIGH[(b - 0x80) as usize],
            },
            // In 1251 the Cyrillic alphabet is contiguous from 0xC0, so only
            // the punctuation block below it needs a table.
            Self::Windows1251 => match b {
                0xC0..=0xFF => {
                    char::from_u32(0x410 + (b as u32 - 0xC0)).expect("in the Cyrillic block")
                }
                _ => CP1251_HIGH[(b - 0x80) as usize],
            },
        }
    }
}

/// Windows-1252, `0x80..=0x9F`. `\u{FFFD}` marks the five undefined bytes.
const CP1252_HIGH: [char; 32] = [
    '€', '\u{FFFD}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{FFFD}', 'Ž',
    '\u{FFFD}', '\u{FFFD}', '\u{2018}', '\u{2019}', '“', '”', '•', '–', '—', '˜', '™', 'š', '›',
    'œ', '\u{FFFD}', 'ž', 'Ÿ',
];

/// Windows-1251, `0x80..=0xBF` — everything below the contiguous Cyrillic run.
const CP1251_HIGH: [char; 64] = [
    'Ђ', 'Ѓ', '‚', 'ѓ', '„', '…', '†', '‡', '€', '‰', 'Љ', '‹', 'Њ', 'Ќ', 'Ћ', 'Џ', 'ђ',
    '\u{2018}', '\u{2019}', '“', '”', '•', '–', '—', '\u{FFFD}', '™', 'љ', '›', 'њ', 'ќ', 'ћ', 'џ',
    '\u{00A0}', 'Ў', 'ў', 'Ј', '¤', 'Ґ', '¦', '§', 'Ё', '©', 'Є', '«', '¬', '\u{00AD}', '®', 'Ї',
    '°', '±', 'І', 'і', 'ґ', 'µ', '¶', '·', 'ё', '№', 'є', '»', 'ј', 'Ѕ', 'ѕ', 'ї',
];

/// The lowest key Ross-Tech uses for the 24-bit ISO/SAE DTC band. Below this
/// (and below 65 536 in practice) the keys are legacy 5-digit fault codes in
/// an unrelated numbering.
pub const ISO_BAND_START: u32 = 90_000;

/// Where the two-byte-DTC band starts: a key of `100 000 + <16-bit DTC>`.
const SHORT_BAND_START: u32 = 100_000;

/// Where the three-byte-DTC band starts: the key **is** the 24-bit DTC.
const LONG_BAND_START: u32 = 1_000_000;

/// The SAE code VCDS prints beside a fault, read out of the key itself.
///
/// A `Codes.dat` key spells its own code, in one of two ways, and which one
/// is decided by magnitude:
///
/// * **`100 000 + <16-bit DTC>`** — the 20 059 six-digit keys, none of which
///   falls outside `100 000..=165 535`. These name a *component* and carry no
///   failure type of their own; the failure type comes from the registry row
///   (`research/fault-naming-hop.md` §3). 137 973 is `100 000 + 0x9455` and
///   VCDS prints `B1455` for it.
/// * **the 24-bit DTC outright** — every key of seven digits or more, i.e.
///   `<16-bit DTC> << 8 | <failure type>`. 9 529 586 is `0x9168F2` and VCDS
///   prints `B1168 F2`.
///
/// The boundary is not a guess: across the whole global fault registry, the
/// row's own failure-type field equals `key & 0xFF` for **19 765 of 19 765**
/// rows whose key is a million or more, and for 0.3 % of the rest — chance.
///
/// The 16-bit value is `system:2 | code:14`, the split ISO 14229-1 uses and
/// which `research/codes-dat.md` §3.1 measured on this file: 22 045 `P`,
/// 5 917 `B`, 1 830 `C`, 99 `U`, with the texts matching the letter.
///
/// Returns `None` below 100 000 — the legacy five-digit band and the block of
/// user-interface strings above it are not DTCs, and naming one would be the
/// same class of wrong answer [`CodesDb::iso_dtc`] refuses.
pub fn sae_code(key: u32) -> Option<String> {
    let short = match key {
        k if k >= LONG_BAND_START => ((k >> 8) & 0xFFFF) as u16,
        k if (SHORT_BAND_START..SHORT_BAND_START + 0x1_0000).contains(&k) => {
            (k - SHORT_BAND_START) as u16
        }
        _ => return None,
    };
    let system = ['P', 'C', 'B', 'U'][(short >> 14) as usize];
    Some(format!("{system}{:04X}", short & 0x3FFF))
}

/// The first-block IV for one record, given the file-wide constant.
///
/// The same construction the `.rod` sections use — `KS` supplies a per-byte
/// addend, `MT` a per-position multiplier — with three differences, all read
/// off the ARM64 build's record fetch (`fcn.1400e1400` in `VCDS-ARM.exe`
/// 26.3, `0x1400e1908`–`0x1400e19dc`):
///
/// * the seed is the key's own `"%08d"` spelling, all eight bytes, where a
///   `.rod` section seeds from its tag;
/// * the `KS` index is driven by seed byte **5**, not byte 1, and the
///   multiplier offsets are [`OFF_ROD`] **in reverse**;
/// * a file-wide byte is added alongside the `KS` term. VCDS holds it in a
///   global it fills at load time, so it is not in the file and has to be
///   recovered — see [`CodesDb::file_constant`]. It is 0 for the English
///   `Codes.dat` and 208 for `Code-RUS.dat`.
///
/// Verified against the reference car: key 9 529 586 with constant 0 gives
/// `47 02 c8 cd 6c 50 dc d3`, which decrypts to text beginning `Steering`.
pub fn block0_iv(key: u32, file_constant: u8) -> [u8; 8] {
    let seed = format!("{key:08}").into_bytes();
    let m = seed[5] as usize;
    let mut iv = [0u8; 8];
    for (i, slot) in iv.iter_mut().enumerate() {
        let s = seed[i]
            .wrapping_add(KS[(m * (i + 2)) & 0xff])
            .wrapping_add(file_constant);
        *slot = s.wrapping_mul(MT[OFF_ROD[7 - i]]);
    }
    iv
}

/// One record as the container framed it, before decryption.
struct RawRecord {
    key: u32,
    text_len: usize,
    cipher: Vec<u8>,
}

/// Recover the file-wide constant (0..=255, not stored in the file).
///
/// The constant only feeds the IV, and the IV is XOR'd in *after* the TEA
/// decrypt, so blocks 1.. are already correct whatever it is. That gives a
/// scoring model for free and without assuming a language: take the byte
/// distribution of everything past the first eight characters, and pick the
/// candidate whose first blocks look most like the rest of the same file.
/// Printability alone does not separate them — on `Code-RUS.dat` two
/// candidates give fully printable first blocks and only one gives Cyrillic.
fn recover_file_constant(records: &[RawRecord]) -> u8 {
    let key_rod = crate::rod::KEY_ROD;
    let mut counts = [0u64; 256];
    for rec in records {
        // Block 0 needs the IV; every later block does not.
        if rec.cipher.len() > 8 {
            let tail = tea_cbc_decrypt(
                &rec.cipher[8..],
                &key_rod,
                rec.cipher[..8].try_into().unwrap(),
            );
            for &b in tail.iter().take(rec.text_len.saturating_sub(8)) {
                counts[b as usize] += 1;
            }
        }
    }
    let total: u64 = counts.iter().sum();
    if total == 0 {
        return 0;
    }
    let logp: Vec<f64> = counts
        .iter()
        .map(|&n| ((n as f64 + 0.5) / (total as f64 + 128.0)).ln())
        .collect();

    // The first block's TEA decrypt does not depend on the candidate, so do
    // it once per record instead of once per (candidate, record) pair.
    let firsts: Vec<(u32, usize, [u8; 8])> = records
        .iter()
        .filter(|r| r.cipher.len() >= 8 && r.text_len >= 8)
        .map(|r| {
            let dec = tea_cbc_decrypt(&r.cipher[..8], &key_rod, [0u8; 8]);
            (
                r.key,
                r.text_len,
                dec.try_into().expect("one block in, one out"),
            )
        })
        .collect();

    let mut best = 0u8;
    let mut best_score = f64::NEG_INFINITY;
    for candidate in 0..=255u8 {
        let mut score = 0.0;
        for (key, _, dec) in &firsts {
            let iv = block0_iv(*key, candidate);
            for i in 0..8 {
                score += logp[(dec[i] ^ iv[i]) as usize];
            }
        }
        if score > best_score {
            best_score = score;
            best = candidate;
        }
    }
    best
}

/// Every record in a `Codes.dat`, keyed by its 8-digit id.
///
/// Malformed records are skipped rather than guessed at; a truncated file
/// yields the records that did parse.
#[derive(Debug, Clone, Default)]
pub struct CodesDb {
    texts: BTreeMap<u32, String>,
    file_constant: u8,
}

impl CodesDb {
    /// Parse and decrypt a whole `Codes.dat`, recovering the file-wide IV
    /// constant on the way. Reads the text as Windows-1252, the English file's
    /// page; use [`CodesDb::parse_in`] for a translation.
    pub fn parse(data: &[u8]) -> Self {
        Self::parse_in(data, CodePage::default())
    }

    /// [`CodesDb::parse`], for a file in a named code page.
    pub fn parse_in(data: &[u8], page: CodePage) -> Self {
        let raw = Self::frame(data);
        let file_constant = recover_file_constant(&raw);
        let texts = raw
            .into_iter()
            .filter_map(|r| {
                let plain = tea_cbc_decrypt(
                    &r.cipher,
                    &crate::rod::KEY_ROD,
                    block0_iv(r.key, file_constant),
                );
                (r.text_len <= plain.len()).then(|| (r.key, page.decode(&plain[..r.text_len])))
            })
            .collect();
        Self {
            texts,
            file_constant,
        }
    }

    /// The file-wide IV byte recovered by [`CodesDb::parse`]. 0 for the
    /// English `Codes.dat`, 208 for `Code-RUS.dat`.
    pub fn file_constant(&self) -> u8 {
        self.file_constant
    }

    /// Walk the container into records without decrypting anything.
    fn frame(data: &[u8]) -> Vec<RawRecord> {
        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos < data.len() {
            // Header: 8 digits, a space, cipher_len, text_len. The cipher is
            // binary and may contain CRLF, so records are walked by their
            // declared length, never by splitting on newlines.
            if pos + 11 > data.len() {
                break;
            }
            let head = &data[pos..pos + 8];
            if !head.iter().all(u8::is_ascii_digit) || data[pos + 8] != b' ' {
                break;
            }
            let cipher_len = data[pos + 9] as usize;
            let text_len = data[pos + 10] as usize;
            let start = pos + 11;
            if cipher_len % 8 != 0 || text_len > cipher_len || start + cipher_len > data.len() {
                break;
            }
            let key: u32 = match std::str::from_utf8(head).ok().and_then(|s| s.parse().ok()) {
                Some(key) => key,
                None => break,
            };
            out.push(RawRecord {
                key,
                text_len,
                cipher: data[start..start + cipher_len].to_vec(),
            });
            // Skip the record's trailing CRLF when it is there.
            pos = start + cipher_len;
            if data[pos..].starts_with(b"\r\n") {
                pos += 2;
            }
        }
        out
    }

    /// The text stored under a raw key, whichever band it falls in.
    pub fn get(&self, key: u32) -> Option<&str> {
        self.texts.get(&key).map(String::as_str)
    }

    /// The text for a 24-bit ISO/SAE DTC as a control unit reports it —
    /// three bytes, big-endian, failure type last.
    ///
    /// Refuses anything below [`ISO_BAND_START`]: down there the file holds
    /// legacy 5-digit codes in a different numbering, and answering from them
    /// would be a confident wrong name rather than an honest absence.
    pub fn iso_dtc(&self, dtc: [u8; 3]) -> Option<&str> {
        let key = u32::from_be_bytes([0, dtc[0], dtc[1], dtc[2]]);
        (key >= ISO_BAND_START).then(|| self.get(key)).flatten()
    }

    pub fn len(&self) -> usize {
        self.texts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.texts.is_empty()
    }

    /// Every `(key, text)` pair, in ascending key order.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &str)> {
        self.texts.iter().map(|(k, v)| (*k, v.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tea::tea_decrypt_block;

    /// The IV recovered from the reference car's own fault text. Key
    /// 9 529 586 is `B1168` failure type `0xF2` — the steering-angle-sensor
    /// initialisation fault the reference car stores — and its record begins
    /// `Steering`, which is exactly the eight bytes an unknown IV used to
    /// destroy.
    #[test]
    fn block0_iv_matches_the_vector_recovered_from_the_car() {
        assert_eq!(
            block0_iv(9_529_586, 0),
            [0x47, 0x02, 0xc8, 0xcd, 0x6c, 0x50, 0xdc, 0xd3]
        );
    }

    /// The IV depends on the whole key, not just the digit at each position:
    /// two keys sharing seven of eight digits differ in more than one IV byte
    /// when the differing digit is the one that drives the `KS` index.
    #[test]
    fn block0_iv_is_driven_by_seed_byte_five() {
        // "09529586" vs "09529686" differ only in seed byte 5, and that byte
        // selects the KS index for every position, so all eight IV bytes move.
        let a = block0_iv(9_529_586, 0);
        let b = block0_iv(9_529_686, 0);
        assert_eq!(a.iter().zip(b.iter()).filter(|(x, y)| x != y).count(), 8);
        // Changing only the last digit moves only the last IV byte.
        let c = block0_iv(9_529_587, 0);
        assert_eq!(a[..7], c[..7]);
        assert_ne!(a[7], c[7]);
    }

    fn tea_cbc_encrypt(plain: &[u8], key: &[u32; 4], iv: [u8; 8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(plain.len());
        let mut prev = iv;
        for chunk in plain.chunks_exact(8) {
            let mut block = [0u8; 8];
            for i in 0..8 {
                block[i] = chunk[i] ^ prev[i];
            }
            // TEA encrypt = the decrypt rounds run backwards; easier here to
            // invert the known-good decrypt by search-free algebra: encrypt is
            // written out directly.
            let mut v0 = u32::from_le_bytes(block[0..4].try_into().unwrap());
            let mut v1 = u32::from_le_bytes(block[4..8].try_into().unwrap());
            let mut s = 0u32;
            for _ in 0..32 {
                s = s.wrapping_add(crate::tea::DELTA);
                v0 = v0.wrapping_add(
                    (v1 << 4).wrapping_add(key[0])
                        ^ v1.wrapping_add(s)
                        ^ (v1 >> 5).wrapping_add(key[1]),
                );
                v1 = v1.wrapping_add(
                    (v0 << 4).wrapping_add(key[2])
                        ^ v0.wrapping_add(s)
                        ^ (v0 >> 5).wrapping_add(key[3]),
                );
            }
            let mut cipher = [0u8; 8];
            cipher[0..4].copy_from_slice(&v0.to_le_bytes());
            cipher[4..8].copy_from_slice(&v1.to_le_bytes());
            out.extend_from_slice(&cipher);
            prev = cipher;
        }
        out
    }

    /// The encrypt helper above has to be the exact inverse of the shipped
    /// decrypt, or the container test below would be testing itself.
    #[test]
    fn encrypt_helper_inverts_the_shipped_decrypt() {
        let block = *b"Steering";
        let cipher = tea_cbc_encrypt(&block, &crate::rod::KEY_ROD, [0u8; 8]);
        let back = tea_decrypt_block(cipher[..8].try_into().unwrap(), &crate::rod::KEY_ROD);
        assert_eq!(back, block);
    }

    fn record_c(key: u32, text: &str, file_constant: u8) -> Vec<u8> {
        let mut plain = text.as_bytes().to_vec();
        let text_len = plain.len();
        plain.resize(text_len.div_ceil(8) * 8, 0);
        let cipher = tea_cbc_encrypt(&plain, &crate::rod::KEY_ROD, block0_iv(key, file_constant));
        let mut out = format!("{key:08} ").into_bytes();
        out.push(cipher.len() as u8);
        out.push(text_len as u8);
        out.extend_from_slice(&cipher);
        out.extend_from_slice(b"\r\n");
        out
    }

    fn record(key: u32, text: &str) -> Vec<u8> {
        record_c(key, text, 0)
    }

    /// A record whose text is bytes rather than a `&str` — the file stores a
    /// code page, so a test about one cannot start from Rust's UTF-8.
    fn record_bytes(key: u32, text: &[u8]) -> Vec<u8> {
        let text_len = text.len();
        let mut plain = text.to_vec();
        plain.resize(text_len.div_ceil(8) * 8, 0);
        let cipher = tea_cbc_encrypt(&plain, &crate::rod::KEY_ROD, block0_iv(key, 0));
        let mut out = format!("{key:08} ").into_bytes();
        out.push(cipher.len() as u8);
        out.push(text_len as u8);
        out.extend_from_slice(&cipher);
        out.extend_from_slice(b"\r\n");
        out
    }

    #[test]
    fn container_round_trips_and_keeps_the_first_eight_characters() {
        let mut file = record(9_529_586, "Steering Angle Sensor: Not Initialized");
        file.extend(record(10_489_840, "Internal Fault: - "));
        let db = CodesDb::parse(&file);
        assert_eq!(db.len(), 2);
        assert_eq!(
            db.get(9_529_586),
            Some("Steering Angle Sensor: Not Initialized")
        );
        assert_eq!(db.get(10_489_840), Some("Internal Fault: - "));
    }

    /// A record whose ciphertext happens to contain `\r\n` must not split the
    /// container. Records are walked by declared length for exactly this
    /// reason, and the real file does contain such records.
    #[test]
    fn a_crlf_inside_the_ciphertext_does_not_end_a_record() {
        let mut file = record(9_529_586, "Steering Angle Sensor: Not Initialized");
        // Splice a CRLF into the middle of the first record's ciphertext.
        file[20] = b'\r';
        file[21] = b'\n';
        file.extend(record(10_489_840, "Internal Fault: - "));
        let db = CodesDb::parse(&file);
        assert_eq!(db.len(), 2, "the second record must still be found");
        assert_eq!(db.get(10_489_840), Some("Internal Fault: - "));
    }

    /// `iso_dtc` must refuse the legacy band rather than answer from it.
    /// Fault 297 is the case that made this rule: the file holds
    /// "Gearbox Speed Sensor (G38)" under key 297, and a control unit
    /// reporting DTC `00 01 29` does not mean that.
    #[test]
    fn a_key_spells_the_sae_code_vcds_prints() {
        // Every one of these is a pair VCDS printed itself, on one of the two
        // cars in research/rd-rod/pairs.tsv — six-digit component keys on the
        // left of the band boundary and 24-bit DTCs on the right.
        assert_eq!(sae_code(137_973).as_deref(), Some("B1455"));
        assert_eq!(sae_code(137_375).as_deref(), Some("B11FF"));
        assert_eq!(sae_code(120_669).as_deref(), Some("C10BD"));
        assert_eq!(sae_code(153_539).as_deref(), Some("U1123"));
        assert_eq!(sae_code(101_089).as_deref(), Some("P0441"));
        assert_eq!(sae_code(9_529_586).as_deref(), Some("B1168"));
        assert_eq!(sae_code(10_485_833).as_deref(), Some("B2000"));
        assert_eq!(sae_code(149_253).as_deref(), Some("U0065"));
    }

    #[test]
    fn a_key_that_is_not_a_dtc_gets_no_code_rather_than_a_wrong_one() {
        // 90 000..99 999 is a block of user-interface strings ("ADP. Run",
        // "Term 15 On"), and below 65 536 are the legacy five-digit codes.
        // Neither is a DTC and neither may be spelled as one.
        assert_eq!(sae_code(90_001), None);
        assert_eq!(sae_code(297), None);
        assert_eq!(sae_code(0), None);
    }

    #[test]
    fn iso_dtc_refuses_the_legacy_band() {
        let file = record(297, "Gearbox Speed Sensor (G38)");
        let db = CodesDb::parse(&file);
        assert_eq!(db.get(297), Some("Gearbox Speed Sensor (G38)"));
        assert_eq!(db.iso_dtc([0x00, 0x01, 0x29]), None);
    }

    #[test]
    fn iso_dtc_reads_three_bytes_big_endian() {
        let file = record(9_529_586, "Steering Angle Sensor: Not Initialized");
        let db = CodesDb::parse(&file);
        assert_eq!(
            db.iso_dtc([0x91, 0x68, 0xf2]),
            Some("Steering Angle Sensor: Not Initialized")
        );
    }

    /// A translated file uses a different file-wide constant, and it is not in
    /// the file. Build one with a nonzero constant and check the recovery
    /// finds it from the text alone.
    #[test]
    fn the_file_wide_constant_is_recovered_from_the_text() {
        const C: u8 = 208;
        let lines = [
            "Steering Angle Sensor: Not Initialized",
            "Steering Angle Sensor: Rate of Change to High",
            "Steering Angle Sensor: Synchronization Failed",
            "Transmission Control Unit: Internal Fault Detected",
            "Control Module for Airbag Deployment: No Communication",
            "Sensor for Engine Coolant Temperature: Implausible Signal",
        ];
        let mut file = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            file.extend(record_c(9_529_580 + i as u32, line, C));
        }
        let db = CodesDb::parse(&file);
        assert_eq!(db.file_constant(), C);
        assert_eq!(db.get(9_529_580), Some(lines[0]));
        assert_eq!(db.get(9_529_585), Some(lines[5]));
    }

    /// A truncated file gives back the records that did parse, and no panic.
    #[test]
    fn truncated_input_yields_what_parsed() {
        let mut file = record(9_529_586, "Steering Angle Sensor: Not Initialized");
        let whole = file.len();
        file.extend(record(10_489_840, "Internal Fault: - "));
        file.truncate(whole + 14);
        let db = CodesDb::parse(&file);
        assert_eq!(db.len(), 1);
        assert_eq!(
            db.get(9_529_586),
            Some("Steering Angle Sensor: Not Initialized")
        );
    }

    /// The byte that made the point: 0x96 appears 191 times in the English
    /// file. Read as ISO 8859-1 it is U+0096, a C1 control, and the text was
    /// reported as clean because nothing looked.
    #[test]
    fn the_english_page_reads_punctuation_and_not_control_bytes() {
        let text = CodePage::Windows1252.decode(&[b'A', 0x96, b'B', 0xA9, 0xB0]);
        assert_eq!(text, "A–B©°");
        assert!(
            !text.chars().any(|c| c.is_control()),
            "a code page that yields control characters is the wrong code page"
        );
    }

    /// `Code-RUS.dat` is Windows-1251, where the same high bytes are Cyrillic.
    /// The word is the one the recovered IV was confirmed against, in Russian.
    #[test]
    fn the_russian_page_reads_cyrillic_where_the_english_one_reads_symbols() {
        let bytes = [0xC4, 0xE0, 0xF2, 0xF7, 0xE8, 0xEA, 0xB8, 0xB9];
        assert_eq!(CodePage::Windows1251.decode(&bytes), "Датчикё№");
        assert_ne!(CodePage::Windows1252.decode(&bytes), "Датчикё№");
    }

    /// Choosing the page is the caller's job, so a file parsed in the wrong
    /// one must differ visibly rather than quietly.
    #[test]
    fn the_page_reaches_the_stored_text() {
        let file = record_bytes(9_529_586, &[0xC4, 0xE0, 0xF2]);
        assert_eq!(
            CodesDb::parse_in(&file, CodePage::Windows1251).get(9_529_586),
            Some("Дат")
        );
        assert_eq!(CodesDb::parse(&file).get(9_529_586), Some("Äà\u{f2}"));
    }
}
