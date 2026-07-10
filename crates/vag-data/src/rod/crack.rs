//! Offline recovery of a `.rod` zlib section's corrupted first-block plaintext
//! bytes (deflate bytes `1..=5` = plaintext `3..8`), for sections whose
//! per-record `product` term is nonzero and thus not stored in the file.
//!
//! Ported from `research/clb-crack/rod_crack/src/main.rs` (multithreaded Rust
//! brute-forcer) and the framing/prep logic in
//! `research/clb-crack/rod_struc_decode.py`. Feature-gated (`rod-crack`): it is
//! CPU-heavy and kept out of the default build.
//!
//! Approach (see `research/rod-labels.md` §1): a `.rod` section is
//! TEA-CBC(`KEY_ROD`) with an 8-byte IV. `IV[0..3]` is tag-derived (exact);
//! `IV[3..8]` carries the low 5 bytes of the runtime `product`. Only the first
//! cipher block's plaintext depends on the IV (CBC), so the zlib magic
//! `78 da` (plaintext `0..2`) survives but deflate bytes `1..=5` are corrupted.
//! Deflate byte 0 (plaintext[2]) is exact. We brute-force the 5 unknown bytes
//! over reduced per-byte candidate sets, filtering with an incremental-Kraft
//! dynamic-Huffman header parse, and confirm with a full `miniz_oxide` inflate
//! that yields exactly `plainlen` bytes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::rod::{KEY_ROD, MT, OFF_ROD, rod_block0_iv};
use crate::tea::{tea_cbc_decrypt, tea_decrypt_block};

const CLCL_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// LSB-first bit reader over `[d0] ++ guess[0..5] ++ tail`.
struct Bits<'a> {
    d0: u8,
    guess: &'a [u8; 5],
    tail: &'a [u8],
    byte: usize,
    nbits: u32,
    cur: u64,
}
impl<'a> Bits<'a> {
    fn new(d0: u8, guess: &'a [u8; 5], tail: &'a [u8]) -> Self {
        Bits {
            d0,
            guess,
            tail,
            byte: 0,
            nbits: 0,
            cur: 0,
        }
    }
    #[inline]
    fn getb(&self, i: usize) -> u8 {
        if i == 0 {
            self.d0
        } else if i <= 5 {
            self.guess[i - 1]
        } else {
            self.tail[i - 6]
        }
    }
    #[inline]
    fn read(&mut self, n: u32) -> u64 {
        while self.nbits < n {
            self.cur |= (self.getb(self.byte) as u64) << self.nbits;
            self.byte += 1;
            self.nbits += 8;
        }
        let v = self.cur & ((1u64 << n) - 1);
        self.cur >>= n;
        self.nbits -= n;
        v
    }
}

fn kraft_ok(lens: &[u8]) -> bool {
    let mut cnt = [0i64; 16];
    let mut maxbl = 0usize;
    for &l in lens {
        if l > 0 {
            cnt[l as usize] += 1;
            if (l as usize) > maxbl {
                maxbl = l as usize;
            }
        }
    }
    if maxbl == 0 {
        return true;
    }
    let mut left: i64 = 1;
    for &c in &cnt[1..=maxbl] {
        left <<= 1;
        left -= c;
        if left < 0 {
            return false;
        }
    }
    true
}

struct Huff {
    counts: [u16; 16],
    symbols: Vec<u16>,
    maxlen: u32,
}
fn build_huff(lens: &[u8]) -> Option<Huff> {
    let mut counts = [0u16; 16];
    let mut maxlen = 0u32;
    for &l in lens {
        if l > 0 {
            counts[l as usize] += 1;
            if l as u32 > maxlen {
                maxlen = l as u32;
            }
        }
    }
    if maxlen == 0 {
        return None;
    }
    let mut offs = [0u16; 16];
    let mut s = 0u16;
    for i in 1..16 {
        offs[i] = s;
        s += counts[i];
    }
    let mut symbols = vec![0u16; s as usize];
    for (sym, &l) in lens.iter().enumerate() {
        if l > 0 {
            symbols[offs[l as usize] as usize] = sym as u16;
            offs[l as usize] += 1;
        }
    }
    Some(Huff {
        counts,
        symbols,
        maxlen,
    })
}

#[inline]
fn decode_sym(bs: &mut Bits, h: &Huff) -> Option<u16> {
    let mut code: i32 = 0;
    let mut first: i32 = 0;
    let mut index: i32 = 0;
    for len in 1..=h.maxlen as usize {
        code |= bs.read(1) as i32;
        let count = h.counts[len] as i32;
        if code - first < count {
            return Some(h.symbols[(index + (code - first)) as usize]);
        }
        index += count;
        first += count;
        first <<= 1;
        code <<= 1;
    }
    None
}

/// Full dynamic-Huffman header parse (must be BTYPE=2). Returns true if the
/// header decodes cleanly with valid Kraft on the code-length, literal and
/// distance code lengths.
fn header_ok(d0: u8, guess: &[u8; 5], tail: &[u8]) -> bool {
    let mut bs = Bits::new(d0, guess, tail);
    let _bfinal = bs.read(1);
    if bs.read(2) != 2 {
        return false;
    }
    let hlit = bs.read(5) as usize + 257;
    let hdist = bs.read(5) as usize + 1;
    let hclen = bs.read(4) as usize + 4;
    let mut cl = [0u8; 19];
    let mut cnt = [0i64; 16];
    for i in 0..hclen {
        let l = bs.read(3) as u8;
        cl[CLCL_ORDER[i]] = l;
        if l > 0 {
            cnt[l as usize] += 1;
            let mut left: i64 = 1;
            let mut over = false;
            for &c in &cnt[1..=7] {
                left <<= 1;
                left -= c;
                if left < 0 {
                    over = true;
                    break;
                }
            }
            if over {
                return false;
            }
        }
    }
    let clh = match build_huff(&cl) {
        Some(h) => h,
        None => return false,
    };
    let n = hlit + hdist;
    let mut lengths: Vec<u8> = Vec::with_capacity(n + 8);
    while lengths.len() < n {
        let sym = match decode_sym(&mut bs, &clh) {
            Some(s) => s,
            None => return false,
        };
        match sym {
            0..=15 => lengths.push(sym as u8),
            16 => {
                if lengths.is_empty() {
                    return false;
                }
                let r = (bs.read(2) + 3) as usize;
                let last = *lengths.last().unwrap();
                lengths.resize(lengths.len() + r, last);
            }
            17 => {
                let r = (bs.read(3) + 3) as usize;
                lengths.resize(lengths.len() + r, 0);
            }
            18 => {
                let r = (bs.read(7) + 11) as usize;
                lengths.resize(lengths.len() + r, 0);
            }
            _ => return false,
        }
        if lengths.len() > n {
            return false;
        }
    }
    if !kraft_ok(&lengths[..hlit]) {
        return false;
    }
    if !kraft_ok(&lengths[hlit..hlit + hdist]) {
        return false;
    }
    true
}

/// The reduced per-byte candidate sets for deflate bytes `d[1..=5]`
/// (= plaintext `3..8`). `t` is the raw ECB decryption of the first cipher
/// block. Because `IV[i] = (s * MT[OFF[i]]) & 0xff` and some multipliers are
/// even, several of these sets are much smaller than 256.
fn candidate_sets(tag_m: u8, t: &[u8; 8]) -> [Vec<u8>; 5] {
    let _ = tag_m; // m only affects IV[0..3]; IV[3..8] range is over all s
    std::array::from_fn(|k| {
        let i = k + 3;
        let mut ivvals: Vec<u8> = (0..=255u16)
            .map(|s| ((s as usize * MT[OFF_ROD[i]] as usize) & 0xff) as u8)
            .collect();
        ivvals.sort_unstable();
        ivvals.dedup();
        let mut cands: Vec<u8> = ivvals.iter().map(|&v| t[i] ^ v).collect();
        cands.sort_unstable();
        cands.dedup();
        cands
    })
}

/// Confirm a candidate `iv[3..8]` for a zlib section in O(1): rebuild the
/// deflate stream and check the dynamic-Huffman header parses AND the full
/// inflate yields exactly `plainlen` bytes. This is the same oracle the search
/// uses per candidate; exposed so the plumbing can be tested without running
/// the multi-minute brute force (which the `vag-rod` acceptance run exercises).
#[cfg(test)]
pub(crate) fn confirm_iv3to8(
    tag: &[u8],
    cipher: &[u8],
    plainlen: usize,
    iv3to8: [u8; 5],
) -> bool {
    if cipher.len() < 8 || cipher.len() % 8 != 0 {
        return false;
    }
    let Ok(first) = <[u8; 8]>::try_from(&cipher[0..8]) else {
        return false;
    };
    let t = tea_decrypt_block(first, &KEY_ROD);
    let tail = tea_cbc_decrypt(&cipher[8..], &KEY_ROD, first);
    let iv0 = rod_block0_iv(tag);
    let d0 = t[2] ^ iv0[2];
    // plaintext[3..8] implied by this iv[3..8]: p = t ^ iv.
    let guess: [u8; 5] = std::array::from_fn(|k| t[k + 3] ^ iv3to8[k]);
    if !header_ok(d0, &guess, &tail) {
        return false;
    }
    let mut zs = Vec::with_capacity(3 + 5 + tail.len());
    zs.extend_from_slice(&[0x78, 0xda, d0]);
    zs.extend_from_slice(&guess);
    zs.extend_from_slice(&tail);
    matches!(
        miniz_oxide::inflate::decompress_to_vec_zlib(&zs),
        Ok(o) if o.len() == plainlen
    )
}

/// Recover the raw first-block `iv[3..8]` for a `product != 0` zlib section.
/// `cipher` is the full section ciphertext; `plainlen` the declared
/// decompressed length. Returns `None` if the first block is not zlib-magic
/// (`78 da`) or the search finds no candidate inflating to `plainlen`.
pub(crate) fn recover_iv3to8(tag: &[u8], cipher: &[u8], plainlen: usize) -> Option<[u8; 5]> {
    if cipher.len() < 8 || cipher.len() % 8 != 0 {
        return None;
    }
    let first: [u8; 8] = cipher[0..8].try_into().ok()?;
    let t = tea_decrypt_block(first, &KEY_ROD);
    let tail = tea_cbc_decrypt(&cipher[8..], &KEY_ROD, first);

    let iv0 = rod_block0_iv(tag);
    // plaintext[0..3] = t[0..3] ^ iv[0..3]; iv[0..3] is exact (product-independent).
    let p0 = t[0] ^ iv0[0];
    let p1 = t[1] ^ iv0[1];
    let d0 = t[2] ^ iv0[2];
    if p0 != 0x78 || p1 != 0xda {
        return None; // not a zlib stream / wrong IV prefix
    }

    let sets = candidate_sets(tag[1], &t);

    let tail = Arc::new(tail);
    let sets = Arc::new(sets);
    let found = Arc::new(AtomicBool::new(false));
    let result: Arc<Mutex<Option<[u8; 5]>>> = Arc::new(Mutex::new(None));
    let nthreads = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8);
    let d1set = sets[0].clone();
    let chunk = d1set.len().div_ceil(nthreads).max(1);

    let mut handles = Vec::new();
    for t_idx in 0..nthreads {
        let lo = t_idx * chunk;
        if lo >= d1set.len() {
            break;
        }
        let hi = usize::min(lo + chunk, d1set.len());
        let d1slice: Vec<u8> = d1set[lo..hi].to_vec();
        let sets = Arc::clone(&sets);
        let tail = Arc::clone(&tail);
        let found = Arc::clone(&found);
        let result = Arc::clone(&result);
        handles.push(thread::spawn(move || {
            let (s1, s2, s3, s4) = (&sets[1], &sets[2], &sets[3], &sets[4]);
            'outer: for &d1 in &d1slice {
                for &d2 in s1 {
                    for &d3 in s2 {
                        for &d4 in s3 {
                            for &d5 in s4 {
                                if found.load(Ordering::Relaxed) {
                                    break 'outer;
                                }
                                let guess = [d1, d2, d3, d4, d5];
                                if !header_ok(d0, &guess, &tail) {
                                    continue;
                                }
                                let mut zs = Vec::with_capacity(3 + 5 + tail.len());
                                zs.push(0x78);
                                zs.push(0xda);
                                zs.push(d0);
                                zs.extend_from_slice(&guess);
                                zs.extend_from_slice(&tail);
                                if let Ok(out) =
                                    miniz_oxide::inflate::decompress_to_vec_zlib(&zs)
                                {
                                    if out.len() == plainlen {
                                        *result.lock().unwrap() = Some(guess);
                                        found.store(true, Ordering::Relaxed);
                                        break 'outer;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }

    // Convert recovered plaintext[3..8] into raw iv[3..8]: iv[i] = t[i] ^ p[i].
    let guess = (*result.lock().unwrap())?;
    Some(std::array::from_fn(|k| t[k + 3] ^ guess[k]))
}
