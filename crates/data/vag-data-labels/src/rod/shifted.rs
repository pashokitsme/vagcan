//! Construction and consumption of the interface exchange for shifted `.rod` files.
//!
//! A shifted file has an eight-digit gate in `[CMP]`. A compatible interface answers
//! command `0x45` with 32 bytes; the last eight, reversed, are the file's IV mask.

use super::KS;

/// Parse the eight-digit gate in the first field of a decoded `[CMP]` section.
/// Six-digit gates identify classic files and return `None`.
pub fn gate(cmp: &str) -> Option<u32> {
	let digits = cmp.split(',').next()?;
	if digits.len() != 8 || !digits.bytes().all(|c| c.is_ascii_digit()) {
		return None;
	}
	digits.parse().ok()
}

/// Build the 32-byte `0x45` challenge for `file` and its eight-digit `gate`.
///
/// The file name is normalized like [`super::name_product`]: only the last path
/// component is used, upper-cased and cut at its first dot.
pub fn challenge(file: &str, gate: u32) -> [u8; 32] {
	let base = file.rsplit(['/', '\\']).next().unwrap_or(file);
	let stem = base.split('.').next().unwrap_or_default().as_bytes();
	let uppercase;
	let stem = if stem.iter().any(u8::is_ascii_lowercase) {
		uppercase = stem.to_ascii_uppercase();
		uppercase.as_slice()
	} else {
		stem
	};
	let mut hash = stem.first().copied().unwrap_or(0);
	for &c in stem {
		hash = KS[0].wrapping_add(c ^ hash);
	}
	std::array::from_fn(|i| {
		let byte = (gate >> (KS[i + 1] >> 3)) as u8;
		if byte != 0 {
			return byte;
		}
		let name = if i % 2 == 0 { hash >> 1 } else { hash };
		KS[(i + name as usize) & 0xff]
	})
}

/// Extract the mask from a 32-byte `0x45` reply payload.
pub fn mask_from_reply(reply: &[u8]) -> Option<[u8; 8]> {
	let reply: &[u8; 32] = reply.try_into().ok()?;
	Some(std::array::from_fn(|i| reply[31 - i]))
}
