//! What to poll, and in what order — the part that can be tested without a car.
//!
//! One serial port means one conversation at a time, so reading measurements
//! that live on different control units is a sequence of re-addressed groups,
//! not a broadcast. This module decides the grouping; the live loop in the
//! parent module just walks it.
//!
//! A channel is keyed by the unit's **request id**, not by a unit number: the
//! two id blocks on this car have different response rules, and a number is
//! only a display convenience over the id (see `vag_protocol::address`).
//!
//! One exception to the no-car rule: [`read_batch`] performs the read a plan
//! describes. It lives here because it is the other half of [`Batch`] — a
//! reader who wants to know what a batch *is* and what asking for one costs
//! should not have to open two files — and because more than one live loop
//! needs it, and a copied one drifts.

use std::collections::BTreeMap;

use vag_data::catalog::{CatalogStore, MeasurementDef, ReadId};
use vag_protocol::address::UnitAddress;

/// Identifiers per request. Measured on the reference car: eight are answered,
/// twelve are refused outright, and asking for more than a unit accepts makes
/// every batch look empty rather than erroring.
pub const BATCH: usize = 8;

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
	pub selected: bool,
}

impl Channel {
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
	/// `vag_data::label_files::odx_match` ranks a variant name by, and both come
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
/// This is what is *known*. Everything else the car answers comes from a
/// survey file — see [`with_survey`] — because a measurement nobody has
/// proven still has bytes worth watching.
pub fn available(store: &CatalogStore, extracted: &crate::extracted::Extracted, units: &[UnitIdentity]) -> Vec<Channel> {
	let mut out = Vec::new();
	for p in vag_data::obd::PIDS {
		out.push(Channel {
			request: ENGINE,
			did: vag_data::obd::did_for_pid(p.pid),
			def: Some(p.to_def()),
			// SAE J1979 names its own parameters, and there is no text id on a
			// standard row to look anything else up by.
			named: None,
			// The standard's, not this car's: `F40D` is one byte of km/h on the
			// engine by convention and demonstrably something else elsewhere.
			proven: false,
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
			// A control unit's own proven row wins over the standard one at
			// the same address: they can mean different things. F40D is one
			// byte of km/h on the engine and two little-endian bytes on the
			// gearbox.
			if let Some(existing) = out.iter_mut().find(|c| c.request == request && c.did == did) {
				existing.def = Some(row.def);
				existing.named = row.named;
				existing.proven = row.proven;
			} else {
				out.push(Channel {
					request,
					did,
					def: Some(row.def),
					named: row.named,
					proven: row.proven,
					selected: false,
				});
			}
		}
	}
	out
}

/// What each unit in a survey said about itself.
///
/// A survey already asked every unit for its identification block, so a
/// recording of one carries the keys its catalogs are found under — no need to
/// re-read the car to know which scalings apply.
pub fn identities_from_survey(survey: &str) -> Vec<UnitIdentity> {
	let mut out = Vec::new();
	for line in survey.lines().filter(|l| !l.trim().is_empty()) {
		let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
			continue;
		};
		let Some(request) = value["request"].as_str().and_then(|s| u16::from_str_radix(s, 16).ok()) else {
			continue;
		};
		let field = |did: &str| -> Option<String> {
			let entry = value["ident"].as_array()?.iter().find(|e| e["did"].as_str() == Some(did))?;
			let bytes = hex_bytes(entry["data"].as_str()?)?;
			let text = String::from_utf8_lossy(&bytes).trim_end_matches(['\0', ' ']).to_string();
			(!text.is_empty()).then_some(text)
		};
		out.push(UnitIdentity {
			request,
			part_number: field("F187"),
			odx_name: field("F19E"),
			odx_version: field("F1A2"),
			component: field("F197"),
		});
	}
	out
}

/// Parse a hex string as bytes; `None` if it is not whole bytes of hex.
fn hex_bytes(text: &str) -> Option<Vec<u8>> {
	if text.len() % 2 != 0 {
		return None;
	}
	(0..text.len() / 2)
		.map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok())
		.collect()
}

/// Add every identifier a `vagcan survey` run found, on every unit it found
/// them on.
///
/// The survey is the only source that covers the whole car: the catalogs know
/// three units, the gateway lists fifteen more, and none of those fifteen has a
/// proven measurement yet. Their channels come through with no definition, so
/// they display as raw bytes — which is the honest rendering and is also
/// exactly what `vagcan recording calibrate` needs as input.
///
/// Identifiers already in `channels` keep their definition; a survey never
/// overrides a proven scaling with nothing.
pub fn with_survey(mut channels: Vec<Channel>, survey: &str) -> Vec<Channel> {
	for line in survey.lines().filter(|l| !l.trim().is_empty()) {
		let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
			continue;
		};
		let Some(request) = value["request"].as_str().and_then(|s| u16::from_str_radix(s, 16).ok()) else {
			continue;
		};
		let Some(dids) = value["dids"].as_array() else { continue };
		for entry in dids {
			let Some(did) = entry["did"].as_str().and_then(|s| u16::from_str_radix(s, 16).ok()) else {
				continue;
			};
			if channels.iter().any(|c| c.request == request && c.did == did) {
				continue;
			}
			channels.push(Channel {
				request,
				did,
				def: None,
				named: None,
				proven: false,
				selected: false,
			});
		}
	}
	channels.sort_by_key(|c| (c.request, c.did));
	channels
}

/// What a survey established about which identifiers this car actually answers.
///
/// Kept beside the channels rather than on them, because it is a different kind
/// of statement. A [`Channel`] is what some data source *declares*; this is what
/// the vehicle *did*, on a particular day, and the two disagree far more than
/// the design assumed: on the reference car an ODIS project declares 2,251
/// identifiers across the fifteen units, the car answered 1,198, and only 505
/// are in both. Nearly two thousand declared channels are on the selection
/// screen and can never produce a value.
///
/// Absence is only evidence about a unit the survey actually visited, which is
/// why `units` is kept alongside: a unit nobody swept says nothing about its
/// identifiers, and treating that as silence would hide a whole control unit
/// on the strength of never having looked at it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Answered {
	/// Units the survey visited. Only these can be argued about.
	pub units: std::collections::BTreeSet<u16>,
	/// `(request, did)` pairs that came back with a body.
	pub dids: std::collections::BTreeSet<(u16, u16)>,
}

impl Answered {
	/// Whether the car has been seen to answer this identifier.
	///
	/// `None` when nothing can be said — the unit was never swept — and that is
	/// deliberately distinct from `Some(false)`. A caller that collapses the two
	/// hides every channel on every unit the survey did not reach.
	pub fn saw(&self, request: u16, did: u16) -> Option<bool> {
		if !self.units.contains(&request) {
			return None;
		}
		Some(self.dids.contains(&(request, did)))
	}
}

/// Read [`Answered`] out of a survey file.
///
/// A caveat that belongs with the data rather than in a commit message: a
/// survey line records what *answered*, never what was *asked*. For a full
/// sweep — blind, or over everything the unit's own data declares — those are
/// the same question and absence means silence. For a run aimed with
/// `--blind --range`, they are not, and an identifier outside the range would
/// be read here as silent when nobody ever asked it. The fix is for a survey to
/// write down its own range; until it does, this is why the filter is a default
/// and not a deletion.
pub fn answered_from_survey(survey: &str) -> Answered {
	let mut out = Answered::default();
	for line in survey.lines().filter(|l| !l.trim().is_empty()) {
		let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
			continue;
		};
		let Some(request) = value["request"].as_str().and_then(|s| u16::from_str_radix(s, 16).ok()) else {
			continue;
		};
		let Some(dids) = value["dids"].as_array() else { continue };
		out.units.insert(request);
		for entry in dids {
			let Some(did) = entry["did"].as_str().and_then(|s| u16::from_str_radix(s, 16).ok()) else {
				continue;
			};
			out.dids.insert((request, did));
		}
	}
	out
}

/// One request: a control unit and the identifiers to ask it for at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
	pub request: u16,
	pub dids: Vec<u16>,
}

/// Group the selected channels into requests.
///
/// Grouped by control unit because addressing changes between them, then split
/// into [`BATCH`]-sized requests. Units come out in ascending order so the
/// polling sequence is stable — a screen whose rows reshuffle between cycles
/// is unreadable.
pub fn plan(channels: &[Channel]) -> Vec<Batch> {
	let mut by_unit: BTreeMap<u16, Vec<u16>> = BTreeMap::new();
	for c in channels.iter().filter(|c| c.selected) {
		let dids = by_unit.entry(c.request).or_default();
		// The same identifier twice in one request wastes a slot and makes the
		// response ambiguous to split.
		if !dids.contains(&c.did) {
			dids.push(c.did);
		}
	}
	by_unit
		.into_iter()
		.flat_map(|(request, dids)| {
			dids
				.chunks(BATCH)
				.map(|chunk| Batch {
					request,
					dids: chunk.to_vec(),
				})
				.collect::<Vec<_>>()
		})
		.collect()
}

/// What one batch read produced.
///
/// `NoAnswer` is explicit because a silent failure leaves the previous value on
/// screen and makes a collapsing poll rate undetectable: a caller that only
/// hears about answers cannot tell a value that is steady from a link that has
/// died, and both look like a table nobody is updating.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchOutcome {
	/// The unit answered. Possibly with fewer records than were asked for —
	/// a response that will not split into records comes back empty rather
	/// than as an error, which is what the car does when a request holds more
	/// identifiers than the unit accepts.
	Answered(Vec<(u16, Vec<u8>)>),
	/// Nothing was sent, so this says nothing about the car: either there is no
	/// adapter to ask with, or no addressing rule for this request id.
	Unaddressable,
	/// Asked, and the unit did not answer within its deadline.
	NoAnswer,
}

/// Read one batch of identifiers, and say when the answer arrived.
///
/// The returned time is seconds since `started`, taken the moment the request
/// resolves — it is the age of every record in the batch, and identifiers are
/// polled in groups, so columns in one cycle are up to a cycle apart.
///
/// **The adapter is `take()`n out of the `Option` and put back after the
/// await.** A dropped future therefore leaves the `Option` empty and the
/// adapter gone for the rest of the run, silently. Do not put this call in a
/// `select!`; drain the keyboard between batches instead, as `watch` does.
pub async fn read_batch<B: vag_can::CanBackend>(backend: &mut Option<B>, batch: &Batch, started: std::time::Instant) -> (f64, BatchOutcome) {
	use vag_can::IsoTpCan;
	use vag_protocol::AsyncUdsClient;
	use vag_transport::CanId;

	let elapsed = || started.elapsed().as_secs_f64();
	let Some(b) = backend.take() else {
		return (elapsed(), BatchOutcome::Unaddressable);
	};
	// Each unit is addressed by the rule its id block uses: the cluster
	// answers on 0x77E, not on 0x7E0 + 16, which is what treating the unit
	// number as an ISO index used to produce.
	let Some(address) = UnitAddress::from_request(batch.request) else {
		*backend = Some(b);
		return (elapsed(), BatchOutcome::Unaddressable);
	};
	let mut uds = AsyncUdsClient::new(IsoTpCan::new(b, CanId::Standard(address.request), CanId::Standard(address.response)));
	let answer = if batch.dids.len() == 1 {
		uds.read_data_by_identifier(batch.dids[0]).await.map(|d| vec![(batch.dids[0], d)])
	} else {
		uds
			.read_data_by_identifiers(&batch.dids)
			.await
			.map(|payload| crate::analyse::split_records(&payload, &batch.dids).unwrap_or_default())
	};
	let at = elapsed();
	*backend = Some(uds.into_transport().into_backend());
	match answer {
		Ok(records) => (at, BatchOutcome::Answered(records)),
		Err(_) => (at, BatchOutcome::NoAnswer),
	}
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
/// The unit before the colon is whatever `vag_protocol::address` accepts: a
/// short number for the units this project has established, or a request id
/// for the rest.
pub fn parse_spec(spec: &str) -> Result<Vec<(u16, u16)>, String> {
	let mut out = Vec::new();
	for group in spec.split_whitespace() {
		let (request, list) = match group.split_once(':') {
			Some((unit, rest)) => (vag_protocol::address::parse(unit)?.request, rest),
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
	use std::borrow::Cow;
	use vag_data::catalog::{ReadId, Scaling};
	use vag_data::measure::{LinearScale, RawForm};

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
	/// So a machine that has never calibrated a car has nothing to assert
	/// against, and these tests say so rather than failing over data they were
	/// never entitled to assume.
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
                         drive and calibrate a car to get some"
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
			selected: true,
		}
	}

	#[test]
	fn one_request_per_control_unit_per_eight_identifiers() {
		// The addressing changes between units, so a batch can never span two.
		let mut chans: Vec<Channel> = (0..10).map(|i| known(ENGINE, 0x2000 + i, "engine")).collect();
		chans.extend((0..3).map(|i| known(GEARBOX, 0x3800 + i, "gearbox")));

		let batches = plan(&chans);
		assert_eq!(batches.len(), 3, "8+2 on the engine, 3 on the gearbox: {batches:?}");
		assert_eq!(batches[0].request, ENGINE);
		assert_eq!(batches[0].dids.len(), 8);
		assert_eq!(batches[1].request, ENGINE);
		assert_eq!(batches[1].dids.len(), 2);
		assert_eq!(batches[2].request, GEARBOX);
		assert_eq!(batches[2].dids.len(), 3);
	}

	#[test]
	fn unselected_channels_are_not_polled_and_duplicates_collapse() {
		let mut chans = vec![known(ENGINE, 0x2029, "boost"), known(ENGINE, 0x2029, "boost again")];
		chans.push(Channel {
			selected: false,
			..known(ENGINE, 0x206E, "rpm")
		});

		let batches = plan(&chans);
		assert_eq!(batches.len(), 1);
		// Asking twice in one request wastes a slot and makes the answer
		// ambiguous to split.
		assert_eq!(batches[0].dids, vec![0x2029]);
	}

	#[test]
	fn nothing_selected_plans_nothing() {
		let chans = vec![Channel {
			selected: false,
			..known(ENGINE, 0x2029, "boost")
		}];
		assert!(plan(&chans).is_empty());
	}

	#[test]
	fn the_polling_order_is_stable_across_cycles() {
		// Rows that reshuffle between cycles are unreadable, so the plan must
		// not depend on hash iteration order. The cluster sorts *before* the
		// powertrain because its id is lower — the order is by id, not by the
		// number people call the unit.
		let chans = vec![known(CLUSTER, 0x2203, "odo"), known(ENGINE, 0x206E, "rpm"), known(GEARBOX, 0x380A, "in")];
		let a = plan(&chans);
		let b = plan(&chans);
		assert_eq!(a, b);
		assert_eq!(a.iter().map(|x| x.request).collect::<Vec<_>>(), vec![CLUSTER, ENGINE, GEARBOX]);
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
			selected: true,
		};
		assert_eq!(c.render(&[0x05]), "4");
		assert_eq!(c.render(&[0x0C]), "R");
		assert_eq!(c.render(&[0x09]), "09 (raw)");
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

	#[test]
	fn what_a_survey_recorded_reads_back_as_answered_and_the_rest_of_that_unit_does_not() {
		// Two claims and one non-claim, which is the whole of the type: a
		// listed identifier answered, an unlisted one on a swept unit did not,
		// and a unit nobody swept is not spoken for.
		let survey = "\
{\"request\":\"713\",\"dids\":[{\"did\":\"1001\",\"data\":\"00\"},{\"did\":\"F187\",\"data\":\"00\"}]}
{\"request\":\"7E1\",\"dids\":[]}
";
		let seen = answered_from_survey(survey);
		assert_eq!(seen.saw(0x713, 0x1001), Some(true));
		assert_eq!(seen.saw(0x713, 0xF187), Some(true));
		assert_eq!(seen.saw(0x713, 0x1002), Some(false), "asked, and it said nothing");
		assert_eq!(
			seen.saw(0x7E1, 0x1001),
			Some(false),
			"a unit that answered none of what it was asked was still asked"
		);
		assert_eq!(seen.saw(0x714, 0x1001), None, "nobody swept the cluster, so nothing is claimed about it");
	}

	#[test]
	fn a_malformed_survey_line_says_nothing_rather_than_claiming_silence() {
		// The failure that would matter: a line this parser cannot read must
		// not register its unit as swept, or every channel on that unit reads
		// as silent and the unit vanishes from the list.
		let seen = answered_from_survey("not json\n{\"request\":\"zz\"}\n{\"request\":\"713\"}\n\n");
		assert!(seen.units.is_empty(), "no unit was established");
		assert_eq!(seen.saw(0x713, 0x1001), None, "a line with no did array is not a sweep of that unit");
	}

	#[test]
	fn this_machines_own_survey_reads_back_consistently() {
		// Run against the owner's real cached survey where there is one, and
		// skipped everywhere else. It asserts the relationship rather than the
		// counts: the numbers belong to one car — 505 of 2,251 declared
		// identifiers answered, on 2026-08-09 — and a car's numbers are not
		// something to write into the program.
		let Some(path) = cached_survey() else {
			eprintln!("skipped: this machine has no cached survey");
			return;
		};
		let text = std::fs::read_to_string(&path).expect("the survey reads");
		let seen = answered_from_survey(&text);
		assert!(!seen.units.is_empty(), "{} has units in it", path.display());
		assert!(!seen.dids.is_empty(), "{} has identifiers in it", path.display());
		for (request, did) in &seen.dids {
			assert_eq!(seen.saw(*request, *did), Some(true), "everything recorded reads back as answered");
			assert!(seen.units.contains(request), "an answer implies its unit was swept");
		}
		// And the whole point: on a unit that was swept, an identifier nobody
		// recorded is a definite no rather than a shrug.
		let unit = *seen.units.iter().next().expect("checked above");
		let absent = (0u16..=u16::MAX)
			.find(|d| !seen.dids.contains(&(unit, *d)))
			.expect("no unit answers all 65,536");
		assert_eq!(seen.saw(unit, absent), Some(false));
	}

	/// The first cached survey this machine holds, whichever car it belongs to.
	fn cached_survey() -> Option<std::path::PathBuf> {
		let cars = crate::datadir::vagcan_dir().ok()?.join("cars");
		std::fs::read_dir(cars)
			.ok()?
			.flatten()
			.map(|e| e.path().join(crate::datadir::SURVEY_FILE))
			.find(|p| p.is_file())
	}

	#[test]
	fn a_survey_adds_every_unit_it_found_without_overriding_a_proven_scaling() {
		// The whole point of surveying: units the catalogs know nothing about
		// become watchable, as raw bytes, on the strength of having answered.
		let survey = "\
{\"request\":\"70E\",\"unit\":\"09\",\"dids\":[{\"did\":\"190B\",\"data\":\"02240010\"},\
{\"did\":\"192F\",\"data\":\"0305AA11\"}]}
{\"request\":\"7E0\",\"unit\":\"01\",\"dids\":[{\"did\":\"2029\",\"data\":\"0B34\"}]}
";
		let channels = with_survey(reference_channels(need_rows!()), survey);
		let bcm: Vec<&Channel> = channels.iter().filter(|c| c.request == 0x70E).collect();
		assert_eq!(bcm.len(), 2, "both BCM identifiers are on offer");
		assert!(bcm.iter().all(|c| c.def.is_none()), "nothing proven, so nothing claimed");
		assert_eq!(bcm[0].label(), "09/190B");

		// 2029 is a proven engine measurement; the survey must not blank it.
		let boost = channels
			.iter()
			.find(|c| c.request == 0x7E0 && c.did == 0x2029)
			.expect("the engine row survives");
		assert!(boost.def.is_some());
	}

	#[test]
	fn a_malformed_survey_line_is_skipped_rather_than_fatal() {
		let dir = need_rows!();
		let before = reference_channels(dir.clone()).len();
		let channels = with_survey(reference_channels(dir), "not json\n{\"request\":\"zz\"}\n\n");
		assert_eq!(channels.len(), before);
	}
}
