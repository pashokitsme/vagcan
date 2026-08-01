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

/// Short unit numbers, as a table that can be replaced without a rebuild.
///
/// The numbers themselves (`01` engine, `02` gearbox, `17` instruments) are a
/// diagnostic-world convention, but **which CAN id each one is answered on is
/// not in any data file this project has found**: the label corpus carries the
/// numbers and the names, and no CAN id anywhere. So the pairing has to be
/// established per car by reading it, and the built-in list below is only the
/// part this project has verified on hardware:
///
/// * `01`/`02` — engine and gearbox, cross-checked against the car's Auto-Scan.
/// * `17` — the instrument cluster: a VCDS log names the unit it came from and
///   four of its identification fields match `0x714`'s answers byte for byte.
/// * `09`/`16` — central electrics and the steering column module, both opened
///   by VCDS in the capture where `0x70E` and `0x70C` identified themselves.
///
/// Everything else must be named by request id. A file at
/// [`OVERRIDE_PATH`] extends or replaces the list for another car, so a user
/// is never blocked on this source being edited.
const BUILT_IN_SHORT_NUMBERS: &[(u8, u16)] =
    &[(1, 0x7E0), (2, 0x7E1), (9, 0x70E), (16, 0x70C), (17, 0x714)];

/// Where a car's own number-to-id pairings are read from, when it has them:
/// a JSON object of `{"03": "713"}` — decimal-looking unit number to hex
/// request id, both as the user writes them.
pub const OVERRIDE_PATH: &str = "catalogs/unit-numbers.json";

/// Read the override file from the working directory or any parent of it, so
/// the tool behaves the same wherever it is run from.
fn read_override() -> std::io::Result<String> {
    let mut at = std::env::current_dir().ok();
    for _ in 0..6 {
        let Some(dir) = at else { break };
        let candidate = dir.join(OVERRIDE_PATH);
        if candidate.exists() {
            return std::fs::read_to_string(candidate);
        }
        at = dir.parent().map(|p| p.to_path_buf());
    }
    std::fs::read_to_string(OVERRIDE_PATH)
}

/// The pairings in force: the file's, then the built-in ones for anything the
/// file does not mention.
fn short_numbers() -> Vec<(u8, u16)> {
    let mut out: Vec<(u8, u16)> = Vec::new();
    if let Ok(text) = read_override() {
        match serde_json::from_str::<std::collections::BTreeMap<String, String>>(&text) {
            Ok(map) => {
                for (number, request) in map {
                    let number = number.trim_start_matches('0');
                    if let (Ok(n), Ok(id)) =
                        (number.parse::<u8>(), u16::from_str_radix(request.trim(), 16))
                    {
                        out.push((n, id));
                    }
                }
            }
            // A malformed override is worth saying out loud: silently falling
            // back would leave the user's own pairings quietly ignored.
            Err(e) => eprintln!("{OVERRIDE_PATH} is not readable ({e}) — using built-ins"),
        }
    }
    for (number, request) in BUILT_IN_SHORT_NUMBERS {
        if !out.iter().any(|(n, _)| n == number) {
            out.push((*number, *request));
        }
    }
    out
}

/// The request id a short unit number denotes, when there is an established
/// pairing for it.
pub fn request_for_short(number: u8) -> Option<u16> {
    short_numbers().into_iter().find(|(n, _)| *n == number).map(|(_, id)| id)
}

/// The short number for a request id, when there is one.
pub fn short_number(request: u16) -> Option<u8> {
    short_numbers().into_iter().find(|(_, id)| *id == request).map(|(n, _)| n)
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
            "control unit {number:02} has no known request id — give the id instead, \
             e.g. 713 (`vagcan units` lists what this car has), or add the pairing to \
             {OVERRIDE_PATH}"
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
    fn a_number_with_no_known_id_is_refused_rather_than_guessed() {
        // VW numbering would give an answer for 03 (brakes) and 19 (gateway).
        // Nothing in the corpus states which CAN id those answer on, so the
        // tool says so and points at the file where a user can record it.
        let err = parse("03").unwrap_err();
        assert!(err.contains("no known request id"), "{err}");
        assert!(err.contains(OVERRIDE_PATH), "{err}");
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
