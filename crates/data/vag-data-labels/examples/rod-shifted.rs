//! Offline PoC for the shifted-ROD `0x45` exchange and direct mask decoding.
//! It consumes CB blocks, not the still-unverified outer USB framing.

use aes::Aes256;
use cbc::cipher::block_padding::NoPadding;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::traits::PublicKeyParts;
use rsa::{Oaep, RsaPrivateKey};
use sha2::Sha256;
use vag_data_labels::rod::{IvCache, RodStatus, decode_rod_with_mask, open_section, shifted};

const COMMAND: u16 = 0x45;
const MAX_CHUNK: usize = 0xdb;
const USAGE: &str = "usage:\n  rod-shifted challenge FILE\n  rod-shifted request FILE KEY.pem WRAPPED_KEY_HEX MSG_TYPE\n  rod-shifted open FILE --mask HEX\n  rod-shifted open FILE --key KEY.pem --wrapped-key HEX --reply ENCRYPTED_CB_HEX";

const IV_TABLE: [[u8; 16]; 16] = [
	[
		0x56, 0x51, 0x54, 0x3b, 0x24, 0x45, 0x15, 0x21, 0x03, 0x54, 0x1a, 0x34, 0x54, 0x82, 0x10, 0x4c,
	],
	[
		0x76, 0x55, 0xed, 0x1c, 0x86, 0x57, 0x12, 0x34, 0xc0, 0x32, 0xdb, 0x2f, 0xf3, 0x54, 0x28, 0x02,
	],
	[
		0x34, 0x26, 0x43, 0x34, 0x39, 0xbd, 0x64, 0x52, 0x8d, 0x11, 0x44, 0x2f, 0x32, 0x07, 0x45, 0x22,
	],
	[
		0x87, 0xe8, 0x23, 0x9f, 0x0c, 0x7b, 0x87, 0x56, 0x0c, 0xfe, 0x1f, 0x5c, 0xbc, 0xd9, 0xfa, 0x89,
	],
	[
		0x4d, 0x39, 0x86, 0xa3, 0xde, 0xe2, 0xba, 0x2a, 0xd0, 0x4c, 0x1c, 0xdf, 0x23, 0x34, 0x45, 0xee,
	],
	[
		0x6a, 0x3d, 0x43, 0x10, 0x30, 0xa3, 0x4a, 0x21, 0x76, 0xa4, 0xcb, 0x64, 0xa3, 0x23, 0x86, 0x87,
	],
	[
		0x5b, 0xdc, 0x54, 0xab, 0xf4, 0xde, 0x7c, 0x4d, 0x5a, 0x83, 0x12, 0x43, 0x65, 0x3f, 0x5c, 0xa5,
	],
	[
		0x54, 0xe5, 0xfd, 0x6b, 0xce, 0xdc, 0x4d, 0x34, 0x22, 0xb5, 0xc0, 0x65, 0x66, 0x5c, 0x3b, 0x89,
	],
	[
		0x87, 0xe4, 0x3c, 0xa3, 0x34, 0x12, 0x36, 0xf4, 0x27, 0x3d, 0x97, 0x36, 0x23, 0x6a, 0x76, 0x32,
	],
	[
		0xbc, 0x68, 0x34, 0xf6, 0x75, 0xb8, 0x76, 0x6c, 0xca, 0x5f, 0xe8, 0xca, 0xbe, 0xdb, 0x3b, 0x90,
	],
	[
		0xa3, 0xa3, 0xbc, 0xbd, 0x46, 0xc3, 0x23, 0x47, 0x2c, 0x8b, 0xe6, 0x5a, 0xf3, 0x23, 0x34, 0x95,
	],
	[
		0x54, 0x21, 0x34, 0x37, 0xce, 0x23, 0x65, 0xa1, 0x0b, 0x6b, 0xc4, 0xf5, 0xc2, 0xb7, 0x65, 0xea,
	],
	[
		0xc3, 0x66, 0x75, 0xc6, 0xf5, 0x73, 0xdd, 0x4b, 0x0d, 0x76, 0xc7, 0xf3, 0x76, 0x6c, 0x23, 0x28,
	],
	[
		0xb5, 0xa8, 0x23, 0xbf, 0x32, 0x5d, 0x44, 0x1a, 0x3b, 0x34, 0xf3, 0x4e, 0x02, 0x78, 0x64, 0xc1,
	],
	[
		0x23, 0x67, 0x65, 0x0c, 0x76, 0x3f, 0x77, 0x1a, 0x64, 0x87, 0x38, 0x48, 0x03, 0xfc, 0x87, 0x7a,
	],
	[
		0xcd, 0x4c, 0x87, 0x3f, 0x2a, 0x2a, 0x33, 0x4e, 0x7a, 0x43, 0xc3, 0x65, 0x40, 0x4a, 0xa4, 0x23,
	],
];

type Result<T> = std::result::Result<T, String>;

fn main() {
	if let Err(error) = run() {
		eprintln!("rod-shifted: {error}");
		std::process::exit(1);
	}
}

fn run() -> Result<()> {
	let args: Vec<String> = std::env::args().skip(1).collect();
	match args.as_slice() {
		[command, file] if command == "challenge" => {
			let data = read(file)?;
			let (gate, challenge) = file_challenge(&data, file)?;
			println!("gate: {gate:08}");
			println!("challenge: {}", hex(&challenge));
			Ok(())
		}
		[command, file, key_path, wrapped, msg_type] if command == "request" => {
			let msg_type = parse_msg_type(msg_type)?;
			let data = read(file)?;
			let (gate, challenge) = file_challenge(&data, file)?;
			let key = load_private_key(key_path)?;
			let session = unwrap_session_key(&key, &unhex(wrapped)?)?;
			let encrypted = encrypt_cb(&session, msg_type, COMMAND, &challenge)?;
			println!("gate: {gate:08}");
			println!("challenge: {}", hex(&challenge));
			println!("cid: {}", channel(msg_type));
			println!("encrypted_cb: {}", hex(&encrypted));
			Ok(())
		}
		[command, file, option, mask] if command == "open" && option == "--mask" => {
			let mask: [u8; 8] = unhex(mask)?
				.try_into()
				.map_err(|v: Vec<u8>| format!("mask must be 8 bytes, got {}", v.len()))?;
			open_with_mask(file, mask)
		}
		[command, file, key_option, key_path, wrapped_option, wrapped, reply_option, reply]
			if command == "open" && key_option == "--key" && wrapped_option == "--wrapped-key" && reply_option == "--reply" =>
		{
			let key = load_private_key(key_path)?;
			let session = unwrap_session_key(&key, &unhex(wrapped)?)?;
			let (_, command, payload) = decrypt_cb(&session, &unhex(reply)?)?;
			if command != COMMAND {
				return Err(format!("reply command is {command:#06x}, expected {COMMAND:#06x}"));
			}
			let mask = shifted::mask_from_reply(&payload).ok_or_else(|| format!("0x45 reply must contain 32 bytes, got {}", payload.len()))?;
			eprintln!("mask: {}", hex(&mask));
			open_with_mask(file, mask)
		}
		_ => Err(USAGE.to_string()),
	}
}

fn read(path: &str) -> Result<Vec<u8>> {
	std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))
}

fn file_challenge(data: &[u8], file: &str) -> Result<(u32, [u8; 32])> {
	let mut cache = IvCache::default();
	let cmp = open_section(data, file, "CMP", &mut cache, false).ok_or("ROD has no [CMP] section")?;
	if cmp.status != RodStatus::Tea {
		return Err(format!("[CMP] did not decode: {:?}", cmp.status));
	}
	let text = cmp.text.as_deref().ok_or("[CMP] has no text")?;
	let gate = shifted::gate(text).ok_or("[CMP] does not have an eight-digit gate; this is not a shifted ROD")?;
	Ok((gate, shifted::challenge(file, gate)))
}

fn open_with_mask(file: &str, mask: [u8; 8]) -> Result<()> {
	let data = read(file)?;
	let sections = decode_rod_with_mask(&data, file, mask);
	if sections.is_empty() {
		return Err("no ROD sections found".to_string());
	}
	let failed = sections
		.iter()
		.any(|s| !matches!(s.status, RodStatus::Tea | RodStatus::Zlib) || s.text.is_none());
	for section in &sections {
		let len = section.text.as_ref().map(|text| text.chars().count()).unwrap_or(0);
		eprintln!("{}: {:?}, {len} bytes", section.tag, section.status);
	}
	if failed {
		return Err("one or more sections did not decode; check the mask, file name and framing".to_string());
	}
	for section in sections {
		let text = section.text.as_deref().unwrap_or_default();
		println!("[{}]", section.tag);
		print!("{text}");
		if !text.ends_with('\n') {
			println!();
		}
	}
	Ok(())
}

fn parse_msg_type(text: &str) -> Result<u8> {
	let value = text.parse::<u8>().map_err(|e| format!("MSG_TYPE must be a decimal byte: {e}"))?;
	if value > 0x0f {
		return Err("MSG_TYPE must be in 0..=15 for this PoC".to_string());
	}
	Ok(value)
}

fn unhex(text: &str) -> Result<Vec<u8>> {
	let bytes = text.as_bytes();
	if bytes.len() % 2 != 0 {
		return Err("hex input must have an even number of digits".to_string());
	}
	(0..bytes.len())
		.step_by(2)
		.map(|i| {
			let digit = |b: u8| (b as char).to_digit(16).ok_or_else(|| format!("invalid hex at byte {}", i / 2));
			Ok((digit(bytes[i])? * 16 + digit(bytes[i + 1])?) as u8)
		})
		.collect()
}

fn hex(bytes: &[u8]) -> String {
	bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn load_private_key(path: &str) -> Result<RsaPrivateKey> {
	let pem = std::fs::read_to_string(path).map_err(|e| format!("cannot read private key {path}: {e}"))?;
	parse_private_key(&pem)
}

fn parse_private_key(pem: &str) -> Result<RsaPrivateKey> {
	let key = match RsaPrivateKey::from_pkcs1_pem(pem) {
		Ok(key) => key,
		Err(pkcs1) => RsaPrivateKey::from_pkcs8_pem(pem).map_err(|pkcs8| format!("private key is neither PKCS#1 ({pkcs1}) nor PKCS#8 ({pkcs8}) PEM"))?,
	};
	key.validate().map_err(|e| format!("invalid RSA private key: {e}"))?;
	if key.size() != 128 {
		return Err(format!("expected an RSA-1024 key, got {} bits", key.size() * 8));
	}
	Ok(key)
}

fn unwrap_session_key(key: &RsaPrivateKey, wrapped: &[u8]) -> Result<[u8; 32]> {
	if wrapped.len() != key.size() {
		return Err(format!(
			"wrapped key must be {} bytes for this RSA key, got {}",
			key.size(),
			wrapped.len()
		));
	}
	let plain = key
		.decrypt(Oaep::new::<Sha256>(), wrapped)
		.map_err(|e| format!("RSA-OAEP-SHA256 unwrap failed: {e}"))?;
	plain
		.try_into()
		.map_err(|v: Vec<u8>| format!("unwrapped session key must be 32 bytes, got {}", v.len()))
}

fn channel(msg_type: u8) -> usize {
	(msg_type as usize + 1) & 0x0f
}

fn xor_checksum(bytes: &[u8]) -> u8 {
	bytes.iter().fold(0, |sum, &b| sum ^ b)
}

fn plain_cb(msg_type: u8, command: u16, payload: &[u8]) -> Result<Vec<u8>> {
	if msg_type > 0x0f || payload.len() > MAX_CHUNK {
		return Err(format!("CB needs msg_type <= 15 and at most {MAX_CHUNK} payload bytes"));
	}
	let total = u16::try_from(payload.len()).map_err(|_| "payload length does not fit CB")?;
	let mut out = Vec::with_capacity(7 + payload.len());
	out.extend_from_slice(&[msg_type, 0]);
	out.extend_from_slice(&command.to_be_bytes());
	out.extend_from_slice(&total.to_be_bytes());
	out.push(payload.len() as u8);
	out.extend_from_slice(payload);
	Ok(out)
}

fn encrypt_cb(session: &[u8; 32], msg_type: u8, command: u16, payload: &[u8]) -> Result<Vec<u8>> {
	let plain = plain_cb(msg_type, command, payload)?;
	let mut body = plain[2..].to_vec();
	let padded = body.len().div_ceil(16) * 16;
	body.resize(padded, 0);
	let cipher = aes_cbc_encrypt(session, &IV_TABLE[channel(msg_type)], &body)?;
	let marker = msg_type | 0xf0;
	let mut out = vec![marker, marker];
	out.extend_from_slice(&cipher);
	out.push(xor_checksum(&out));
	Ok(out)
}

fn decrypt_cb(session: &[u8; 32], block: &[u8]) -> Result<(u8, u16, Vec<u8>)> {
	if block.len() < 19 || block[0] != block[1] || block[0] & 0xf0 != 0xf0 {
		return Err("invalid encrypted CB markers or length".to_string());
	}
	if block.last() != Some(&xor_checksum(&block[..block.len() - 1])) {
		return Err("encrypted CB XOR checksum mismatch".to_string());
	}
	let cipher = &block[2..block.len() - 1];
	if cipher.len() % 16 != 0 {
		return Err("encrypted CB ciphertext is not a whole number of AES blocks".to_string());
	}
	let msg_type = block[0] & 0x0f;
	let body = aes_cbc_decrypt(session, &IV_TABLE[channel(msg_type)], cipher)?;
	let mut plain = vec![msg_type, 0];
	plain.extend_from_slice(&body);
	if plain.len() < 7 {
		return Err("decrypted CB is shorter than its header".to_string());
	}
	let command = u16::from_be_bytes([plain[2], plain[3]]);
	let total = u16::from_be_bytes([plain[4], plain[5]]) as usize;
	let chunk = plain[6] as usize;
	if total != chunk || chunk > MAX_CHUNK {
		return Err(format!(
			"only one CB chunk of at most {MAX_CHUNK} bytes is supported, got total={total}, chunk={chunk}"
		));
	}
	let used = 7 + chunk;
	if used > plain.len() {
		return Err("decrypted CB payload is shorter than its declared length".to_string());
	}
	if plain[used..].iter().any(|&b| b != 0) {
		return Err("decrypted CB has nonzero padding".to_string());
	}
	Ok((msg_type, command, plain[7..used].to_vec()))
}

fn aes_cbc_encrypt(session: &[u8; 32], iv: &[u8; 16], plain: &[u8]) -> Result<Vec<u8>> {
	if plain.len() % 16 != 0 {
		return Err("AES-CBC input is not block-aligned after zero padding".to_string());
	}
	let mut buffer = plain.to_vec();
	cbc::Encryptor::<Aes256>::new_from_slices(session, iv)
		.map_err(|e| format!("AES-CBC init failed: {e}"))?
		.encrypt_padded_mut::<NoPadding>(&mut buffer, plain.len())
		.map_err(|e| format!("AES-CBC encryption failed: {e}"))?;
	Ok(buffer)
}

fn aes_cbc_decrypt(session: &[u8; 32], iv: &[u8; 16], cipher: &[u8]) -> Result<Vec<u8>> {
	if cipher.len() % 16 != 0 {
		return Err("AES-CBC ciphertext is not block-aligned".to_string());
	}
	let mut buffer = cipher.to_vec();
	cbc::Decryptor::<Aes256>::new_from_slices(session, iv)
		.map_err(|e| format!("AES-CBC init failed: {e}"))?
		.decrypt_padded_mut::<NoPadding>(&mut buffer)
		.map_err(|e| format!("AES-CBC decryption failed: {e}"))?;
	Ok(buffer)
}

#[cfg(test)]
mod tests {
	use super::*;
	use rsa::pkcs1::EncodeRsaPrivateKey;
	use rsa::pkcs8::{EncodePrivateKey, LineEnding};

	#[test]
	fn hex_parser_rejects_bad_and_non_ascii_input_without_panicking() {
		assert_eq!(unhex("00aF").unwrap(), vec![0, 0xaf]);
		for bad in ["0", "0é0", "gg"] {
			assert!(unhex(bad).is_err(), "{bad:?}");
		}
	}

	#[test]
	fn aes_256_cbc_matches_nist_sp_800_38a() {
		let key: [u8; 32] = unhex("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4")
			.unwrap()
			.try_into()
			.unwrap();
		let iv: [u8; 16] = unhex("000102030405060708090a0b0c0d0e0f").unwrap().try_into().unwrap();
		let plain = unhex("6bc1bee22e409f96e93d7e117393172a").unwrap();
		let expected = unhex("f58c4c04d6e5f1ba779eabfb5f7bfbd6").unwrap();
		let cipher = aes_cbc_encrypt(&key, &iv, &plain).unwrap();
		assert_eq!(cipher, expected);
		assert_eq!(aes_cbc_decrypt(&key, &iv, &cipher).unwrap(), plain);
	}

	#[test]
	fn cb_roundtrip_and_an_emulated_45_reply() {
		let session: [u8; 32] = std::array::from_fn(|i| (i * 7) as u8);
		let challenge = shifted::challenge("EV_Demo.rod", 12345678);
		let request = encrypt_cb(&session, 3, COMMAND, &challenge).unwrap();
		assert_eq!(&request[..2], &[0xf3, 0xf3]);
		assert_eq!(request.len(), 2 + 48 + 1);
		let (msg_type, command, payload) = decrypt_cb(&session, &request).unwrap();
		assert_eq!((msg_type, command), (3, COMMAND));
		assert_eq!(payload, challenge);

		let mask = [0x5e, 0xe6, 0x10, 0x51, 0x77, 0x19, 0x40, 0x8c];
		let mut reply_payload = [0x55; 32];
		for i in 0..8 {
			reply_payload[24 + i] = mask[7 - i];
		}
		let reply = encrypt_cb(&session, 4, COMMAND, &reply_payload).unwrap();
		let (_, command, payload) = decrypt_cb(&session, &reply).unwrap();
		assert_eq!(command, COMMAND);
		assert_eq!(shifted::mask_from_reply(&payload), Some(mask));

		let mut bad_checksum = reply.clone();
		*bad_checksum.last_mut().unwrap() ^= 1;
		assert!(decrypt_cb(&session, &bad_checksum).unwrap_err().contains("checksum"));
		let mut bad_padding = reply;
		let at = bad_padding.len() - 18;
		bad_padding[at] ^= 1;
		let last = bad_padding.len() - 1;
		bad_padding[last] = xor_checksum(&bad_padding[..last]);
		assert!(decrypt_cb(&session, &bad_padding).unwrap_err().contains("padding"));
	}

	#[test]
	fn rsa_oaep_sha256_roundtrip_tamper_refusal_and_pem_formats() {
		let mut rng = rand::thread_rng();
		let private = RsaPrivateKey::new(&mut rng, 1024).unwrap();
		let session: [u8; 32] = std::array::from_fn(|i| (i as u8) ^ 0xa5);
		let wrapped = private.to_public_key().encrypt(&mut rng, Oaep::new::<Sha256>(), &session).unwrap();
		assert_eq!(wrapped.len(), 128);
		assert_eq!(unwrap_session_key(&private, &wrapped).unwrap(), session);
		let mut damaged = wrapped;
		damaged[17] ^= 1;
		assert!(unwrap_session_key(&private, &damaged).is_err());

		let pkcs1 = private.to_pkcs1_pem(LineEnding::LF).unwrap();
		let pkcs8 = private.to_pkcs8_pem(LineEnding::LF).unwrap();
		for pem in [pkcs1, pkcs8] {
			let parsed = parse_private_key(&pem).unwrap();
			assert_eq!(parsed.n(), private.n());
		}
	}
}
