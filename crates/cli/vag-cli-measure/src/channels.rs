//! Which channels a run reads, chosen by what the label files **call** them:
//! by VW's text id first; by the words in the name, for a row with no id where
//! no usable row carries one of the role's; and, for an engine the project has
//! no rows for, by SAE J1979's own identifiers.
//!
//! Nothing here names a car's identifier or control unit. Which identifier
//! carries road speed, and on which control unit, is a fact about one car. What
//! is shared is the vocabulary: an ODIS project and a VCDS registry give the
//! same channel the same text id while wording its name differently — joined by
//! id, the two sources' names agree on 34.6 % of the rows (357 of 1,033,
//! `research/vcds-registry/README.md`); the reference engine's boost is
//! `IDE00191` in both, under a name the two word differently. So every role
//! lists its ids, best first, and is found by them over the rows the car's own
//! units report catalogs for — and, on an engine whose variant the project has
//! no rows for, over the legislated SAE J1979 set standing in. The words match as
//! [`crate::plan::select_basics`] matches the basics — a substring of the
//! lower-cased name — and when they apply is this module's rule: to a row a
//! drive proved always, because its name is its prover's and a proven field no
//! source declares has no id to be found by; to any other row only when it
//! carries no text id itself and no usable row on the car carries one of the
//! role's ids, as in a project set up before ids were recovered. A row under
//! another id is another quantity whatever its name says — an air-flow ceiling
//! is named with the air mass's words. A J1979 row carries no id and is a hit
//! at the PID the standard defines for the quantity. It is predicted for the
//! engine alone, and only when the project declares no OBD-II row for the
//! engine's variant: a variant that declares its OBD-II rows says what the
//! engine answers and outranks the standard's prediction — the reference
//! engine answered 38 `F4xx` values in its parked survey besides the four
//! support bitmaps (`research/dumps/survey-parked.jsonl`, kept out of git, on
//! the owner's machine), its variant declares every one of them and no `F410`,
//! which a prediction would have polled for nothing. A variant of one unrelated
//! row — the reference project has such stubs, a key counter and nothing else —
//! declares nothing of the kind, and the prediction stands in. The basis is
//! VW's files, not the standard's addressing: every engine variant in the
//! reference project that declares `F40C`, `F40D`, `F449`, `F410`, `F433` or
//! `F446` declares it in J1979's layout, while the gearboxes do not — all ten
//! declare `F40C` at factor 1 where J1979 has a quarter, and the six DSG
//! variants `F40D` as sixteen bits of 0.01 km/h — so no other unit is offered
//! the set at all. And a row
//! counts only in the physical units the role's
//! consumers read it in — a cluster's boost gauge in `%` of a dial sits under
//! the same id as boost in bar, and rows in `%/s` under the pedal's id are not
//! a position — so a wrong unit is no channel rather than a wrong number. A car
//! whose files use the same ids or the same words works without a line of this
//! module changing, and a car with neither is told what was looked for rather
//! than given another car's numbers.
//!
//! Two consequences of that shape are worth stating at the door.
//!
//! **The leading unit is derived, not declared.** It is whichever unit owns the
//! speed channel that won resolution. Writing "the gearbox" would be one
//! particular car's accident: on the reference car three units publish road
//! speed at the same 0.01 km/h — the gearbox, the body controller and the
//! telematics unit — while the cluster publishes whole km/h and the engine's
//! OBD mirror is a single byte. That makes the tie-break load-bearing, because
//! more than one unit answers to the same id with a copy of road speed. So
//! equal steps fall first to a row a drive proved, then to a **powertrain**
//! unit — one at ISO 15765-4's emissions addresses, the engine and the gearbox
//! — and only then to the request id. Measure's roles are powertrain
//! quantities; the other units hold copies of them, for display, comfort or
//! telematics, or event records such as park assist's snapshots. Before this
//! rule the request id decided, and the body controller led the reference car.
//! (Nothing is claimed about how fresh a copy is: no latency was measured.)
//!
//! **A channel that will not resolve is a channel this command does without.**
//! `measure` is an instrument, not a search: an unproven byte cannot be timed,
//! integrated or differentiated. So a row is admitted only when its meaning is
//! whole — a fully linear scaling for a quantity, an enumeration for a state —
//! and a required role that finds none is a refusal naming what it tried, never
//! an empty column and never raw bytes.

use vag_data_labels::catalog::{CatalogStore, MeasurementDef, ReadId, Scaling};
use vag_uds_client::address::UnitAddress;

use crate::plan::{self, UnitIdentity};

/// The stopwatch's own channel — the one every mark is timed from.
const SPEED: &str = "speed";
/// A speed on some other unit: read, and never used for timing.
///
/// It earns its place twice over — as a cross-check on the leading channel,
/// and because two speeds with their own timestamps are what makes a unit's
/// refresh rate observable at all.
const CROSS_SPEED: &str = "cross-check speed";

/// One resolved channel: what it is for, where to ask for it, and how to read
/// the answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
	/// The role this channel fills, as the session file and the screen name it.
	pub key: &'static str,
	pub request: u16,
	pub did: u16,
	pub def: MeasurementDef,
}

impl Resolved {
	/// How this channel is written down as a source — `"7E1:F40D"`.
	///
	/// The winner of the speed tie-break goes into the session file's
	/// `config.speed_source` in this form, so that two runs are never compared
	/// across a silent change of which unit was timing them.
	pub fn source(&self) -> String {
		format!("{:03X}:{:04X}", self.request, self.did)
	}

	/// The value in the response's data bytes, or `None` when they carry no
	/// value this definition can honestly produce.
	///
	/// Deliberately **not** [`crate::plan::Channel::render`]: that falls
	/// back to `"… (raw)"`, which is exactly the class of number this command
	/// excludes. `watch` shows raw bytes because its job is to *find*
	/// measurements; here `None` means the channel is absent for this sample,
	/// not that there are bytes worth showing.
	pub fn value(&self, data: &[u8]) -> Option<f64> {
		self.def.interpret(data)
	}

	/// The catalog's own label for a discrete state — a gear, a selector
	/// position — or `None` for a code it does not list.
	///
	/// Labels, never codes: this car's gear codes are neither contiguous nor
	/// ordered by ratio and two of the levels are not gears at all. An unlisted
	/// code is an admission, not a claim, and it enters no derived figure.
	pub fn state(&self, data: &[u8]) -> Option<String> {
		self.def.describe(data)
	}
}

/// Everything a run polls, split by the cadence it is polled at.
///
/// The split is by unit and not by taste: everything on the unit that owns the
/// leading speed is read at the leading rate, everything else half as often, and
/// the speed itself alone at the stopwatch's. Marks are timed from the leading
/// speed alone, so its rate is the only one that sets a stopwatch.
#[derive(Debug, Clone, PartialEq)]
pub struct Set {
	/// The speed channel that won, and by owning it, the unit that leads.
	pub leading: Resolved,
	/// Everything on the leading unit, the speed included.
	pub leading_unit: Vec<Resolved>,
	/// Everything on every other unit.
	pub background: Vec<Resolved>,
	/// The speeds that did not win. These also appear in one of the lists
	/// above — the list exists so the caller can tell which rows are speeds
	/// without matching names a second time.
	pub cross_check_speeds: Vec<Resolved>,
}

impl Set {
	/// Every channel that will be polled, in priority order.
	pub fn all(&self) -> impl Iterator<Item = &Resolved> {
		self.leading_unit.iter().chain(self.background.iter())
	}
}

/// A required role that nothing answered to, and what it answered to nothing
/// under: the ids, then the words, in the order they were tried.
///
/// That list is the whole of the report on purpose: a car this project has
/// never seen fails here at a standstill, and the only useful thing to tell
/// its owner is which ids or which words their label files would have to use.
#[derive(Debug, Clone, PartialEq)]
pub struct Missing {
	pub key: &'static str,
	pub tried: Vec<String>,
}

/// What a role's rows have to be for its answers to mean anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wants {
	/// A quantity with a fully proven linear scaling. An anchored row proves
	/// one point and nothing between it and the next, so it cannot be
	/// differentiated or integrated and is not admitted.
	Quantity,
	/// A discrete state, read as the catalog's own label.
	State,
}

/// How a role chooses among several hits. Both orders are total, so the choice
/// is the same on every run whatever order the car reported its units in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prefer {
	/// The finest quantisation wins — the step in m/s, so a row in m/s and one
	/// in km/h compare as speeds. This is the stopwatch's rule and only the
	/// stopwatch's: the leading channel's step is the step every mark is
	/// resolved to, and a whole km/h would quantise a 0-100 to tenths of a
	/// second. Equal steps fall to a drive-proven row, then to a powertrain
	/// unit, then to request id and identifier — see the module doc for why.
	Finest,
	/// Evidence before inference, which is the rule this project already
	/// applies wherever two sources meet at one address: a row a drive proved
	/// first; then a powertrain unit, so a copy on another unit beats neither
	/// the engine's nor the gearbox's row whichever id each is under; then the
	/// id the role lists first (a word hit and a J1979 row rank after every id
	/// hit); then the unit's own row before the standard table's prediction;
	/// then request id and identifier.
	Proven,
}

/// The physical units a quantity role's rows are read in — the units its
/// consumers assume: [`crate::speed_to_ms`] converts the speed,
/// `density_from` takes the barometer in kPa and the ambient sensor in °C,
/// `report`'s `boost_reference` reads boost in bar, and the coastdown's
/// foot-off check (`coastdown::PEDAL_ZERO_PERCENT`, fed by `setup`) reads the
/// pedal in %. A row in any other unit is not the channel, whatever id it is
/// under. The units are the quantities' physical conventions, not a fact about
/// a car.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Units {
	/// Whatever [`crate::speed_to_ms`] converts: the stopwatch's one list of
	/// speed units, not a second copy of it.
	Speed,
	/// Exactly one of these spellings, trimmed.
	OneOf(&'static [&'static str]),
	/// A state carries no unit of measure.
	Unitless,
}

/// One thing a run wants to read, described by what label files call it —
/// VW's ids and the English words — rather than by where it lives.
struct RoleSpec {
	key: &'static str,
	/// The text ids this role answers to, best first. An id is VW's key for a
	/// piece of text, not a fact about a car: ODIS and VCDS share it, and it
	/// survives the wording. Evidence for each, cited at the role, is how many of
	/// the reference project's 717 variants declare a row under it in the role's
	/// units (for a state, as an enumeration).
	ids: &'static [&'static str],
	/// SAE J1979's PID for the quantity, where the standard defines one. The
	/// legislated row at that PID carries no text id and is a hit by this. It is
	/// predicted for the engine alone ([`plan::ENGINE`]), and only when the
	/// project declares no OBD-II row for the engine's variant ([`declares_obd`]). The basis
	/// is VW's files, not the standard's addressing: of the reference project's
	/// 385 engine variants, 381, 379, 381, 255, 381 and 380 declare `F40C`,
	/// `F40D`, `F449`, `F410`, `F433` and `F446`, every one in J1979's layout,
	/// while the gearboxes declare `F40D` as sixteen bits of 0.01 km/h (the six
	/// DSG variants) or J1979's eight (the four AQ automatics and the base
	/// variant) and `F40C` at factor 1 on all ten — a prediction there would read
	/// `F40C` at a quarter of its value (byte-swapped where it is little-endian)
	/// and a DSG's `F40D` as one byte of sixteen.
	pid: Option<u8>,
	/// Matched as a substring of the lower-cased name, exactly as
	/// [`plan::select_basics`] matches the basics — by a drive-proven row
	/// always, by any other only when it carries no text id itself and no usable
	/// row on the car carries one of `ids`.
	names: &'static [&'static str],
	wants: Wants,
	units: Units,
	prefer: Prefer,
	/// A missing one of these is a refusal: a run with no speed has no
	/// stopwatch, and a run with no engine speed, gear or pedal explains
	/// nothing about the time it did measure.
	required: bool,
	/// Read only under `--full`. These exist solely to feed the power model,
	/// and a cycle spent on a number nobody will look at is a cycle not spent
	/// on speed.
	full_only: bool,
	/// Which half of an actual/specified pair, where a unit publishes one.
	pair: Option<plan::Role>,
}

/// Every role this command knows how to fill, by the key the session file uses.
///
/// Reachable so that a reader can turn a file's channel names back into the
/// roles they came from. A reader that carried its own list would drift from
/// the writer's the first time a role was added, and the drift would show up as
/// a channel quietly missing from a recomputed run rather than as a failure.
pub fn known_roles() -> impl Iterator<Item = &'static str> {
	ROLES.iter().map(|spec| spec.key)
}

/// Rotational speed as the label files spell it: ODIS writes `1/min`, VCDS and
/// SAE J1979's row `/min`, and `rpm` is the same unit in English.
const PER_MINUTE: Units = Units::OneOf(&["/min", "1/min", "rpm"]);

/// The roles, in the order they earn their place in a request.
///
/// The order is the priority order: where a unit has more rows than a single
/// request holds, the ones lower down are what goes. Cross-check speeds follow
/// the leading speed immediately, because a second speed is what makes the
/// leading one's refresh period observable.
const ROLES: &[RoleSpec] = &[
	RoleSpec {
		key: SPEED,
		// Vehicle speed: in km/h in 474 variants, in m/s in 3 (radars).
		ids: &["IDE00075"],
		// SAE J1979 PID 0D, vehicle speed.
		pid: Some(0x0D),
		names: &["vehicle speed", "road speed"],
		wants: Wants::Quantity,
		units: Units::Speed,
		prefer: Prefer::Finest,
		required: true,
		full_only: false,
		pair: None,
	},
	RoleSpec {
		key: "engine speed",
		// The crankshaft speed, 62 variants, before the engine RPM that is also
		// the OBD mirror's, 472: the engine's own row before the legislated copy.
		ids: &["IDE00405", "IDE00021"],
		// SAE J1979 PID 0C, engine speed.
		pid: Some(0x0C),
		names: &["engine speed"],
		wants: Wants::Quantity,
		units: PER_MINUTE,
		prefer: Prefer::Proven,
		required: true,
		full_only: false,
		pair: None,
	},
	RoleSpec {
		key: "gear",
		// The gearbox's displayed gear, 6 variants — all four DSG families of the
		// project (DQ200, DQ250, DQ381, DQ500) — before the engine's selected
		// gear, 62 engine variants.
		ids: &["IDE02736", "IDE00090"],
		pid: None,
		names: &["selected gear"],
		wants: Wants::State,
		units: Units::Unitless,
		prefer: Prefer::Proven,
		required: true,
		full_only: false,
		pair: None,
	},
	RoleSpec {
		key: "pedal",
		// Accelerator pedal position in %, 418 variants. The 10 rows in %/s under
		// the same id are named as the position and are not one.
		ids: &["IDE00086"],
		// SAE J1979 PID 49, accelerator pedal position D.
		pid: Some(0x49),
		names: &["accelerator pedal position"],
		wants: Wants::Quantity,
		units: Units::OneOf(&["%"]),
		prefer: Prefer::Proven,
		required: true,
		full_only: false,
		pair: None,
	},
	RoleSpec {
		key: "selector",
		// The plausibilised shift position, 6 variants (the DSG families), before
		// the plain shift position, 13: all ten gearbox variants and three of a
		// steering-column module family.
		ids: &["IDE02719", "IDE00096"],
		pid: None,
		names: &["selector lever"],
		wants: Wants::State,
		units: Units::Unitless,
		prefer: Prefer::Proven,
		required: false,
		full_only: false,
		pair: None,
	},
	RoleSpec {
		key: "input shaft speed",
		// 7 variants: all four DSG families and the transmission base variant.
		ids: &["IDE00022"],
		pid: None,
		names: &["input shaft speed"],
		wants: Wants::Quantity,
		units: PER_MINUTE,
		prefer: Prefer::Proven,
		required: false,
		full_only: false,
		pair: None,
	},
	RoleSpec {
		key: "output shaft speed",
		// 6 variants: three of the four DSG families — the DQ250's variant does
		// not declare it — and the transmission base variant.
		ids: &["IDE00023"],
		pid: None,
		names: &["output shaft speed"],
		wants: Wants::Quantity,
		units: PER_MINUTE,
		prefer: Prefer::Proven,
		required: false,
		full_only: false,
		pair: None,
	},
	// Actual before specified: that is the order `watch` already puts a pair
	// in, and the gap between the two is the whole diagnostic. The ids carry
	// the pair themselves — actual and specified are two ids — so by id there
	// is no suffix to split; `pair` applies to the words alone.
	RoleSpec {
		key: "boost actual",
		// In bar in 22 variants. The 12 in % under the same id are clusters'
		// boost gauges, in % of the dial.
		ids: &["IDE00191"],
		pid: None,
		names: &["boost pressure"],
		wants: Wants::Quantity,
		units: Units::OneOf(&["bar"]),
		prefer: Prefer::Proven,
		required: false,
		full_only: false,
		pair: Some(plan::Role::Actual),
	},
	RoleSpec {
		key: "boost specified",
		// In bar in 58 variants.
		ids: &["IDE00190"],
		pid: None,
		names: &["boost pressure"],
		wants: Wants::Quantity,
		units: Units::OneOf(&["bar"]),
		prefer: Prefer::Proven,
		required: false,
		full_only: false,
		pair: Some(plan::Role::Specified),
	},
	RoleSpec {
		key: "air mass",
		// The air flow rate from the MAF sensor, in g/s in 260 variants. The
		// reference car's engine carries the id only as a support bit — a state —
		// and its known variant declares no row at PID 10, so there the role
		// resolves to nothing.
		ids: &["IDE00347"],
		// SAE J1979 PID 10, mass air flow rate.
		pid: Some(0x10),
		names: &["mass air flow", "air mass"],
		wants: Wants::Quantity,
		units: Units::OneOf(&["g/s", "kg/h"]),
		prefer: Prefer::Proven,
		required: false,
		full_only: false,
		pair: None,
	},
	RoleSpec {
		key: "barometer",
		// In kPa in 399 variants.
		ids: &["IDE00474"],
		// SAE J1979 PID 33, absolute barometric pressure.
		pid: Some(0x33),
		names: &["barometric pressure"],
		wants: Wants::Quantity,
		units: Units::OneOf(&["kPa"]),
		prefer: Prefer::Proven,
		required: false,
		full_only: true,
		pair: None,
	},
	RoleSpec {
		key: "ambient",
		// In °C in 398 variants.
		ids: &["IDE00556"],
		// SAE J1979 PID 46, ambient air temperature.
		pid: Some(0x46),
		names: &["ambient air temperature", "outside air temperature"],
		wants: Wants::Quantity,
		units: Units::OneOf(&["°C"]),
		prefer: Prefer::Proven,
		required: false,
		full_only: true,
		pair: None,
	},
];

/// One row on offer, and where its meaning came from.
struct Candidate {
	request: u16,
	did: u16,
	def: MeasurementDef,
	/// True for a row this unit's own files carry — proven on its part number
	/// or declared for its variant. False for one the standard table predicts
	/// from the unit's address alone.
	own: bool,
	/// True when a drive established this scaling on a car. Neither a declared
	/// row nor a standard one is — see [`crate::extracted::tagged`].
	drive_proven: bool,
	/// The row's text id, when its source carried one. A drive-proven row has
	/// the id of the field the project declares at the same identifier and
	/// offset: proving a channel must not lose it its name.
	text_id: Option<String>,
}

/// Whether a request id is one of ISO 15765-4's emissions addresses — the
/// engine and the gearbox, the powertrain: between equal candidates these
/// units are read first.
fn powertrain(request: u16) -> bool {
	UnitAddress::from_request(request).is_some_and(|address| address.is_emissions_related())
}

/// Whether the project declares any OBD-II row for this unit's variant: an
/// identifier in the block SAE J1979's PIDs map into over UDS, `F400`–`F4FF`
/// ([`vag_data_labels::obd::did_for_pid`]).
///
/// Not "any row at all": the reference project holds engine variants of one
/// row — a key counter, a project id — which say nothing about what the engine
/// answers, and taking them as a declaration cost the engine its speed, its
/// engine speed and its pedal.
fn declares_obd(extracted: &crate::extracted::Extracted, unit: &UnitIdentity) -> bool {
	let block = vag_data_labels::obd::did_for_pid(0x00)..=vag_data_labels::obd::did_for_pid(0xFF);
	extracted
		.for_unit(unit.odx_name.as_deref(), unit.odx_version.as_deref())
		.iter()
		.any(|def| matches!(def.address, ReadId::Uds(did) if block.contains(&did)))
}

/// Everything readable on the units the car reported.
///
/// The standard SAE J1979 set is predicted for the engine alone, and only when
/// the project declares no OBD-II row for the engine's variant
/// ([`declares_obd`]). A variant that declares its OBD-II rows says what the
/// engine answers and outranks the prediction: the reference engine's declares
/// every `F4xx` value it answered and no `F410`, which the prediction would
/// have polled for nothing. No other unit is offered the set — gearboxes
/// declare `F40C`, and the DSG ones `F40D`, in layouts of their own
/// ([`RoleSpec::pid`]).
/// Proven rows are a drive's evidence, not the project's declaration, and do
/// not count against the prediction. Where the engine's own row covers
/// an identifier the standard also names, the row wins: the two can mean
/// different things there, and one of them would otherwise be silently wrong.
fn candidates(store: &CatalogStore, extracted: &crate::extracted::Extracted, units: &[UnitIdentity]) -> Vec<Candidate> {
	let mut out: Vec<Candidate> = Vec::new();
	for unit in units {
		let request = unit.request;
		if request == plan::ENGINE && !declares_obd(extracted, unit) {
			for p in vag_data_labels::obd::PIDS {
				out.push(Candidate {
					request,
					did: vag_data_labels::obd::did_for_pid(p.pid),
					def: p.to_def(),
					own: false,
					drive_proven: false,
					text_id: None,
				});
			}
		}
		for row in crate::extracted::tagged(
			store,
			extracted,
			unit.part_number.as_deref(),
			unit.odx_name.as_deref(),
			unit.odx_version.as_deref(),
		) {
			let ReadId::Uds(did) = row.def.address;
			// One row per identifier: measure reads an identifier once, as one
			// channel ([`push`]). The unit's own row replaces the standard's there;
			// of the unit's own rows the first stays, because `tagged` gives them
			// in the order the sources rank — proven, ODIS, VCDS — and a later field
			// of the same response replacing it let the lower-ranked source win.
			match out.iter_mut().find(|c| c.request == request && c.did == did) {
				Some(standard) if !standard.own => {
					standard.def = row.def;
					standard.own = true;
					standard.drive_proven = row.proven;
					standard.text_id = row.text_id;
				}
				Some(_) => {}
				None => out.push(Candidate {
					request,
					did,
					def: row.def,
					own: true,
					drive_proven: row.proven,
					text_id: row.text_id,
				}),
			}
		}
	}
	out
}

/// Whether a row's scaling and unit are ones the role can use at all.
fn usable(spec: &RoleSpec, def: &MeasurementDef) -> bool {
	let scaling = match spec.wants {
		Wants::Quantity => matches!(def.scaling, Scaling::Linear(_)),
		Wants::State => matches!(def.scaling, Scaling::Enum { .. }),
	};
	let unit = match spec.units {
		Units::Speed => crate::speed_to_ms(&def.unit, 1.0).is_some(),
		Units::OneOf(units) => units.contains(&def.unit.trim()),
		Units::Unitless => true,
	};
	scaling && unit
}

/// Whether a row is the engine's legislated one for the role: the standard
/// table's row — predicted only for an engine whose variant declares no OBD-II row, and
/// only where no row of the engine's own claimed the identifier
/// ([`candidates`]) — at the PID SAE J1979 defines for the quantity, in a form
/// the role can use.
fn by_standard(spec: &RoleSpec, candidate: &Candidate) -> bool {
	!candidate.own && spec.pid.is_some_and(|pid| candidate.did == vag_data_labels::obd::did_for_pid(pid)) && usable(spec, &candidate.def)
}

/// Where a candidate's id stands in the role's list, or one past the end for
/// a row under none of them — a word hit, which ranks after every id hit.
fn id_rank(spec: &RoleSpec, candidate: &Candidate) -> usize {
	candidate
		.text_id
		.as_deref()
		.and_then(|id| spec.ids.iter().position(|wanted| *wanted == id))
		.unwrap_or(spec.ids.len())
}

/// Whether a row answers to one of a role's ids, in a form the role can use.
fn by_id(spec: &RoleSpec, candidate: &Candidate) -> bool {
	usable(spec, &candidate.def) && id_rank(spec, candidate) < spec.ids.len()
}

/// Whether a row answers to a role's words, in a form the role can use.
fn matches(spec: &RoleSpec, def: &MeasurementDef) -> bool {
	if !usable(spec, def) {
		return false;
	}
	// A paired role matches on the base name and insists on its own half:
	// reading specified boost as actual would present a request as a result.
	let name = match spec.pair {
		Some(wanted) => match plan::split_role(&def.name) {
			Some((base, found)) if found == wanted => base.to_lowercase(),
			_ => return false,
		},
		None => def.name.to_lowercase(),
	};
	spec.names.iter().any(|word| name.contains(word))
}

/// What a role was looked for under, in the order it was tried: ids, then words.
fn tried(spec: &RoleSpec) -> Vec<String> {
	spec.ids.iter().chain(spec.names.iter()).map(|s| s.to_string()).collect()
}

/// The size of one step of a row's value, which is what "finest" means — for a
/// speed in m/s through [`crate::speed_to_ms`], so a row in m/s and one in
/// km/h compare as speeds: 1/256 m/s is a smaller factor than 0.01 km/h and a
/// coarser step.
///
/// A row that is not linear has no step, nor has a speed in a unit the
/// conversion does not know. Only [`Wants::Quantity`] roles rank by fineness
/// and [`usable`] admits neither, so the fallback ranks last rather than
/// inventing an order.
fn step(spec: &RoleSpec, def: &MeasurementDef) -> f64 {
	let Scaling::Linear(scale) = &def.scaling else {
		return f64::INFINITY;
	};
	match spec.units {
		Units::Speed => crate::speed_to_ms(&def.unit, scale.factor.abs()).unwrap_or(f64::INFINITY),
		_ => scale.factor.abs(),
	}
}

/// Order two hits for a role, best first — the orders [`Prefer`] describes.
///
/// A `false` sorts before a `true`, so each rank is written as the condition
/// that ranks *after*: `!drive_proven` puts the proven row first.
fn better(a: &Candidate, b: &Candidate, spec: &RoleSpec) -> std::cmp::Ordering {
	// Request id then identifier last in every rule, so the answer does not
	// depend on the order the car happened to report its units in.
	let stable = |c: &Candidate| (c.request, c.did);
	match spec.prefer {
		Prefer::Finest => step(spec, &a.def)
			.total_cmp(&step(spec, &b.def))
			.then_with(|| (!a.drive_proven, !powertrain(a.request), stable(a)).cmp(&(!b.drive_proven, !powertrain(b.request), stable(b)))),
		Prefer::Proven => {
			let rank = |c: &Candidate| (!c.drive_proven, !powertrain(c.request), id_rank(spec, c), !c.own, stable(c));
			rank(a).cmp(&rank(b))
		}
	}
}

/// Add a channel, unless that identifier is already being asked for, and say
/// whether it was added.
///
/// The same identifier twice in one request wastes a slot and makes the
/// response ambiguous to split.
fn push(into: &mut Vec<Resolved>, channel: Resolved) -> bool {
	let taken = into.iter().any(|r| r.request == channel.request && r.did == channel.did);
	if !taken {
		into.push(channel);
	}
	!taken
}

/// Resolve every channel a run needs — by id, then by words — against what the
/// car reported.
///
/// `full` adds the barometer and the ambient sensor, which exist only to feed
/// the power model: without it there is no power figure for them to feed, and
/// reading them would spend bus time on two numbers nobody will look at.
///
/// The error is the whole list of required roles that found nothing, not the
/// first — this is the pre-flight check, and a driver reading it at a
/// standstill wants to know everything that is wrong at once.
///
/// (The design also asks that engine speed be polled at the leading cadence
/// under `--full`. That is a cadence decision for the poll loop and not a
/// grouping one: a request addresses one control unit, so a channel cannot
/// join the leading batch of a unit it does not live on.)
pub fn resolve(store: &CatalogStore, extracted: &crate::extracted::Extracted, units: &[UnitIdentity], full: bool) -> Result<Set, Vec<Missing>> {
	let pool = candidates(store, extracted, units);
	let mut found: Vec<Resolved> = Vec::new();
	let mut missing: Vec<Missing> = Vec::new();

	for spec in ROLES {
		if spec.full_only && !full {
			continue;
		}
		// By id, or as the engine's legislated row at the role's PID. A row a
		// drive proved answers to its words as well: its name is the one whoever
		// proved it gave, and a proven field no source declares carries no id —
		// dropping it would make proving a channel cost it. Any other row answers
		// to the words only when it carries no text id itself — under another id
		// it is another quantity, an air-flow ceiling named with the air mass's
		// words — and only when no usable row on the car carries one of the ids:
		// where an id answers, a finer step or a lower request id under another
		// name is not a reason to read that row instead.
		let with_id = pool.iter().any(|c| by_id(spec, c));
		let by_words = |c: &Candidate| (c.drive_proven || (!with_id && c.text_id.is_none())) && matches(spec, &c.def);
		let mut hits: Vec<&Candidate> = pool.iter().filter(|c| by_id(spec, c) || by_standard(spec, c) || by_words(c)).collect();
		if hits.is_empty() {
			if spec.required {
				missing.push(Missing {
					key: spec.key,
					tried: tried(spec),
				});
			}
			continue;
		}
		hits.sort_by(|a, b| better(a, b, spec));
		let resolved = |key: &'static str, c: &Candidate| Resolved {
			key,
			request: c.request,
			did: c.did,
			def: c.def.clone(),
		};
		// The best hit whose identifier no earlier role holds: a row answering to
		// two roles goes to the first, and the second takes its next row rather
		// than vanishing.
		let Some(at) = hits.iter().position(|c| push(&mut found, resolved(spec.key, c))) else {
			if spec.required {
				missing.push(Missing {
					key: spec.key,
					tried: tried(spec),
				});
			}
			continue;
		};
		if spec.key == SPEED {
			for extra in &hits[at + 1..] {
				push(&mut found, resolved(CROSS_SPEED, extra));
			}
		}
	}

	if !missing.is_empty() {
		return Err(missing);
	}

	let leading = found
		.iter()
		.find(|r| r.key == SPEED)
		.cloned()
		.expect("speed is required, so it is either resolved or already reported missing");

	// How many identifiers go in one request is the scheduler's to decide, so a
	// unit keeps every role it answers to.
	let (leading_unit, background): (Vec<Resolved>, Vec<Resolved>) = found.into_iter().partition(|channel| channel.request == leading.request);

	let cross_check_speeds = leading_unit
		.iter()
		.chain(background.iter())
		.filter(|r| r.key == CROSS_SPEED)
		.cloned()
		.collect();

	Ok(Set {
		leading,
		leading_unit,
		background,
		cross_check_speeds,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::borrow::Cow;
	use std::collections::BTreeMap;
	use std::path::PathBuf;
	use vag_data_labels::catalog::MeasurementCatalog;
	use vag_data_labels::measure::{LinearScale, RawForm};
	use vag_data_labels::odis::Reading;

	/// Request ids of the reference car's units, for tests only — the module
	/// itself names no car's control unit and no car's identifier, only SAE
	/// J1979's PIDs and the quantities' physical units.
	const ENGINE: u16 = 0x7E0;
	const GEARBOX: u16 = 0x7E1;
	const CLUSTER: u16 = 0x714;
	/// Two units outside the powertrain that publish copies of its quantities on
	/// the reference car: the body controller and park assist.
	const BODY: u16 = 0x70E;
	const PARK_ASSIST: u16 = 0x70A;

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

	/// The reference car's rows and identities, in the style of
	/// `watch::plan`'s helper of the same name.
	fn reference(dir: std::path::PathBuf) -> (CatalogStore, Vec<UnitIdentity>) {
		let store = CatalogStore::open(dir);
		(
			store,
			vec![unit(ENGINE, "8V0906264H"), unit(GEARBOX, "0CW300041G"), unit(CLUSTER, "5E0920740D")],
		)
	}

	fn unit(request: u16, part: &str) -> UnitIdentity {
		UnitIdentity {
			request,
			part_number: Some(part.to_string()),
			odx_name: None,
			odx_version: None,
			component: None,
		}
	}

	/// A unit that reported the ODX variant `odx` and no part number: its rows
	/// are what the project declares, none of them proven on a drive.
	fn described(request: u16, odx: &str) -> UnitIdentity {
		UnitIdentity {
			request,
			part_number: None,
			odx_name: Some(odx.to_string()),
			odx_version: Some("001001".to_string()),
			component: None,
		}
	}

	/// A unit with both: proven rows under its part number, declared rows under
	/// its variant.
	fn unit_described(request: u16, part: &str, odx: &str) -> UnitIdentity {
		UnitIdentity {
			part_number: Some(part.to_string()),
			..described(request, odx)
		}
	}

	/// A catalog store written for one test, under the system temp directory —
	/// and beside it, when a test declares rows, a cache in the shape `setup`
	/// leaves one, so the ids reach a candidate through the same join the
	/// command uses.
	struct Synthetic {
		dir: PathBuf,
	}

	impl Synthetic {
		fn new(tag: &str) -> Self {
			let dir = std::env::temp_dir().join(format!(
				"vagcan-measure-channels-{tag}-{}-{:?}",
				std::process::id(),
				std::thread::current().id()
			));
			std::fs::create_dir_all(&dir).unwrap();
			Synthetic { dir }
		}

		fn write(&self, key: &str, defs: Vec<MeasurementDef>) {
			let json = MeasurementCatalog::new(defs).to_json().unwrap();
			std::fs::write(self.dir.join(format!("{key}.json")), json).unwrap();
		}

		fn store(&self) -> CatalogStore {
			CatalogStore::open(&self.dir)
		}

		fn cache(&self) -> PathBuf {
			self.dir.join("cache.sqlite")
		}

		/// Declare `readings` for one ODX variant. **Every byte is invented** except
		/// the text ids, which are either VW's real ones used as vocabulary — the
		/// way `ROLES` uses them — or invented ones in a range no role lists
		/// (`IDE9…`). No test reads a real project.
		fn declare(&self, variant: &str, readings: &[Reading]) {
			vag_data_db::put_readings(&self.cache(), "/nowhere/TEST", variant, readings).expect("the fixture writes");
		}

		/// The project the declared rows make up. Built after the last `declare`,
		/// because it reads the variant list once.
		fn extracted(&self) -> crate::extracted::Extracted {
			crate::extracted::Extracted::synthetic(self.cache(), BTreeMap::new())
		}
	}

	/// One declared row — what an ODIS project or a VCDS registry says about a
	/// field, as the cache holds it: sixteen big-endian bits, linear, in `unit`.
	fn declared(did: u16, name: &str, unit: &str, text_id: Option<&str>, factor: f64) -> Reading {
		Reading {
			did,
			name: name.to_string(),
			unit: Some(unit.to_string()),
			bit_offset: 0,
			bit_length: 16,
			signed: false,
			big_endian: true,
			scaling: Scaling::Linear(LinearScale { factor, offset: 0.0 }),
			text_id: text_id.map(str::to_string),
		}
	}

	/// A declared state: one byte, the catalog's own labels.
	fn declared_state(did: u16, name: &str, text_id: Option<&str>, levels: &[(i32, &str)]) -> Reading {
		Reading {
			bit_length: 8,
			unit: None,
			scaling: Scaling::Enum {
				levels: levels.iter().map(|(v, n)| vag_data_labels::Level::point(*v, *n)).collect(),
			},
			..declared(did, name, "", text_id, 1.0)
		}
	}

	/// The four required roles as a project declares them: under the names the
	/// words in `ROLES` match **and** under their text ids, in the units the
	/// roles read, at identifiers no car in this repository uses. What a test
	/// wants to isolate — the id, the unit, the tie-break — it changes from here.
	fn declared_required(speed_factor: f64) -> Vec<Reading> {
		vec![
			declared(0x1001, "Vehicle speed", "km/h", Some("IDE00075"), speed_factor),
			declared(0x1002, "Engine speed", "1/min", Some("IDE00405"), 1.0),
			declared_state(0x1003, "Selected gear", Some("IDE02736"), &[(0, "not engaged"), (2, "1"), (3, "2")]),
			declared(0x1004, "Accelerator pedal position", "%", Some("IDE00086"), 0.5),
		]
	}

	impl Drop for Synthetic {
		fn drop(&mut self) {
			let _ = std::fs::remove_dir_all(&self.dir);
		}
	}

	fn quantity(name: &'static str, unit: &'static str, did: u16, factor: f64) -> MeasurementDef {
		MeasurementDef {
			name: Cow::Borrowed(name),
			unit: Cow::Borrowed(unit),
			address: ReadId::Uds(did),
			raw_form: RawForm::U16Be,
			scaling: Scaling::Linear(LinearScale { factor, offset: 0.0 }),
		}
	}

	fn state(name: &'static str, did: u16, levels: &[(i32, &str)]) -> MeasurementDef {
		MeasurementDef {
			name: Cow::Borrowed(name),
			unit: Cow::Borrowed(""),
			address: ReadId::Uds(did),
			raw_form: RawForm::U8First,
			scaling: Scaling::Enum {
				levels: levels.iter().map(|(v, n)| vag_data_labels::Level::point(*v, *n)).collect(),
			},
		}
	}

	/// The four required roles, at identifiers no car in this repository uses.
	fn invented_required(speed_factor: f64) -> Vec<MeasurementDef> {
		vec![
			quantity("Vehicle speed", "km/h", 0x1001, speed_factor),
			quantity("Engine speed", "/min", 0x1002, 1.0),
			state("Selected gear", 0x1003, &[(0, "not engaged"), (2, "1"), (3, "2")]),
			quantity("Accelerator pedal position", "%", 0x1004, 0.5),
		]
	}

	fn resolved(set: &Set, key: &str) -> Option<Resolved> {
		set.all().find(|r| r.key == key).cloned()
	}

	#[test]
	fn a_channel_is_found_by_the_name_its_catalog_gives_it_and_not_by_identifier() {
		// Every identifier here is invented, and none of them appears anywhere
		// in this project's catalogs. If resolution went by number rather than
		// by word, nothing below would be found. Proven rows carry no text id of
		// their own and nothing is declared here, so the words apply to all of them.
		let synthetic = Synthetic::new("by-name");
		synthetic.write("SYN0000001", invented_required(0.05));
		let set = resolve(
			&synthetic.store(),
			&crate::extracted::Extracted::none(),
			&[unit(CLUSTER, "SYN0000001")],
			false,
		)
		.expect("a catalog using the same words needs no code change");

		assert_eq!(set.leading.source(), "714:1001");
		assert_eq!(resolved(&set, "engine speed").unwrap().did, 0x1002);
		assert_eq!(resolved(&set, "gear").unwrap().did, 0x1003);
		assert_eq!(resolved(&set, "pedal").unwrap().did, 0x1004);
		// One unit, so everything it owns is read at the leading cadence.
		assert_eq!(set.leading_unit.len(), 4);
		assert!(set.background.is_empty());
		assert!(set.cross_check_speeds.is_empty());
	}

	#[test]
	fn a_second_field_of_one_identifier_does_not_push_out_the_first() {
		// A response can pack several fields, and each is its own channel. Kept
		// by identifier, the later field replaced the earlier one — and with a
		// VCDS row filling a field ODIS lacks after ODIS's own, that made the
		// lower-ranked source win.
		let synthetic = Synthetic::new("two-fields");
		let mut defs = invented_required(0.05);
		defs.push(MeasurementDef {
			raw_form: RawForm::for_field(16, 16, false, true).unwrap(),
			..quantity("Invented status word", "", 0x1001, 1.0)
		});
		synthetic.write("SYN0000003", defs);
		let set = resolve(
			&synthetic.store(),
			&crate::extracted::Extracted::none(),
			&[unit(CLUSTER, "SYN0000003")],
			false,
		)
		.expect("the speed field is still there");
		assert_eq!(set.leading.source(), "714:1001");
	}

	#[test]
	fn a_role_whose_best_identifier_is_taken_falls_to_its_next() {
		// Measure reads an identifier once, as one channel. A row answering to
		// two roles — here a rotational speed named for both — is taken by the
		// first, engine speed, and the second then takes its next row rather
		// than vanishing. The shared row outranks the cluster's for both roles
		// (equal in everything down to the request id), so only being taken
		// explains the shaft speed landing on the cluster.
		let synthetic = Synthetic::new("taken");
		let mut cluster = invented_required(0.05);
		cluster.push(quantity("Input shaft speed", "/min", 0x1005, 1.0));
		synthetic.write("SYN0000004", cluster);
		synthetic.write(
			"SYN0000005",
			vec![quantity("Engine speed at the input shaft speed sensor, invented", "/min", 0x2000, 1.0)],
		);
		let set = resolve(
			&synthetic.store(),
			&crate::extracted::Extracted::none(),
			&[unit(0x710, "SYN0000005"), unit(CLUSTER, "SYN0000004")],
			false,
		)
		.expect("every required role resolves");
		assert_eq!(set.leading.source(), "714:1001");
		assert_eq!(resolved(&set, "engine speed").map(|r| r.source()).as_deref(), Some("710:2000"));
		assert_eq!(resolved(&set, "input shaft speed").map(|r| r.source()).as_deref(), Some("714:1005"));
	}

	#[test]
	fn the_reference_car_leads_on_whichever_unit_owns_its_finest_speed() {
		let (store, units) = reference(need_rows!());
		let set = resolve(&store, &crate::extracted::Extracted::none(), &units, true).expect("the reference car resolves");

		// Derived, not declared: the gearbox leads because its speed is the
		// finest one on the car, not because it is the gearbox.
		assert_eq!(set.leading.source(), "7E1:F40D");
		assert_eq!(set.leading.request, GEARBOX);
		assert_eq!(set.leading.def.scaling, Scaling::Linear(LinearScale { factor: 0.01, offset: 0.0 }));

		// Everything the gearbox owns is read at the leading cadence, and
		// everything else at the background one.
		let leading_keys: Vec<&str> = set.leading_unit.iter().map(|r| r.key).collect();
		assert!(leading_keys.contains(&"gear"), "{leading_keys:?}");
		assert!(leading_keys.contains(&"pedal"), "{leading_keys:?}");
		assert!(leading_keys.contains(&"selector"), "{leading_keys:?}");
		assert!(leading_keys.contains(&"input shaft speed"), "{leading_keys:?}");
		assert!(set.leading_unit.iter().all(|r| r.request == GEARBOX));

		let background_keys: Vec<&str> = set.background.iter().map(|r| r.key).collect();
		for wanted in ["engine speed", "boost actual", "boost specified", "air mass"] {
			assert!(background_keys.contains(&wanted), "{wanted} missing from {background_keys:?}");
		}
		assert_eq!(resolved(&set, "engine speed").unwrap().request, ENGINE);

		// Both other speeds are read, and neither times anything.
		let crossed: Vec<String> = set.cross_check_speeds.iter().map(|r| r.source()).collect();
		assert!(crossed.contains(&"714:22D2".to_string()), "{crossed:?}");
		assert!(crossed.iter().all(|s| s != "7E1:F40D"));
		// A cross-check is polled: it is in one of the lists too.
		assert!(set.all().filter(|r| r.key == CROSS_SPEED).count() == crossed.len());
	}

	#[test]
	fn the_finest_speed_wins_and_a_remaining_tie_breaks_by_unit_id() {
		// Four units answer to a speed name. The finest is neither the first
		// nor the last to be reported, so neither reporting order nor unit id
		// can produce this answer by accident. The two finest are both proven
		// and neither is a powertrain unit, so nothing above the request id in
		// the tie-break separates them, and the request id is what decides.
		let synthetic = Synthetic::new("tie-break");
		synthetic.write("SYNCOARSE1", invented_required(1.0));
		synthetic.write("SYNFINE001", invented_required(0.01));
		synthetic.write("SYNMIDDLE1", invented_required(0.1));
		synthetic.write("SYNFINE002", invented_required(0.01));

		let set = resolve(
			&synthetic.store(),
			&crate::extracted::Extracted::none(),
			&[
				unit(CLUSTER, "SYNCOARSE1"),
				unit(0x730, "SYNFINE001"),
				unit(0x744, "SYNMIDDLE1"),
				unit(0x760, "SYNFINE002"),
			],
			false,
		)
		.expect("four units, all of them complete");

		assert_eq!(set.leading.source(), "730:1001");
		assert_eq!(set.leading.request, 0x730);
		// The other three are read as cross-checks and time nothing.
		assert_eq!(set.cross_check_speeds.len(), 3);
		assert!(set.cross_check_speeds.iter().all(|r| r.request != 0x730));
	}

	#[test]
	fn a_role_is_found_by_its_odx_id_under_a_name_no_word_matches() {
		// The two sources word one channel differently and share VW's id for it.
		// These names are invented: no word in `ROLES` is in them, and they end
		// in no suffix the pair rule splits — so only the id can find them.
		let synthetic = Synthetic::new("by-id");
		synthetic.write("SYN0000010", invented_required(0.05));
		synthetic.declare(
			"EV_InventedEngine",
			&[
				declared(0x2110, "Turbo outlet pressure, measured", "bar", Some("IDE00191"), 0.1),
				declared(0x2111, "Turbo outlet pressure, wanted", "bar", Some("IDE00190"), 0.1),
			],
		);
		let set = resolve(
			&synthetic.store(),
			&synthetic.extracted(),
			&[unit(CLUSTER, "SYN0000010"), described(ENGINE, "EV_InventedEngine")],
			false,
		)
		.expect("the required roles are on the cluster");
		assert_eq!(resolved(&set, "boost actual").map(|r| r.source()).as_deref(), Some("7E0:2110"));
		assert_eq!(resolved(&set, "boost specified").map(|r| r.source()).as_deref(), Some("7E0:2111"));
	}

	#[test]
	fn an_id_hit_is_not_outranked_by_a_finer_row_that_only_answers_to_words() {
		// A filtered, lagging speed listed on the engine at a finer step than the
		// sensor's, under no id. By words alone the finer step took the stopwatch;
		// with an id on the car the words are not consulted at all.
		let synthetic = Synthetic::new("id-before-words");
		let mut rows = declared_required(0.01);
		rows.push(declared(0x1005, "Vehicle speed: filtered", "km/h", None, 0.001));
		synthetic.declare("EV_InventedEngine", &rows);
		let set = resolve(
			&synthetic.store(),
			&synthetic.extracted(),
			&[described(ENGINE, "EV_InventedEngine")],
			false,
		)
		.expect("everything required is declared");
		assert_eq!(set.leading.source(), "7E0:1001");
	}

	#[test]
	fn equal_steps_fall_to_the_powertrain_unit_before_the_lower_request_id() {
		// The body controller and the gearbox both publish road speed to 0.01 km/h
		// under one id. The tie used to fall to the lower request id, and a
		// comfort unit's copy timed the run. Powertrain first: the gearbox leads
		// and the body controller's copy is the cross-check.
		let synthetic = Synthetic::new("powertrain");
		synthetic.declare("EV_InventedGearbox", &declared_required(0.01));
		synthetic.declare("EV_InventedBody", &[declared(0x2B00, "Vehicle speed", "km/h", Some("IDE00075"), 0.01)]);
		let set = resolve(
			&synthetic.store(),
			&synthetic.extracted(),
			&[described(BODY, "EV_InventedBody"), described(GEARBOX, "EV_InventedGearbox")],
			false,
		)
		.expect("both units are complete");
		assert_eq!(set.leading.source(), "7E1:1001");
		// The body controller's copy alone: the gearbox's J1979 prediction at
		// `F40D` is not a hit — the standard's layout holds on the engine, and a
		// gearbox declares that identifier in another one.
		let crossed: Vec<String> = set.cross_check_speeds.iter().map(|r| r.source()).collect();
		assert_eq!(crossed, vec!["70E:2B00"]);
	}

	#[test]
	fn an_unknown_gearboxs_j1979_prediction_is_not_a_speed_when_an_id_answers_elsewhere() {
		// The project has no rows for this gearbox's variant, and the body
		// controller carries road speed under its id. The gearbox is offered no
		// J1979 prediction at all — the set stands in for an engine alone — so its
		// `F40D` is neither a speed hit nor a cross-check: in the reference project
		// the six DSG variants declare that identifier as sixteen bits of
		// 0.01 km/h, which a one-byte prediction would misread.
		let synthetic = Synthetic::new("gearbox-prediction");
		synthetic.declare("EV_InventedBody", &declared_required(0.01));
		let set = resolve(
			&synthetic.store(),
			&synthetic.extracted(),
			&[unit(GEARBOX, "SYNNOTHING"), described(BODY, "EV_InventedBody")],
			false,
		)
		.expect("the body controller is complete");
		assert_eq!(set.leading.source(), "70E:1001");
		assert!(set.cross_check_speeds.is_empty(), "{:?}", set.cross_check_speeds);
	}

	#[test]
	fn a_known_engine_variant_gets_no_j1979_prediction_for_what_it_does_not_declare() {
		// The project knows this engine's variant and declares no row at PID 10.
		// Its files say what the engine answers, and they outrank the standard's
		// prediction: no `F410` candidate stands in, so with no air-mass id on the
		// car the role resolves to nothing rather than to an identifier the engine
		// may never answer.
		let synthetic = Synthetic::new("known-engine");
		let mut engine = declared_required(0.01);
		// Its OBD-II rows are declared — here one, the engine speed's — and PID 10's
		// is not among them.
		engine.push(declared(0xF40C, "Invented engine speed", "/min", Some("IDE00021"), 0.25));
		synthetic.declare("EV_InventedEngine", &engine);
		let set = resolve(
			&synthetic.store(),
			&synthetic.extracted(),
			&[described(ENGINE, "EV_InventedEngine")],
			false,
		)
		.expect("everything required is declared");
		assert_eq!(resolved(&set, "air mass"), None);
	}

	#[test]
	fn an_engine_variant_that_declares_no_obd_row_still_gets_the_prediction() {
		// A variant the project knows but whose file holds no OBD-II row at all —
		// the reference project has one-row stubs of that kind, a key counter and
		// nothing else. It says nothing about what the engine answers, so the
		// standard's prediction stands in as it does for an unknown variant.
		let synthetic = Synthetic::new("stub-engine");
		synthetic.declare("EV_InventedStub", &[declared(0x0103, "Invented counter", "", None, 1.0)]);
		let set = resolve(&synthetic.store(), &synthetic.extracted(), &[described(ENGINE, "EV_InventedStub")], false);
		// Gear has no PID, so the stub's car is refused — for gear alone: speed,
		// engine speed and pedal come from the standard's rows.
		let missing = set.expect_err("nothing declares a gear");
		let keys: Vec<&str> = missing.iter().map(|m| m.key).collect();
		assert_eq!(keys, vec!["gear"]);
	}

	#[test]
	fn the_gearbox_is_offered_no_j1979_prediction() {
		// Neither unit's variant is known and the car carries no ids, so the words
		// apply to everything on offer. The engine's J1979 set is offered; the
		// gearbox's is not — its `F40D` prediction would have been a cross-check
		// reading one byte of what a DSG gearbox declares as sixteen.
		let synthetic = Synthetic::new("no-gearbox-prediction");
		synthetic.write("GBX0000004", vec![state("Selected gear", 0x3001, &[(0, "not engaged"), (2, "1")])]);
		let set = resolve(
			&synthetic.store(),
			&crate::extracted::Extracted::none(),
			&[unit(ENGINE, "SYNNOTHING"), unit(GEARBOX, "GBX0000004")],
			false,
		)
		.expect("the engine's standard set and one proven gear");
		assert_eq!(set.leading.source(), "7E0:F40D");
		assert!(set.cross_check_speeds.is_empty(), "{:?}", set.cross_check_speeds);
	}

	#[test]
	fn a_drive_proven_row_outranks_an_extracted_one_on_another_unit() {
		// The gearbox's gear was proven on a drive; the engine's is a declared
		// row under the id the role lists first. A drive's evidence outranks the
		// id order — and the proven row is an id hit at all only because the
		// join carries the declared field's id onto it.
		let synthetic = Synthetic::new("drive-proven");
		synthetic.write(
			"GBX0000001",
			vec![state("Selected gear", 0x3001, &[(0, "not engaged"), (2, "1"), (12, "R")])],
		);
		let mut gearbox = declared_required(0.01);
		gearbox.retain(|r| r.did != 0x1003);
		gearbox.push(declared_state(
			0x3001,
			"Selected gear",
			Some("IDE00090"),
			&[(0, "not engaged"), (2, "1"), (12, "9")],
		));
		synthetic.declare("EV_InventedGearbox", &gearbox);
		synthetic.declare(
			"EV_InventedEngine",
			&[declared_state(0x3002, "Selected gear", Some("IDE02736"), &[(0, "N"), (2, "1")])],
		);
		let set = resolve(
			&synthetic.store(),
			&synthetic.extracted(),
			&[
				described(ENGINE, "EV_InventedEngine"),
				unit_described(GEARBOX, "GBX0000001", "EV_InventedGearbox"),
			],
			false,
		)
		.expect("the gearbox is complete");
		let gear = resolved(&set, "gear").unwrap();
		assert_eq!(gear.source(), "7E1:3001");
		// And it is read the way the drive established, not the way the file says.
		assert_eq!(gear.state(&[12]).as_deref(), Some("R"));
	}

	#[test]
	fn with_neither_proven_the_id_the_role_lists_first_wins() {
		// Two declared gears named alike: the engine's under the engine's id, the
		// gearbox's under the one the role lists first. Neither proven, both
		// powertrain — so the id order decides, over the lower request id.
		let synthetic = Synthetic::new("id-order");
		let mut engine = declared_required(0.01);
		engine.retain(|r| r.did != 0x1003);
		engine.push(declared_state(0x3002, "Selected gear", Some("IDE00090"), &[(0, "N"), (2, "1")]));
		synthetic.declare("EV_InventedEngine", &engine);
		synthetic.declare(
			"EV_InventedGearbox",
			&[declared_state(0x3001, "Selected gear", Some("IDE02736"), &[(0, "N"), (2, "1")])],
		);
		let set = resolve(
			&synthetic.store(),
			&synthetic.extracted(),
			&[described(ENGINE, "EV_InventedEngine"), described(GEARBOX, "EV_InventedGearbox")],
			false,
		)
		.expect("the engine is complete");
		assert_eq!(resolved(&set, "gear").map(|r| r.source()).as_deref(), Some("7E1:3001"));
	}

	#[test]
	fn a_copy_on_another_unit_under_the_roles_id_loses_to_the_powertrains() {
		// Park assist lists the pedal under the same id — its snapshots record
		// it. Equal in every other way, the powertrain's row is read, although
		// the other unit's request id is the lower.
		let synthetic = Synthetic::new("copy");
		synthetic.declare("EV_InventedEngine", &declared_required(0.01));
		synthetic.declare(
			"EV_InventedParkAssist",
			&[declared(0x2520, "Accelerator pedal position", "%", Some("IDE00086"), 0.5)],
		);
		let set = resolve(
			&synthetic.store(),
			&synthetic.extracted(),
			&[described(PARK_ASSIST, "EV_InventedParkAssist"), described(ENGINE, "EV_InventedEngine")],
			false,
		)
		.expect("the engine is complete");
		assert_eq!(resolved(&set, "pedal").map(|r| r.source()).as_deref(), Some("7E0:1004"));
	}

	#[test]
	fn a_drive_proven_row_answers_to_its_words_beside_the_ids() {
		// The cluster's road speed was proven on a drive, at a field no source
		// declares, so it carries no id. The gearbox answers to the role's id.
		// A proven row is the owner's own evidence under the owner's own name:
		// it is not dropped for want of an id. Its whole km/h does not lead, and
		// it stays what it was before ids — a cross-check.
		let synthetic = Synthetic::new("proven-words");
		synthetic.write("CLU0000001", vec![quantity("Road speed", "km/h", 0x2201, 1.0)]);
		synthetic.declare("EV_InventedGearbox", &declared_required(0.01));
		let set = resolve(
			&synthetic.store(),
			&synthetic.extracted(),
			&[unit(CLUSTER, "CLU0000001"), described(GEARBOX, "EV_InventedGearbox")],
			false,
		)
		.expect("the gearbox is complete");
		assert_eq!(set.leading.source(), "7E1:1001");
		let crossed: Vec<String> = set.cross_check_speeds.iter().map(|r| r.source()).collect();
		assert_eq!(crossed, vec!["714:2201"]);
	}

	#[test]
	fn a_percent_gauge_under_the_boost_id_is_not_boost() {
		// A cluster's boost gauge is declared under the same id as the engine's
		// boost, in `%` of the dial. `boost_reference` reads boost in bar, so the
		// gauge is not the channel — and with nothing in bar on the car, there is
		// no boost rather than a wrong one.
		let synthetic = Synthetic::new("percent-boost");
		synthetic.write("SYN0000011", invented_required(0.05));
		synthetic.declare("EV_InventedCluster", &[declared(0x2210, "Turbo dial", "%", Some("IDE00191"), 0.5)]);
		let set = resolve(
			&synthetic.store(),
			&synthetic.extracted(),
			&[unit_described(CLUSTER, "SYN0000011", "EV_InventedCluster")],
			false,
		)
		.expect("boost is not required");
		assert_eq!(resolved(&set, "boost actual"), None);
	}

	#[test]
	fn a_pedal_gradient_under_the_pedal_id_is_not_the_pedal() {
		// The engine declares a row in `%/s` under the pedal's own id, at a lower
		// identifier than the position. The coastdown's foot-off check reads the
		// pedal in `%`, so that row is not admitted, whatever its id.
		let synthetic = Synthetic::new("pedal-gradient");
		let mut rows = declared_required(0.01);
		rows.push(declared(0x1000, "Accelerator pedal position", "%/s", Some("IDE00086"), 0.5));
		synthetic.declare("EV_InventedEngine", &rows);
		let set = resolve(
			&synthetic.store(),
			&synthetic.extracted(),
			&[described(ENGINE, "EV_InventedEngine")],
			false,
		)
		.expect("the position is there");
		assert_eq!(resolved(&set, "pedal").map(|r| r.source()).as_deref(), Some("7E0:1004"));
	}

	#[test]
	fn a_speed_in_metres_per_second_is_ranked_by_its_step_as_a_speed() {
		// A radar publishes road speed in m/s at 1/256 — a smaller factor than the
		// gearbox's 0.01, and a coarser step: 1/256 m/s is 0.014 km/h. Compared as
		// raw factors it "won"; compared as speeds the gearbox is the finer, and
		// the radar's row is read as a cross-check.
		let synthetic = Synthetic::new("metres-per-second");
		synthetic.declare("EV_InventedGearbox", &declared_required(0.01));
		synthetic.declare(
			"EV_InventedRadar",
			&[declared(0x2600, "Vehicle speed", "m/s", Some("IDE00075"), 1.0 / 256.0)],
		);
		let set = resolve(
			&synthetic.store(),
			&synthetic.extracted(),
			&[described(0x757, "EV_InventedRadar"), described(GEARBOX, "EV_InventedGearbox")],
			false,
		)
		.expect("both units are complete");
		assert_eq!(set.leading.source(), "7E1:1001");
		let crossed: Vec<String> = set.cross_check_speeds.iter().map(|r| r.source()).collect();
		assert_eq!(crossed, vec!["757:2600"]);
	}

	#[test]
	fn an_unmatched_engines_legislated_row_outranks_a_copy_on_another_unit() {
		// The project has no rows for this engine's variant, so the engine offers
		// only SAE J1979's set, which carries no id; the climate unit declares a
		// coarse copy of engine speed — 100 /min a bit — under the role's id. The
		// standard defines the quantity at its PID, so the engine's own legislated
		// row is a hit and, being the powertrain's, it is the channel.
		let synthetic = Synthetic::new("legislated");
		let mut gearbox = declared_required(0.01);
		gearbox.retain(|r| r.did != 0x1002);
		synthetic.declare("EV_InventedGearbox", &gearbox);
		synthetic.declare(
			"EV_InventedClimate",
			&[declared(0x2700, "Engine speed", "1/min", Some("IDE00021"), 100.0)],
		);
		let set = resolve(
			&synthetic.store(),
			&synthetic.extracted(),
			&[
				unit(ENGINE, "SYNNOTHING"),
				described(0x746, "EV_InventedClimate"),
				described(GEARBOX, "EV_InventedGearbox"),
			],
			false,
		)
		.expect("the gearbox and the standard set are complete");
		assert_eq!(resolved(&set, "engine speed").map(|r| r.source()).as_deref(), Some("7E0:F40C"));
	}

	#[test]
	fn a_powertrain_row_under_a_later_id_outranks_a_copy_under_the_first() {
		// A unit outside the powertrain declares a gear under the id the role
		// lists first (invented — no such row is in the reference project); the
		// engine has its own gear under the second. The engine's is the channel:
		// a powertrain row before a copy, whichever id the copy is under.
		let synthetic = Synthetic::new("powertrain-before-id");
		let mut engine = declared_required(0.01);
		engine.retain(|r| r.did != 0x1003);
		engine.push(declared_state(0x3002, "Selected gear", Some("IDE00090"), &[(0, "N"), (2, "1")]));
		synthetic.declare("EV_InventedEngine", &engine);
		synthetic.declare(
			"EV_InventedParkAssist",
			&[declared_state(0x2530, "Selected gear", Some("IDE02736"), &[(0, "N"), (2, "1")])],
		);
		let set = resolve(
			&synthetic.store(),
			&synthetic.extracted(),
			&[described(PARK_ASSIST, "EV_InventedParkAssist"), described(ENGINE, "EV_InventedEngine")],
			false,
		)
		.expect("the engine is complete");
		assert_eq!(resolved(&set, "gear").map(|r| r.source()).as_deref(), Some("7E0:3002"));
	}

	#[test]
	fn a_drive_proven_speed_leads_over_an_equal_powertrain_step() {
		// Equal steps, and only the cluster's row was proven on a drive. A drive's
		// evidence comes before the powertrain rule: the cluster leads, the
		// gearbox's declared row cross-checks it.
		let synthetic = Synthetic::new("proven-leads");
		synthetic.write("CLU0000002", vec![quantity("Vehicle speed", "km/h", 0x2202, 0.01)]);
		synthetic.declare("EV_InventedGearbox", &declared_required(0.01));
		let set = resolve(
			&synthetic.store(),
			&synthetic.extracted(),
			&[unit(CLUSTER, "CLU0000002"), described(GEARBOX, "EV_InventedGearbox")],
			false,
		)
		.expect("the gearbox is complete");
		assert_eq!(set.leading.source(), "714:2202");
		let crossed: Vec<String> = set.cross_check_speeds.iter().map(|r| r.source()).collect();
		assert_eq!(crossed, vec!["7E1:1001"]);
	}

	#[test]
	fn a_constant_named_with_the_roles_words_under_another_id_is_not_the_channel() {
		// The engine declares a ceiling for the air flow — a constant — under an
		// id of its own, named with the role's words. Nothing on the car carries
		// the role's id, so the words apply; but a row under another id is another
		// quantity whatever its name says, and the words take only rows with no id.
		// What is left is nothing: the engine's variant is known, declares its
		// OBD-II rows and none at PID 10, so no J1979 prediction stands in either.
		let synthetic = Synthetic::new("constant");
		let mut rows = declared_required(0.01);
		rows.push(declared(0x2050, "Air mass ceiling", "g/s", Some("IDE90001"), 0.1));
		rows.push(declared(0xF40C, "Invented engine speed", "/min", Some("IDE00021"), 0.25));
		synthetic.declare("EV_InventedEngine", &rows);
		let set = resolve(
			&synthetic.store(),
			&synthetic.extracted(),
			&[described(ENGINE, "EV_InventedEngine")],
			false,
		)
		.expect("everything required is declared");
		assert_eq!(resolved(&set, "air mass"), None);
	}

	#[test]
	fn a_units_own_row_outranks_the_legislated_prediction_at_equal_rank() {
		// No ids on the car. The engine offers only the standard set; the gearbox
		// declares its own copy of engine speed, unproven and under no id. Both
		// powertrain, neither proven, neither under an id: the unit's own row
		// comes before the standard's prediction, over the lower request id.
		let synthetic = Synthetic::new("own-before-standard");
		synthetic.write("GBX0000003", vec![state("Selected gear", 0x3001, &[(0, "not engaged"), (2, "1")])]);
		synthetic.declare("EV_InventedGearbox", &[declared(0x3010, "Engine speed", "1/min", None, 1.0)]);
		let set = resolve(
			&synthetic.store(),
			&synthetic.extracted(),
			&[unit(ENGINE, "SYNNOTHING"), unit_described(GEARBOX, "GBX0000003", "EV_InventedGearbox")],
			false,
		)
		.expect("the standard set, a proven gear and a declared engine speed");
		assert_eq!(resolved(&set, "engine speed").map(|r| r.source()).as_deref(), Some("7E1:3010"));
	}

	#[test]
	fn a_car_whose_rows_carry_no_ids_still_resolves_by_words() {
		// No extracted rows at all: the engine offers only the legislated set,
		// which carries no text id, and the gearbox one proven gear. Every role
		// falls to its words, as before ids were consulted.
		let synthetic = Synthetic::new("no-ids");
		synthetic.write("GBX0000002", vec![state("Selected gear", 0x3001, &[(0, "not engaged"), (2, "1")])]);
		let set = resolve(
			&synthetic.store(),
			&crate::extracted::Extracted::none(),
			&[unit(ENGINE, "SYNNOTHING"), unit(GEARBOX, "GBX0000002")],
			false,
		)
		.expect("the standard set and one proven gear are enough");
		assert_eq!(set.leading.source(), "7E0:F40D");
		assert_eq!(resolved(&set, "engine speed").map(|r| r.source()).as_deref(), Some("7E0:F40C"));
		assert_eq!(resolved(&set, "pedal").map(|r| r.source()).as_deref(), Some("7E0:F449"));
		assert_eq!(resolved(&set, "gear").map(|r| r.source()).as_deref(), Some("7E1:3001"));
	}

	/// The reference car's units as they identify themselves — `F19E`, `F1A2`,
	/// `F187` — for the test on the owner's own project only.
	fn identity(request: u16, odx: &str, version: &str, part: &str) -> UnitIdentity {
		UnitIdentity {
			request,
			part_number: Some(part.to_string()),
			odx_name: Some(odx.to_string()),
			odx_version: Some(version.to_string()),
			component: None,
		}
	}

	#[test]
	fn the_owners_project_leads_on_the_gearbox_and_reads_its_drive_proven_rows() {
		// The owner's case, on the owner's data: the ODIS project's rows under
		// the drive-proven ones. By words the body controller's speed led (an
		// equal step and a lower request id), the engine's gear and pedal were
		// read although the gearbox's are drive-proven. By id, with the tie-break
		// above, every role lands on the row the drive proved where there is one.
		let dir = need_rows!();
		let extracted = crate::extracted::current();
		let gearbox = identity(GEARBOX, "EV_TCMDQ200021", "001001", "0CW300041G");
		// Proven rows and a project are not enough: they have to be the reference
		// car's — its gearbox's proven file, and rows for its gearbox's variant.
		// Another owner's machine has its own of both; it has these two only with
		// the same gearbox on the same project, which is as close to the reference
		// car as this check can come.
		if !dir.join("0CW300041G.json").exists() || extracted.for_unit(gearbox.odx_name.as_deref(), gearbox.odx_version.as_deref()).is_empty() {
			eprintln!("skipped: this machine's project is not the reference car's — no proven gearbox file, or no rows for its variant");
			return;
		}
		let store = CatalogStore::open(dir);
		let units = vec![
			identity(ENGINE, "EV_ECM18TFS0208V0906264H", "001007", "8V0906264H"),
			gearbox,
			identity(BODY, "EV_BCMMQB", "017001", "5Q0937084CF"),
			identity(0x767, "EV_OCULowMQBLGE", "003042", "3Q0035284"),
			identity(CLUSTER, "EV_DashBoardVDDMQBAB", "009051", "5E0920740D"),
			identity(PARK_ASSIST, "EV_EPHVA14AU3700000", "009029", "5QA919283A"),
			identity(0x712, "EV_SteerAssisMQB", "013144", "5Q0909144T"),
			identity(0x746, "EV_ACClimaBHBVW37X", "006145", "5E0907044AM"),
			identity(0x773, "EV_MUEnt4CGen2LGE", "001039", "5E0035871C"),
		];
		let set = resolve(&store, &extracted, &units, false).expect("the owner's car resolves");
		let source = |key: &str| resolved(&set, key).map(|r| r.source()).unwrap_or_else(|| format!("{key}: not resolved"));
		assert_eq!(set.leading.source(), "7E1:F40D");
		assert_eq!(source("engine speed"), "7E0:206E");
		assert_eq!(source("gear"), "7E1:3816");
		assert_eq!(source("pedal"), "7E1:3804");
		assert_eq!(source("selector"), "7E1:3809");
		assert_eq!(source("input shaft speed"), "7E1:380A");
		assert_eq!(source("output shaft speed"), "7E1:380B");
		assert_eq!(source("boost actual"), "7E0:202A");
		assert_eq!(source("boost specified"), "7E0:2029");
		// No air mass on this car. The engine's variant declares its OBD-II rows
		// and none at PID 10 — the parked survey's `F400` support bitmap has PID 10
		// clear and `F410` is not among the 38 `F4xx` values it answered besides
		// the four support bitmaps (review data, 2026-09-28) — so no J1979
		// prediction stands in. No value row on the engine carries an air-mass id
		// (`IDE00347` is only its `F400` support bit): `13CD` has no id and a name
		// no word matches, and `2037` is what both sources name a setpoint, which
		// the 2026-09-26 capture shows following load (8–80 kg/h).
		assert_eq!(resolved(&set, "air mass"), None, "{}", source("air mass"));
	}

	#[test]
	fn a_store_with_no_speed_channel_says_what_it_looked_for() {
		// A car this project has never seen: the honest answer is a refusal
		// naming the words its label files would have to use, never a number
		// borrowed from another car.
		let synthetic = Synthetic::new("no-speed");
		synthetic.write("SYN0000009", vec![quantity("Odometer", "km", 0x1010, 1.0)]);

		let missing = resolve(
			&synthetic.store(),
			&crate::extracted::Extracted::none(),
			&[unit(CLUSTER, "SYN0000009")],
			false,
		)
		.expect_err("no speed means no stopwatch");
		let speed = missing
			.iter()
			.find(|m| m.key == "speed")
			.unwrap_or_else(|| panic!("speed is not among {missing:?}"));
		// The id first, then the words — the order they are tried in.
		assert_eq!(speed.tried, vec!["IDE00075", "vehicle speed", "road speed"]);
		// And it reports everything that is wrong at once, not just the first.
		let keys: Vec<&str> = missing.iter().map(|m| m.key).collect();
		assert_eq!(keys, vec!["speed", "engine speed", "gear", "pedal"]);
	}

	#[test]
	fn without_full_the_barometer_and_the_ambient_sensor_are_not_resolved() {
		// They exist only to feed air density, which feeds only power. With no
		// power figure there is nothing for them to feed, and a cycle spent on
		// them is a cycle not spent on speed.
		let synthetic = Synthetic::new("full-only");
		synthetic.write("SYN0000002", invented_required(0.05));
		synthetic.write(
			"SYN0000003",
			vec![
				quantity("Absolute barometric pressure", "kPa", 0x1020, 1.0),
				quantity("Ambient air temperature", "°C", 0x1021, 1.0),
			],
		);
		let units = [unit(CLUSTER, "SYN0000002"), unit(0x730, "SYN0000003")];

		let plain = resolve(&synthetic.store(), &crate::extracted::Extracted::none(), &units, false).expect("resolves without them");
		assert!(resolved(&plain, "barometer").is_none());
		assert!(resolved(&plain, "ambient").is_none());
		assert!(plain.background.is_empty(), "nothing to read on the other unit");

		let full = resolve(&synthetic.store(), &crate::extracted::Extracted::none(), &units, true).expect("resolves with them");
		assert_eq!(resolved(&full, "barometer").unwrap().source(), "730:1020");
		assert_eq!(resolved(&full, "ambient").unwrap().source(), "730:1021");
		// They are on another unit, so they are read at the background cadence.
		assert_eq!(full.background.len(), 2);
	}

	#[test]
	fn a_paired_measurement_resolves_to_the_half_it_says_it_is() {
		let (store, units) = reference(need_rows!());
		let set = resolve(&store, &crate::extracted::Extracted::none(), &units, false).expect("the reference car resolves");
		let actual = resolved(&set, "boost actual").unwrap();
		let specified = resolved(&set, "boost specified").unwrap();
		assert_eq!(actual.def.name, "Boost pressure, actual");
		assert_eq!(specified.def.name, "Boost pressure, specified");
		assert_ne!(actual.did, specified.did);
	}

	#[test]
	fn a_value_comes_through_its_definition_and_never_through_the_raw_fallback() {
		let (store, units) = reference(need_rows!());
		let set = resolve(&store, &crate::extracted::Extracted::none(), &units, false).expect("the reference car resolves");

		// A quantity is a number, at the resolution its scaling proves.
		assert_eq!(set.leading.value(&[0x0A, 0x1E]), Some(76.9));

		// A state is the catalog's own label, and a code the catalog does not
		// list is absent — where `watch` would show `"09 (raw)"`, which is the
		// class of value this command does not admit.
		let gear = resolved(&set, "gear").unwrap();
		assert_eq!(gear.state(&[0x0C]).as_deref(), Some("R"));
		assert_eq!(gear.state(&[0x09]), None);
		assert_eq!(gear.value(&[0x02]), None, "a gear is not a quantity");
	}

	#[test]
	fn an_anchored_row_is_not_admitted_because_it_proves_only_one_point() {
		// Half a scaling cannot be differentiated or integrated, so it is not
		// a channel a stopwatch can use — and the honest outcome is a refusal
		// rather than a column that reads only at one value.
		let synthetic = Synthetic::new("anchor");
		synthetic.write(
			"SYN0000004",
			vec![MeasurementDef {
				name: Cow::Borrowed("Vehicle speed"),
				unit: Cow::Borrowed("km/h"),
				address: ReadId::Uds(0x1030),
				raw_form: RawForm::U16Be,
				scaling: Scaling::Anchor { raw: 0, value: 0.0 },
			}],
		);
		let missing = resolve(
			&synthetic.store(),
			&crate::extracted::Extracted::none(),
			&[unit(CLUSTER, "SYN0000004")],
			false,
		)
		.expect_err("an anchor is not a speed channel");
		assert!(missing.iter().any(|m| m.key == "speed"));
	}
}
