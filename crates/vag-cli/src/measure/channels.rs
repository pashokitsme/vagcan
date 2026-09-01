//! Which channels a run reads, chosen by what the catalogs **call** them.
//!
//! Nothing here names an identifier. Which identifier carries road speed is a
//! fact about one car; the words label files use for it are shared, so every
//! channel is found the way [`crate::watch::plan::select_basics`] already finds
//! the basics — by name, over the rows the car's own units report catalogs for,
//! plus the legislated SAE J1979 set on the units ISO 15765-4 obliges to answer
//! it. A car whose catalog uses the same words works without a line of this
//! module changing, and a car with no catalog is told what was looked for
//! rather than given another car's numbers.
//!
//! Two consequences of that shape are worth stating at the door.
//!
//! **The leading unit is derived, not declared.** It is whichever unit owns the
//! speed channel that won resolution. Writing "the gearbox" would be one
//! particular car's accident: on the reference car the finest road speed
//! happens to sit on the gearbox, while the cluster publishes whole km/h and
//! the engine's OBD mirror is a single byte. That makes the tie-break
//! load-bearing, because more than one unit answers to those names.
//!
//! **A channel that will not resolve is a channel this command does without.**
//! `measure` is an instrument, not a search: an unproven byte cannot be timed,
//! integrated or differentiated. So a row is admitted only when its meaning is
//! whole — a fully linear scaling for a quantity, an enumeration for a state —
//! and a required role that finds none is a refusal naming what it tried, never
//! an empty column and never raw bytes.

use std::collections::BTreeMap;

use vag_data_labels::catalog::{CatalogStore, MeasurementDef, ReadId, Scaling};
use vag_uds_client::address::UnitAddress;

use crate::watch::plan::{self, UnitIdentity};

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
	/// Deliberately **not** [`crate::watch::plan::Channel::render`]: that falls
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
/// One request addresses one control unit, so the split is by unit and not by
/// taste: everything on the unit that owns the leading speed is read every
/// cycle, everything else half as often. Marks are timed from the leading
/// speed alone, so its rate is the only one that sets a stopwatch.
#[derive(Debug, Clone, PartialEq)]
pub struct Set {
	/// The speed channel that won, and by owning it, the unit that leads.
	pub leading: Resolved,
	/// Everything on the leading unit, within [`plan::BATCH`].
	pub leading_batch: Vec<Resolved>,
	/// Everything on every other unit.
	pub background: Vec<Resolved>,
	/// The speeds that did not win. These also appear in one of the batches
	/// above — the list exists so the caller can tell which rows are speeds
	/// without matching names a second time.
	pub cross_check_speeds: Vec<Resolved>,
}

impl Set {
	/// Every channel that will be polled, in priority order.
	pub fn all(&self) -> impl Iterator<Item = &Resolved> {
		self.leading_batch.iter().chain(self.background.iter())
	}
}

/// A required role that nothing answered to, and the names it answered to
/// nothing under.
///
/// The names are the whole of the report on purpose: a car this project has
/// never seen fails here at a standstill, and the only useful thing to tell
/// its owner is which words their label files would have to use.
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

/// How a role chooses among several name matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prefer {
	/// The finest quantisation wins. This is the stopwatch's rule and only the
	/// stopwatch's: the leading channel's step is the step every mark is
	/// resolved to, and a whole km/h would quantise a 0-100 to tenths of a
	/// second. Remaining ties break by unit id, so the choice is stable
	/// across runs.
	Finest,
	/// A row the unit itself proved, before one the standard table merely
	/// predicts; then by unit id and identifier. The design fixes no rule for
	/// these roles, and evidence before inference is the one this project
	/// already applies wherever the two meet at the same address.
	Proven,
}

/// One thing a run wants to read, described by the words label files would use
/// for it rather than by where it lives.
struct RoleSpec {
	key: &'static str,
	/// Matched as a substring of the lower-cased name, exactly as
	/// [`plan::select_basics`] matches the basics.
	names: &'static [&'static str],
	wants: Wants,
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

/// The roles, in the order they earn their place in a request.
///
/// The order is the priority order: where a unit has more rows than a single
/// request holds, the ones lower down are what goes. Cross-check speeds follow
/// the leading speed immediately, because a second speed is what makes the
/// leading one's refresh period observable.
/// Every role this command knows how to fill, by the key the session file uses.
///
/// Reachable so that a reader can turn a file's channel names back into the
/// roles they came from. A reader that carried its own list would drift from
/// the writer's the first time a role was added, and the drift would show up as
/// a channel quietly missing from a recomputed run rather than as a failure.
pub fn known_roles() -> impl Iterator<Item = &'static str> {
	ROLES.iter().map(|spec| spec.key)
}

const ROLES: &[RoleSpec] = &[
	RoleSpec {
		key: SPEED,
		names: &["vehicle speed", "road speed"],
		wants: Wants::Quantity,
		prefer: Prefer::Finest,
		required: true,
		full_only: false,
		pair: None,
	},
	RoleSpec {
		key: "engine speed",
		names: &["engine speed"],
		wants: Wants::Quantity,
		prefer: Prefer::Proven,
		required: true,
		full_only: false,
		pair: None,
	},
	RoleSpec {
		key: "gear",
		names: &["selected gear"],
		wants: Wants::State,
		prefer: Prefer::Proven,
		required: true,
		full_only: false,
		pair: None,
	},
	RoleSpec {
		key: "pedal",
		names: &["accelerator pedal position"],
		wants: Wants::Quantity,
		prefer: Prefer::Proven,
		required: true,
		full_only: false,
		pair: None,
	},
	RoleSpec {
		key: "selector",
		names: &["selector lever"],
		wants: Wants::State,
		prefer: Prefer::Proven,
		required: false,
		full_only: false,
		pair: None,
	},
	RoleSpec {
		key: "input shaft speed",
		names: &["input shaft speed"],
		wants: Wants::Quantity,
		prefer: Prefer::Proven,
		required: false,
		full_only: false,
		pair: None,
	},
	RoleSpec {
		key: "output shaft speed",
		names: &["output shaft speed"],
		wants: Wants::Quantity,
		prefer: Prefer::Proven,
		required: false,
		full_only: false,
		pair: None,
	},
	// Actual before specified: that is the order `watch` already puts a pair
	// in, and the gap between the two is the whole diagnostic.
	RoleSpec {
		key: "boost actual",
		names: &["boost pressure"],
		wants: Wants::Quantity,
		prefer: Prefer::Proven,
		required: false,
		full_only: false,
		pair: Some(plan::Role::Actual),
	},
	RoleSpec {
		key: "boost specified",
		names: &["boost pressure"],
		wants: Wants::Quantity,
		prefer: Prefer::Proven,
		required: false,
		full_only: false,
		pair: Some(plan::Role::Specified),
	},
	RoleSpec {
		key: "air mass",
		names: &["mass air flow", "air mass"],
		wants: Wants::Quantity,
		prefer: Prefer::Proven,
		required: false,
		full_only: false,
		pair: None,
	},
	RoleSpec {
		key: "barometer",
		names: &["barometric pressure"],
		wants: Wants::Quantity,
		prefer: Prefer::Proven,
		required: false,
		full_only: true,
		pair: None,
	},
	RoleSpec {
		key: "ambient",
		names: &["ambient air temperature", "outside air temperature"],
		wants: Wants::Quantity,
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
	/// True for a row this unit's own catalog carries — a measurement somebody
	/// proved on this part number. False for one the standard table predicts
	/// from the unit's address alone.
	proven: bool,
}

/// Everything readable on the units the car reported.
///
/// The standard set is offered only on the units ISO 15765-4 addresses for
/// emissions diagnostics, because nothing obliges any other unit's `F4xx`
/// identifiers to be the standard's — and on the reference car they
/// demonstrably are not. Where a unit's own catalog covers an address the
/// standard also names, the unit's row wins: the two can mean different things
/// at the same identifier, and one of them would otherwise be silently wrong.
fn candidates(store: &CatalogStore, extracted: &crate::extracted::Extracted, units: &[UnitIdentity]) -> Vec<Candidate> {
	let mut out: Vec<Candidate> = Vec::new();
	for unit in units {
		let request = unit.request;
		let emissions = UnitAddress::from_request(request).is_some_and(|address| address.is_emissions_related());
		if emissions {
			for p in vag_data_labels::obd::PIDS {
				out.push(Candidate {
					request,
					did: vag_data_labels::obd::did_for_pid(p.pid),
					def: p.to_def(),
					proven: false,
				});
			}
		}
		for def in crate::extracted::for_unit(
			store,
			extracted,
			unit.part_number.as_deref(),
			unit.odx_name.as_deref(),
			unit.odx_version.as_deref(),
		) {
			let ReadId::Uds(did) = def.address;
			match out.iter_mut().find(|c| c.request == request && c.did == did) {
				Some(existing) => {
					existing.def = def;
					existing.proven = true;
				}
				None => out.push(Candidate {
					request,
					did,
					def,
					proven: true,
				}),
			}
		}
	}
	out
}

/// Whether a row answers to a role's words, in a form the role can use.
fn matches(spec: &RoleSpec, def: &MeasurementDef) -> bool {
	let usable = match spec.wants {
		Wants::Quantity => matches!(def.scaling, Scaling::Linear(_)),
		Wants::State => matches!(def.scaling, Scaling::Enum { .. }),
	};
	if !usable {
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

/// The size of one step of a row's value, which is what "finest" means.
///
/// A row that is not linear has no step. Only [`Wants::Quantity`] roles rank by
/// fineness and those admit nothing else, so the fallback ranks last rather
/// than inventing an order.
fn quantum(def: &MeasurementDef) -> f64 {
	match &def.scaling {
		Scaling::Linear(s) => s.factor.abs(),
		_ => f64::INFINITY,
	}
}

/// Order two matches for a role, best first.
fn better(a: &Candidate, b: &Candidate, prefer: Prefer) -> std::cmp::Ordering {
	// Unit id then identifier last in every rule, so the answer does not depend
	// on the order the car happened to report its units in.
	let stable = |c: &Candidate| (c.request, c.did);
	match prefer {
		Prefer::Finest => quantum(&a.def).total_cmp(&quantum(&b.def)).then_with(|| stable(a).cmp(&stable(b))),
		Prefer::Proven => (u8::from(!a.proven), stable(a)).cmp(&(u8::from(!b.proven), stable(b))),
	}
}

/// Add a channel, unless that identifier is already being asked for.
///
/// The same identifier twice in one request wastes a slot and makes the
/// response ambiguous to split.
fn push(into: &mut Vec<Resolved>, channel: Resolved) {
	if !into.iter().any(|r| r.request == channel.request && r.did == channel.did) {
		into.push(channel);
	}
}

/// Resolve every channel a run needs, by name, against what the car reported.
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
		let mut hits: Vec<&Candidate> = pool.iter().filter(|c| matches(spec, &c.def)).collect();
		if hits.is_empty() {
			if spec.required {
				missing.push(Missing {
					key: spec.key,
					tried: spec.names.iter().map(|n| n.to_string()).collect(),
				});
			}
			continue;
		}
		hits.sort_by(|a, b| better(a, b, spec.prefer));
		let resolved = |key: &'static str, c: &Candidate| Resolved {
			key,
			request: c.request,
			did: c.did,
			def: c.def.clone(),
		};
		push(&mut found, resolved(spec.key, hits[0]));
		if spec.key == SPEED {
			for extra in &hits[1..] {
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

	// One request holds `BATCH` identifiers and asking for more makes the whole
	// response look empty, so a unit with more rows than that loses the ones
	// furthest down the priority order rather than losing all of them.
	let mut used: BTreeMap<u16, usize> = BTreeMap::new();
	let (mut leading_batch, mut background) = (Vec::new(), Vec::new());
	for channel in found {
		let taken = used.entry(channel.request).or_default();
		if *taken >= plan::BATCH {
			continue;
		}
		*taken += 1;
		match channel.request == leading.request {
			true => leading_batch.push(channel),
			false => background.push(channel),
		}
	}

	let cross_check_speeds = leading_batch
		.iter()
		.chain(background.iter())
		.filter(|r| r.key == CROSS_SPEED)
		.cloned()
		.collect();

	Ok(Set {
		leading,
		leading_batch,
		background,
		cross_check_speeds,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::borrow::Cow;
	use std::path::PathBuf;
	use vag_data_labels::catalog::MeasurementCatalog;
	use vag_data_labels::measure::{LinearScale, RawForm};

	/// Request ids of the reference car's units, for tests only — the module
	/// itself never names a unit, and never names an identifier at all.
	const ENGINE: u16 = 0x7E0;
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

	/// A catalog store written for one test, under the system temp directory.
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
				levels: levels.iter().map(|(v, n)| (*v, n.to_string())).collect(),
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
		// by word, nothing below would be found.
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
		assert_eq!(set.leading_batch.len(), 4);
		assert!(set.background.is_empty());
		assert!(set.cross_check_speeds.is_empty());
	}

	#[test]
	fn the_reference_car_leads_on_whichever_unit_owns_its_finest_speed() {
		let (store, units) = reference(need_rows!());
		let set = resolve(&store, &crate::extracted::Extracted::none(), &units, true).expect("the reference car resolves");

		// Derived, not declared: the gearbox leads because its speed is the
		// finest one on the car, not because it is the gearbox.
		assert_eq!(set.leading.source(), "7E1:F40D");
		assert_eq!(set.leading.request, GEARBOX);
		assert_eq!(quantum(&set.leading.def), 0.01);

		// Everything the gearbox owns is read at the leading cadence, and
		// everything else at the background one.
		let leading_keys: Vec<&str> = set.leading_batch.iter().map(|r| r.key).collect();
		assert!(leading_keys.contains(&"gear"), "{leading_keys:?}");
		assert!(leading_keys.contains(&"pedal"), "{leading_keys:?}");
		assert!(leading_keys.contains(&"selector"), "{leading_keys:?}");
		assert!(leading_keys.contains(&"input shaft speed"), "{leading_keys:?}");
		assert!(set.leading_batch.iter().all(|r| r.request == GEARBOX));

		let background_keys: Vec<&str> = set.background.iter().map(|r| r.key).collect();
		for wanted in ["engine speed", "boost actual", "boost specified", "air mass"] {
			assert!(background_keys.contains(&wanted), "{wanted} missing from {background_keys:?}");
		}
		assert_eq!(resolved(&set, "engine speed").unwrap().request, ENGINE);

		// Both other speeds are read, and neither times anything.
		let crossed: Vec<String> = set.cross_check_speeds.iter().map(|r| r.source()).collect();
		assert!(crossed.contains(&"714:22D2".to_string()), "{crossed:?}");
		assert!(crossed.iter().all(|s| s != "7E1:F40D"));
		// A cross-check is polled: it is in one of the batches too.
		assert!(set.all().filter(|r| r.key == CROSS_SPEED).count() == crossed.len());

		// No request may exceed what a unit answers in one go.
		let mut per_unit: BTreeMap<u16, usize> = BTreeMap::new();
		for channel in set.all() {
			*per_unit.entry(channel.request).or_default() += 1;
		}
		assert!(per_unit.values().all(|n| *n <= plan::BATCH), "{per_unit:?}");
	}

	#[test]
	fn the_finest_speed_wins_and_a_remaining_tie_breaks_by_unit_id() {
		// Four units answer to a speed name. The finest is neither the first
		// nor the last to be reported, so neither reporting order nor unit id
		// can produce this answer by accident.
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
		assert_eq!(speed.tried, vec!["vehicle speed", "road speed"]);
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
