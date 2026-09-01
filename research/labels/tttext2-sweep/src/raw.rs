//! Thin wrappers over the shipped `.rod` framing, for this driver's use.
//!
//! Everything here delegates to `crate::rod` — the real
//! `crates/vag-data-labels/src/rod.rs`, compiled into this crate by a `#[path]`
//! module rather than copied. So the framing this driver sees and the framing
//! the CLI sees cannot drift: they are the same functions.

use crate::rod::{KEY_ROD, decode_latin1, deflate_anchors, find_close, find_next_tag, parse_section_cipher, rod_block0_iv};
use crate::tea::{tea_cbc_decrypt, tea_decrypt_block};

/// One framed section: tag, zlib-vs-plain-TEA, declared plaintext length,
/// ciphertext.
pub struct Framed {
	pub tag: String,
	pub compressed: bool,
	pub plainlen: usize,
	pub cipher: Vec<u8>,
}

pub fn framed(data: &[u8]) -> Vec<Framed> {
	let mut out = Vec::new();
	let mut pos = 0usize;
	while let Some((tag, start)) = find_next_tag(data, pos) {
		let Some((end, next)) = find_close(data, start, &tag) else { break };
		if let Some(sc) = parse_section_cipher(&data[start..end]) {
			out.push(Framed {
				tag: decode_latin1(&tag),
				compressed: sc.compressed,
				plainlen: sc.plainlen,
				cipher: sc.cipher.to_vec(),
			});
		}
		pos = next;
	}
	out
}

/// The tag-derived ("model") IV — exact for a classic file, and for the
/// `[CMP]` section of every file.
pub fn model_iv(tag: &[u8]) -> [u8; 8] {
	rod_block0_iv(tag)
}

/// Raw ECB decryption of the first cipher block: `plaintext[i] = t[i] ^ iv[i]`.
pub fn first_block(cipher: &[u8]) -> [u8; 8] {
	tea_decrypt_block(cipher[0..8].try_into().unwrap(), &KEY_ROD)
}

pub fn cbc(cipher: &[u8], iv: [u8; 8]) -> Vec<u8> {
	tea_cbc_decrypt(cipher, &KEY_ROD, iv)
}

/// The 60 values deflate byte 0 can take: `BTYPE = 2`, `HLIT ≤ 29`, either
/// `BFINAL`.
pub fn anchors() -> Vec<u8> {
	deflate_anchors().collect()
}
