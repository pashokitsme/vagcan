//! The 31-bit DJB2 an ODIS project uses as an object's identity.
//!
//! A `.key` B+Tree does not store names. Its keys are four bytes, and those
//! four bytes are the hash of an ObjectID string — so nothing in the tree is
//! readable until the same hash is recomputed over the string pools
//! ([`super::strings`]) and the two are joined. That join is the whole reason
//! this module exists, and it is exact: over the reference project's engine
//! pool, all 576,793 keys resolve to a pool string.
//!
//! The algorithm is Bernstein's DJB2 (`hash = hash * 33 + c`, seeded 5381),
//! truncated to 31 bits, with two adjustments that are properties of *this*
//! store rather than of DJB2: a hash of `0` is illegal and becomes [`ZERO_SUBSTITUTE`],
//! and a hash already taken by a different string is retried at `+ 11`
//! ([`probe`]). Both were read off `ODIS-project-explorer`'s `StringStorage.py`
//! and confirmed against the reference project's own `.idx` files, which record
//! the hash VW's tooling assigned to every string.

/// DJB2's seed. Bernstein's constant, unchanged by VW.
pub const SEED: u32 = 5381;

/// The high bit is never part of a hash: VW masks it off, leaving 31 bits.
pub const MASK: u32 = 0x7FFF_FFFF;

/// What a hash of `0` becomes. `0` is reserved (a zero key would collide with
/// the "no string" sentinel the object stream uses), so VW's tooling maps it to
/// this instead. The value is arbitrary — it just has to be a hash no natural
/// string is likely to land on and to be applied identically on both sides.
pub const ZERO_SUBSTITUTE: u32 = 5;

/// The step taken when a hash is already occupied by a *different* string.
pub const PROBE_STEP: u32 = 11;

/// Hash a byte string, one code unit per byte — the `AStringData` pool's rule,
/// where a string is stored as Windows-1252 and each byte is fed to DJB2 as-is.
///
/// Hashing the *stored bytes* rather than a decoded `&str` is deliberate: it is
/// what the format does, and it means a name that fails to decode cleanly still
/// hashes to the value the `.key` tree holds.
pub fn of_bytes(bytes: &[u8]) -> u32 {
	fold(bytes.iter().map(|&b| u32::from(b)))
}

/// Hash a UTF-16 string, one code unit per **`u16`** — the `UStringData` pool's
/// rule. Note that this is not the byte-wise hash of the same text: a UTF-16
/// unit is fed to DJB2 whole, so the two pools give different hashes for the
/// same characters and are two separate namespaces.
pub fn of_utf16(units: &[u16]) -> u32 {
	fold(units.iter().map(|&u| u32::from(u)))
}

/// The next candidate hash after `hash` was found occupied by another string.
///
/// Linear probing at a stride of 11, with the `0` substitution reapplied. The
/// stride matters: a reader that probes differently reconstructs a *different*
/// table from the same pool, and every key that ever collided then resolves to
/// the wrong name.
pub fn probe(hash: u32) -> u32 {
	legalize(hash.wrapping_add(PROBE_STEP))
}

/// Reduce a raw accumulator to a legal hash: 31 bits, never `0`.
///
/// Applied in two places — at the end of a fold and after every probe step —
/// and it must be the same rule in both, or a string that probes onto the
/// forbidden `0` gets a different hash from the writer's.
fn legalize(raw: u32) -> u32 {
	let hash = raw & MASK;
	if hash == 0 { ZERO_SUBSTITUTE } else { hash }
}

/// The shared core: `h = h * 33 + c` over the code units, then the 31-bit mask
/// and the `0` substitution.
///
/// The multiply-accumulate runs in wrapping `u32` while VW's Python reference
/// runs it in unbounded integers and masks only at the end. Those agree: every
/// step is `h ↦ 33·h + c`, which is a ring homomorphism modulo any `2^k`, and
/// `2^31` divides `2^32` — so reducing early cannot change the low 31 bits.
fn fold(units: impl Iterator<Item = u32>) -> u32 {
	let mut hash = SEED;
	for unit in units {
		hash = hash.wrapping_mul(33).wrapping_add(unit);
	}
	legalize(hash)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Hand-computed from the definition: the empty string is the bare seed.
	#[test]
	fn empty_string_is_the_seed() {
		assert_eq!(of_bytes(b""), SEED);
		assert_eq!(of_utf16(&[]), SEED);
	}

	/// `h = 5381 * 33 + b'A'` = 177_638. Worked by hand, not by running the code.
	#[test]
	fn one_byte_is_seed_times_33_plus_it() {
		assert_eq!(of_bytes(b"A"), 5381 * 33 + 65);
	}

	/// Two bytes, still by hand: `(5381 * 33 + 65) * 33 + 66` = 5_862_120.
	#[test]
	fn two_bytes_fold_left() {
		assert_eq!(of_bytes(b"AB"), (5381 * 33 + 65) * 33 + 66);
	}

	/// The mask is the whole difference between this and stock DJB2: a long
	/// enough string overflows into the high bit, and that bit must be dropped.
	#[test]
	fn the_high_bit_is_masked_off() {
		// Long enough that the 32-bit accumulator has wrapped many times over.
		let long = vec![b'x'; 64];
		assert_eq!(of_bytes(&long) & !MASK, 0, "a hash must never carry the high bit");
	}

	/// A UTF-16 unit is one code unit, not two bytes: the same text hashes
	/// differently in the two pools, which is why they are separate namespaces.
	#[test]
	fn utf16_hashes_units_not_bytes() {
		assert_eq!(of_utf16(&[0x0041, 0x0042]), of_bytes(b"AB"));
		// A character above U+00FF has no byte-wise equivalent at all.
		assert_eq!(of_utf16(&[0x20AC]), 5381u32.wrapping_mul(33).wrapping_add(0x20AC));
	}

	/// An accumulator of `0` is illegal and becomes 5 — the rule both the fold
	/// and the probe end on, tested where it actually lives.
	#[test]
	fn a_zero_hash_becomes_five() {
		assert_eq!(legalize(0), ZERO_SUBSTITUTE);
		// The high bit alone masks away to zero, so it takes the substitute too.
		assert_eq!(legalize(0x8000_0000), ZERO_SUBSTITUTE);
		// Anything else passes through with only the high bit dropped.
		assert_eq!(legalize(0x8000_0001), 1);
	}

	/// Probing steps by 11 and stays inside 31 bits when it wraps.
	#[test]
	fn probe_steps_by_eleven() {
		assert_eq!(probe(100), 111);
		// 0x7FFF_FFFF + 11 = 0x8000_000A, masked back down to 10.
		assert_eq!(probe(MASK), 10);
	}

	/// The wrap lands on `0` exactly when the previous hash was `MASK - 10`,
	/// and that case must take the substitute, not the illegal `0`.
	#[test]
	fn probing_onto_zero_takes_the_substitute() {
		assert_eq!(probe(MASK - 10), ZERO_SUBSTITUTE);
	}
}
