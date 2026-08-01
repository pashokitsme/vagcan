//! Which CAN ids to talk to a given control unit on.
//!
//! This car answers diagnostics on two different id blocks, with two different
//! response rules, and a command that only knows the first can reach the engine
//! and the gearbox and nothing else:
//!
//! * the ISO 15765-4 block, `0x7E0..0x7E7`, whose response is request + 8 —
//!   engine and gearbox;
//! * VW's own block, `0x700..0x7BF`, whose response is request + `0x6A` — every
//!   other unit in the gateway's installation list.
//!
//! Both rules are established from captures of the reference car
//! (`research/other-ecus.md` §1): eight units were observed answering, each on
//! the id its rule predicts.
//!
//! The short numbers people use for units (`01` engine, `17` instruments) are a
//! VCDS convention, not something the car transmits. Only the ones this project
//! has evidence for are listed below; anything else has to be named by its
//! request id, which is honest rather than convenient.

/// A unit to address: the id we send on and the id it answers on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitAddress {
    pub request: u16,
    pub response: u16,
}

/// Lowest and highest request id of the ISO 15765-4 diagnostic block.
const ISO_FIRST: u16 = 0x7E0;
const ISO_LAST: u16 = 0x7E7;
/// The ISO block's response offset.
const ISO_OFFSET: u16 = 8;

/// VW's block and its response offset.
const VW_FIRST: u16 = 0x700;
const VW_LAST: u16 = 0x7BF;
const VW_OFFSET: u16 = 0x6A;

impl UnitAddress {
    /// The address to use for a request id, by whichever rule covers it.
    ///
    /// `None` for an id in neither block: there is no third rule to guess with.
    pub fn from_request(request: u16) -> Option<UnitAddress> {
        let response = match request {
            ISO_FIRST..=ISO_LAST => request + ISO_OFFSET,
            VW_FIRST..=VW_LAST => request + VW_OFFSET,
            _ => return None,
        };
        Some(UnitAddress { request, response })
    }

    /// How this unit is written on screen and on the command line: the short
    /// number when one is established, otherwise the request id.
    pub fn label(&self) -> String {
        match short_number(self.request) {
            Some(n) => format!("{n:02}"),
            None => format!("{:03X}", self.request),
        }
    }
}

/// Short unit numbers this project has evidence for, and nothing else.
///
/// * `01`/`02` — engine and gearbox, read live and cross-checked against the
///   car's own Auto-Scan.
/// * `17` — the instrument cluster: the VCDS log `LOG-17-…` names the unit it
///   was recorded from, and four of its identification fields match `0x714`'s
///   answers byte for byte (`research/other-ecus.md` §2).
/// * `09`/`16` — central electrics and the steering column module: VCDS opened
///   those two units in the capture where `0x70E` and `0x70C` answered their
///   identification blocks.
///
/// Deliberately absent: the gateway and everything unidentified. VW numbering
/// would supply a plausible answer for all of them, and a plausible answer is
/// exactly what this project keeps out of its tables.
const SHORT_NUMBERS: &[(u8, u16)] = &[(1, 0x7E0), (2, 0x7E1), (9, 0x70E), (16, 0x70C), (17, 0x714)];

/// The request id a short unit number denotes, when it is an established one.
pub fn request_for_short(number: u8) -> Option<u16> {
    SHORT_NUMBERS.iter().find(|(n, _)| *n == number).map(|(_, id)| *id)
}

/// The short number for a request id, when there is one.
pub fn short_number(request: u16) -> Option<u8> {
    SHORT_NUMBERS.iter().find(|(_, id)| *id == request).map(|(n, _)| *n)
}

/// Parse how a user names a unit: a short number (`01`, `17`) or a request id
/// (`714`, `7E0`).
///
/// Two-digit input is read as a short number and three-digit as a hex id, which
/// is unambiguous — every diagnostic request id on this car is three hex
/// digits, and no short number reaches three.
pub fn parse(text: &str) -> Result<UnitAddress, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("no control unit given".to_string());
    }
    if text.len() >= 3 {
        let id = u16::from_str_radix(text, 16)
            .map_err(|_| format!("{text:?} is not a hex request id like 714"))?;
        return UnitAddress::from_request(id).ok_or_else(|| {
            format!("{id:03X} is in neither diagnostic block (700-7BF or 7E0-7E7)")
        });
    }
    let number: u8 = text
        .trim_start_matches('0')
        .parse()
        .map_err(|_| format!("{text:?} is not a control-unit number like 01 or 17"))?;
    let request = request_for_short(number).ok_or_else(|| {
        format!(
            "control unit {number:02} has no established address in this project — \
             give its request id instead, e.g. 713 (see `vagcan units`)"
        )
    })?;
    Ok(UnitAddress { request, response: request + if request >= ISO_FIRST { ISO_OFFSET } else { VW_OFFSET } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_block_uses_its_own_response_rule() {
        // Both observed on the car: 7E0 answers on 7E8, 714 answers on 77E.
        assert_eq!(UnitAddress::from_request(0x7E0).unwrap().response, 0x7E8);
        assert_eq!(UnitAddress::from_request(0x7E1).unwrap().response, 0x7E9);
        assert_eq!(UnitAddress::from_request(0x714).unwrap().response, 0x77E);
        assert_eq!(UnitAddress::from_request(0x70E).unwrap().response, 0x778);
        assert_eq!(UnitAddress::from_request(0x773).unwrap().response, 0x7DD);
    }

    #[test]
    fn an_id_in_neither_block_has_no_address() {
        // No third rule exists, so guessing one would invent traffic.
        assert!(UnitAddress::from_request(0x123).is_none());
        assert!(UnitAddress::from_request(0x7F0).is_none());
    }

    #[test]
    fn the_cluster_is_not_addressed_as_an_iso_unit() {
        // The bug this module exists to stop: treating `17` as an index into
        // the ISO block gives 0x7F0, which nothing on this car answers.
        let cluster = parse("17").unwrap();
        assert_eq!(cluster.request, 0x714);
        assert_ne!(cluster.request, 0x7E0 + 16);
    }

    #[test]
    fn a_unit_may_be_named_by_short_number_or_by_request_id() {
        assert_eq!(parse("01").unwrap().request, 0x7E0);
        assert_eq!(parse("2").unwrap().request, 0x7E1);
        assert_eq!(parse("714").unwrap(), parse("17").unwrap());
        assert_eq!(parse("713").unwrap().request, 0x713);
        assert_eq!(parse("7E1").unwrap().request, 0x7E1);
    }

    #[test]
    fn an_unproven_short_number_is_refused_rather_than_guessed() {
        // VW numbering would give an answer for 03 (brakes) and 19 (gateway).
        // This project has not verified either, so it says so instead.
        let err = parse("03").unwrap_err();
        assert!(err.contains("no established address"), "{err}");
        assert!(parse("19").is_err());
        assert!(parse("zz").is_err());
    }

    #[test]
    fn a_unit_is_labelled_by_number_when_known_and_by_id_when_not() {
        assert_eq!(UnitAddress::from_request(0x7E0).unwrap().label(), "01");
        assert_eq!(UnitAddress::from_request(0x714).unwrap().label(), "17");
        assert_eq!(UnitAddress::from_request(0x713).unwrap().label(), "713");
    }
}
