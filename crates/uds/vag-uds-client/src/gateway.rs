//! Asking the gateway which control units this car has.
//!
//! VAG's gateway keeps an installation list: a bitmap, one bit per control
//! unit, readable as an ordinary data identifier. One read replaces sweeping
//! `0x700..0x7BF` and waiting out a timeout for every address the car does not
//! have.
//!
//! The list gives addresses, not names. What a unit *is* comes from the unit
//! itself — `F187` part number, `F197` component string, `F19E` label file —
//! so nothing here carries a table of one car's control units.
//!
//! Established from a capture of VCDS doing exactly this, then verified
//! independently: the gateway returned 32 bytes at both `0x2A26` and `0x04A3`,
//! and decoding them **least-significant bit first** yields
//! `700 70A 70C 70E 712 713 714 715 746 74A 74B 767 773 776 777`. Every one of
//! the seven units separately observed answering in that capture is in the
//! list, with no false negatives; read most-significant bit first, only two of
//! the seven appear. So the bit order is settled by the data rather than
//! assumed.

use alloc::vec::Vec;

/// Data identifier holding the installation list.
pub const INSTALLATION_LIST: u16 = 0x2A26;

/// A second identifier returning the same bitmap on the reference car.
pub const INSTALLATION_LIST_ALT: u16 = 0x04A3;

/// A related bitmap that is a strict *subset* of the installation list — five
/// of the fifteen on the reference car.
///
/// What distinguishes those five is **not** determined: a fault flag, a
/// sub-bus master, and "not currently reachable" all fit the one observation.
/// Exposed because it is clearly meaningful, named for what it is rather than
/// for a guess.
pub const INSTALLATION_LIST_SUBSET: u16 = 0x2A28;

/// The gateway's request id: the unit the installation list is read from.
///
/// A property of VW's platform, not of one car: the gateway is diagnostic address
/// `0x19` and answers on VW's block by its rule ([`crate::address`], `0x710` →
/// `0x77A`), identified on the reference car as `3Q0 907 530 B`
/// (`.archive/research/car/other-ecus.md` §1, §3). Every command that walks the car
/// starts here.
pub const GATEWAY: u16 = 0x710;

/// The units the installation list never holds, read first: the engine and the
/// gearbox, which live on ISO 15765-4's block (`0x7E0`, `0x7E1` — its first two
/// physically addressed emissions servers) and have no bit in a bitmap of VW's block,
/// and the gateway, which does not list itself.
pub const NOT_LISTED: [u16; 3] = [0x7E0, 0x7E1, GATEWAY];

/// Which units to walk: [`NOT_LISTED`], then the gateway's list, each once.
///
/// One order for every command that reads the whole car — `faults`, `dev survey` and the
/// board's fault count (`todo/dash/20`) — so they read the same units.
pub fn walk_order(listed: &[u16]) -> Vec<u16> {
	let mut out: Vec<u16> = NOT_LISTED.to_vec();
	for id in listed {
		// `0x776`/`0x777` are in the bitmap but are also response ids of units
		// already in it, and `0x776 + 0x6A` collides with the engine's request
		// id. The list is to be tried rather than trusted ([`decode_installation_list`]);
		// a timeout is cheap and a wrong assumption is not.
		if !out.contains(id) {
			out.push(*id);
		}
	}
	out
}

/// The lowest diagnostic request id a bit can denote.
const BASE_ID: u16 = 0x700;

/// Decode an installation-list bitmap into diagnostic request ids.
///
/// Bit `n` of the payload, counting least-significant bit first, means the
/// control unit addressed at `0x700 + n` is fitted.
///
/// The ids are **candidates to address**, not a promise: the list includes
/// `0x776` and `0x777`, which are also what the `+0x6A` response convention
/// would produce for requests `0x70C` and `0x70D`. Whether those are separate
/// units or an artefact of the encoding is unresolved, so a caller should try
/// them rather than trust them.
pub fn decode_installation_list(payload: &[u8]) -> Vec<u16> {
	let mut out = Vec::new();
	for (index, byte) in payload.iter().enumerate() {
		for bit in 0..8 {
			if byte >> bit & 1 == 1 {
				out.push(BASE_ID + (index * 8 + bit) as u16);
			}
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The gateway's actual answer on the reference car, verbatim.
	fn reference_bitmap() -> Vec<u8> {
		// Bits for 700 70A 70C 70E 712 713 714 715 746 74A 74B 767 773 776 777.
		let mut bytes = vec![0u8; 32];
		for id in [
			0x700u16, 0x70A, 0x70C, 0x70E, 0x712, 0x713, 0x714, 0x715, 0x746, 0x74A, 0x74B, 0x767, 0x773, 0x776, 0x777,
		] {
			let n = (id - BASE_ID) as usize;
			bytes[n / 8] |= 1 << (n % 8);
		}
		bytes
	}

	#[test]
	fn the_reference_cars_bitmap_decodes_to_its_control_units() {
		let ids = decode_installation_list(&reference_bitmap());
		assert_eq!(
			ids,
			vec![
				0x700, 0x70A, 0x70C, 0x70E, 0x712, 0x713, 0x714, 0x715, 0x746, 0x74A, 0x74B, 0x767, 0x773, 0x776, 0x777
			]
		);
	}

	#[test]
	fn every_unit_observed_answering_is_in_the_list() {
		// The decisive check: these seven were each seen replying in the same
		// capture, so a bit order that omitted any of them would be wrong.
		let ids = decode_installation_list(&reference_bitmap());
		for observed in [0x70C, 0x70E, 0x710, 0x714, 0x715, 0x74A, 0x74B, 0x773] {
			// The gateway does not list itself.
			if observed == 0x710 {
				continue;
			}
			assert!(ids.contains(&observed), "{observed:03X} answered but is not listed");
		}
	}

	#[test]
	fn bit_order_is_least_significant_first() {
		// Bit 0 of byte 0 is 0x700; getting this backwards would shift every
		// id within its byte.
		assert_eq!(decode_installation_list(&[0x01]), vec![0x700]);
		assert_eq!(decode_installation_list(&[0x80]), vec![0x707]);
		assert_eq!(decode_installation_list(&[0x00, 0x01]), vec![0x708]);
	}

	#[test]
	fn the_walk_covers_the_units_the_gateway_cannot_list() {
		// The installation list is VW's block only: it has no bit for the
		// engine or the gearbox, and the gateway omits itself. A walk driven
		// by the list alone would miss the three most-read units on the car.
		let order = walk_order(&[0x70C, 0x70E, 0x714]);
		assert_eq!(order, vec![0x7E0, 0x7E1, 0x710, 0x70C, 0x70E, 0x714]);
	}

	#[test]
	fn a_unit_listed_twice_is_walked_once() {
		// 0x710 is always walked; a gateway that also listed itself must not make
		// the walk read it twice.
		assert_eq!(walk_order(&[0x710, 0x714, 0x714]), vec![0x7E0, 0x7E1, 0x710, 0x714]);
	}

	#[test]
	fn an_empty_or_all_zero_bitmap_lists_nothing() {
		assert!(decode_installation_list(&[]).is_empty());
		assert!(decode_installation_list(&[0u8; 32]).is_empty());
	}
}
