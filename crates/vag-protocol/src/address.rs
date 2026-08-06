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
//! (`research/car/other-ecus.md` §1): eight units were observed answering, each on
//! the id its rule predicts.
//!
//! The short numbers people use for units (`01` engine, `17` instruments) are a
//! VCDS convention, not something the car transmits, and they are **hex bytes**
//! — the label files write them `(#17)`, `(#4B)`, `(#86)` — so they are parsed and
//! printed as hex here. Which request id each one is answered on is not a
//! property of the protocol, so this module does not decide it: it accepts a
//! table from whoever can establish it ([`install`]) and keeps a small built-in
//! list as a fallback for a run that has no other source.

use std::sync::PoisonError;

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

	/// Whether this unit is one of the **emissions-related** control units that
	/// ISO 15765-4 addresses — and therefore one on which the legislated
	/// OBD-II parameter set, mirrored at `F400 + PID`, is required to exist and
	/// to mean what SAE J1979 says it means.
	///
	/// This is a property of the protocol, not of a car: ISO 15765-4 reserves
	/// `0x7E0..0x7E7` for the physically-addressed emissions-related servers
	/// (`0x7DF` being their functional address), and it is those servers that
	/// legislation obliges to answer mode 01. A unit on VW's own `0x700..0x7BF`
	/// block is outside that obligation, so nothing says its `F4xx`
	/// identifiers are the standard's.
	///
	/// The reference car shows they are not. Its climate unit (`0x746`,
	/// `5E0907044AM`) answers `F405` with 87 / 90 / 109 at three moments where
	/// the engine's own `F405` reads 129 / 93 / 137: fitting a line through the
	/// first two pairs predicts −135 for the third, and between the first two
	/// the engine's coolant fell 36 °C while the climate value *rose* by 3. No
	/// conversion carries one to the other. The same unit answers `F40C` with
	/// one byte where J1979 defines PID `0C` as two.
	pub fn is_emissions_related(&self) -> bool {
		(ISO_FIRST..=ISO_LAST).contains(&self.request)
	}

	/// How this unit is written on screen and on the command line: the short
	/// number when one is established, otherwise the request id.
	pub fn label(&self) -> String {
		match short_number(self.request) {
			Some(n) => format!("{n:02X}"),
			None => format!("{:03X}", self.request),
		}
	}
}

/// One "short number ↔ request id" pairing, from whichever source established
/// it.
///
/// Both halves are optional-ish on purpose: label files can say that `44` is
/// `J500 - Power Steering` without saying which CAN id answers it, and an
/// override file can say which id answers `44` without naming it. The tiers are
/// merged field by field, so a run gets the id from whoever knows the id and
/// the name from whoever knows the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitNumber {
	/// The number as the diagnostic world writes it — a **hex** byte: `01`
	/// engine, `17` instruments, `4B` a multi-function module.
	pub number: u8,
	/// The CAN request id it is answered on, when something has established
	/// that. Nothing derives it from `number`: the two numberings are
	/// unrelated (`17` answers on `0x714`, `19` on `0x710`).
	pub request: Option<u16>,
	/// What the label files calls the unit, when label files said.
	pub name: Option<String>,
}

/// Pairings installed by the program at run time — see [`install`].
///
/// A process-wide table rather than an argument because the callers that need
/// it have nowhere to put one: [`UnitAddress::label`] takes no context, and
/// [`parse`] runs while the command line is still being read. This module
/// already resolves a process-wide external source ([`OVERRIDE_PATH`]) from
/// inside those same free functions, so the installed table follows the shape
/// that is there.
///
/// Nothing about label files reaches this module: the caller does the
/// label files work and hands in plain numbers and strings.
static INSTALLED: std::sync::RwLock<Vec<UnitNumber>> = std::sync::RwLock::new(Vec::new());

/// Install pairings learned at run time — from the label_files, and from what
/// the car itself answered.
///
/// Merged into whatever is already installed rather than replacing it, so a
/// command can install the label files' numbering up front and add the request ids
/// it learns from the car as it goes. **First writer of a field wins**: a name
/// or an id already established is not silently overwritten by a later,
/// vaguer source.
pub fn install(pairings: impl IntoIterator<Item = UnitNumber>) {
	let mut installed = INSTALLED.write().unwrap_or_else(PoisonError::into_inner);
	for entry in pairings {
		match installed.iter_mut().find(|e| e.number == entry.number) {
			Some(existing) => {
				if existing.request.is_none() {
					existing.request = entry.request;
				}
				if existing.name.is_none() {
					existing.name = entry.name;
				}
			}
			None => installed.push(entry),
		}
	}
}

/// **The fallback, not the table.** The pairings this project has verified on
/// hardware, kept so a run with no label files and no override file still reaches
/// the units it always could.
///
/// The numbers are hex, as the label files write them:
///
/// * `01`/`02` — engine and gearbox, cross-checked against the car's Auto-Scan.
/// * `17` — the instrument cluster: a VCDS log names the unit it came from and
///   four of its identification fields match `0x714`'s answers byte for byte.
/// * `09`/`16` — central electrics and the steering column module, both opened
///   by VCDS in the capture where `0x70E` and `0x70C` identified themselves.
///
/// Everything else comes from data at run time. **Which CAN id a number is
/// answered on is in no data file this project has found** — the label files
/// carries the numbers and the names and no CAN id anywhere, and the two
/// numberings are genuinely unrelated (VCDS's `17` answers on `0x714`, whose
/// own UDS address is `0x14`; VCDS's `19` answers on `0x710`, address `0x10` —
/// `research/car/other-ecus.md` §3). So the id half is established either by
/// reading the car through the label files (`vagcan units --identify --labels`, which
/// asks each id for its part number and asks the label files whose part number that
/// is) or by a user writing it down in [`OVERRIDE_PATH`].
const BUILT_IN_SHORT_NUMBERS: &[(u8, u16)] = &[(0x01, 0x7E0), (0x02, 0x7E1), (0x09, 0x70E), (0x16, 0x70C), (0x17, 0x714)];

/// Where a car's own number-to-id pairings are read from, when it has them:
/// a JSON object of `{"03": "713"}` — decimal-looking unit number to hex
/// request id, both as the user writes them.
///
/// Under the tool's own directory, not the working one. It used to be looked
/// for in the checkout and every parent of it, which meant the same command
/// behaved differently depending on where the shell was standing, and put a
/// file describing somebody's car inside a repository.
pub const OVERRIDE_PATH: &str = ".vagcan/data/measured/unit-numbers.json";

/// Read the override file, when the user has written one.
///
/// The home directory is resolved from the environment rather than through a
/// crate: this layer has no business growing a dependency to find one path, and
/// a machine with no `HOME` simply has no override.
fn read_override() -> std::io::Result<String> {
	let home = std::env::var_os("HOME").ok_or_else(|| std::io::Error::other("no HOME, so no override file"))?;
	std::fs::read_to_string(std::path::Path::new(&home).join(OVERRIDE_PATH))
}

/// The pairings [`OVERRIDE_PATH`] states, or nothing when there is no such file.
fn override_pairings() -> Vec<UnitNumber> {
	let Ok(text) = read_override() else {
		return Vec::new();
	};
	match serde_json::from_str::<std::collections::BTreeMap<String, String>>(&text) {
		Ok(map) => map
			.into_iter()
			.filter_map(|(number, request)| {
				// Both halves as the user writes them: a hex unit number and a
				// hex request id.
				let number = u8::from_str_radix(number.trim(), 16).ok()?;
				let request = u16::from_str_radix(request.trim(), 16).ok()?;
				Some(UnitNumber {
					number,
					request: Some(request),
					name: None,
				})
			})
			.collect(),
		// A malformed override is worth saying out loud: silently falling
		// back would leave the user's own pairings quietly ignored.
		Err(e) => {
			eprintln!("{OVERRIDE_PATH} is not readable ({e}) — using built-ins");
			Vec::new()
		}
	}
}

/// The built-in fallback as pairings.
fn built_in_pairings() -> Vec<UnitNumber> {
	BUILT_IN_SHORT_NUMBERS
		.iter()
		.map(|&(number, request)| UnitNumber {
			number,
			request: Some(request),
			name: None,
		})
		.collect()
}

/// Merge the tiers, field by field: for each number, the request id in force is
/// the earliest tier that states one, and the name likewise.
///
/// Field by field rather than whole entries, so label files that names `17`
/// without knowing its CAN id adds the name without hiding the built-in id.
fn pairings_in_force(tiers: &[&[UnitNumber]]) -> Vec<UnitNumber> {
	let mut out: Vec<UnitNumber> = Vec::new();
	for tier in tiers {
		for entry in *tier {
			match out.iter_mut().find(|e| e.number == entry.number) {
				Some(existing) => {
					existing.request = existing.request.or(entry.request);
					if existing.name.is_none() {
						existing.name = entry.name.clone();
					}
				}
				None => out.push(entry.clone()),
			}
		}
	}
	out
}

/// The pairings in force, in precedence order:
///
/// 1. **the override file** ([`OVERRIDE_PATH`]) — a pairing someone has
///    evidence for and wrote down; it beats everything, including label files
///    that disagrees;
/// 2. **the label files and the car** — whatever [`install`] was handed at startup;
/// 3. **the built-in fallback** — the five pairings proven on the reference
///    car, so a run with neither of the above still works as it always did.
fn short_numbers() -> Vec<UnitNumber> {
	// Borrow the installed list under the read guard rather than cloning it:
	// `pairings_in_force` only reads its inputs, and neither `override_pairings`
	// nor `built_in_pairings` touches `INSTALLED`, so holding the read lock here
	// cannot deadlock against the one writer (`install`).
	let installed = INSTALLED.read().unwrap_or_else(PoisonError::into_inner);
	pairings_in_force(&[&override_pairings(), &installed, &built_in_pairings()])
}

/// The request id a short unit number denotes, when there is an established
/// pairing for it.
pub fn request_for_short(number: u8) -> Option<u16> {
	short_numbers().into_iter().find(|e| e.number == number).and_then(|e| e.request)
}

/// The short number for a request id, when there is one.
pub fn short_number(request: u16) -> Option<u8> {
	short_numbers().into_iter().find(|e| e.request == Some(request)).map(|e| e.number)
}

/// What the label files calls a unit number, when label files said. Used to make a
/// refusal name the unit it is refusing.
pub fn name_for_short(number: u8) -> Option<String> {
	short_numbers().into_iter().find(|e| e.number == number).and_then(|e| e.name)
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
		let id = u16::from_str_radix(text, 16).map_err(|_| format!("{text:?} is not a hex request id like 714"))?;
		return UnitAddress::from_request(id).ok_or_else(|| format!("{id:03X} is in neither diagnostic block (700-7BF or 7E0-7E7)"));
	}
	// A short number is a hex byte, the way the label files write it: `4B` is a
	// unit, not a typo.
	let number = u8::from_str_radix(text, 16).map_err(|_| format!("{text:?} is not a control-unit number like 01 or 17"))?;
	let named = name_for_short(number).map(|n| format!(" ({n})")).unwrap_or_default();
	let request = request_for_short(number).ok_or_else(|| {
		format!(
			"control unit {number:02X}{named} has no known request id — give the id instead, \
             e.g. 713 (`vagcan units` lists what this car has), or add the pairing to \
             {OVERRIDE_PATH}"
		)
	})?;
	// The id comes from data now, so it gets the same check a user-typed id
	// gets: a pairing pointing outside both blocks is refused, not extrapolated
	// into traffic no rule predicts.
	UnitAddress::from_request(request).ok_or_else(|| {
		format!(
			"control unit {number:02X} is paired with {request:03X}, which is in neither \
             diagnostic block (700-7BF or 7E0-7E7)"
		)
	})
}

/// Parse a comma-separated list of units, e.g. `01,713,70E`.
///
/// Every token is checked, and one bad token fails the whole list: the callers
/// (`faults --ecu`, `survey --only`) use this to validate before they open the
/// adapter, which is a single-user resource — opening it and then failing on a
/// typo leaves the port held while the user retypes.
pub fn parse_list(spec: &str) -> Result<Vec<UnitAddress>, String> {
	let units: Vec<UnitAddress> = spec
		.split(',')
		.map(str::trim)
		.filter(|token| !token.is_empty())
		.map(parse)
		.collect::<Result<_, _>>()?;
	if units.is_empty() {
		return Err("no control unit given".to_string());
	}
	Ok(units)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_list_is_parsed_whole_or_not_at_all() {
		let units = parse_list("01, 713,70E").unwrap();
		assert_eq!(units.iter().map(|u| u.request).collect::<Vec<_>>(), vec![0x7E0, 0x713, 0x70E]);

		// One bad token fails the list — the caller is about to open the
		// adapter on the strength of this having parsed.
		assert!(parse_list("01,zz").is_err());
		// Nothing at all is not an empty selection; it is a mistake.
		assert!(parse_list("").is_err());
		assert!(parse_list(" , ").is_err());
	}

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
		// Nothing in the label files states which CAN id those answer on, so the
		// tool says so and points at the file where a user can record it.
		let err = parse("03").unwrap_err();
		assert!(err.contains("no known request id"), "{err}");
		assert!(err.contains(OVERRIDE_PATH), "{err}");
		assert!(parse("19").is_err());
		assert!(parse("zz").is_err());
	}

	/// A pairing as a tier states it.
	fn pairing(number: u8, request: Option<u16>, name: Option<&str>) -> UnitNumber {
		UnitNumber {
			number,
			request,
			name: name.map(str::to_string),
		}
	}

	#[test]
	fn the_built_in_list_is_the_fallback_when_nothing_is_installed() {
		// No label_files, no override: the five pairings proven on hardware still
		// answer, so a run on a machine with no VCDS installation reaches the
		// units it always could.
		let built_in = built_in_pairings();
		let in_force = pairings_in_force(&[&built_in]);
		for (number, request) in [(0x01, 0x7E0), (0x02, 0x7E1), (0x09, 0x70E), (0x16, 0x70C), (0x17, 0x714)] {
			let found = in_force.iter().find(|e| e.number == number).expect("built in");
			assert_eq!(found.request, Some(request), "{number:02X}");
		}
	}

	#[test]
	fn the_tiers_are_override_then_label_files_then_built_in() {
		// 01: all three tiers claim it, and the override's id is the one that
		//     survives — someone wrote it down because they had evidence.
		// 17: the label files name it but does not know its id; the built-in id
		//     must still come through, so a name never costs an address.
		// 44: only the label files have it — the built-in list is not the source of
		//     truth any more.
		// 86: only the label_files, and only a name: a number with no id at all.
		let over = vec![pairing(0x01, Some(0x7E2), None)];
		let label_files = vec![
			pairing(0x01, Some(0x70A), Some("label files engine")),
			pairing(0x17, None, Some("J285 - Instrument Cluster")),
			pairing(0x44, Some(0x712), Some("J500 - Power Steering")),
			pairing(0x86, None, Some("R - Radio")),
		];
		let in_force = pairings_in_force(&[&over, &label_files, &built_in_pairings()]);
		let at = |n: u8| in_force.iter().find(|e| e.number == n).cloned().expect("present");

		assert_eq!(at(0x01).request, Some(0x7E2), "the override outranks the label files");
		assert_eq!(at(0x01).name.as_deref(), Some("label files engine"), "and still takes its name");
		assert_eq!(at(0x17).request, Some(0x714), "the built-in id survives label files name");
		assert_eq!(at(0x17).name.as_deref(), Some("J285 - Instrument Cluster"));
		assert_eq!(at(0x44).request, Some(0x712), "the label files outranks the absent built-in");
		assert_eq!(at(0x86).request, None, "a name is not an address");
	}

	#[test]
	fn an_installed_pairing_resolves_a_number_no_built_in_knows() {
		// The whole point: 44 is not in the built-in list and never will be —
		// it comes from the label_files, which knows about a hundred numbers this
		// code does not. 55 is installed pointing outside both blocks, and has
		// to be refused rather than turned into traffic no rule predicts.
		install([
			pairing(0x44, Some(0x712), Some("J500 - Power Steering")),
			pairing(0x55, Some(0x123), None),
		]);

		let eps = parse("44").expect("an installed pairing resolves");
		assert_eq!(eps.request, 0x712);
		assert_eq!(eps.response, 0x712 + VW_OFFSET);
		assert_eq!(UnitAddress::from_request(0x712).unwrap().label(), "44");
		assert_eq!(name_for_short(0x44).as_deref(), Some("J500 - Power Steering"));

		let err = parse("55").unwrap_err();
		assert!(err.contains("neither diagnostic block"), "{err}");

		// Installing does not disturb the fallback.
		assert_eq!(parse("01").unwrap().request, 0x7E0);
		assert_eq!(parse("17").unwrap().request, 0x714);
	}

	#[test]
	fn a_short_number_is_a_hex_byte_as_the_label_files_write_it() {
		// The label files write `(#4B)`, and VCDS calls that unit `4B`. Read as
		// decimal it is not a number at all, and `17` would be 11 hex — a
		// different unit.
		assert_eq!(u8::from_str_radix("4B", 16), Ok(0x4B));
		install([pairing(0x4B, Some(0x74B), Some("J608 - Multifunction Module"))]);
		assert_eq!(parse("4B").unwrap().request, 0x74B);
		assert_eq!(parse("4b").unwrap().request, 0x74B);
	}

	#[test]
	fn this_module_knows_nothing_about_label_files() {
		// The seam, asserted rather than remembered. The crate depends on
		// `vag-data` for measurement catalogs (`read.rs`), so a stray `use`
		// here would compile — and would put "what a label file is" inside the
		// protocol layer. Everything the label files know arrives through
		// [`install`], as numbers and strings.
		let source = include_str!("address.rs");
		let module = source.split("#[cfg(test)]").next().expect("the module body");
		assert!(!module.contains("vag_data"), "address.rs must not reach into vag-data");
		assert!(!module.contains("LabelDb"), "address.rs must not know what label files are");
	}

	#[test]
	fn a_unit_is_labelled_by_number_when_known_and_by_id_when_not() {
		assert_eq!(UnitAddress::from_request(0x7E0).unwrap().label(), "01");
		assert_eq!(UnitAddress::from_request(0x714).unwrap().label(), "17");
		assert_eq!(UnitAddress::from_request(0x713).unwrap().label(), "713");
	}
}
