//! What to read, and what it means — the part that can be tested without a car.
//!
//! One link to the car means one conversation at a time. Which identifiers go out
//! together, and when, is decided by the bus scheduler (`crate::bus`); this module
//! only says which channels exist and which of them are wanted.
//!
//! A channel is keyed by the unit's **request id**, not by a unit number: the
//! two id blocks on this car have different response rules, and a number is
//! only a display convenience over the id (see `vag_uds_client::address`).

use vag_data_labels::catalog::{CatalogStore, MeasurementDef, ReadId};
use vag_uds_client::address::UnitAddress;

/// One value the user can put on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
	/// Diagnostic request id of the unit that owns it — `0x7E0` engine,
	/// `0x7E1` gearbox, `0x714` instrument cluster.
	pub request: u16,
	pub did: u16,
	/// How to read it, when this project has proven or standardised it.
	/// `None` means the bytes are shown raw.
	pub def: Option<MeasurementDef>,
	/// What the label files call it, found through the text id the row carried.
	///
	/// Preferred over [`MeasurementDef::name`] on screen. An ODIS long name is
	/// written for a diagnostic engineer and reads like one —
	/// `Brake_pedal_information_plausibility` — while the same channel's text
	/// id reaches a sentence somebody can read at an open driver's door. It is
	/// a lookup through an id the data itself carries; nothing here holds a
	/// name for an identifier.
	pub named: Option<String>,
	/// Whether a drive on a car established this scaling.
	///
	/// **Not the same question as "is there a `def`".** A channel can be fully
	/// named and scaled from an ODIS project or from the OBD-II standard and
	/// still never have been confirmed against the vehicle in front of the tool.
	/// The coverage report has to tell those apart, or it reports a compu
	/// formula somebody extracted as something a drive established.
	pub proven: bool,
	/// The row's own text id — the key `~/.vagcan/names.csv` is written under.
	///
	/// Carried so that somebody who has just read a bad name on screen can find
	/// the line to write a better one; `watch`'s `show_key` setting is what puts
	/// it on the row. `None` for a proven row and for an identifier `--did`
	/// named that no source describes.
	pub text_id: Option<String>,
	pub selected: bool,
}

/// What identifies one channel — **not** `(request, did)`.
///
/// One `0x22` response carries as many fields as the control unit put in it, and
/// the byte each starts at is what tells them apart. Keying by identifier alone
/// meant a unit could show one field per identifier and silently dropped the
/// rest: measured on the reference car's fifteen units, 3,963 sayable fields
/// arriving as 2,011 rows.
///
/// The response body itself stays keyed by `(request, did)` — see
/// [`crate::watch::App::latest`] — because one read still answers one
/// identifier, and every field of it is cut from those same bytes.
///
/// **Bits, not bytes.** One byte can carry eight flags, and 155,426 of the
/// reference project's channels are single bits; a key counted in bytes would
/// give all eight of them one identity and repeat the loss one level down.
pub type Key = (u16, u16, u32);

impl Channel {
	/// Whether this is the standard's OBD-II row: what [`available`] puts on
	/// the engine for a parameter SAE J1979 defines, asked of the row itself.
	/// Not inferred from a missing text id or from not being proven: a VCDS row
	/// can have neither, and it is this car's control unit's, not the
	/// standard's.
	pub fn is_standard(&self) -> bool {
		!self.proven
			&& self.request == ENGINE
			&& self.did >> 8 == 0xF4
			&& self
				.def
				.as_ref()
				.is_some_and(|def| vag_data_labels::obd::pid((self.did & 0xFF) as u8).is_some_and(|p| p.to_def() == *def))
	}

	/// This channel's identity: unit, identifier, and where in the response it
	/// starts. A channel nothing describes reads from byte 0, which is also
	/// where a lone field would be.
	pub fn key(&self) -> Key {
		(self.request, self.did, self.def.as_ref().map_or(0, |d| d.raw_form.bit_offset()))
	}

	/// How the unit is written on screen: its short number when this project
	/// has established one, otherwise its request id.
	pub fn unit(&self) -> String {
		UnitAddress::from_request(self.request)
			.map(|a| a.label())
			.unwrap_or_else(|| format!("{:03X}", self.request))
	}

	/// Column heading, in the order of how much it tells a reader: the label
	/// files' wording, then whatever the row's own source called it, then the
	/// address — because a channel nothing describes has nothing honest to be
	/// called.
	pub fn label(&self) -> String {
		if let Some(name) = &self.named {
			return name.clone();
		}
		match &self.def {
			Some(d) => d.name.to_string(),
			None => format!("{}/{:04X}", self.unit(), self.did),
		}
	}

	/// Whether anything at all describes this channel.
	///
	/// False means [`Self::label`] is the identifier written twice — the row
	/// the selection screen hides by default, because two thousand of them
	/// bury the ones a person can read. It is the *only* thing that decides
	/// that, so a channel that gains a name gains a place on the list with it.
	pub fn is_named(&self) -> bool {
		self.named.is_some() || self.def.is_some()
	}

	pub fn unit_of_measure(&self) -> &str {
		self.def.as_ref().map(|d| d.unit.as_ref()).unwrap_or("")
	}

	/// What to display for a response body.
	///
	/// A discrete state shows the state's name; a measured quantity shows its
	/// value; anything else shows its bytes tagged `(raw)`. Never a bare
	/// number for something unproven — a reader cannot tell those apart, and
	/// this project has twice caught itself believing an invented one.
	pub fn render(&self, data: &[u8]) -> String {
		let hex = || data.iter().map(|b| format!("{b:02X}")).collect::<String>();
		let Some(def) = &self.def else {
			return format!("{} (raw)", hex());
		};
		match def.describe(data) {
			Some(text) => text,
			None => format!("{} (raw)", hex()),
		}
	}
}

/// Which half of an actual/specified pair a measurement is.
///
/// A control unit publishes what it *asked for* and what it *got* as two
/// separate identifiers — boost pressure is `0x2029` specified and `0x202A`
/// actual. Read on two screen rows they say much less than side by side: the
/// gap between them is the whole diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
	Actual,
	Specified,
}

/// Suffixes that mark a measurement as one half of a pair.
///
/// Boost pressure is only the pair this project proved first — a gearbox
/// publishes specified and actual clutch pressure, an engine specified and
/// actual throttle angle, and so on. The label files write the distinction several
/// ways, so more than one spelling is recognised; the first two are what this
/// project's own catalogs use.
const ROLE_SUFFIXES: &[(&str, Role)] = &[
	(", actual", Role::Actual),
	(", specified", Role::Specified),
	(", current", Role::Actual),
	(", target", Role::Specified),
	(", requested", Role::Specified),
];

/// Split `"Boost pressure, actual"` into its base name and its role.
///
/// Matching is on the suffix only. A name that merely contains "actual"
/// somewhere is left alone: pairing two unrelated measurements onto one line
/// would present them as a comparison that nobody established.
pub fn split_role(name: &str) -> Option<(&str, Role)> {
	ROLE_SUFFIXES
		.iter()
		.find_map(|(suffix, role)| name.strip_suffix(suffix).map(|base| (base, *role)))
}

/// The engine's request id on the ISO block.
///
/// Not a fact about a particular car: ISO 15765-4 puts the first emissions
/// unit at `0x7E0`, which is also where the legislated OBD-II parameter set is
/// answered. Everything else about a unit comes from the car.
pub const ENGINE: u16 = 0x7E0;

/// What one control unit said about itself, which is how its catalog is found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitIdentity {
	pub request: u16,
	/// `F187`, the part number.
	pub part_number: Option<String>,
	/// `F19E`, the ODX label file the unit names for itself.
	pub odx_name: Option<String>,
	/// `F1A2`, the coding index whose leading three digits pick the variant.
	///
	/// Paired with `F19E` and never used alone: together they are what
	/// `vag_data_labels::label_files::odx_match` ranks a variant name by, and both come
	/// off the car, so the lookup stays something the vehicle answers rather
	/// than a table about one vehicle.
	pub odx_version: Option<String>,
	/// `F197`, the component string — what the unit calls itself. Used to
	/// label its tab; a unit that did not say goes by its number alone.
	pub component: Option<String>,
}

/// Everything on offer: the standard OBD-II parameters on the engine, then
/// whatever the catalog store holds for each unit the car reported.
///
/// No unit number appears here. A scaling belongs to the control unit that was
/// measured, so it is looked up by that unit's own part number — the same
/// mechanism works on a car this project has never seen, and finds nothing
/// rather than misapplying another car's numbers.
///
/// This is what is *known*; an identifier `--did` names that nothing here
/// describes is watched as bytes beside it.
pub fn available(store: &CatalogStore, extracted: &crate::extracted::Extracted, units: &[UnitIdentity]) -> Vec<Channel> {
	let mut out = Vec::new();
	for p in vag_data_labels::obd::PIDS {
		out.push(Channel {
			request: ENGINE,
			did: vag_data_labels::obd::did_for_pid(p.pid),
			def: Some(p.to_def()),
			// SAE J1979 names its own parameters, and there is no text id on a
			// standard row to look anything else up by.
			named: None,
			// The standard's, not this car's: `F40D` is one byte of km/h on the
			// engine by convention and demonstrably something else elsewhere.
			proven: false,
			text_id: None,
			selected: false,
		});
	}
	for unit in units {
		let request = unit.request;
		let defs = crate::extracted::tagged(
			store,
			extracted,
			unit.part_number.as_deref(),
			unit.odx_name.as_deref(),
			unit.odx_version.as_deref(),
		);
		for row in defs {
			let ReadId::Uds(did) = row.def.address;
			let bit_offset = row.def.raw_form.bit_offset();
			// A control unit's own proven row wins over the standard one at
			// the same address: they can mean different things. F40D is one
			// byte of km/h on the engine and two little-endian bytes on the
			// gearbox.
			//
			// **The same field, not the same identifier** — the rule [`Key`]
			// states and `extracted::tagged` enforces, and which this loop
			// missed: keyed by identifier alone it kept one field per response
			// and overwrote the rest, so a unit that packs two flags into one
			// byte offered the second and lost the first.
			if let Some(existing) = out
				.iter_mut()
				.find(|c| c.request == request && c.did == did && c.def.as_ref().map_or(0, |d| d.raw_form.bit_offset()) == bit_offset)
			{
				existing.def = Some(row.def);
				existing.named = row.named;
				existing.proven = row.proven;
				existing.text_id = row.text_id;
			} else {
				out.push(Channel {
					request,
					did,
					def: Some(row.def),
					named: row.named,
					proven: row.proven,
					text_id: row.text_id,
					selected: false,
				});
			}
		}
	}
	out
}

/// Parse a hex string as bytes; `None` if it is not whole bytes of hex.
///
/// **The inverse of the packed hex this tool writes into its own files**, and
/// public because every reader of that format needed it and each had grown a
/// copy — `watch`'s recording replay and the dash replay's columns today. Three
/// parsers for one format were three chances to disagree about a malformed one,
/// and they already did — one of the copies rejected the empty string and two
/// returned no bytes for it.
///
/// The empty string parses as no bytes, which is what "a hex string of length
/// zero" means. A caller for whom an empty *cell* is not a reading says so
/// itself; see `watch::replay::cell_to_bytes`.
pub fn hex_bytes(text: &str) -> Option<Vec<u8>> {
	// Hex is ASCII; anything else is not hex, and slicing it by byte could cut a
	// character in half.
	if text.len() % 2 != 0 || !text.is_ascii() {
		return None;
	}
	(0..text.len() / 2)
		.map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok())
		.collect()
}

/// Every `(request id, identifier)` the selected channels read, each once, in order.
///
/// One identifier answers every field cut from it, so two selected fields of one
/// identifier are one read. Grouping reads into requests, and how many identifiers go
/// in one, is the scheduler's business (`crate::bus`), not the plan's: the plan says
/// what is wanted, and the order is by unit and identifier so that anything listing
/// them — the table included — lists them the same way every time.
pub fn reads(channels: &[Channel]) -> Vec<(u16, u16)> {
	let wanted: std::collections::BTreeSet<(u16, u16)> = channels.iter().filter(|c| c.selected).map(|c| (c.request, c.did)).collect();
	wanted.into_iter().collect()
}

/// What to put on screen when the user asked for nothing in particular.
///
/// The things a driver would look at first: engine and shaft speeds, road
/// speed, boost, the pedal, the gear and the selector. Chosen by **what the
/// catalogs call them**, not by identifier — a rule of thumb over names is
/// data-driven and works on any car whose catalog uses the same words, where
/// a list of identifiers would be this Škoda written into the source.
///
/// Anything unproven is left out: a screenful of `(raw)` is a poor first
/// impression and teaches nothing.
const BASIC_MEASUREMENTS: &[&str] = &[
	"engine speed",
	"input shaft speed",
	"output shaft speed",
	"vehicle speed",
	"road speed",
	"boost pressure",
	"accelerator pedal",
	"selected gear",
	"selector lever",
	"coolant",
];

/// Select the basics, and report how many were found.
pub fn select_basics(channels: &mut [Channel]) -> usize {
	let mut count = 0;
	for channel in channels.iter_mut() {
		let Some(def) = &channel.def else { continue };
		let name = def.name.to_lowercase();
		if BASIC_MEASUREMENTS.iter().any(|basic| name.contains(basic)) {
			channel.selected = true;
			count += 1;
		}
	}
	count
}

/// Parse `01:2029,202A 714:2203` or a bare `2029,202A` (engine assumed).
///
/// The unit before the colon is whatever `vag_uds_client::address` accepts: a
/// short number for the units this project has established, or a request id
/// for the rest.
pub fn parse_spec(spec: &str) -> Result<Vec<(u16, u16)>, String> {
	let mut out = Vec::new();
	for group in spec.split_whitespace() {
		let (request, list) = match group.split_once(':') {
			Some((unit, rest)) => (vag_uds_client::address::parse(unit)?.request, rest),
			None => (ENGINE, group),
		};
		for did in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
			let did = u16::from_str_radix(did, 16).map_err(|_| format!("{did:?} is not a hex data identifier"))?;
			out.push((request, did));
		}
	}
	if out.is_empty() {
		return Err("no identifiers given".to_string());
	}
	Ok(out)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn hex_that_is_not_ascii_is_not_hex_rather_than_a_panic() {
		// `aéb` is four bytes, so the length check passed and the slice cut `é` in half.
		assert_eq!(hex_bytes("aéb"), None);
		assert_eq!(hex_bytes("éé"), None);
		assert_eq!(hex_bytes("0B34"), Some(vec![0x0B, 0x34]));
		assert_eq!(hex_bytes(""), Some(vec![]));
	}
	use std::borrow::Cow;
	use vag_data_labels::catalog::{ReadId, Scaling};
	use vag_data_labels::measure::{LinearScale, RawForm};

	/// Request ids of the reference car's units, for tests only — the code
	/// itself never names a unit by number.
	const GEARBOX: u16 = 0x7E1;
	const CLUSTER: u16 = 0x714;

	/// The reference car's own proven rows, when this machine has any.
	///
	/// They used to be committed under `catalogs/vehicles/` and are now one
	/// owner's measured data under `~/.vagcan/data/<id>/measurements`, like
	/// everybody
	/// else's — nothing measured on a vehicle lives in the checkout any more.
	/// So a machine that holds no proven rows has nothing to assert against,
	/// and these tests say so rather than failing over data they were never
	/// entitled to assume.
	fn measured_rows() -> Option<std::path::PathBuf> {
		let dir = crate::project::current().ok()?.measurements_dir();
		let any = std::fs::read_dir(&dir)
			.ok()?
			.flatten()
			.any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"));
		any.then_some(dir)
	}

	/// Give up on a test that needs rows this machine has not got.
	macro_rules! need_rows {
		() => {
			match measured_rows() {
				Some(dir) => dir,
				None => {
					eprintln!(
						"skipped: no proven rows in this machine's project — \
                         they are one owner's measured data, under ~/.vagcan"
					);
					return;
				}
			}
		};
	}

	/// The reference car's rows, and its identities.
	fn reference(dir: std::path::PathBuf) -> (CatalogStore, Vec<UnitIdentity>) {
		let store = CatalogStore::open(dir);
		let ident = |request, part: &str| UnitIdentity {
			request,
			part_number: Some(part.to_string()),
			odx_name: None,
			odx_version: None,
			component: None,
		};
		(
			store,
			vec![ident(ENGINE, "8V0906264H"), ident(GEARBOX, "0CW300041G"), ident(CLUSTER, "5E0920740D")],
		)
	}

	/// Two fields of one identifier are two channels, on this path too. The
	/// merge in `extracted::tagged` already kept them apart; this loop keyed
	/// by identifier and quietly overwrote the first with the second.
	#[test]
	fn two_fields_of_one_identifier_are_two_channels() {
		use vag_data_labels::catalog::MeasurementCatalog;
		use vag_data_labels::measure::{LinearScale, RawForm};
		let here = tempfile::tempdir().unwrap();
		let def = |name: &str, raw_form| MeasurementDef {
			name: name.to_string().into(),
			unit: "".into(),
			address: ReadId::Uds(0x3816),
			raw_form,
			scaling: Scaling::Linear(LinearScale { factor: 1.0, offset: 0.0 }),
		};
		let catalog = MeasurementCatalog::new(vec![def("gear", RawForm::U8First), def("clutch", RawForm::U8Second)]);
		std::fs::write(here.path().join("PART2.json"), catalog.to_json().unwrap()).unwrap();
		let store = CatalogStore::open(here.path().to_path_buf());
		let unit = UnitIdentity {
			request: GEARBOX,
			part_number: Some("PART2".to_string()),
			..UnitIdentity::default()
		};
		let got: Vec<(u16, u32, String)> = available(&store, &crate::extracted::Extracted::none(), &[unit])
			.into_iter()
			.filter(|c| c.request == GEARBOX)
			.map(|c| (c.did, c.key().2, c.label()))
			.collect();
		assert_eq!(got, vec![(0x3816, 0, "gear".to_string()), (0x3816, 8, "clutch".to_string())]);
	}

	fn reference_channels(dir: std::path::PathBuf) -> Vec<Channel> {
		let (store, units) = reference(dir);
		available(&store, &crate::extracted::Extracted::none(), &units)
	}

	fn known(request: u16, did: u16, name: &'static str) -> Channel {
		Channel {
			request,
			did,
			def: Some(MeasurementDef {
				name: Cow::Borrowed(name),
				unit: Cow::Borrowed("bar"),
				address: ReadId::Uds(did),
				raw_form: RawForm::U16Be,
				scaling: Scaling::Linear(LinearScale { factor: 0.001, offset: 0.0 }),
			}),
			named: None,
			proven: true,
			text_id: None,
			selected: true,
		}
	}

	#[test]
	fn every_selected_identifier_is_one_read_whatever_its_unit() {
		// How many go in one request is the scheduler's to decide; the plan lists
		// each read once, every unit's.
		let mut chans: Vec<Channel> = (0..10).map(|i| known(ENGINE, 0x2000 + i, "engine")).collect();
		chans.extend((0..3).map(|i| known(GEARBOX, 0x3800 + i, "gearbox")));

		let wanted = reads(&chans);
		assert_eq!(wanted.len(), 13, "{wanted:?}");
		assert_eq!(wanted.iter().filter(|(request, _)| *request == ENGINE).count(), 10);
		assert_eq!(wanted.iter().filter(|(request, _)| *request == GEARBOX).count(), 3);
	}

	#[test]
	fn unselected_channels_are_not_polled_and_duplicates_collapse() {
		let mut chans = vec![known(ENGINE, 0x2029, "boost"), known(ENGINE, 0x2029, "boost again")];
		chans.push(Channel {
			text_id: None,
			selected: false,
			..known(ENGINE, 0x206E, "rpm")
		});

		// Two fields of one identifier are one read: the answer carries both.
		assert_eq!(reads(&chans), vec![(ENGINE, 0x2029)]);
	}

	#[test]
	fn nothing_selected_plans_nothing() {
		let chans = vec![Channel {
			text_id: None,
			selected: false,
			..known(ENGINE, 0x2029, "boost")
		}];
		assert!(reads(&chans).is_empty());
	}

	#[test]
	fn the_polling_order_is_stable_across_cycles() {
		// Rows that reshuffle between cycles are unreadable, so the plan must
		// not depend on hash iteration order. The cluster sorts *before* the
		// powertrain because its id is lower — the order is by id, not by the
		// number people call the unit.
		let chans = vec![known(CLUSTER, 0x2203, "odo"), known(ENGINE, 0x206E, "rpm"), known(GEARBOX, 0x380A, "in")];
		let a = reads(&chans);
		assert_eq!(a, reads(&chans));
		assert_eq!(a.iter().map(|(request, _)| *request).collect::<Vec<_>>(), vec![CLUSTER, ENGINE, GEARBOX]);
	}

	#[test]
	fn a_units_own_row_overrides_the_standard_one_at_the_same_address() {
		// F40D is one byte of km/h on the engine (the OBD mirror) and two
		// little-endian bytes on the gearbox. Listing both under one entry
		// would make one of them wrong.
		let all = reference_channels(need_rows!());
		let engine = all.iter().find(|c| c.request == ENGINE && c.did == 0xF40D).unwrap();
		let gearbox = all.iter().find(|c| c.request == GEARBOX && c.did == 0xF40D).unwrap();
		assert_eq!(engine.def.as_ref().unwrap().raw_form, RawForm::U8First);
		assert_eq!(gearbox.def.as_ref().unwrap().raw_form, RawForm::U16Le);
	}

	#[test]
	fn an_unknown_channel_is_labelled_by_address_and_renders_raw() {
		let c = Channel {
			request: GEARBOX,
			did: 0x38F0,
			def: None,
			named: None,
			proven: false,
			text_id: None,
			selected: true,
		};
		assert_eq!(c.label(), "02/38F0");
		assert_eq!(c.render(&[0x0B, 0x34]), "0B34 (raw)");
		// A unit with no established short number is named by its id, not by a
		// guessed number.
		let brakes = Channel {
			request: 0x713,
			did: 0x1234,
			def: None,
			named: None,
			proven: false,
			text_id: None,
			selected: true,
		};
		assert_eq!(brakes.label(), "713/1234");
	}

	#[test]
	fn a_discrete_state_shows_its_name_and_an_unlisted_code_shows_raw() {
		let (store, _) = reference(need_rows!());
		let gear = store
			.load("0CW300041G")
			.unwrap()
			.into_iter()
			.find(|d| matches!(d.address, ReadId::Uds(0x3816)))
			.unwrap();
		let c = Channel {
			request: GEARBOX,
			did: 0x3816,
			def: Some(gear),
			named: None,
			proven: false,
			text_id: None,
			selected: true,
		};
		assert_eq!(c.render(&[0x05]), "4");
		assert_eq!(c.render(&[0x0C]), "R");
		assert_eq!(c.render(&[0x09]), "09 (raw)");
	}

	#[test]
	fn a_state_read_inside_its_band_shows_the_bands_name_on_the_live_screen() {
		// A lever read as a voltage answers anywhere in its band, a count or two
		// either side of any listed value. Synthetic bands, second field of two.
		let form = RawForm::for_field(8, 8, false, true).unwrap();
		let c = Channel {
			request: 0x70C,
			did: 0x1000,
			def: Some(vag_data_labels::catalog::MeasurementDef {
				name: Cow::Borrowed("Lever"),
				unit: Cow::Borrowed(""),
				address: ReadId::Uds(0x1000),
				raw_form: form,
				scaling: Scaling::Enum {
					levels: vec![
						vag_data_labels::Level::range(10, 49, "pulled"),
						vag_data_labels::Level::range(50, 99, "rest"),
					],
				},
			}),
			named: None,
			proven: false,
			text_id: None,
			selected: true,
		};
		assert_eq!(c.render(&[0xFF, 12]), "pulled");
		assert_eq!(c.render(&[0xFF, 77]), "rest");
		// Between bands, or past them, it is still bytes: nothing is guessed.
		assert_eq!(c.render(&[0xFF, 5]), "FF05 (raw)");
	}

	#[test]
	fn a_label_prefers_the_label_files_wording_over_the_projects_own() {
		// The reported defect, in one row: an ODIS long name is written for a
		// diagnostic engineer, and the same channel's text id reaches a
		// sentence. Both beat the identifier, which is what a row nothing
		// describes is left with.
		let odis = Channel {
			named: None,
			..known(ENGINE, 0x0283, "Brake_pedal_information_plausibility")
		};
		assert_eq!(odis.label(), "Brake_pedal_information_plausibility");
		assert!(odis.is_named());

		let from_labels = Channel {
			named: Some("Brake pedal plausibility".to_string()),
			..odis.clone()
		};
		assert_eq!(from_labels.label(), "Brake pedal plausibility");

		// Nothing describes it, so the label is the address — and this is the
		// row the selection screen has to be able to tell apart from the rest.
		let nameless = Channel {
			def: None,
			named: None,
			..odis
		};
		assert_eq!(nameless.label(), "01/0283");
		assert!(!nameless.is_named());
	}

	#[test]
	fn a_spec_names_control_units_by_number_or_by_request_id() {
		assert_eq!(parse_spec("2029,202A").unwrap(), vec![(ENGINE, 0x2029), (ENGINE, 0x202A)]);
		assert_eq!(
			parse_spec("01:2029 02:380A,3816").unwrap(),
			vec![(ENGINE, 0x2029), (GEARBOX, 0x380A), (GEARBOX, 0x3816)]
		);
		// The cluster by number and by id are the same unit.
		assert_eq!(parse_spec("17:2203").unwrap(), vec![(CLUSTER, 0x2203)]);
		assert_eq!(parse_spec("714:2203").unwrap(), parse_spec("17:2203").unwrap());
		// A unit with no established number is still reachable by its id.
		assert_eq!(parse_spec("713:1001").unwrap(), vec![(0x713, 0x1001)]);
		assert!(parse_spec("zz").is_err());
		assert!(parse_spec("").is_err());
	}

	#[test]
	fn the_default_selection_is_what_a_driver_would_look_at_first() {
		let (store, units) = reference(need_rows!());
		let mut channels = available(&store, &crate::extracted::Extracted::none(), &units);
		let count = select_basics(&mut channels);
		assert!(count >= 6, "found only {count}");

		let chosen: Vec<String> = channels.iter().filter(|c| c.selected).map(|c| c.label()).collect();
		for wanted in ["Engine speed", "Input shaft speed", "Selector lever"] {
			assert!(chosen.iter().any(|n| n == wanted), "{wanted} missing from {chosen:?}");
		}
		// Nothing unproven: a screenful of `(raw)` teaches nothing.
		assert!(channels.iter().filter(|c| c.selected).all(|c| c.def.is_some()));
		// And it is chosen by name, so a catalog using the same words works
		// on a car this project has never seen.
		assert!(chosen.iter().all(|n| BASIC_MEASUREMENTS.iter().any(|b| n.to_lowercase().contains(b))));
	}

	#[test]
	fn a_pair_is_recognised_by_its_suffix_in_any_of_the_spellings_used() {
		assert_eq!(split_role("Boost pressure, actual"), Some(("Boost pressure", Role::Actual)));
		assert_eq!(split_role("Clutch 1 pressure, specified"), Some(("Clutch 1 pressure", Role::Specified)));
		assert_eq!(split_role("Engine torque, requested"), Some(("Engine torque", Role::Specified)));
		// Not a pair: a name that merely mentions the word. Joining two
		// unrelated rows would show a comparison nobody established.
		assert_eq!(split_role("Actual gear"), None);
		assert_eq!(split_role("Engine speed"), None);
	}
}
