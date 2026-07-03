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
pub(crate) fn tea_cbc_decrypt(cipher: &[u8], key: &[u32; 4], iv: [u8; 8]) -> Vec<u8> {
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
