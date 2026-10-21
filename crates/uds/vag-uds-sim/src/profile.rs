//! The car a sim plays, as a person writes it: a TOML file (`docs/virtual-car.md`).
//!
//! A profile names control units by their request id and says what each answers. It holds
//! no rule of its own: the id a unit answers on comes from `vag_uds_client::address`, the
//! installation list's identifier from `vag_uds_client::gateway`, and everything a unit
//! says is bytes the profile spells out. What is not in the profile is not on the car — an
//! identifier it lacks is refused (NRC `0x31`) and a unit it lacks says nothing.
//!
//! [`Profile`] is the file as written, kept so a builder (`--from`) can write one back;
//! [`Profile::resolve`] checks it and turns it into the [`Car`] model the engine runs.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use vag_uds_client::UnitAddress;

/// Bytes, written as hex: `"0F A0"`, `"0FA0"` and `"0f a0"` are the same two bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hex(pub Vec<u8>);

impl Hex {
	pub fn parse(text: &str) -> Result<Hex, String> {
		let digits: String = text.chars().filter(|c| !c.is_whitespace()).collect();
		if digits.len() % 2 != 0 {
			return Err(format!("{text:?} is not whole bytes of hex: an odd number of digits"));
		}
		(0..digits.len())
			.step_by(2)
			.map(|at| u8::from_str_radix(&digits[at..at + 2], 16).map_err(|_| format!("{text:?} is not hex")))
			.collect::<Result<Vec<u8>, String>>()
			.map(Hex)
	}
}

impl fmt::Display for Hex {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		for (at, byte) in self.0.iter().enumerate() {
			if at > 0 {
				f.write_str(" ")?;
			}
			write!(f, "{byte:02X}")?;
		}
		Ok(())
	}
}

impl Serialize for Hex {
	fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
		s.serialize_str(&self.to_string())
	}
}

impl<'de> Deserialize<'de> for Hex {
	fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Hex, D::Error> {
		let text = String::deserialize(d)?;
		Hex::parse(&text).map_err(serde::de::Error::custom)
	}
}

/// A CAN request id, written as hex text (`"7E0"`) or as a TOML integer (`0x7E0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Id(pub u16);

impl Serialize for Id {
	fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
		s.serialize_str(&format!("{:03X}", self.0))
	}
}

impl<'de> Deserialize<'de> for Id {
	fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Id, D::Error> {
		#[derive(Deserialize)]
		#[serde(untagged)]
		enum Written {
			Text(String),
			Number(u16),
		}
		match Written::deserialize(d)? {
			Written::Number(n) => Ok(Id(n)),
			Written::Text(text) => {
				let digits = text.trim().trim_start_matches("0x").trim_start_matches("0X");
				u16::from_str_radix(digits, 16)
					.map(Id)
					.map_err(|_| serde::de::Error::custom(format!("{text:?} is not a CAN id in hex")))
			}
		}
	}
}

/// The file as written.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
	/// What this car is, for the person reading the file. Not used.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub about: Option<String>,
	#[serde(default, skip_serializing_if = "BusSettings::is_default")]
	pub bus: BusSettings,
	#[serde(default, rename = "unit", skip_serializing_if = "Vec::is_empty")]
	pub units: Vec<UnitSpec>,
}

/// How the sim's ISO-TP side frames what it sends.
/// Every field defaults to zero: padding `00`, all frames in one block, no gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BusSettings {
	/// The byte a frame shorter than eight is padded with.
	pub padding: u8,
	/// The block size the sim's flow control asks for when the tester sends a long
	/// request: `0` is all of it at once.
	pub block_size: u8,
	/// The separation time (ISO 15765-2 STmin byte) the sim's flow control asks for.
	pub st_min: u8,
}

impl BusSettings {
	fn is_default(&self) -> bool {
		*self == BusSettings::default()
	}
}

/// One control unit.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitSpec {
	/// The id the tester asks it on. The id it answers on follows from the addressing rule.
	pub request: Id,
	/// A name for the sim's own log. Not on the bus.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub name: Option<String>,
	/// `F187`, the spare part number, as text.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub part: Option<String>,
	/// `F197`, the system name, as text.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub component: Option<String>,
	/// `F19E`, the ODX file name, as text.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub odx: Option<String>,
	/// `F1A2`, the ODX file version, as text.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub odx_version: Option<String>,
	/// `F190`, the VIN, as text.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub vin: Option<String>,
	/// The units this one lists as fitted: the gateway's installation list, answered at
	/// `vag_uds_client::gateway::INSTALLATION_LIST` as the bitmap that decodes to them.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub installed: Vec<Id>,
	/// Milliseconds before every answer.
	#[serde(default, skip_serializing_if = "is_zero")]
	pub delay_ms: u64,
	/// In the profile and never answers: a unit the gateway lists that is not there.
	#[serde(default, skip_serializing_if = "is_false")]
	pub silent: bool,
	/// Answers this many requests in a session, then nothing more.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub quiet_after: Option<u32>,
	/// Answers for this long after the channel opens, then nothing more.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub quiet_after_ms: Option<u64>,
	/// The diagnostic sessions `10` accepts. Default: default (`01`) and extended (`03`).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub sessions: Option<Vec<u8>>,
	/// The status availability mask a `19` answer carries. Default `FF`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub dtc_availability: Option<u8>,
	/// Identifiers, by their hex number: a value, a text, a series, or a refusal.
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub did: BTreeMap<String, DidSpec>,
	/// Stored fault codes, as `19 02` lists them.
	#[serde(default, rename = "fault", skip_serializing_if = "Vec::is_empty")]
	pub faults: Vec<FaultSpec>,
	/// Codes the unit can report beyond the stored ones, for `19 0A`, with status `00`.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub supported: Vec<Hex>,
}

fn is_zero(n: &u64) -> bool {
	*n == 0
}

fn is_false(b: &bool) -> bool {
	!*b
}

/// One identifier: `F189 = "30 30 31 30"` is a value; the table form says more.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DidSpec {
	Value(Hex),
	Full(DidTable),
}

/// The table form of an identifier. Exactly one of `value`, `text` and `series` says what
/// it reads, or `nrc` alone says it is refused.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DidTable {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub value: Option<Hex>,
	/// The value as text, sent as its bytes.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub text: Option<String>,
	/// The value over time from the channel opening: each point holds until the next.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub series: Option<Vec<Point>>,
	/// The series starts over after this many milliseconds.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub loop_ms: Option<u64>,
	/// Refuse every read with this negative response code: `0x22` conditions not correct,
	/// `0x31` out of range, `0x33` security access denied.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub nrc: Option<u8>,
	/// Answer `7F 22 78` (response pending) this many times before the answer.
	#[serde(default, skip_serializing_if = "is_zero_u32")]
	pub pending: u32,
	/// Milliseconds between two pending answers, and from the last to the answer.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub pending_ms: Option<u64>,
	/// Milliseconds before the answer, on top of the unit's own.
	#[serde(default, skip_serializing_if = "is_zero")]
	pub delay_ms: u64,
	/// Answer this many reads in a session, then nothing at all to a read of it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub silent_after: Option<u32>,
}

fn is_zero_u32(n: &u32) -> bool {
	*n == 0
}

/// One point of a series.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Point {
	pub at_ms: u64,
	pub value: Hex,
}

/// One stored fault code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaultSpec {
	/// The three bytes of the code: `"00 01 29"`.
	pub code: Hex,
	/// Its status byte (ISO 14229-1 D.2): `0x09` is test failed and confirmed.
	pub status: u8,
	/// What `19 06 <code> FF` answers after the status byte: each record's number, then its data.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub extended: Option<Hex>,
	/// What `19 04 <code> FF` answers after the status byte: each snapshot's number, its
	/// identifier count, and the identifiers with their data.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub snapshot: Option<Hex>,
}

/// What is wrong with a profile, said where it is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileError(pub String);

impl fmt::Display for ProfileError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.0)
	}
}

impl std::error::Error for ProfileError {}

impl Profile {
	/// Read a profile from its text.
	pub fn parse(text: &str) -> Result<Profile, ProfileError> {
		toml::from_str(text).map_err(|e| ProfileError(e.to_string()))
	}

	/// The profile as TOML, as `--from` writes it.
	pub fn to_toml(&self) -> String {
		toml::to_string(self).expect("a profile always serialises")
	}

	/// Check the profile and build the model the engine runs.
	pub fn resolve(&self) -> Result<Car, ProfileError> {
		let mut units: Vec<Unit> = Vec::new();
		for spec in &self.units {
			let unit = resolve_unit(spec)?;
			if units.iter().any(|u| u.address.request == unit.address.request) {
				return Err(ProfileError(format!("unit {:03X} is in the profile twice", unit.address.request)));
			}
			// One id, two meanings: the sim could not tell a request from an answer on it.
			let clash = units.iter().find_map(|u| match () {
				_ if u.address.response == unit.address.request => Some((unit.address.request, u.address.request)),
				_ if u.address.request == unit.address.response => Some((u.address.request, unit.address.request)),
				_ => None,
			});
			if let Some((asked, answering)) = clash {
				return Err(ProfileError(format!(
					"unit {asked:03X} is asked on the id unit {answering:03X} answers on: one of them cannot be in a profile with the other"
				)));
			}
			units.push(unit);
		}
		Ok(Car { bus: self.bus, units })
	}
}

/// The car the engine runs: every unit with its answers worked out.
#[derive(Debug, Clone, PartialEq)]
pub struct Car {
	pub bus: BusSettings,
	pub units: Vec<Unit>,
}

/// One unit, resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
	pub address: UnitAddress,
	pub name: String,
	pub delay: Duration,
	pub silent: bool,
	pub quiet_after: Option<u32>,
	pub quiet_after_ms: Option<Duration>,
	pub sessions: Vec<u8>,
	pub dtc_availability: u8,
	pub dids: BTreeMap<u16, Did>,
	pub faults: Vec<Fault>,
	/// Every code `19 0A` lists, stored ones first, with their status.
	pub supported: Vec<([u8; 3], u8)>,
}

/// One identifier, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Did {
	pub reads: Reads,
	pub pending: u32,
	pub pending_gap: Duration,
	pub delay: Duration,
	pub silent_after: Option<u32>,
}

/// What a read of an identifier gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reads {
	Value(Vec<u8>),
	/// Points by time from the channel opening, sorted; `period` wraps it.
	Series {
		points: Vec<(Duration, Vec<u8>)>,
		period: Option<Duration>,
	},
	Refused(u8),
}

impl Reads {
	/// The bytes at `t` after the channel opened, or the code it is refused with.
	pub fn at(&self, t: Duration) -> Result<&[u8], u8> {
		match self {
			Reads::Value(bytes) => Ok(bytes),
			Reads::Refused(nrc) => Err(*nrc),
			Reads::Series { points, period } => {
				let t = match period {
					Some(period) if !period.is_zero() => Duration::from_nanos((t.as_nanos() % period.as_nanos()) as u64),
					_ => t,
				};
				// Before the first point, the first point: a series says what the unit reads
				// from the start.
				let at = points.iter().rposition(|(at, _)| *at <= t).unwrap_or(0);
				Ok(&points[at].1)
			}
		}
	}
}

/// One stored fault, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
	pub code: [u8; 3],
	pub status: u8,
	pub extended: Option<Vec<u8>>,
	pub snapshot: Option<Vec<u8>>,
}

/// How long a pending answer waits for the next when the profile does not say.
const PENDING_GAP: Duration = Duration::from_millis(50);

/// The identifiers the unit-level shorthands stand for (ISO 14229-1 annex C.1).
const PART: u16 = 0xF187;
const VIN: u16 = 0xF190;
const COMPONENT: u16 = 0xF197;
const ODX: u16 = 0xF19E;
const ODX_VERSION: u16 = 0xF1A2;

fn resolve_unit(spec: &UnitSpec) -> Result<Unit, ProfileError> {
	let request = spec.request.0;
	let at = |what: String| ProfileError(format!("unit {request:03X}: {what}"));
	let address = UnitAddress::from_request(request)
		.ok_or_else(|| at("not a diagnostic request id — ISO 15765-4's 7E0–7E7 or VW's 700–795 (vag_uds_client::address)".to_string()))?;
	let mut dids: BTreeMap<u16, Did> = BTreeMap::new();
	let mut put = |did: u16, value: Did| -> Result<(), ProfileError> {
		match dids.insert(did, value) {
			Some(_) => Err(at(format!("{did:04X} is given twice"))),
			None => Ok(()),
		}
	};
	for (did, text) in [
		(PART, &spec.part),
		(VIN, &spec.vin),
		(COMPONENT, &spec.component),
		(ODX, &spec.odx),
		(ODX_VERSION, &spec.odx_version),
	] {
		if let Some(text) = text {
			put(did, plain(text.as_bytes().to_vec()))?;
		}
	}
	if !spec.installed.is_empty() {
		let ids: Vec<u16> = spec.installed.iter().map(|id| id.0).collect();
		let bitmap = installation_list(&ids).map_err(at)?;
		put(vag_uds_client::gateway::INSTALLATION_LIST, plain(bitmap))?;
	}
	for (key, did) in &spec.did {
		let number = parse_did(key).map_err(at)?;
		put(number, resolve_did(did).map_err(|e| at(format!("{key}: {e}")))?)?;
	}

	let faults: Vec<Fault> = spec
		.faults
		.iter()
		.map(|f| {
			Ok(Fault {
				code: code(&f.code).map_err(at)?,
				status: f.status,
				extended: f.extended.as_ref().map(|h| h.0.clone()),
				snapshot: f.snapshot.as_ref().map(|h| h.0.clone()),
			})
		})
		.collect::<Result<_, ProfileError>>()?;
	let mut supported: Vec<([u8; 3], u8)> = faults.iter().map(|f| (f.code, f.status)).collect();
	for extra in &spec.supported {
		let extra = code(extra).map_err(at)?;
		if !supported.iter().any(|(c, _)| *c == extra) {
			supported.push((extra, 0x00));
		}
	}
	Ok(Unit {
		address,
		name: spec.name.clone().unwrap_or_else(|| format!("{request:03X}")),
		delay: Duration::from_millis(spec.delay_ms),
		silent: spec.silent,
		quiet_after: spec.quiet_after,
		quiet_after_ms: spec.quiet_after_ms.map(Duration::from_millis),
		sessions: spec.sessions.clone().unwrap_or_else(|| vec![0x01, 0x03]),
		dtc_availability: spec.dtc_availability.unwrap_or(0xFF),
		dids,
		faults,
		supported,
	})
}

/// An identifier that reads `bytes`, with no behaviour.
fn plain(bytes: Vec<u8>) -> Did {
	Did {
		reads: Reads::Value(bytes),
		pending: 0,
		pending_gap: PENDING_GAP,
		delay: Duration::ZERO,
		silent_after: None,
	}
}

fn parse_did(key: &str) -> Result<u16, String> {
	match key.len() {
		4 => u16::from_str_radix(key, 16).map_err(|_| format!("identifier {key:?} is not four hex digits")),
		_ => Err(format!("identifier {key:?} is not four hex digits")),
	}
}

fn code(hex: &Hex) -> Result<[u8; 3], String> {
	<[u8; 3]>::try_from(hex.0.as_slice()).map_err(|_| format!("fault code {hex} is not three bytes"))
}

fn resolve_did(did: &DidSpec) -> Result<Did, String> {
	let table = match did {
		DidSpec::Value(hex) => return Ok(plain(hex.0.clone())),
		DidSpec::Full(table) => table,
	};
	let said = [table.value.is_some(), table.text.is_some(), table.series.is_some()]
		.iter()
		.filter(|&&b| b)
		.count();
	let reads = match (said, table.nrc) {
		(0, Some(nrc)) => {
			if table.pending > 0 {
				return Err("`pending` with `nrc`: a refusal has nothing to be pending for".into());
			}
			Reads::Refused(nrc)
		}
		(0, None) => return Err("says nothing: give `value`, `text`, `series` or `nrc`".into()),
		(1, None) => match (&table.value, &table.text, &table.series) {
			(Some(value), _, _) => Reads::Value(value.0.clone()),
			(_, Some(text), _) => Reads::Value(text.as_bytes().to_vec()),
			(_, _, Some(series)) => {
				if series.is_empty() {
					return Err("an empty series".into());
				}
				let mut points: Vec<(Duration, Vec<u8>)> = series.iter().map(|p| (Duration::from_millis(p.at_ms), p.value.0.clone())).collect();
				points.sort_by_key(|(at, _)| *at);
				Reads::Series {
					points,
					period: table.loop_ms.map(Duration::from_millis),
				}
			}
			_ => unreachable!("one of the three is given"),
		},
		(_, Some(_)) => return Err("`nrc` with a value: it is either refused or read".into()),
		(_, None) => return Err("more than one of `value`, `text` and `series`".into()),
	};
	if table.loop_ms.is_some() && table.series.is_none() {
		return Err("`loop_ms` without a series".into());
	}
	Ok(Did {
		reads,
		pending: table.pending,
		pending_gap: table.pending_ms.map(Duration::from_millis).unwrap_or(PENDING_GAP),
		delay: Duration::from_millis(table.delay_ms),
		silent_after: table.silent_after,
	})
}

/// The installation-list bitmap that `vag_uds_client::gateway::decode_installation_list`
/// decodes to `ids`: bit `n`, least-significant first, for `0x700 + n`, over VW's block.
pub fn installation_list(ids: &[u16]) -> Result<Vec<u8>, String> {
	let mut bytes = vec![0u8; vag_uds_client::gateway::VW_BLOCK_BYTES];
	for id in ids {
		let n = id
			.checked_sub(0x700)
			.map(usize::from)
			.filter(|n| n / 8 < bytes.len())
			.ok_or_else(|| format!("installed: {id:03X} is outside VW's block, 700–7BF, which the list covers"))?;
		bytes[n / 8] |= 1 << (n % 8);
	}
	Ok(bytes)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn car(text: &str) -> Result<Car, ProfileError> {
		Profile::parse(text)?.resolve()
	}

	#[test]
	fn hex_reads_with_and_without_spaces_and_writes_spaced() {
		assert_eq!(Hex::parse("0F a0").unwrap().0, vec![0x0F, 0xA0]);
		assert_eq!(Hex::parse("0FA0").unwrap().0, vec![0x0F, 0xA0]);
		assert_eq!(Hex::parse("").unwrap().0, Vec::<u8>::new());
		assert!(Hex::parse("0FA").is_err());
		assert!(Hex::parse("GG").is_err());
		assert_eq!(Hex(vec![0x0F, 0xA0]).to_string(), "0F A0");
	}

	#[test]
	fn a_unit_answers_on_the_id_the_clients_rule_gives() {
		let car = car("[[unit]]\nrequest = \"7E0\"\n[[unit]]\nrequest = 0x713\n").unwrap();
		assert_eq!(car.units[0].address, UnitAddress::from_request(0x7E0).unwrap());
		assert_eq!(car.units[1].address, UnitAddress::from_request(0x713).unwrap());
	}

	#[test]
	fn an_id_outside_both_blocks_is_refused() {
		let err = car("[[unit]]\nrequest = \"7DF\"\n").unwrap_err();
		assert!(err.0.contains("7DF: not a diagnostic request id"), "{err}");
	}

	#[test]
	fn the_shorthands_are_their_identifiers_and_a_second_spelling_is_refused() {
		let car = car("[[unit]]\nrequest = \"7E0\"\npart = \"AB\"\nvin = \"V\"\n").unwrap();
		assert_eq!(car.units[0].dids[&0xF187].reads, Reads::Value(b"AB".to_vec()));
		assert_eq!(car.units[0].dids[&0xF190].reads, Reads::Value(b"V".to_vec()));
		let err = super::Profile::parse("[[unit]]\nrequest = \"7E0\"\npart = \"AB\"\n[unit.did]\nF187 = \"00\"\n")
			.unwrap()
			.resolve()
			.unwrap_err();
		assert!(err.0.contains("F187 is given twice"), "{err}");
	}

	#[test]
	fn the_installation_list_decodes_to_what_was_written() {
		let bitmap = installation_list(&[0x713, 0x70E, 0x7BF]).unwrap();
		assert_eq!(vag_uds_client::gateway::decode_installation_list(&bitmap), vec![0x70E, 0x713, 0x7BF]);
		assert!(installation_list(&[0x7E0]).is_err());
		let car = car("[[unit]]\nrequest = \"710\"\ninstalled = [\"713\"]\n").unwrap();
		let Reads::Value(bytes) = &car.units[0].dids[&vag_uds_client::gateway::INSTALLATION_LIST].reads else {
			panic!()
		};
		assert_eq!(vag_uds_client::gateway::decode_installation_list(bytes), vec![0x713]);
	}

	#[test]
	fn a_series_holds_each_point_and_wraps_at_its_period() {
		let car = car(
			"[[unit]]\nrequest = \"7E0\"\n[unit.did]\nF40D = { series = [ { at_ms = 1000, value = \"01\" }, { at_ms = 0, value = \"00\" } ], loop_ms = 2000 }\n",
		)
		.unwrap();
		let reads = &car.units[0].dids[&0xF40D].reads;
		let at = |ms| reads.at(Duration::from_millis(ms)).unwrap().to_vec();
		assert_eq!(at(0), vec![0x00]);
		assert_eq!(at(999), vec![0x00]);
		assert_eq!(at(1000), vec![0x01]);
		assert_eq!(at(2500), vec![0x00]);
		assert_eq!(at(3500), vec![0x01]);
	}

	#[test]
	fn a_table_says_exactly_one_thing() {
		for (did, why) in [
			("{ }", "says nothing"),
			("{ value = \"00\", text = \"a\" }", "more than one"),
			("{ value = \"00\", nrc = 0x22 }", "either refused or read"),
			("{ nrc = 0x22, pending = 1 }", "nothing to be pending"),
			("{ value = \"00\", loop_ms = 5 }", "without a series"),
		] {
			let err = car(&format!("[[unit]]\nrequest = \"7E0\"\n[unit.did]\n0100 = {did}\n")).unwrap_err();
			assert!(err.0.contains(why), "{did}: {err}");
		}
	}

	#[test]
	fn a_unit_twice_or_asked_on_another_units_answer_id_is_refused() {
		assert!(car("[[unit]]\nrequest = \"7E0\"\n[[unit]]\nrequest = \"7E0\"\n").is_err());
		// 70C answers on 70C + 6A = 776, so a unit asked on 776 cannot be told from its answers,
		// in either order.
		for text in [
			"[[unit]]\nrequest = \"70C\"\n[[unit]]\nrequest = \"776\"\n",
			"[[unit]]\nrequest = \"776\"\n[[unit]]\nrequest = \"70C\"\n",
		] {
			let err = car(text).unwrap_err();
			assert!(err.0.contains("unit 776 is asked on the id unit 70C answers on"), "{err}");
		}
	}

	#[test]
	fn an_unknown_key_is_an_error_not_a_silent_default() {
		assert!(Profile::parse("[[unit]]\nrequest = \"7E0\"\npart_number = \"x\"\n").is_err());
	}

	#[test]
	fn a_profile_written_back_reads_as_the_same_profile() {
		let text = "about = \"x\"\n[[unit]]\nrequest = \"7E0\"\npart = \"P\"\n[unit.did]\nF40D = \"00\"\n0100 = { nrc = 0x22 }\n\
                    0101 = { series = [ { at_ms = 0, value = \"01\" } ] }\n[[unit.fault]]\ncode = \"00 01 29\"\nstatus = 9\n";
		let profile = Profile::parse(text).unwrap();
		assert_eq!(Profile::parse(&profile.to_toml()).unwrap(), profile);
	}
}
