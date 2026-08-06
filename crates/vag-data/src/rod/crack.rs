//! Offline recovery of a `.rod` zlib section's corrupted first-block plaintext
//! bytes (deflate bytes `1..=5` = plaintext `3..8`), for sections whose
//! per-record `product` term is nonzero and thus not stored in the file.
//!
//! Ported from `research/clb-crack/rod_crack/src/main.rs` (multithreaded Rust
//! brute-forcer) and the framing/prep logic in
//! `research/clb-crack/rod_struc_decode.py`. CPU-heavy, so nothing on the live
//! path runs it — only `vagcan vcds rod` does.
//!
//! Approach (see `research/labels/rod-labels.md` §1): a `.rod` section is
//! TEA-CBC(`KEY_ROD`) with an 8-byte IV. `IV[0..3]` is tag-derived (exact);
//! `IV[3..8]` carries the low 5 bytes of the runtime `product`. Only the first
//! cipher block's plaintext depends on the IV (CBC), so the zlib magic
//! `78 da` (plaintext `0..2`) survives but deflate bytes `1..=5` are corrupted.
//! Deflate byte 0 (plaintext[2]) is exact. We recover the 5 unknown bytes from
//! reduced per-byte candidate sets with a **pruned DFS** over a dynamic-Huffman
//! header parse, and confirm survivors with a full `miniz_oxide` inflate that
//! yields exactly `plainlen` bytes.
//!
//! ## Search shape, and where the speed actually came from
//!
//! The search is a DFS, mirroring the Python reference
//! (`rod_struc_decode.py`): parse the header over a *partially* assigned
//! guess, and when the parse reaches a byte that is not assigned yet, stop and
//! branch on exactly that byte ([`Probe::Need`]). Bytes the header never reads
//! are left to the inflate oracle.
//!
//! **The tree-shape pruning is worth far less than it looks, and it is worth
//! recording why.** The intuition — "reject at depth 3 and you kill a
//! 65,536-leaf subtree" — is right about the mechanism and wrong about the
//! magnitude, because of where the bits fall. Deflate byte 0 is exact and
//! covers BFINAL/BTYPE/HLIT; HDIST and HCLEN then run to bit 17, so the 3-bit
//! code-length-code entries only start inside `d2`. With `d1..d3` pinned there
//! are five entries to test, with `d1..d4` seven — and five or seven entries
//! drawn from a wrong candidate over-subscribe the Kraft inequality only
//! sometimes. Measured on `STRUC.rod`: the DFS visits 1.25e9 leaves where the
//! flat loop visited 2.06e9. That is 1.65×, not the hoped-for 50×.
//!
//! What actually paid, in order:
//!
//! 1. **The leaf test got cheap and early.** Kraft as an O(1) running total
//!    instead of an O(maxlen) rescan per length; the code-length code required
//!    to be *complete*, which rejects ~99% of leaves before a table is built;
//!    and the literal/distance Kraft totals checked as lengths arrive instead
//!    of after all 300-odd are decoded.
//! 2. **Siblings share the prefix.** A stall inside the code-length loop hands
//!    its parse state to every child ([`Resume`]), so the 17-bit preamble and
//!    the entries already read are not re-read once per leaf.
//! 3. **The hot path carries nothing.** [`cl_scan`] keeps a bit position and
//!    one running total; the table, the arrays and the length decode live in
//!    [`parse_full`], reached about once per hundred leaves.
//!
//! Net on `STRUC.rod`: ~306 CPU-seconds to ~35, the answer unchanged.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::rod::{KEY_ROD, MT, OFF_ROD, rod_block0_iv};
use crate::tea::{tea_cbc_decrypt, tea_decrypt_block};

const CLCL_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// All five guess bytes assigned — the mask a fully-specified candidate uses.
/// The search itself never needs it (it grows the mask one branch at a time);
/// it is what a one-shot full-header check passes.
#[cfg(test)]
const ALL_KNOWN: u8 = 0b1_1111;

/// Why a speculative header parse stopped early.
#[derive(Clone, Copy)]
enum Stop {
	/// The parse wants guess byte `idx` (1..=5) and it is not assigned yet.
	Need(usize),
	/// A read ran past the end of the section.
	///
	/// A short section can hold fewer bytes than a deflate header wants to
	/// read, and the header oracle is speculative by nature — it is fed
	/// candidate bytes that are usually wrong. Indexing past the tail used to
	/// panic, which killed the worker thread silently and made the whole
	/// search look like it had simply found nothing. Overrunning means this
	/// candidate cannot be evaluated, which is a rejection, not a crash.
	Overrun,
}

/// The verdict on a header parse over a partially assigned guess.
enum Probe {
	/// Parsed cleanly using only the bytes assigned so far.
	Valid,
	/// Impossible — prune this whole subtree.
	Reject,
	/// Stalled on unassigned guess byte `idx` (1..=5); branch there. `at` is
	/// the parse state to hand each child, when the stall happened somewhere
	/// that can be resumed.
	Need { idx: usize, at: Option<Resume> },
}

/// Parse state captured at a stall inside the code-length-code loop.
///
/// Without this every child re-reads the header from bit 0: the 17-bit
/// preamble plus every code-length entry its parent already read. Those bits
/// are identical across all siblings — the only thing that differs is the byte
/// being branched on, which comes *after* them. Since the last unknown byte is
/// deflate byte 5 and the code-length entries run well past it, that repeated
/// prefix is most of the work at the leaves, where nearly all the time goes.
///
/// The bit reader stops on a whole-byte boundary with a self-consistent
/// buffer, so resuming is exactly equivalent to re-parsing (asserted by
/// `resuming_matches_a_fresh_parse`).
#[derive(Clone, Copy)]
struct Resume {
	byte: usize,
	nbits: u32,
	cur: u64,
	hclen: usize,
	/// Index of the next code-length-code entry to read.
	i: usize,
	/// Kraft running total over the entries read so far.
	wcl: u32,
}

/// LSB-first bit reader over `[d0] ++ guess[0..5] ++ tail`, where only the
/// guess bytes flagged in `known` may be read. Reading an unknown byte or
/// running off the end stops the parse and records why in `stop`.
struct Bits<'a> {
	d0: u8,
	guess: &'a [u8; 5],
	known: u8,
	tail: &'a [u8],
	byte: usize,
	nbits: u32,
	cur: u64,
	stop: Option<Stop>,
}
impl<'a> Bits<'a> {
	fn new(d0: u8, guess: &'a [u8; 5], known: u8, tail: &'a [u8]) -> Self {
		Bits {
			d0,
			guess,
			known,
			tail,
			byte: 0,
			nbits: 0,
			cur: 0,
			stop: None,
		}
	}
	#[inline]
	fn getb(&mut self, i: usize) -> Option<u8> {
		if i == 0 {
			Some(self.d0)
		} else if i <= 5 {
			if self.known & (1 << (i - 1)) != 0 {
				Some(self.guess[i - 1])
			} else {
				self.stop = Some(Stop::Need(i));
				None
			}
		} else {
			match self.tail.get(i - 6) {
				Some(b) => Some(*b),
				None => {
					self.stop = Some(Stop::Overrun);
					None
				}
			}
		}
	}
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
	/// Turn a stopped read into a verdict. An overrun — or a plain decode
	/// failure with no recorded stop — is a rejection; only a missing guess
	/// byte is worth branching on.
	#[inline]
	fn verdict(&self) -> Probe {
		match self.stop {
			Some(Stop::Need(i)) => Probe::Need { idx: i, at: None },
			Some(Stop::Overrun) | None => Probe::Reject,
		}
	}
}

/// Read `n` bits or hand the caller's verdict back up.
macro_rules! rd {
	($bs:expr, $n:expr) => {
		match $bs.read($n) {
			Some(v) => v,
			None => return $bs.verdict(),
		}
	};
}

// Kraft, done in O(1) per code length instead of O(maxlen) per length.
//
// The textbook check walks `left = 2*left - cnt[bl]` over bl = 1..maxbl and
// fails if `left` ever goes negative. Dividing that by 2^bl, the condition at
// bl is `Σ_{j<=bl} cnt[j]·2^-j <= 1` — a *prefix* of the Kraft sum. Every term
// is non-negative, so the prefix conditions are all implied by the total one
// and vice versa: the walk fails exactly when `Σ 2^-l > 1`. Scaling by the
// deepest possible code makes that an integer running total, which can be
// maintained as lengths arrive and compared in one instruction.
//
// This matters because it sits on the hottest path in the search: the CL loop
// runs it per entry, and the lit/dist decode runs it per emitted length.

/// Scale for the code-length-code Kraft total: those lengths are 3 bits, so 7
/// is the deepest code and `2^7` represents a full (complete) code.
const CL_KRAFT_FULL: u32 = 1 << 7;
/// Scale for literal/distance Kraft totals: those lengths cap at 15.
const LEN_KRAFT_FULL: u32 = 1 << 15;

/// Canonical Huffman decode table for the 19 code-length codes. The symbol
/// array is fixed-size on purpose: this is built once per surviving node, and
/// a heap allocation there showed up as pure overhead.
struct Huff {
	counts: [u16; 16],
	symbols: [u16; 19],
	maxlen: u32,
}
fn build_huff(lens: &[u8; 19]) -> Option<Huff> {
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
	let mut symbols = [0u16; 19];
	for (sym, &l) in lens.iter().enumerate() {
		if l > 0 {
			symbols[offs[l as usize] as usize] = sym as u16;
			offs[l as usize] += 1;
		}
	}
	Some(Huff { counts, symbols, maxlen })
}

/// `None` means either "no such code" or "the reader stalled"; the caller
/// distinguishes them via [`Bits::verdict`].
#[inline]
fn decode_sym(bs: &mut Bits, h: &Huff) -> Option<u16> {
	let mut code: i32 = 0;
	let mut first: i32 = 0;
	let mut index: i32 = 0;
	for len in 1..=h.maxlen as usize {
		code |= bs.read(1)? as i32;
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

/// Dynamic-Huffman header **filter** over a partially assigned guess.
///
/// This is the hot path: it is entered once per node, and the overwhelming
/// majority of nodes are leaves that it rejects. So it deliberately does the
/// least work that can decide the question — read the preamble, then walk the
/// code-length-code entries keeping nothing but a Kraft running total. No
/// table, no arrays, nothing that has to be copied into a resumed frame.
///
/// A candidate that survives is handed to [`parse_full`], which is the
/// authority: it re-parses from bit 0 and decides properly. That split is what
/// keeps the hot loop in registers, and it is sound in one direction only —
/// every test here must be one `parse_full` also applies, or the search would
/// discard an answer it never re-checked.
fn probe_header(d0: u8, guess: &[u8; 5], known: u8, tail: &[u8]) -> Probe {
	let mut bs = Bits::new(d0, guess, known, tail);
	let _bfinal = rd!(bs, 1);
	if rd!(bs, 2) != 2 {
		return Probe::Reject;
	}
	let _hlit = rd!(bs, 5);
	let _hdist = rd!(bs, 5);
	let hclen = rd!(bs, 4) as usize + 4;
	cl_scan(bs, hclen, 0, 0)
}

/// Continue the filter from where a sibling stalled, with one more byte pinned.
fn probe_resume(at: &Resume, d0: u8, guess: &[u8; 5], known: u8, tail: &[u8]) -> Probe {
	let mut bs = Bits::new(d0, guess, known, tail);
	bs.byte = at.byte;
	bs.nbits = at.nbits;
	bs.cur = at.cur;
	cl_scan(bs, at.hclen, at.i, at.wcl)
}

/// Walk code-length-code entries `i..hclen`, carrying the Kraft total `wcl`.
#[inline]
fn cl_scan(mut bs: Bits, hclen: usize, i0: usize, mut wcl: u32) -> Probe {
	for i in i0..hclen {
		let l = match bs.read(3) {
			Some(v) => v as u32,
			None => {
				// Stalled mid-table. Before branching, check the subtree can
				// still *reach* a complete code: every entry left contributes
				// at most 2^-1, so if all of them at the shortest legal length
				// still fall short, nothing below here completes it. Sound, not
				// heuristic — a wrong prune would silently drop the answer.
				let remaining = (hclen - i) as u32;
				if wcl + remaining * (CL_KRAFT_FULL / 2) < CL_KRAFT_FULL {
					return Probe::Reject;
				}
				return match bs.stop {
					Some(Stop::Need(idx)) => Probe::Need {
						idx,
						at: Some(Resume {
							byte: bs.byte,
							nbits: bs.nbits,
							cur: bs.cur,
							hclen,
							i,
							wcl,
						}),
					},
					Some(Stop::Overrun) | None => Probe::Reject,
				};
			}
		};
		if l > 0 {
			wcl += 1 << (7 - l);
			if wcl > CL_KRAFT_FULL {
				return Probe::Reject;
			}
		}
	}
	// RFC 1951 requires the code-length code itself to be *complete*, and
	// zlib's `inflate_table` rejects an incomplete one outright — so a
	// candidate that lands short can never inflate, whatever follows. This is
	// the single most selective cheap test there is: about 1% of leaves get
	// past it, and everything below this line runs only for those.
	//
	// Note it also subsumes the over-subscription test above: the entries are
	// non-negative and must total exactly `CL_KRAFT_FULL`, so no prefix of
	// them can exceed it. The early check is kept because it fires sooner.
	if wcl != CL_KRAFT_FULL {
		return Probe::Reject;
	}
	parse_full(bs.d0, bs.guess, bs.known, bs.tail)
}

/// The complete header parse, and the authority on what is a valid header:
/// code-length table built, all `HLIT + HDIST` code lengths decoded against
/// the exact tail bytes, Kraft honoured on both alphabets.
///
/// Reached for roughly one leaf in a hundred, so it re-reads the header from
/// bit 0 rather than complicating the state the hot path carries. Kept out of
/// line for the same reason — inlined, its length spills the caller's loop.
#[inline(never)]
fn parse_full(d0: u8, guess: &[u8; 5], known: u8, tail: &[u8]) -> Probe {
	let mut bs = Bits::new(d0, guess, known, tail);
	let _bfinal = rd!(bs, 1);
	if rd!(bs, 2) != 2 {
		return Probe::Reject;
	}
	let hlit = rd!(bs, 5) as usize + 257;
	let hdist = rd!(bs, 5) as usize + 1;
	let hclen = rd!(bs, 4) as usize + 4;
	let mut cl = [0u8; 19];
	let mut wcl: u32 = 0;
	for i in 0..hclen {
		let l = rd!(bs, 3) as u8;
		cl[CLCL_ORDER[i]] = l;
		if l > 0 {
			wcl += 1 << (7 - l);
			if wcl > CL_KRAFT_FULL {
				return Probe::Reject;
			}
		}
	}
	if wcl != CL_KRAFT_FULL {
		return Probe::Reject;
	}
	let clh = match build_huff(&cl) {
		Some(h) => h,
		None => return Probe::Reject,
	};
	// Decode the HLIT+HDIST code lengths against the (exact) tail bytes,
	// running the literal and distance Kraft totals as they arrive.
	//
	// Checking those totals only at the end — as the reference does — is what
	// made a wrong candidate expensive: it decoded all 300-odd lengths before
	// noticing. A wrong candidate's lengths over-subscribe after roughly a
	// dozen symbols, and since the totals only ever grow, bailing there is the
	// same predicate reached far sooner.
	let n = hlit + hdist;
	let mut nlen = 0usize;
	let mut last = 0u8; // previous code length, for the repeat symbol (16)
	let mut wlit: u32 = 0;
	let mut wdist: u32 = 0;
	while nlen < n {
		let sym = match decode_sym(&mut bs, &clh) {
			Some(s) => s,
			None => return bs.verdict(),
		};
		let start = nlen;
		let l = match sym {
			0..=15 => {
				nlen += 1;
				sym as u8
			}
			16 => {
				if nlen == 0 {
					return Probe::Reject;
				}
				nlen += (rd!(bs, 2) + 3) as usize;
				last
			}
			17 => {
				nlen += (rd!(bs, 3) + 3) as usize;
				0
			}
			18 => {
				nlen += (rd!(bs, 7) + 11) as usize;
				0
			}
			_ => return Probe::Reject,
		};
		if nlen > n {
			return Probe::Reject;
		}
		last = l;
		if l > 0 {
			// A run can straddle the literal/distance boundary; the two code
			// spaces are independent, so split it there.
			let w = 1u32 << (15 - l);
			if start < hlit {
				wlit += w * (nlen.min(hlit) - start) as u32;
				if wlit > LEN_KRAFT_FULL {
					return Probe::Reject;
				}
			}
			if nlen > hlit {
				wdist += w * (nlen - start.max(hlit)) as u32;
				if wdist > LEN_KRAFT_FULL {
					return Probe::Reject;
				}
			}
		}
	}
	Probe::Valid
}

/// Full-header oracle for a completely specified guess.
#[cfg(test)]
fn header_ok(d0: u8, guess: &[u8; 5], tail: &[u8]) -> bool {
	matches!(probe_header(d0, guess, ALL_KNOWN, tail), Probe::Valid)
}

/// The reduced per-byte candidate sets for deflate bytes `d[1..=5]`
/// (= plaintext `3..8`). `t` is the raw ECB decryption of the first cipher
/// block. Because `IV[i] = (s * MT[OFF[i]]) & 0xff` and some multipliers are
/// even, several of these sets are much smaller than 256.
fn candidate_sets(tag_m: u8, t: &[u8; 8]) -> [Vec<u8>; 5] {
	let _ = tag_m; // m only affects IV[0..3]; IV[3..8] range is over all s
	std::array::from_fn(|k| {
		let i = k + 3;
		let mut ivvals: Vec<u8> = (0..=255u16).map(|s| ((s as usize * MT[OFF_ROD[i]] as usize) & 0xff) as u8).collect();
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
/// the multi-minute brute force (which the `vagcan vcds rod` acceptance run exercises).
#[cfg(test)]
pub(crate) fn confirm_iv3to8(tag: &[u8], cipher: &[u8], plainlen: usize, iv3to8: [u8; 5]) -> bool {
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

/// Scratch buffers a worker reuses across confirmations.
///
/// The confirming inflate used to allocate twice per candidate: it rebuilt the
/// whole zlib stream when only five bytes differ, and grew a fresh output
/// vector to the section's full plaintext length. On a 7.4 MB section that
/// allocation churn dominated everything else — a `TTDOP` sweep spent 152 s of
/// user time against 537 s of system time. Both buffers are now allocated once
/// per worker and rewritten in place.
struct Scratch {
	/// `78 da` + `d0` + the five candidate bytes + the section tail. Only
	/// bytes 3..8 ever change.
	stream: Vec<u8>,
	/// Inflate output, sized to the declared plaintext length up front.
	out: Vec<u8>,
	inflater: Box<miniz_oxide::inflate::core::DecompressorOxide>,
}

impl Scratch {
	fn new(d0: u8, tail: &[u8], plainlen: usize) -> Self {
		let mut stream = Vec::with_capacity(3 + 5 + tail.len());
		stream.extend_from_slice(&[0x78, 0xda, d0]);
		stream.extend_from_slice(&[0u8; 5]);
		stream.extend_from_slice(tail);
		Scratch {
			stream,
			out: vec![0u8; plainlen],
			inflater: Box::new(miniz_oxide::inflate::core::DecompressorOxide::new()),
		}
	}

	/// True when `guess` makes the stream inflate to exactly `plainlen` bytes.
	///
	/// Same verdict as building a fresh stream and calling
	/// `decompress_to_vec_zlib`, which a test asserts directly — this is the
	/// definitive oracle, so the optimisation must not change what it accepts.
	fn inflates_to_plainlen(&mut self, guess: &[u8; 5]) -> bool {
		use miniz_oxide::inflate::TINFLStatus;
		use miniz_oxide::inflate::core::inflate_flags;

		self.stream[3..8].copy_from_slice(guess);
		self.inflater.init();
		let flags = inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER | inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
		let (status, _read, written) = miniz_oxide::inflate::core::decompress(&mut self.inflater, &self.stream, &mut self.out, 0, flags);
		status == TINFLStatus::Done && written == self.out.len()
	}
}

/// The read-only search context shared by every worker.
///
/// Workers no longer own a contiguous slice of the first branch point. They
/// pull one deflate-byte-1 value at a time off a shared cursor (see
/// [`search_anchor`]), so the tree is split at run time by who is free rather
/// than up front by index — the fix for the idle cores a fixed partition left
/// when one worker drew a slice of cheap (fast-rejecting) subtrees and another
/// a slice of deep ones.
struct Search<'a> {
	d0: u8,
	tail: &'a [u8],
	sets: &'a [Vec<u8>; 5],
	found: &'a AtomicBool,
}

impl Search<'_> {
	#[inline]
	fn set_for(&self, idx: usize) -> &[u8] {
		&self.sets[idx - 1]
	}

	/// Probe with what is assigned so far; branch only where the header
	/// actually needs a byte, and only on candidates that survive. `at` is the
	/// parent's stalled parse state, when it had one to hand down.
	fn dfs(&self, guess: &mut [u8; 5], known: u8, at: Option<&Resume>, scratch: &mut Scratch) -> Option<[u8; 5]> {
		if self.found.load(Ordering::Relaxed) {
			return None;
		}
		let probe = match at {
			Some(r) => probe_resume(r, self.d0, guess, known, self.tail),
			None => probe_header(self.d0, guess, known, self.tail),
		};
		match probe {
			Probe::Reject => None,
			Probe::Need { idx, at: next } => {
				for &c in self.set_for(idx) {
					guess[idx - 1] = c;
					if let Some(hit) = self.dfs(guess, known | (1 << (idx - 1)), next.as_ref(), scratch) {
						return Some(hit);
					}
				}
				None
			}
			Probe::Valid => self.confirm(guess, known, scratch),
		}
	}

	/// A header can parse without ever reading some of the five bytes (a small
	/// HCLEN plus a short code-length list). Those are unconstrained by the
	/// oracle, so they get enumerated in full and settled by the inflate.
	fn confirm(&self, guess: &mut [u8; 5], known: u8, scratch: &mut Scratch) -> Option<[u8; 5]> {
		if let Some(k) = (0..5).find(|k| known & (1 << k) == 0) {
			for &c in self.set_for(k + 1) {
				guess[k] = c;
				if let Some(hit) = self.confirm(guess, known | (1 << k), scratch) {
					return Some(hit);
				}
			}
			return None;
		}
		// The inflate is the definitive oracle: the header parse has false
		// positives, a stream that inflates to exactly `plainlen` does not.
		if scratch.inflates_to_plainlen(guess) {
			self.found.store(true, Ordering::Relaxed);
			return Some(*guess);
		}
		None
	}
}

/// Recover the raw first-block `iv[3..8]` for a `product != 0` zlib section.
/// `cipher` is the full section ciphertext; `plainlen` the declared
/// decompressed length. Returns `None` if the search finds no candidate
/// inflating to exactly `plainlen`.
///
/// Two regimes, and the difference is expensive (`research/labels/tttext2.md`):
///
/// * **classic** — the tag-derived IV is exact, so `plaintext[0..3]` reads
///   `78 da <anchor>` and the anchor is free. One search over the reduced
///   candidate sets, ~2 minutes.
/// * **shifted** — the file XORs a runtime 8-byte mask over the finished IV
///   (§3.3a), so neither the anchor nor the multiplicative structure of
///   `iv[3..8]` survives. The magic itself still does, because it is
///   *plaintext* and the searcher substitutes it directly rather than deriving
///   it — what is lost is only deflate byte 0, which is swept over the values a
///   dynamic-Huffman header admits, against the full candidate sets. Up to 60
///   searches at ~6.5× each: hours, not minutes.
///
/// `known_anchor` short-circuits the sweep. The mask is a property of the file,
/// so once any one of its sections has been opened the anchor for the rest is
/// arithmetic — and skipping 59 of 60 full-space searches is the difference
/// between a multi-section shifted file being openable in principle and in
/// practice.
pub(crate) fn recover_iv3to8(tag: &[u8], cipher: &[u8], plainlen: usize, known_anchor: Option<u8>) -> Option<[u8; 5]> {
	if cipher.len() < 8 || cipher.len() % 8 != 0 {
		return None;
	}
	let first: [u8; 8] = cipher[0..8].try_into().ok()?;
	let t = tea_decrypt_block(first, &KEY_ROD);
	let tail = tea_cbc_decrypt(&cipher[8..], &KEY_ROD, first);

	let iv0 = rod_block0_iv(tag);
	// plaintext[0..3] = t[0..3] ^ iv[0..3]; for a classic file iv[0..3] is
	// exact, so the anchor comes for free and the reduced sets are valid.
	let classic = t[0] ^ iv0[0] == 0x78 && t[1] ^ iv0[1] == 0xda;
	let tail = Arc::new(tail);
	if classic {
		return search_anchor(tag, &t, &tail, plainlen, t[2] ^ iv0[2], false);
	}
	match known_anchor {
		Some(d0) => search_anchor(tag, &t, &tail, plainlen, d0, true),
		None => super::deflate_anchors().find_map(|d0| search_anchor(tag, &t, &tail, plainlen, d0, true)),
	}
}

/// One full search for a single assumed deflate byte 0.
///
/// `full_sets` widens `iv[3..8]` from the multiplicatively-reachable values to
/// all 256 per byte. That reduction is a property of the *documented* IV
/// construction, and a shifted file XORs a mask over its output — so on those
/// files the true bytes sit outside the reduced sets and a reduced search
/// returns a clean miss however long it runs.
fn search_anchor(tag: &[u8], t: &[u8; 8], tail: &Arc<Vec<u8>>, plainlen: usize, d0: u8, full_sets: bool) -> Option<[u8; 5]> {
	let sets: [Vec<u8>; 5] = match full_sets {
		true => std::array::from_fn(|k| (0..=255u8).map(|v| t[k + 3] ^ v).collect()),
		false => candidate_sets(tag[1], t),
	};

	let tail = Arc::clone(tail);
	let sets = Arc::new(sets);
	let found = Arc::new(AtomicBool::new(false));
	let result: Arc<Mutex<Option<[u8; 5]>>> = Arc::new(Mutex::new(None));
	let nthreads = thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
	// The work unit is one first-branch byte (deflate byte 1): every worker
	// pulls the next index off this shared cursor and walks that whole subtree,
	// then comes back for another. A subtree can reject at the header or open
	// deep, so their costs differ by orders of magnitude; handing them out on
	// demand keeps every core busy until the space is exhausted, where a fixed
	// contiguous partition left the workers that drew cheap slices idle while
	// one ground through a deep one.
	let d1len = sets[0].len();
	let cursor = Arc::new(AtomicUsize::new(0));

	let mut handles = Vec::new();
	for _ in 0..nthreads {
		let sets = Arc::clone(&sets);
		let tail = Arc::clone(&tail);
		let found = Arc::clone(&found);
		let result = Arc::clone(&result);
		let cursor = Arc::clone(&cursor);
		handles.push(thread::spawn(move || {
			let search = Search {
				d0,
				tail: &tail,
				sets: &sets,
				found: &found,
			};
			// One set of buffers per worker, reused for every confirmation.
			let mut scratch = Scratch::new(d0, &tail, plainlen);
			loop {
				if found.load(Ordering::Relaxed) {
					return;
				}
				let k = cursor.fetch_add(1, Ordering::Relaxed);
				if k >= d1len {
					return;
				}
				// Pin deflate byte 1 to this work unit and search the rest. The
				// header always reads byte 1, so this is exactly the subtree the
				// old top-level branch over `sets[0]` explored for that value.
				let mut guess = [0u8; 5];
				guess[0] = search.sets[0][k];
				if let Some(hit) = search.dfs(&mut guess, 0b0_0001, None, &mut scratch) {
					*result.lock().unwrap() = Some(hit);
					return;
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

#[cfg(test)]
mod overrun_tests {
	use super::*;

	#[test]
	fn a_section_too_short_for_a_deflate_header_is_rejected_not_a_panic() {
		// A candidate that needs more bytes than the section holds cannot be
		// evaluated. This used to index past the tail and panic, and because
		// the panic happened on a worker thread the search reported "no hit"
		// — a real answer elsewhere in the space would have been lost with it.
		// 0x8C is the byte that says "dynamic Huffman", so the parser commits
		// to reading a header that is not there.
		let tail: [u8; 2] = [0x00, 0x00];
		assert!(!header_ok(0x8C, &[0x9d, 0x69, 0x92, 0x24, 0x29], &tail));
	}

	#[test]
	fn an_empty_tail_is_rejected_too() {
		assert!(!header_ok(0x8C, &[0; 5], &[]));
	}

	/// An overrun must read as "reject", never as "branch on a byte" — a DFS
	/// that branched there would loop forever on a truncated section.
	#[test]
	fn a_short_section_stalls_as_reject_even_with_bytes_unassigned() {
		assert!(matches!(probe_header(0x8C, &[0; 5], ALL_KNOWN, &[]), Probe::Reject));
	}
}

#[cfg(test)]
mod dfs_tests {
	use super::*;

	/// A real deflate header, with the five guess bytes progressively hidden.
	/// The probe must ask for exactly the first byte it cannot see — that is
	/// the whole basis of the pruning — and accept the full assignment.
	#[test]
	fn probe_asks_for_the_first_byte_it_cannot_see() {
		let plain: Vec<u8> = (0..2048u32).map(|i| (i % 251) as u8).collect();
		let z = miniz_oxide::deflate::compress_to_vec_zlib(&plain, 9);
		let d0 = z[2];
		let guess: [u8; 5] = z[3..8].try_into().unwrap();
		let tail = &z[8..];

		assert!(matches!(probe_header(d0, &guess, ALL_KNOWN, tail), Probe::Valid));
		// Hiding byte k must stall on k (the header reads bytes in order and
		// consumes well past byte 5).
		for k in 1..=5usize {
			let known = ALL_KNOWN & !(1 << (k - 1));
			match probe_header(d0, &guess, known, tail) {
				Probe::Need { idx, .. } => assert_eq!(idx, k, "hid byte {k}, stalled on {idx}"),
				_ => panic!("hiding byte {k} should stall the parse"),
			}
		}
	}

	/// The pruned DFS must find the same answer a flat five-deep sweep would.
	/// The candidate sets are kept small (the truth plus decoys, truth *last*
	/// so the search cannot get it by luck) — the point is the tree walk, not
	/// throughput; the full-width sweep is `recovers_via_full_search`.
	#[test]
	fn dfs_finds_the_true_bytes() {
		let plain: Vec<u8> = (0..4096u32)
			.map(|i| 0x20 + ((i.wrapping_mul(7).wrapping_add(i / 13)) % 60) as u8)
			.collect();
		let mut z = miniz_oxide::deflate::compress_to_vec_zlib(&plain, 9);
		z[0] = 0x78;
		z[1] = 0xda;
		let truth: [u8; 5] = z[3..8].try_into().unwrap();
		let sets: [Vec<u8>; 5] = std::array::from_fn(|k| {
			let mut s: Vec<u8> = (0..16u8).map(|j| j.wrapping_mul(17).wrapping_add(k as u8)).collect();
			s.retain(|&c| c != truth[k]);
			s.push(truth[k]);
			s
		});
		let found = AtomicBool::new(false);
		let search = Search {
			d0: z[2],
			tail: &z[8..],
			sets: &sets,
			found: &found,
		};
		let mut scratch = Scratch::new(z[2], &z[8..], plain.len());
		// known=0 lets the dfs itself branch the first byte over sets[0]; the
		// threaded path instead pins byte 1 per work unit, but the tree walked
		// is the same, which is what this single-threaded check pins down.
		assert_eq!(search.dfs(&mut [0u8; 5], 0, None, &mut scratch), Some(truth));
	}

	/// The cheap filter must agree with the authority. `cl_scan` may only
	/// reject what `parse_full` would also reject — if it ever rejected more,
	/// the search would drop candidates that were never re-examined, and the
	/// failure mode is a silent "no hit" rather than anything that looks like
	/// a bug. Checked over random streams (nearly all rejects, which is what
	/// the search actually sees) and real deflate headers (the accepts).
	#[test]
	fn the_reusable_buffer_agrees_with_a_fresh_allocation() {
		// The confirming inflate is the definitive oracle, so reusing its
		// buffers must not change a single verdict. Compare the in-place
		// version against building a fresh stream and calling
		// decompress_to_vec_zlib, over a real header plus deliberate misses.
		let plain: Vec<u8> = (0..4096u32).map(|i| (i * 7 % 251) as u8).collect();
		let zs = miniz_oxide::deflate::compress_to_vec_zlib(&plain, 6);
		assert!(zs.len() > 8, "need a stream with a tail");
		let d0 = zs[2];
		let truth: [u8; 5] = zs[3..8].try_into().unwrap();
		let tail = zs[8..].to_vec();

		let mut scratch = Scratch::new(d0, &tail, plain.len());
		let fresh = |guess: &[u8; 5]| {
			let mut s = vec![0x78, 0xda, d0];
			s.extend_from_slice(guess);
			s.extend_from_slice(&tail);
			matches!(miniz_oxide::inflate::decompress_to_vec_zlib(&s),
                     Ok(o) if o.len() == plain.len())
		};

		assert!(scratch.inflates_to_plainlen(&truth), "the true bytes must confirm");
		assert!(fresh(&truth));
		// And a reused buffer must not let a previous success leak into a
		// later miss — run several wrong guesses after the hit.
		for k in 0..5 {
			let mut wrong = truth;
			wrong[k] = wrong[k].wrapping_add(1);
			assert_eq!(
				scratch.inflates_to_plainlen(&wrong),
				fresh(&wrong),
				"verdict differs for a one-byte miss at {k}"
			);
		}
		assert!(scratch.inflates_to_plainlen(&truth), "still confirms after misses");
	}

	#[test]
	fn the_cheap_filter_never_rejects_what_the_full_parse_accepts() {
		let mut state = 0x9e37_79b9_7f4a_7c15u64;
		let mut rng = move || {
			state ^= state << 13;
			state ^= state >> 7;
			state ^= state << 17;
			state
		};
		for _ in 0..50_000 {
			let d0 = (rng() & 0xff) as u8;
			let guess: [u8; 5] = std::array::from_fn(|_| (rng() & 0xff) as u8);
			let tail: Vec<u8> = (0..80).map(|_| (rng() & 0xff) as u8).collect();
			let filtered = matches!(probe_header(d0, &guess, ALL_KNOWN, &tail), Probe::Valid);
			let authority = matches!(parse_full(d0, &guess, ALL_KNOWN, &tail), Probe::Valid);
			assert_eq!(filtered, authority, "d0={d0:#x} guess={guess:02x?}");
		}
		for level in [1u8, 6, 9] {
			let plain: Vec<u8> = (0..8192u32).map(|i| (i.wrapping_mul(37) % 211) as u8).collect();
			let z = miniz_oxide::deflate::compress_to_vec_zlib(&plain, level);
			let guess: [u8; 5] = z[3..8].try_into().unwrap();
			assert!(matches!(probe_header(z[2], &guess, ALL_KNOWN, &z[8..]), Probe::Valid));
			assert!(matches!(parse_full(z[2], &guess, ALL_KNOWN, &z[8..]), Probe::Valid));
		}
	}

	/// Resuming a stalled parse must be indistinguishable from parsing the
	/// same fully-pinned stream from bit 0. If it ever diverged the search
	/// would quietly explore the wrong subtree, so this walks a spread of
	/// pseudo-random streams (most of which are garbage headers — exactly what
	/// the search actually sees) and compares the two routes at every depth.
	#[test]
	fn resuming_matches_a_fresh_parse() {
		let mut state = 0x2545_f491_4f6c_dd1du64;
		let mut rng = move || {
			state ^= state << 13;
			state ^= state >> 7;
			state ^= state << 17;
			state
		};
		let mut resumed = 0usize;
		for _ in 0..20_000 {
			let d0 = (rng() & 0xff) as u8;
			let guess: [u8; 5] = std::array::from_fn(|_| (rng() & 0xff) as u8);
			let tail: Vec<u8> = (0..64).map(|_| (rng() & 0xff) as u8).collect();
			// Walk depth by depth, carrying the resume state the way dfs does.
			let mut at: Option<Resume> = None;
			for known in [0b0_0001u8, 0b0_0011, 0b0_0111, 0b0_1111, 0b1_1111] {
				let fresh = probe_header(d0, &guess, known, &tail);
				let via = match &at {
					Some(r) => {
						resumed += 1;
						probe_resume(r, d0, &guess, known, &tail)
					}
					None => probe_header(d0, &guess, known, &tail),
				};
				let tag = |p: &Probe| match p {
					Probe::Valid => (0u8, 0usize),
					Probe::Reject => (1, 0),
					Probe::Need { idx, .. } => (2, *idx),
				};
				assert_eq!(tag(&fresh), tag(&via), "d0={d0:#x} guess={guess:02x?}");
				match via {
					Probe::Need { at: next, .. } => at = next,
					_ => break,
				}
			}
		}
		// Guard against the test silently never exercising the resume path.
		assert!(resumed > 1000, "only {resumed} resumes exercised");
	}

	/// A guard on the two prunes that could silently lose the answer: the
	/// code-length code must be complete, and the literal/distance Kraft
	/// totals are checked as lengths arrive rather than at the end. A real
	/// deflate header must survive both.
	#[test]
	fn a_real_header_survives_the_completeness_and_incremental_kraft_prunes() {
		for level in [1u8, 6, 9] {
			let plain: Vec<u8> = (0..8192u32).map(|i| (i.wrapping_mul(31) % 200) as u8).collect();
			let z = miniz_oxide::deflate::compress_to_vec_zlib(&plain, level);
			assert!(
				matches!(probe_header(z[2], &z[3..8].try_into().unwrap(), ALL_KNOWN, &z[8..]), Probe::Valid),
				"level {level} header rejected"
			);
		}
	}
}
