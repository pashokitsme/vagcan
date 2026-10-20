//! Shared TEA (Tiny Encryption Algorithm) primitives used by both the `.clb`
//! and `.rod` decoders. TEA here is 32 rounds, little-endian 32-bit words,
//! used in CBC mode with a format-specific key and IV derivation.

pub(crate) const DELTA: u32 = 0x9E37_79B9;
/// Initial `sum` for decryption: `DELTA.wrapping_mul(32)`.
pub(crate) const SUM0: u32 = 0xC6EF_3720;

/// Decrypt one 8-byte TEA block (32 rounds), before the CBC xor with the
/// previous ciphertext block / IV.
pub(crate) fn tea_decrypt_block(block: [u8; 8], key: &[u32; 4]) -> [u8; 8] {
	let mut v0 = u32::from_le_bytes(block[0..4].try_into().unwrap());
	let mut v1 = u32::from_le_bytes(block[4..8].try_into().unwrap());
	let mut s = SUM0;
	for _ in 0..32 {
		v1 = v1.wrapping_sub((v0 << 4).wrapping_add(key[2]) ^ v0.wrapping_add(s) ^ (v0 >> 5).wrapping_add(key[3]));
		v0 = v0.wrapping_sub((v1 << 4).wrapping_add(key[0]) ^ v1.wrapping_add(s) ^ (v1 >> 5).wrapping_add(key[1]));
		s = s.wrapping_sub(DELTA);
	}
	let mut out = [0u8; 8];
	out[0..4].copy_from_slice(&v0.to_le_bytes());
	out[4..8].copy_from_slice(&v1.to_le_bytes());
	out
}

/// Encrypt one 8-byte TEA block — the inverse of [`tea_decrypt_block`].
///
/// Test-only, and one copy of it: the tool never writes an encrypted file, but
/// its tests have to, to build label files with no Ross-Tech data in them.
#[cfg(test)]
pub(crate) fn tea_encrypt_block(block: [u8; 8], key: &[u32; 4]) -> [u8; 8] {
	let mut v0 = u32::from_le_bytes(block[0..4].try_into().unwrap());
	let mut v1 = u32::from_le_bytes(block[4..8].try_into().unwrap());
	let mut s = 0u32;
	for _ in 0..32 {
		s = s.wrapping_add(DELTA);
		v0 = v0.wrapping_add((v1 << 4).wrapping_add(key[0]) ^ v1.wrapping_add(s) ^ (v1 >> 5).wrapping_add(key[1]));
		v1 = v1.wrapping_add((v0 << 4).wrapping_add(key[2]) ^ v0.wrapping_add(s) ^ (v0 >> 5).wrapping_add(key[3]));
	}
	let mut out = [0u8; 8];
	out[0..4].copy_from_slice(&v0.to_le_bytes());
	out[4..8].copy_from_slice(&v1.to_le_bytes());
	out
}

/// TEA-CBC encrypt, the inverse of [`tea_cbc_decrypt`]:
/// `C_i = TEA_enc(P_i XOR C_{i-1})`, with `C_{-1} = iv`. `plain.len()` must be
/// a multiple of 8. Test-only, like [`tea_encrypt_block`].
#[cfg(test)]
pub(crate) fn tea_cbc_encrypt(plain: &[u8], key: &[u32; 4], iv: [u8; 8]) -> Vec<u8> {
	assert_eq!(plain.len() % 8, 0, "TEA-CBC works on whole blocks");
	let mut out = Vec::with_capacity(plain.len());
	let mut prev = iv;
	for block in plain.chunks_exact(8) {
		let mut xored = [0u8; 8];
		for i in 0..8 {
			xored[i] = block[i] ^ prev[i];
		}
		let cipher = tea_encrypt_block(xored, key);
		out.extend_from_slice(&cipher);
		prev = cipher;
	}
	out
}

/// TEA-CBC decrypt: `cipher` is processed in 8-byte blocks (any trailing
/// partial block, which should not occur for well-formed records, is
/// ignored). `P_i = TEA_dec(C_i) XOR C_{i-1}`, with `C_{-1} = iv`.
///
/// Each plaintext block needs only its own cipher block and the one before it,
/// never the plaintext before it, so the blocks decrypt independently: runs of
/// 64 KiB go to rayon's pool. The global tables are megabytes of
/// cipher — `TTTEXT`, `RM`, `RD` — and one core spent tens of milliseconds on
/// each. Anything up to one run — a label file, a fault record — is decrypted
/// right here, as before: handing it to the pool would cost more than it does.
pub(crate) fn tea_cbc_decrypt(cipher: &[u8], key: &[u32; 4], iv: [u8; 8]) -> Vec<u8> {
	use rayon::prelude::*;
	/// Bytes per task: big enough that a task is far more work than handing it out.
	const CHUNK: usize = 64 * 1024;
	let whole = cipher.len() / 8 * 8;
	let mut out = vec![0u8; whole];
	let run = |n: usize, plain: &mut [u8]| {
		let start = n * CHUNK;
		for (k, plain) in plain.chunks_exact_mut(8).enumerate() {
			let at = start + 8 * k;
			let prev: [u8; 8] = match at {
				0 => iv,
				_ => cipher[at - 8..at].try_into().unwrap(),
			};
			let dec = tea_decrypt_block(cipher[at..at + 8].try_into().unwrap(), key);
			for i in 0..8 {
				plain[i] = dec[i] ^ prev[i];
			}
		}
	};
	match whole <= CHUNK {
		true => run(0, &mut out),
		false => out.par_chunks_mut(CHUNK).enumerate().for_each(|(n, plain)| run(n, plain)),
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_cipher_of_many_runs_decrypts_as_one_block_after_another_would() {
		// Several 64 KiB runs and a ragged end, so every run boundary is crossed:
		// the first block of each run takes the previous run's last cipher block
		// as its IV, and only the very first takes `iv`.
		let key = [0x0123_4567, 0x89ab_cdef, 0xfedc_ba98, 0x7654_3210];
		let iv = [1, 2, 3, 4, 5, 6, 7, 8];
		let plain: Vec<u8> = (0..(3 * 64 * 1024 + 8 * 13))
			.map(|i: u32| (i.wrapping_mul(31) ^ (i >> 7)) as u8)
			.collect();
		let cipher = tea_cbc_encrypt(&plain, &key, iv);
		assert_eq!(tea_cbc_decrypt(&cipher, &key, iv), plain);
		// A trailing partial block is ignored, as before.
		let mut ragged = cipher.clone();
		ragged.extend_from_slice(&[9, 9, 9]);
		assert_eq!(tea_cbc_decrypt(&ragged, &key, iv), plain);
	}
}
