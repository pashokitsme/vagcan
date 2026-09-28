//! Which control units a car has, and what each one says it is.
//!
//! Every scaling this tool applies is looked up by what a unit reported about
//! itself — its part number, and the ODX file it names — so the walk that
//! collects those answers is the first thing `watch`, `measure` and
//! `units --identify` do. It was written inside `watch`, where a second live
//! command cannot reach it; both `measure` and its setup need the same walk, and
//! a copy of it would drift from this one the first time either learned
//! something.
//!
//! Nothing here changes a unit: four identifiers are read and no session is
//! opened. The danger in this tool is what a sweep can provoke, not what a read
//! changes, and this is not a sweep.
//!
//! # The record
//!
//! What the walk found is written down, per car, in
//! `~/.vagcan/cars/<VIN>/units.json` ([`record`]) — because two things need the
//! car's units when the car is not there. `setup` reads a VCDS installation's
//! measurement registry for the units of every car this machine has met, and
//! the dash build resolves a plan against what each unit said it is; neither
//! can ask the car, and until this record existed the only offline list of a
//! car's units was a sweep somebody had to run on purpose. `watch`, `measure`
//! and `units --identify` write it silently, so a car that has been watched
//! once is known.
//!
//! Identity only: the request id and the four identifiers [`identify`] reads,
//! nothing swept and nothing asked that the walk does not ask anyway. **Every
//! run asks every unit again** — the record is for the commands that cannot
//! ask, never a reason not to — and what a unit answered is merged into its
//! entry **field by field**: an identifier it answered now replaces the one on
//! record, one it did not answer this time (a missed deadline, a unit still
//! starting up) leaves the recorded one alone. A value is never replaced with
//! nothing, so one bad read cannot cost a good record. **Another part number
//! at the address is another unit**, and its entry is replaced whole: a
//! swapped unit is never recorded under the old one's ODX file
//! ([`merge_into`]). A unit not seen this time (asleep, or not among the units
//! a command asked) keeps its entry. The file is replaced whole,
//! through [`crate::datadir::replace_file`], so a reader never sees half a
//! record — and only when what it says has changed: the firmware's build script
//! watches it, and a rewrite of the same bytes is a rebuild of the image.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use vag_uds_can::UnitLink;
use vag_uds_client::AsyncUdsClient;
use vag_uds_client::address::UnitAddress;
use vag_uds_client::uds::UdsError;
use vag_uds_transport::CanId;

use crate::datadir::UNITS_FILE;
use crate::plan::{self, UnitIdentity};

/// One car's record: which car, and what each of its units said it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
	pub vin: String,
	/// In request-id order.
	pub units: Vec<UnitIdentity>,
}

/// Write what the car's units said about themselves into the car's record.
///
/// Merged into what is already there (see the module docs), and the file is
/// replaced whole. The VIN is checked as a directory name by
/// [`crate::datadir::units_record`]: a unit's answer is not trusted as a path.
pub fn record(vin: &str, identities: &[UnitIdentity]) -> anyhow::Result<PathBuf> {
	let path = crate::datadir::units_record(vin)?;
	record_to(&path, vin, identities)?;
	Ok(path)
}

/// [`record`], and a write that fails is one line on stderr — never a stop.
///
/// The record is a convenience for the next command, not a condition of this
/// one: a read-only home directory must not take `watch` off the car.
pub fn record_quietly(vin: &str, identities: &[UnitIdentity]) {
	if let Err(e) = record(vin, identities) {
		eprintln!("the car's control units were not recorded: {e:#}");
	}
}

/// The one line a live command prints when the engine gave no VIN: the record
/// is filed by VIN, so there is nowhere to write it, and a dash owner who was
/// told to connect to the car once would otherwise wait for a file that never
/// comes.
pub const NOT_RECORDED_WITHOUT_A_VIN: &str =
	"the engine gave no VIN, so the car's control units were not recorded (the record is filed by VIN); `vagcan dev dash build` needs it";

/// The units recorded for one car, or `None` when no command has recorded it.
///
/// A record that is there and does not read is an error, not `None`: the
/// dash build and `watch` act on this, and a corrupt file taken for a car
/// with no units would refuse to build for a car that was watched yesterday.
pub fn recorded(vin: &str) -> anyhow::Result<Option<Vec<UnitIdentity>>> {
	read_record(&crate::datadir::units_record(vin)?)
}

/// Every car recorded on this machine, in directory order.
///
/// What `setup` reads the VCDS registry for, and what a read of it covers: a
/// project holds one set of registry rows for every car, so each read is over
/// all of them at once.
pub fn recorded_cars() -> anyhow::Result<Vec<Recorded>> {
	recorded_cars_in(&crate::datadir::cars_dir()?)
}

/// The rule behind [`record`], on a path — so it can be tested against a
/// directory that is not the owner's own.
///
/// Merged field by field into what is on record (see the module docs), and
/// written only when the merge changed something: a record that reads back as
/// it was is left untouched, so nothing watching the file sees a write.
pub fn record_to(path: &Path, vin: &str, identities: &[UnitIdentity]) -> anyhow::Result<()> {
	let before = read_file(path)?;
	let mut by_request: BTreeMap<u16, UnitIdentity> = before
		.as_ref()
		.map(|car| car.units.iter().map(|u| (u.request, u.clone())).collect())
		.unwrap_or_default();
	for identity in identities {
		match by_request.get_mut(&identity.request) {
			Some(recorded) => merge_into(recorded, identity),
			None => {
				by_request.insert(identity.request, identity.clone());
			}
		}
	}
	let merged = Recorded {
		vin: vin.to_string(),
		units: by_request.into_values().collect(),
	};
	if before.as_ref() == Some(&merged) {
		return Ok(());
	}
	let file = File {
		vin: merged.vin,
		units: merged.units.into_iter().map(Row::from).collect(),
	};
	let mut text = serde_json::to_string_pretty(&file).context("encoding the record of the car's units")?;
	text.push('\n');
	crate::datadir::replace_file(path, text.as_bytes())
}

/// What a unit answered now, over its entry: an identifier answered replaces
/// the recorded one, an identifier not answered leaves it — a value is never
/// replaced with nothing — **for the same unit**. A part number (`F187`) other
/// than the one on record is another unit at this address, swapped in, and
/// nothing the old one said is true of it: its entry is replaced whole, so a
/// replacement that does not answer `F19E` is recorded without an ODX file
/// rather than under the old unit's (found in review, 2026-09-28: the plan
/// then bound the old variant's scalings to the new unit, and the board's
/// `F187` check passed).
fn merge_into(recorded: &mut UnitIdentity, now: &UnitIdentity) {
	if let (Some(was), Some(is)) = (&recorded.part_number, &now.part_number)
		&& was != is
	{
		*recorded = now.clone();
		return;
	}
	fn take(field: &mut Option<String>, now: &Option<String>) {
		if now.is_some() {
			field.clone_from(now);
		}
	}
	take(&mut recorded.part_number, &now.part_number);
	take(&mut recorded.odx_name, &now.odx_name);
	take(&mut recorded.odx_version, &now.odx_version);
	take(&mut recorded.component, &now.component);
}

/// The rule behind [`recorded`], on a path.
pub fn read_record(path: &Path) -> anyhow::Result<Option<Vec<UnitIdentity>>> {
	Ok(read_file(path)?.map(|file| file.units))
}

/// The file at `path` as a [`Recorded`], `None` when there is none.
fn read_file(path: &Path) -> anyhow::Result<Option<Recorded>> {
	let text = match std::fs::read_to_string(path) {
		Ok(text) => text,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
		Err(e) => return Err(anyhow::Error::new(e).context(format!("reading {}", path.display()))),
	};
	let file: File = serde_json::from_str(&text).map_err(|e| NotARecord {
		path: path.to_path_buf(),
		cause: e.to_string(),
	})?;
	let mut units: Vec<UnitIdentity> = file.units.into_iter().map(Row::into_identity).collect::<anyhow::Result<_>>()?;
	units.sort_by_key(|u| u.request);
	Ok(Some(Recorded { vin: file.vin, units }))
}

/// The rule behind [`recorded_cars`], on a `cars/` directory.
///
/// A car directory without a record is a car something else was kept for — a
/// car file, a drive — and is skipped; a `cars/` that is not there is a
/// machine no car has been connected to.
pub fn recorded_cars_in(cars: &Path) -> anyhow::Result<Vec<Recorded>> {
	let mut dirs: Vec<PathBuf> = match std::fs::read_dir(cars) {
		Ok(entries) => entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect(),
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
		Err(e) => return Err(anyhow::Error::new(e).context(format!("reading {}", cars.display()))),
	};
	dirs.sort();
	let mut out = Vec::new();
	for dir in dirs {
		if let Some(car) = read_file(&dir.join(UNITS_FILE))? {
			out.push(car);
		}
	}
	Ok(out)
}

/// A file under a car's record name that this tool did not write: the path and
/// what the parser said. A type of its own so that `setup`'s report can lay the
/// two out on lines of their own — the path is as long as the car's home makes
/// it — while everywhere else it is the one line below.
#[derive(Debug)]
pub struct NotARecord {
	pub path: PathBuf,
	pub cause: String,
}

impl std::fmt::Display for NotARecord {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(
			f,
			"{} is not a record this tool wrote ({}) — move it aside; `vagcan watch`, `measure` or `units --identify` with that car writes a fresh one",
			self.path.display(),
			self.cause
		)
	}
}

impl std::error::Error for NotARecord {}

/// The file: which car, and one row per unit.
#[derive(Serialize, Deserialize)]
struct File {
	vin: String,
	units: Vec<Row>,
}

/// One unit as the file holds it: the request id as a person writes it
/// (`"7E1"`), and only the identifiers the unit answered.
#[derive(Serialize, Deserialize)]
struct Row {
	request: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	part_number: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	odx_name: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	odx_version: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	component: Option<String>,
}

impl From<UnitIdentity> for Row {
	fn from(u: UnitIdentity) -> Row {
		Row {
			request: format!("{:03X}", u.request),
			part_number: u.part_number,
			odx_name: u.odx_name,
			odx_version: u.odx_version,
			component: u.component,
		}
	}
}

impl Row {
	fn into_identity(self) -> anyhow::Result<UnitIdentity> {
		let request = u16::from_str_radix(&self.request, 16).with_context(|| format!("{:?} is not a request id", self.request))?;
		Ok(UnitIdentity {
			request,
			part_number: self.part_number,
			odx_name: self.odx_name,
			odx_version: self.odx_version,
			component: self.component,
		})
	}
}

/// Ask the car which control units it has, then ask each of them what it is.
///
/// `also` is the units the caller wants identified whatever the gateway says —
/// what it means to poll, in practice. **Every unit is asked, every run.** The
/// car's record is not consulted here: a record is for the commands that
/// cannot ask the car, and a walk that skipped what was on record would never
/// notice a swapped unit, or recover an identifier one bad read left out
/// (`record_to` merges what is answered now into what was, field by field).
/// **The gateway is asked for its list either way.** Trusting the record to be
/// the whole car instead would be a trap: a record written by a command that
/// asked one unit would then leave `watch` seeing that unit and the engine,
/// with the other thirteen silently absent.
///
/// The link comes back out because it is a single-user resource with no way
/// to borrow it across an await, so it is handed over and handed back rather
/// than shared.
pub async fn identify<B: UnitLink>(backend: B, also: &[u16], progress: &mut crate::progress::Line) -> (B, Vec<UnitIdentity>) {
	// Which units the car has. Without this the view would only ever show the
	// engine, because a unit with no identity contributes no channels and so
	// no tab — which is what "switching between units does nothing" looked
	// like. One read of the gateway's installation list answers it, the same
	// read `vagcan units` makes; a car whose gateway does not answer falls
	// back to whatever was asked for.
	let (backend, listed) = installed(backend, progress).await;
	identify_listed(backend, &to_identify(&listed.unwrap_or_default(), also), progress).await
}

/// Ask the gateway which control units this car has: its installation list,
/// as request ids, in the gateway's order. The error is what the gateway said,
/// or did not — `vagcan units` prints it, and [`identify`] takes it as an empty
/// list.
///
/// The one read [`identify`] and `vagcan units` both start from, so the two
/// cannot come to different lists.
pub async fn installed<B: UnitLink>(backend: B, progress: &mut crate::progress::Line) -> (B, Result<Vec<u16>, UdsError>) {
	progress.update("asking the gateway which control units this car has");
	let gateway = UnitAddress::from_request(vag_uds_client::gateway::GATEWAY).expect("the gateway is in VW's block");
	let mut uds = AsyncUdsClient::new(backend.to_unit(CanId::Standard(gateway.request), CanId::Standard(gateway.response)));
	let listed = uds
		.read_data_by_identifier(vag_uds_client::gateway::INSTALLATION_LIST)
		.await
		.map(|bitmap| vag_uds_client::gateway::decode_installation_list(&bitmap));
	(B::release(uds.into_transport()), listed)
}

/// The units to ask once the gateway has listed: what the caller wants
/// whatever it says, then every whole-car walk's order
/// ([`vag_uds_client::gateway::walk_order`]) — the engine and the gearbox,
/// never in that list because they live on the other id block; the gateway,
/// which does not list itself; then what it listed.
///
/// One rule, so `vagcan units`, `watch` and `measure` ask the same units
/// `faults` and the board's fault count read — and the gateway is one of them:
/// its channels are in a VCDS installation like any other unit's, and a record
/// without it would never have them read.
pub fn to_identify(listed: &[u16], also: &[u16]) -> Vec<u16> {
	let mut wanted: Vec<u16> = also.to_vec();
	wanted.extend(vag_uds_client::gateway::walk_order(listed));
	wanted
}

/// Read the vehicle identification number off the engine.
///
/// The VIN is what every per-car file this tool keeps is named after — the car
/// file, the saved sessions, the record of its units — so more than one live command
/// needs it, and the read lives here rather than being copied into each of
/// them. Only `0xF190` is asked for: `read_identity` would answer the same
/// question with seven requests and throw six of the answers away.
///
/// A car that will not say is not a failure; it simply has no files of its own.
pub async fn read_vin<B: UnitLink>(backend: B) -> (B, Option<String>) {
	let Some(engine) = UnitAddress::from_request(plan::ENGINE) else {
		return (backend, None);
	};
	let mut uds = AsyncUdsClient::new(backend.to_unit(CanId::Standard(engine.request), CanId::Standard(engine.response)));
	let vin = uds
		.read_data_by_identifier(vag_uds_client::identity::did::VIN)
		.await
		.ok()
		// VW pads its text fields with a trailing space or NUL; a VIN is
		// seventeen printable characters and nothing else.
		.map(|bytes| {
			String::from_utf8_lossy(&bytes)
				.trim_matches(|c: char| c.is_control() || c == ' ')
				.to_string()
		})
		.filter(|text| !text.is_empty());
	(B::release(uds.into_transport()), vin)
}

/// Identify a named list of units.
///
/// The walk [`identify`] performs once it knows the list. Kept separate because
/// the two halves answer different questions — *which units are there* is one
/// read of the gateway, *what each of them is* is a probe apiece. Every unit
/// named is asked; nothing is taken from a record (see [`identify`]). Only the
/// units that answered come back. Sorted by request id and deduplicated first;
/// a request id outside both blocks this tool addresses is skipped.
pub async fn identify_listed<B: UnitLink>(mut backend: B, requests: &[u16], progress: &mut crate::progress::Line) -> (B, Vec<UnitIdentity>) {
	let mut wanted = requests.to_vec();
	wanted.sort_unstable();
	wanted.dedup();
	let mut identities: Vec<UnitIdentity> = Vec::new();
	let total = wanted.len();
	for (at, request) in wanted.into_iter().enumerate() {
		progress.update(&format!("identifying control units — {request:03X}, {} of {total}", at + 1));
		let Some(address) = UnitAddress::from_request(request) else {
			continue;
		};
		let mut uds = AsyncUdsClient::new(backend.to_unit(CanId::Standard(address.request), CanId::Standard(address.response)));
		let text = |data: Option<Vec<u8>>| {
			data
				.map(|b| String::from_utf8_lossy(&b).trim_end_matches(['\0', ' ']).to_string())
				.filter(|s| !s.is_empty())
		};
		// One short probe decides whether the unit is there. A unit that is
		// not costs this deadline once, instead of the full two-second one
		// three times over — fifteen listed addresses at that price is what
		// made startup take several seconds.
		const PROBE: Duration = Duration::from_millis(300);
		let part = text(uds.read_data_by_identifier_within(0xF187, PROBE).await.ok());
		if part.is_none() && request != plan::ENGINE {
			backend = B::release(uds.into_transport());
			continue;
		}
		// Identification only — no session change and no sweep. The danger is
		// what a sweep can provoke; this is not one.
		let component = text(uds.read_data_by_identifier_within(0xF197, PROBE).await.ok());
		let odx = text(uds.read_data_by_identifier_within(0xF19E, PROBE).await.ok());
		// `F1A2` beside `F19E`: the two together pick a unit's variant, and a
		// name without a version can only ever match a whole family. One more
		// `0x22` read, on the allowlist, in the identification pass that is
		// already asking this unit three questions.
		let version = text(uds.read_data_by_identifier_within(0xF1A2, PROBE).await.ok());
		identities.push(UnitIdentity {
			request,
			part_number: part,
			odx_name: odx,
			odx_version: version,
			component,
		});
		backend = B::release(uds.into_transport());
	}
	(backend, identities)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn unit(request: u16, part: &str, odx: Option<&str>) -> UnitIdentity {
		UnitIdentity {
			request,
			part_number: Some(part.to_string()),
			odx_name: odx.map(str::to_string),
			odx_version: odx.map(|_| "001007".to_string()),
			component: Some(format!("unit {request:03X}")),
		}
	}

	#[test]
	fn the_units_to_identify_are_what_the_caller_wants_and_every_whole_car_walks_units() {
		// One rule for `watch`, `measure` and `vagcan units`, and the same units
		// `faults` walks: the engine and the gearbox are never in the gateway's
		// list — they live on the other id block — and the gateway does not list
		// itself, so all three are added rather than discovered; what the caller
		// polls is asked whatever the gateway says. `identify_listed` sorts and
		// deduplicates; this does not.
		let gateway = vag_uds_client::gateway::GATEWAY;
		assert_eq!(
			to_identify(&[0x70E, 0x713], &[plan::ENGINE]),
			vec![plan::ENGINE, plan::ENGINE, 0x7E1, gateway, 0x70E, 0x713]
		);
		assert_eq!(
			to_identify(&[], &[]),
			vec![plan::ENGINE, 0x7E1, gateway],
			"a gateway that did not answer still leaves the units no list holds"
		);
	}

	#[test]
	fn a_record_is_read_back_as_it_was_written_with_request_ids_as_people_write_them() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("units.json");
		assert_eq!(read_record(&path).unwrap(), None, "no file is no record, not an error");
		let units = vec![unit(0x7E1, "0CW300041G", Some("EV_TCMDQ200021")), unit(0x7E0, "8V0906264H", None)];
		record_to(&path, "XW8AD4NE9JH008917", &units).unwrap();
		let text = std::fs::read_to_string(&path).unwrap();
		assert!(text.contains("\"request\": \"7E0\""), "hex as a person writes it, not a number:\n{text}");
		assert!(text.contains("\"vin\": \"XW8AD4NE9JH008917\""), "{text}");
		assert!(
			!text.contains("null"),
			"a field the unit did not answer is left out, not written as null:\n{text}"
		);
		// Sorted by request on the way out, whatever order they were identified in.
		let back = read_record(&path).unwrap().expect("a record");
		assert_eq!(
			back,
			vec![unit(0x7E0, "8V0906264H", None), unit(0x7E1, "0CW300041G", Some("EV_TCMDQ200021"))]
		);
	}

	#[test]
	fn a_unit_identified_again_replaces_its_entry_and_one_not_seen_is_kept() {
		// Merged by request id: the unit that answered now is the newest word on
		// what it is, and a unit that did not answer this time (asleep, or a
		// command that asked fewer units) keeps its entry — a record that forgot
		// units on every shorter run would never hold the whole car.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("units.json");
		record_to(&path, "VIN", &[unit(0x7E0, "OLD-ENGINE", None), unit(0x713, "ESC", Some("EV_Brake1"))]).unwrap();
		record_to(&path, "VIN", &[unit(0x7E0, "NEW-ENGINE", Some("EV_ECM"))]).unwrap();
		let back = read_record(&path).unwrap().unwrap();
		assert_eq!(
			back,
			vec![unit(0x713, "ESC", Some("EV_Brake1")), unit(0x7E0, "NEW-ENGINE", Some("EV_ECM"))]
		);
		// Recording nothing changes nothing.
		record_to(&path, "VIN", &[]).unwrap();
		assert_eq!(read_record(&path).unwrap().unwrap(), back);
	}

	#[test]
	fn an_identifier_a_unit_did_not_answer_this_time_never_erases_the_one_on_record() {
		// Found in review (2026-09-28): a unit that missed its `F19E` deadline
		// once was recorded without its ODX name, and a later, complete answer
		// was the only thing that could put it back — so the merge is per
		// field. An identifier answered now replaces the recorded one; one not
		// answered leaves it. Nothing on record is ever replaced with nothing.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("units.json");
		let good = UnitIdentity {
			request: 0x713,
			part_number: Some("PART-ESC".into()),
			odx_name: Some("EV_Brake".into()),
			odx_version: Some("001003".into()),
			component: Some("esc".into()),
		};
		record_to(&path, "VIN", std::slice::from_ref(&good)).unwrap();
		// The ESC answers `F187` and `F197` and misses the deadline on the rest.
		let partial = UnitIdentity {
			request: 0x713,
			part_number: Some("PART-ESC".into()),
			odx_name: None,
			odx_version: None,
			component: Some("esc".into()),
		};
		record_to(&path, "VIN", &[partial]).unwrap();
		assert_eq!(
			read_record(&path).unwrap().unwrap(),
			vec![good.clone()],
			"the ODX name and version stay on record"
		);
		// A unit that answered nothing but its part number: the same.
		let bare = UnitIdentity {
			request: 0x713,
			part_number: Some("PART-ESC".into()),
			..Default::default()
		};
		record_to(&path, "VIN", &[bare]).unwrap();
		assert_eq!(read_record(&path).unwrap().unwrap(), vec![good.clone()]);
		// Another part number at the address is another unit, swapped in: its
		// entry is replaced whole, and nothing the old unit said — its ODX file
		// and version, its name — is carried onto the new one (found in review,
		// 2026-09-28: the mixed entry bound the old variant's scalings to it).
		let swapped = UnitIdentity {
			request: 0x713,
			part_number: Some("PART-ESC-2".into()),
			odx_name: Some("EV_Brake2".into()),
			odx_version: None,
			component: None,
		};
		record_to(&path, "VIN", std::slice::from_ref(&swapped)).unwrap();
		assert_eq!(read_record(&path).unwrap().unwrap(), vec![swapped.clone()]);
		// The replacement answering more next time fills its own entry in.
		let more = UnitIdentity {
			odx_version: Some("002001".into()),
			component: Some("esc 2".into()),
			..swapped
		};
		record_to(&path, "VIN", std::slice::from_ref(&more)).unwrap();
		assert_eq!(read_record(&path).unwrap().unwrap(), vec![more]);
		// A unit that answered no part number at all (the engine is asked on
		// whatever it answers) cannot be told apart from the one on record, so
		// it is merged as the same unit.
		let nameless = UnitIdentity {
			request: 0x713,
			part_number: None,
			odx_name: Some("EV_Brake3".into()),
			..Default::default()
		};
		record_to(&path, "VIN", std::slice::from_ref(&nameless)).unwrap();
		let merged = read_record(&path).unwrap().unwrap();
		assert_eq!(merged[0].part_number.as_deref(), Some("PART-ESC-2"), "the part number on record stands");
		assert_eq!(merged[0].odx_name.as_deref(), Some("EV_Brake3"));
		// The reverse order, as the second reviewer's probe had it: a partial
		// answer first, then a complete one, is complete on record.
		let path = dir.path().join("second.json");
		record_to(
			&path,
			"VIN",
			&[UnitIdentity {
				request: 0x713,
				part_number: Some("PART-ESC".into()),
				odx_name: None,
				odx_version: None,
				component: Some("esc".into()),
			}],
		)
		.unwrap();
		record_to(&path, "VIN", std::slice::from_ref(&good)).unwrap();
		assert_eq!(read_record(&path).unwrap().unwrap(), vec![good]);
	}

	#[test]
	fn the_file_is_written_only_when_the_record_changes() {
		// The firmware's build script names the record as an input, so a
		// rewrite of the same content is a rebuild of the image — observed in
		// review. `replace_file` renames a new file over the old one, so a write
		// is a new inode; a record that came out as it went in keeps its inode.
		use std::os::unix::fs::MetadataExt as _;
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("units.json");
		let esc = unit(0x713, "ESC", Some("EV_Brake1"));
		record_to(&path, "VIN", std::slice::from_ref(&esc)).unwrap();
		let written = std::fs::metadata(&path).unwrap().ino();
		record_to(&path, "VIN", std::slice::from_ref(&esc)).unwrap();
		assert_eq!(std::fs::metadata(&path).unwrap().ino(), written, "the same answer is not a write");
		let partial = UnitIdentity {
			request: 0x713,
			part_number: Some("ESC".into()),
			..Default::default()
		};
		record_to(&path, "VIN", &[partial]).unwrap();
		assert_eq!(
			std::fs::metadata(&path).unwrap().ino(),
			written,
			"an answer with less in it changes nothing"
		);
		record_to(&path, "VIN", &[]).unwrap();
		assert_eq!(std::fs::metadata(&path).unwrap().ino(), written);
		record_to(&path, "VIN", &[esc, unit(0x7E0, "ENGINE", None)]).unwrap();
		assert_ne!(std::fs::metadata(&path).unwrap().ino(), written, "a unit not on record is a write");
	}

	#[test]
	fn a_file_that_is_not_a_record_is_refused_rather_than_overwritten_or_read_as_empty() {
		// The record is read at run time — by the dash build and by `watch` —
		// so a corrupt one is said, not silently replaced or taken for a car
		// with no units.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("units.json");
		std::fs::write(&path, "not json").unwrap();
		let err = read_record(&path).unwrap_err().to_string();
		assert!(err.contains("units.json"), "{err}");
		assert!(record_to(&path, "VIN", &[unit(0x7E0, "P", None)]).is_err());
		assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json", "the file was overwritten");
	}

	#[test]
	fn every_car_recorded_on_this_machine_is_read_and_a_car_without_a_record_is_skipped() {
		let cars = tempfile::tempdir().unwrap();
		record_to(
			&cars.path().join("VIN00000000000001").join(UNITS_FILE),
			"VIN00000000000001",
			&[unit(0x7E0, "A", None)],
		)
		.unwrap();
		record_to(
			&cars.path().join("old-name-VIN00000000000002").join(UNITS_FILE),
			"VIN00000000000002",
			&[unit(0x7E1, "B", Some("EV_TCM"))],
		)
		.unwrap();
		std::fs::create_dir_all(cars.path().join("VIN00000000000003")).unwrap();
		std::fs::write(cars.path().join("VIN00000000000003").join("car.json"), "{}").unwrap();
		let recorded = recorded_cars_in(cars.path()).unwrap();
		let vins: Vec<&str> = recorded.iter().map(|car| car.vin.as_str()).collect();
		assert_eq!(
			vins,
			vec!["VIN00000000000001", "VIN00000000000002"],
			"by directory name; the car with no record is skipped"
		);
		assert_eq!(recorded[1].units, vec![unit(0x7E1, "B", Some("EV_TCM"))]);
		// A `cars/` that does not exist yet is a machine no car has been connected to.
		assert!(recorded_cars_in(&cars.path().join("nowhere")).unwrap().is_empty());
	}

	#[test]
	fn the_whole_file_is_replaced_at_once_and_a_write_that_fails_says_so_in_one_line() {
		// `record_to` goes through `datadir::replace_file`, so a reader never sees a
		// half-written record; what is asserted here is that no temporary file is
		// left beside it, and that a directory that cannot be written is an error
		// the caller can print rather than a panic.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("units.json");
		record_to(&path, "VIN", &[unit(0x7E0, "P", None)]).unwrap();
		assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
		let blocked = dir.path().join("a-file-not-a-directory");
		std::fs::write(&blocked, b"").unwrap();
		assert!(record_to(&blocked.join("units.json"), "VIN", &[unit(0x7E0, "P", None)]).is_err());
	}
}
