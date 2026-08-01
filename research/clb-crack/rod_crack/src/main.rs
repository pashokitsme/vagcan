//! rod_crack -- multithreaded brute-forcer for a .rod zlib section's corrupted
//! first-block plaintext bytes (deflate bytes 1..5 = plaintext[3..8]).
//!
//! See research/rod-labels.md and research/label-linkage.md.
//! Reads `crack_input.bin` produced by the Python prep step:
//!   u32 plainlen | u8 d0 | 5x (u16 nset, nset bytes) | u32 taillen, tail bytes
//! Deflate stream = [d0] ++ [d1..d5 unknown] ++ tail ; zlib stream = 78 da ++ deflate.
//!
//! Search shape: a depth-first walk over d1..d5 that prunes a subtree as soon as
//! the bytes pinned at that depth make a valid dynamic-Huffman header
//! impossible, rather than a flat five-deep loop that re-parses the header from
//! bit 0 for every one of the ~2^36 leaves. The header bit layout is what makes
//! this pay: d0 is exact and pins BFINAL/BTYPE/HLIT, d1 pins HDIST and most of
//! HCLEN, and by d3 there are five complete code-length-code entries to test —
//! so one rejection there replaces 65,536 leaf parses.
//!
//! Both pruning rules are *sound*, not heuristic, which matters because a wrong
//! prune would silently discard the answer and report NO HIT:
//!   - over-subscription: a Kraft sum above 1 admits no Huffman code, ever;
//!   - unreachability: if every remaining entry took the shortest legal length
//!     and the sum still could not reach 1, no completion exists below here.
//! Anything surviving to a leaf is confirmed by a real miniz_oxide inflate that
//! must yield exactly `plainlen` bytes.

use std::fs;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

const CLCL_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Kraft weights are integers scaled by 2^7: a code length `l` costs
/// `1 << (7 - l)`, and a complete code sums to exactly `KRAFT_FULL`.
const KRAFT_FULL: u32 = 128;

/// LSB-first bit reader over `[d0] ++ guess[0..5] ++ tail`, with a hard limit on
/// how many leading bytes are actually known.
///
/// The limit is what makes a partial parse possible: at depth `k` only bytes
/// `0..=k` are pinned, so a read running past them returns `None` and the caller
/// stops rather than consuming a byte it has not chosen yet.
///
/// Reads past the end of the *section* are a separate hazard and are handled
/// here too. A short section can hold fewer bytes than a deflate header wants,
/// and this oracle is speculative by nature — it is fed candidate bytes that are
/// almost always wrong. Indexing past the tail used to panic, which killed the
/// worker thread silently and made the search look like it had simply found
/// nothing. Overrunning means the candidate cannot be evaluated: a rejection,
/// not a crash.
struct Bits<'a> {
    d0: u8,
    guess: &'a [u8; 5],
    tail: &'a [u8],
    known_bytes: usize,
    byte: usize,
    nbits: u32,
    cur: u64,
}

impl<'a> Bits<'a> {
    fn new(d0: u8, guess: &'a [u8; 5], tail: &'a [u8], known_bytes: usize) -> Self {
        Bits { d0, guess, tail, known_bytes, byte: 0, nbits: 0, cur: 0 }
    }

    /// Whole stream pinned: `d0`, five guess bytes, and the exact tail.
    fn full(d0: u8, guess: &'a [u8; 5], tail: &'a [u8]) -> Self {
        Self::new(d0, guess, tail, 6 + tail.len())
    }

    #[inline]
    fn getb(&self, i: usize) -> Option<u8> {
        if i >= self.known_bytes {
            return None;
        }
        if i == 0 {
            Some(self.d0)
        } else if i <= 5 {
            Some(self.guess[i - 1])
        } else {
            self.tail.get(i - 6).copied()
        }
    }

    /// Read `n` bits, or `None` if they are not all known.
    #[inline]
    fn read(&mut self, n: u32) -> Option<u64> {
        while self.nbits < n {
            let b = self.getb(self.byte)?;
            self.cur |= (b as u64) << self.nbits;
            self.byte += 1;
            self.nbits += 8;
        }
        let v = self.cur & ((1u64 << n) - 1);
        self.cur >>= n;
        self.nbits -= n;
        Some(v)
    }
}

/// Parse the dynamic-Huffman header prefix as far as `known_bytes` allows and
/// report whether this subtree can still hold a valid header.
///
/// Only the code-length-code alphabet is examined. That is deliberate: its
/// entries sit at bits 17.. in three-bit fields, exactly the region the unknown
/// bytes control, and they admit the two sound tests above. The literal and
/// distance lengths that follow are left to the leaf.
///
/// Returns `false` only when the subtree is provably dead.
#[inline]
fn prefix_viable(d0: u8, guess: &[u8; 5], tail: &[u8], known_bytes: usize) -> bool {
    let mut bs = Bits::new(d0, guess, tail, known_bytes);

    // BFINAL(1) BTYPE(2) HLIT(5) all sit inside the exact d0.
    let Some(_bfinal) = bs.read(1) else { return true };
    match bs.read(2) {
        Some(2) => {}
        Some(_) => return false, // not a dynamic-Huffman block
        None => return true,
    }
    let Some(_hlit) = bs.read(5) else { return true };
    let Some(_hdist) = bs.read(5) else { return true };
    let Some(hclen_raw) = bs.read(4) else { return true };
    let hclen = hclen_raw as usize + 4;

    let mut weight: u32 = 0;
    let mut nread = 0usize;
    for _ in 0..hclen {
        let Some(l) = bs.read(3) else { break };
        nread += 1;
        if l > 0 {
            weight += 1 << (7 - l);
            // Sound rule 1: a Kraft sum above 1 admits no Huffman code.
            if weight > KRAFT_FULL {
                return false;
            }
        }
    }

    // Sound rule 2: each entry still to come adds at most 2^-1, so if all of
    // them at the shortest legal length cannot reach a complete code, nothing
    // below this node completes it either.
    let remaining = (hclen - nread) as u32;
    weight + remaining * (KRAFT_FULL / 2) >= KRAFT_FULL
}

struct Huff {
    counts: [u16; 16],
    /// The code-length alphabet has 19 symbols, so this never allocates.
    symbols: [u16; 19],
    maxlen: u32,
}

fn build_huff_cl(lens: &[u8; 19]) -> Option<Huff> {
    let mut counts = [0u16; 16];
    let mut maxlen = 0u32;
    for &l in lens.iter() {
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
    let mut symbols = [0u16; 19];
    for (sym, &l) in lens.iter().enumerate() {
        if l > 0 {
            symbols[offs[l as usize] as usize] = sym as u16;
            offs[l as usize] += 1;
        }
    }
    Some(Huff { counts, symbols, maxlen })
}

#[inline]
fn decode_sym(bs: &mut Bits, h: &Huff) -> Option<u16> {
    let mut code: i32 = 0;
    let mut first: i32 = 0;
    let mut index: i32 = 0;
    for len in 1..=h.maxlen as usize {
        code |= bs.read(1)? as i32;
        let count = h.counts[len] as i32;
        if code - first < count {
            return h.symbols.get((index + (code - first)) as usize).copied();
        }
        index += count;
        first += count;
        first <<= 1;
        code <<= 1;
    }
    None
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
    for bl in 1..=maxbl {
        left <<= 1;
        left -= cnt[bl];
        if left < 0 {
            return false;
        }
    }
    true
}

/// Full dynamic-Huffman header parse over a completely pinned stream.
fn header_ok(d0: u8, guess: &[u8; 5], tail: &[u8]) -> bool {
    let mut bs = Bits::full(d0, guess, tail);
    macro_rules! rd {
        ($n:expr) => {
            match bs.read($n) {
                Some(v) => v,
                None => return false,
            }
        };
    }
    let _bfinal = rd!(1);
    if rd!(2) != 2 {
        return false;
    }
    let hlit = rd!(5) as usize + 257;
    let hdist = rd!(5) as usize + 1;
    let hclen = rd!(4) as usize + 4;

    let mut cl = [0u8; 19];
    let mut weight: u32 = 0;
    for i in 0..hclen {
        let l = rd!(3) as u8;
        cl[CLCL_ORDER[i]] = l;
        if l > 0 {
            weight += 1 << (7 - l);
            if weight > KRAFT_FULL {
                return false;
            }
        }
    }
    // RFC 1951 requires the code-length code itself to be complete.
    if weight != KRAFT_FULL {
        return false;
    }

    let clh = match build_huff_cl(&cl) {
        Some(h) => h,
        None => return false,
    };
    let n = hlit + hdist;
    // HLIT <= 288 and HDIST <= 32, so 320 always suffices.
    let mut lengths = [0u8; 320];
    let mut nl = 0usize;
    while nl < n {
        let sym = match decode_sym(&mut bs, &clh) {
            Some(s) => s,
            None => return false,
        };
        let (val, rep) = match sym {
            0..=15 => (sym as u8, 1u64),
            16 => {
                if nl == 0 {
                    return false;
                }
                (lengths[nl - 1], rd!(2) + 3)
            }
            17 => (0, rd!(3) + 3),
            18 => (0, rd!(7) + 11),
            _ => return false,
        };
        if nl + rep as usize > n {
            return false;
        }
        for _ in 0..rep {
            lengths[nl] = val;
            nl += 1;
        }
    }
    kraft_ok(&lengths[..hlit]) && kraft_ok(&lengths[hlit..n])
}

fn main() {
    let data = fs::read("crack_input.bin").expect("crack_input.bin");
    let mut p = 0usize;
    let rd_u32 = |d: &[u8], p: &mut usize| {
        let v = u32::from_le_bytes([d[*p], d[*p + 1], d[*p + 2], d[*p + 3]]);
        *p += 4;
        v
    };
    let plainlen = rd_u32(&data, &mut p) as usize;
    let d0 = data[p];
    p += 1;
    let mut sets: Vec<Vec<u8>> = Vec::new();
    for _ in 0..5 {
        let n = u16::from_le_bytes([data[p], data[p + 1]]) as usize;
        p += 2;
        sets.push(data[p..p + n].to_vec());
        p += n;
    }
    let taillen = rd_u32(&data, &mut p) as usize;
    let tail = data[p..p + taillen].to_vec();
    eprintln!(
        "plainlen={} d0={:#x} setsizes={:?} taillen={}",
        plainlen,
        d0,
        sets.iter().map(|s| s.len()).collect::<Vec<_>>(),
        taillen
    );

    let tail = Arc::new(tail);
    let sets = Arc::new(sets);
    let found = Arc::new(AtomicBool::new(false));
    let result: Arc<Mutex<Option<[u8; 5]>>> = Arc::new(Mutex::new(None));
    let nthreads = thread::available_parallelism().map(|n| n.get()).unwrap_or(8);

    // Work-stealing over (d1, d2) pairs. The previous fixed chunking of d1
    // across threads left the last thread with a fraction of a chunk while the
    // rest idled; a shared counter keeps every core busy to the final node.
    let n1 = sets[0].len();
    let n2 = sets[1].len();
    let njobs = n1 * n2;
    let next = Arc::new(AtomicUsize::new(0));
    let leaves = Arc::new(AtomicU64::new(0));
    let pruned3 = Arc::new(AtomicU64::new(0));
    let pruned4 = Arc::new(AtomicU64::new(0));
    let inflates = Arc::new(AtomicU64::new(0));
    eprintln!("threads={nthreads} jobs={njobs} (d1 x d2)");

    let mut handles = Vec::new();
    for _ in 0..nthreads {
        let sets = Arc::clone(&sets);
        let tail = Arc::clone(&tail);
        let found = Arc::clone(&found);
        let result = Arc::clone(&result);
        let next = Arc::clone(&next);
        let leaves = Arc::clone(&leaves);
        let pruned3 = Arc::clone(&pruned3);
        let pruned4 = Arc::clone(&pruned4);
        let inflates = Arc::clone(&inflates);
        handles.push(thread::spawn(move || {
            let (s0, s1, s2, s3, s4) = (&sets[0], &sets[1], &sets[2], &sets[3], &sets[4]);
            let (mut nleaf, mut np3, mut np4, mut ninf) = (0u64, 0u64, 0u64, 0u64);
            loop {
                let job = next.fetch_add(1, Ordering::Relaxed);
                if job >= njobs || found.load(Ordering::Relaxed) {
                    break;
                }
                let mut guess = [s0[job / n2], s1[job % n2], 0, 0, 0];
                // Depth 2: HCLEN is complete and two code-length entries are in.
                if !prefix_viable(d0, &guess, &tail, 3) {
                    continue;
                }
                for &d3 in s2 {
                    guess[2] = d3;
                    // Depth 3: five complete entries — the first real filter.
                    if !prefix_viable(d0, &guess, &tail, 4) {
                        np3 += 1;
                        continue;
                    }
                    for &d4 in s3 {
                        guess[3] = d4;
                        // Depth 4: seven entries.
                        if !prefix_viable(d0, &guess, &tail, 5) {
                            np4 += 1;
                            continue;
                        }
                        for &d5 in s4 {
                            if found.load(Ordering::Relaxed) {
                                break;
                            }
                            guess[4] = d5;
                            nleaf += 1;
                            if !header_ok(d0, &guess, &tail) {
                                continue;
                            }
                            ninf += 1;
                            let mut zs = Vec::with_capacity(8 + tail.len());
                            zs.push(0x78);
                            zs.push(0xda);
                            zs.push(d0);
                            zs.extend_from_slice(&guess);
                            zs.extend_from_slice(&tail);
                            // Bound the confirmation: the true answer inflates
                            // to exactly `plainlen`, so anything longer is
                            // wrong by definition and a wrong header that
                            // happens to keep decoding must not be allowed to
                            // run away. This matters most on the big sections —
                            // TTTEXT's tail is 4.8 MB against STRUC's 78 KB,
                            // and the header oracle still hands ~18M candidates
                            // to the inflate.
                            let lim = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(
                                &zs, plainlen,
                            );
                            if let Ok(out) = lim {
                                if out.len() == plainlen {
                                    *result.lock().unwrap() = Some(guess);
                                    found.store(true, Ordering::Relaxed);
                                    eprintln!(
                                        "HIT d[1..5]={:02x?} inflated {} bytes",
                                        guess,
                                        out.len()
                                    );
                                }
                            }
                        }
                    }
                }
            }
            leaves.fetch_add(nleaf, Ordering::Relaxed);
            pruned3.fetch_add(np3, Ordering::Relaxed);
            pruned4.fetch_add(np4, Ordering::Relaxed);
            inflates.fetch_add(ninf, Ordering::Relaxed);
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    let full: u64 = sets.iter().map(|s| s.len() as u64).product();
    let nleaves = leaves.load(Ordering::Relaxed);
    eprintln!(
        "leaves={} of {} ({:.3}% reached a leaf), pruned d3={} d4={}, inflates={}",
        nleaves,
        full,
        100.0 * nleaves as f64 / full as f64,
        pruned3.load(Ordering::Relaxed),
        pruned4.load(Ordering::Relaxed),
        inflates.load(Ordering::Relaxed)
    );

    let res = *result.lock().unwrap();
    if let Some(g) = res {
        println!("{:02x}{:02x}{:02x}{:02x}{:02x}", g[0], g[1], g[2], g[3], g[4]);
    } else {
        eprintln!("NO HIT");
        std::process::exit(2);
    }
}
