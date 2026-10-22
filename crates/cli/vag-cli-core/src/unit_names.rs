//! Which name and which number a control unit has in this run: the one place that says.
//!
//! **One source per run** (CLAUDE.md, "ODIS and VCDS are independent sources"), for unit names
//! and numbering as for everything else (owner, 2026-10-21). [`UnitNames`] is opened over the
//! run's one [`Run`] and answers from that source alone:
//!
//! - **ODIS**: the variant a unit's `F19E`/`F1A2` pick ([`crate::extracted::best_variants`]),
//!   the logical link that reaches its base variant — the link's long name — and the
//!   diagnostic address VW's table of logical links gives that link, as `setup` cached them
//!   ([`vag_data_labels::odis::Project::unit_names`]);
//! - **VCDS**: the label file a unit's `F187` part number resolves to, and the address and name
//!   its header gives ([`vag_data_labels::LabelDb::unit_for_part`]); a number alone, by the
//!   label files' own numbering.
//!
//! Where the run's source names nothing, the unit goes by its request id, and the commands
//! say so. Nothing of the other source fills the gap — not a name, not a number.
//!
//! **Nothing here asks the car anything.** Which unit is which comes from what the car's
//! units already said to `units --identify` (`~/.vagcan/cars/<VIN>/units.json`) or say in the
//! read in progress. Which request id a number is answered on is learned from the car the
//! same way, and handed to the address layer ([`UnitNames::install`]).

use vag_data_labels::odis::UnitName;
use vag_uds_client::address::UnitNumber;

use crate::plan::UnitIdentity;
use crate::source::{Run, Source};

/// The names and numbers of one run's source.
pub struct UnitNames {
	/// The run's source and its store; `None` with nothing set up.
	run: Option<Run>,
	held: Held,
}

/// What the run's store holds for naming units.
enum Held {
	/// No source set up, or a source asked for and not set up.
	Nothing,
	/// The store would not open: why.
	Unreadable(String),
	/// An ODIS project read before unit names were kept: its folder.
	ReadBefore(String),
	Vcds(Box<vag_data_labels::LabelDb>),
	Odis(Vec<UnitName>),
}

/// What the run's source calls one unit, and the number it gives it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Named {
	/// The unit's diagnostic address, the two-digit number it goes by (`17`).
	pub number: Option<u8>,
	pub name: Option<String>,
}

/// Why a run names no unit at all, as a command says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unnamed {
	/// `warning:` rather than `note:`: something is broken, not merely not set up.
	pub warning: bool,
	/// What comes after `the units are not named: `.
	pub why: String,
	/// The hint under it, in parentheses, where there is something to type.
	pub hint: Option<String>,
}

impl UnitNames {
	/// This run's source's names: the project or the VCDS store, whichever the run reads
	/// ([`crate::source::run_here`]). A source asked for and not set up names nothing; the
	/// command refuses it on its own ([`crate::source::check`]).
	pub fn for_run() -> UnitNames {
		UnitNames::of_run(crate::source::run_here().ok().flatten())
	}

	/// The names of one run's store.
	pub fn of_run(run: Option<Run>) -> UnitNames {
		let held = match &run {
			None => Held::Nothing,
			Some(run) => match run.source {
				Source::Vcds => match vag_data_db::load_db(&run.cache()) {
					Ok(db) => Held::Vcds(Box::new(db)),
					Err(e) => Held::Unreadable(e.to_string()),
				},
				Source::Odis => match (
					vag_data_db::unit_names_of(&run.cache()),
					vag_data_db::odis_sources_without_unit_names(&run.cache()),
				) {
					(Ok(rows), Ok(unread)) => match (rows.is_empty(), unread.into_iter().next()) {
						(true, Some(dir)) => Held::ReadBefore(dir),
						_ => Held::Odis(rows),
					},
					(Err(e), _) | (_, Err(e)) => Held::Unreadable(e.to_string()),
				},
			},
		};
		UnitNames { run, held }
	}

	/// The source the names are from; `None` with none.
	pub fn source(&self) -> Option<Source> {
		self.run.as_ref().map(|run| run.source)
	}

	/// The source in a sentence — `the ODIS project SK37X`, `the VCDS installation` — when
	/// it can name units.
	pub fn describe(&self) -> Option<String> {
		match self.held {
			Held::Vcds(_) | Held::Odis(_) => self.run.as_ref().map(Run::describe),
			_ => None,
		}
	}

	/// Why this run names no unit, or `None` when its source can.
	pub fn unnamed(&self) -> Option<Unnamed> {
		let what = || self.run.as_ref().map(Run::describe).unwrap_or_default();
		match &self.held {
			Held::Vcds(_) | Held::Odis(_) => None,
			Held::Nothing => Some(Unnamed {
				warning: false,
				why: "no label data on this machine".to_string(),
				hint: Some("use \"vagcan setup\" to read it".to_string()),
			}),
			Held::Unreadable(why) => Some(Unnamed {
				warning: true,
				why: format!("{} would not open: {why}", what()),
				hint: None,
			}),
			Held::ReadBefore(dir) => Some(Unnamed {
				warning: false,
				why: format!("{} was read before unit names were kept", what()),
				hint: Some(format!(
					"use \"vagcan setup {}\" to read it again",
					crate::datadir::tilde(std::path::Path::new(dir))
				)),
			}),
		}
	}

	/// What the run's source calls a unit the car identified, and its number — from the
	/// unit's part number on a VCDS run, from its `F19E`/`F1A2` on an ODIS run. Either half
	/// may be missing; `None` when the source names nothing of it.
	pub fn of(&self, unit: &UnitIdentity) -> Option<Named> {
		let named = match &self.held {
			Held::Vcds(db) => {
				let label = db.unit_for_part(unit.part_number.as_deref()?)?;
				Named {
					number: Some(label.address),
					name: Some(label.name.clone()),
				}
			}
			Held::Odis(rows) => {
				let variants = odis_variants(rows);
				let picked = crate::extracted::best_variants(&variants, unit.odx_name.as_deref(), unit.odx_version.as_deref(), &[]);
				let rows: Vec<&UnitName> = rows.iter().filter(|r| picked.contains(&&r.variant)).collect();
				Named {
					number: one(rows.iter().map(|r| r.address)).flatten().and_then(|a| u8::try_from(a).ok()),
					name: one(rows.iter().map(|r| r.name.clone())).flatten(),
				}
			}
			_ => return None,
		};
		(named.number.is_some() || named.name.is_some()).then_some(named)
	}

	/// What the run's source calls the unit it numbers `number`, when it names one.
	pub fn name_of_number(&self, number: u8) -> Option<String> {
		match &self.held {
			Held::Vcds(db) => db.unit_name(number).map(str::to_owned),
			Held::Odis(rows) => one(rows.iter().filter(|r| r.address == Some(u32::from(number))).map(|r| r.name.clone())).flatten(),
			_ => None,
		}
	}

	/// Every number the run's source gives a unit, with its name, by number.
	pub fn numbering(&self) -> Vec<(u8, String)> {
		match &self.held {
			Held::Vcds(db) => db.unit_numbers().to_vec(),
			Held::Odis(rows) => {
				let mut numbers: Vec<u8> = rows.iter().filter_map(|r| r.address.and_then(|a| u8::try_from(a).ok())).collect();
				numbers.sort_unstable();
				numbers.dedup();
				numbers.into_iter().filter_map(|n| self.name_of_number(n).map(|name| (n, name))).collect()
			}
			_ => Vec::new(),
		}
	}

	/// Hand this source's numbering to the address layer, with the request id each number
	/// answered on in the cars recorded here: what `--unit 17` and a table's `UNIT` column go
	/// by for the rest of the run.
	///
	/// `vag-uds-client` reads no label file — it is the protocol layer — so what crosses is
	/// plain numbers and strings. A number two records put on two ids, or an id they give two
	/// numbers, is left unpaired: a guess there addresses another unit than the one named.
	pub fn install(&self, cars: &[crate::units::Recorded]) {
		vag_uds_client::address::install(self.numbering().into_iter().map(|(number, name)| UnitNumber {
			number,
			request: None,
			name: Some(name),
		}));
		vag_uds_client::address::install(self.pairings(cars));
	}

	/// Each recorded unit's number in this source, paired with the request id it answered on —
	/// over every recorded car, since which one is plugged in is not known before it is asked.
	fn pairings(&self, cars: &[crate::units::Recorded]) -> Vec<UnitNumber> {
		let mut found: Vec<(u8, u16, Option<String>)> = Vec::new();
		for unit in cars.iter().flat_map(|car| &car.units) {
			let Some(Named { number: Some(number), name }) = self.of(unit) else {
				continue;
			};
			if !found.iter().any(|(n, r, _)| *n == number && *r == unit.request) {
				found.push((number, unit.request, name));
			}
		}
		found
			.iter()
			.filter(|(number, request, _)| found.iter().all(|(n, r, _)| (n == number) == (r == request)))
			.map(|(number, request, name)| UnitNumber {
				number: *number,
				request: Some(*request),
				name: name.clone(),
			})
			.collect()
	}
}

/// This run's numbering in force for a command that takes a unit (`faults --unit`, `sensors
/// --unit`, `watch --did`, `adaptations`, and every table's `UNIT` column): the run's source's
/// numbers, paired with the request ids the cars recorded here answered on. Nothing is asked
/// of the car; a machine with no source, or no record, keeps the numbers it had — request ids
/// still work.
pub fn install() -> UnitNames {
	let names = UnitNames::for_run();
	names.install(&crate::units::recorded_cars().unwrap_or_default());
	names
}

/// A unit as typed on the command line — a number, `17`, or a request id, `714` — in this
/// run's numbering ([`install`] first). A number with no request id in force is refused, cargo's
/// way, with what to type instead: the request id, the identification that pairs numbers with
/// ids, or the other source where it numbers units and is set up.
pub fn parse(flag: &str, text: &str) -> anyhow::Result<vag_uds_client::address::UnitAddress> {
	vag_uds_client::address::parse(text).map_err(|why| refusal(flag, text, why))
}

/// [`parse`] over a comma-separated list, `01,713,70E`: one bad token fails the list.
pub fn parse_list(flag: &str, spec: &str) -> anyhow::Result<Vec<vag_uds_client::address::UnitAddress>> {
	let units: Vec<_> = spec
		.split(',')
		.map(str::trim)
		.filter(|token| !token.is_empty())
		.map(|token| parse(flag, token))
		.collect::<anyhow::Result<_>>()?;
	if units.is_empty() {
		return Err(anyhow::anyhow!("{flag}: no control unit given"));
	}
	Ok(units)
}

/// Why `text` names no unit to address: a number this run's source does not give a unit, or
/// one it gives and nothing paired with a request id — or simply not a unit, in the address
/// layer's words.
fn refusal(flag: &str, text: &str, why: String) -> anyhow::Error {
	use crate::report::Advice as _;
	let number = (text.len() < 3).then(|| u8::from_str_radix(text, 16).ok()).flatten();
	let Some(number) = number.filter(|n| vag_uds_client::address::request_for_short(*n).is_none()) else {
		return anyhow::anyhow!("{flag}: {why}");
	};
	let by_id = "give its request id instead: `vagcan units` lists them";
	let run = crate::source::run_here().ok().flatten();
	// The other source, where it is set up: the one way to the number as it numbers units.
	let other = run
		.as_ref()
		.map(|run| run.source.other())
		.filter(|other| crate::source::held_here().contains(other))
		.map(|other| format!("or number the units as {other} does: `{}`", other.flag()));
	let err = match (vag_uds_client::address::name_for_short(number), &run) {
		(Some(name), _) => anyhow::anyhow!("{flag}: control unit {number:02X} ({name}) has no known request id")
			.help(by_id)
			.help("or pair each unit's number with its id: `vagcan units --identify` with the car"),
		(None, Some(run)) => anyhow::anyhow!("{flag}: {} numbers no control unit {number:02X}", run.describe()).help(by_id),
		(None, None) => anyhow::anyhow!("{flag}: no control unit is numbered {number:02X} on this machine").help(by_id),
	};
	match other {
		Some(other) => err.help(other),
		None => err,
	}
}

/// The distinct variant names of an ODIS source's rows, the list a unit's `F19E`/`F1A2` are
/// matched against.
fn odis_variants(rows: &[UnitName]) -> Vec<String> {
	let mut variants: Vec<String> = rows.iter().map(|r| r.variant.clone()).collect();
	variants.sort_unstable();
	variants.dedup();
	variants
}

/// The one value every item agrees on; `None` with none, or two that differ.
fn one<T: PartialEq>(mut items: impl Iterator<Item = T>) -> Option<T> {
	let first = items.next()?;
	items.all(|item| item == first).then_some(first)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::project::{Project, Stores};

	/// A made-up ODIS row.
	fn row(variant: &str, link: &str, name: &str, address: Option<u32>) -> UnitName {
		UnitName {
			variant: variant.into(),
			base_variant: format!("BV_{link}"),
			link: link.into(),
			name: Some(name.into()),
			address,
		}
	}

	fn unit(request: u16, part: Option<&str>, odx: Option<&str>) -> UnitIdentity {
		UnitIdentity {
			request,
			part_number: part.map(str::to_string),
			odx_name: odx.map(str::to_string),
			odx_version: Some("001001".into()),
			component: Some("SIM COMPONENT".into()),
		}
	}

	/// Made-up VCDS label files, each named after its part number, with a unit header giving its
	/// address and name.
	fn label_files(dir: &std::path::Path, files: &[(&str, u8, &str)]) -> std::path::PathBuf {
		let labels = dir.join("Labels");
		std::fs::create_dir_all(&labels).unwrap();
		for (file, address, name) in files {
			std::fs::write(
				labels.join(format!("{file}.lbl")),
				format!("; Component: {name} (#{address:02X})\n001,1,Sim value,,\n"),
			)
			.unwrap();
		}
		labels
	}

	/// Both sources set up, each naming the units its own way, every name marked with its
	/// source: ODIS names the dash `17` and the engine `01`, VCDS names the engine `01` and a
	/// gearbox ODIS does not know. Invented.
	fn both(here: &std::path::Path) -> Stores {
		let project = Project {
			id: "SIM00".into(),
			dir: here.join("data").join("SIM00"),
		};
		let vcds = crate::project::vcds_at(&here.join("vcds"));
		for dir in [&project.dir, &vcds.dir] {
			std::fs::create_dir_all(dir).unwrap();
		}
		vag_data_db::put_unit_names(
			&project.cache(),
			"/nowhere/SIM00",
			&[
				row("EV_SimDash_001", "LL_SimDash", "odis dash", Some(0x17)),
				row("EV_SimEngine_001", "LL_SimEngine", "odis engine", Some(0x01)),
				row("EV_SimRadio_001", "LL_SimRadio", "odis radio", None),
			],
		)
		.unwrap();
		let labels = label_files(here, &[("SIM906000A", 0x01, "vcds engine"), ("SIM300000B", 0x02, "vcds gearbox")]);
		vag_data_db::build_db(&labels, &vcds.cache()).unwrap();
		Stores {
			project: Some(project),
			vcds,
		}
	}

	fn names(stores: &Stores, source: Source) -> UnitNames {
		UnitNames::of_run(Run::of(stores, source))
	}

	const DASH: (u16, Option<&str>, Option<&str>) = (0x714, None, Some("EV_SimDash"));
	const ENGINE: (u16, Option<&str>, Option<&str>) = (0x7E0, Some("SIM906000A"), Some("EV_SimEngine"));
	const GEARBOX: (u16, Option<&str>, Option<&str>) = (0x7E1, Some("SIM300000B"), Some("EV_SimGearbox"));
	const RADIO: (u16, Option<&str>, Option<&str>) = (0x773, Some("SIM035000E"), Some("EV_SimRadio"));

	fn of(names: &UnitNames, (request, part, odx): (u16, Option<&str>, Option<&str>)) -> Option<Named> {
		names.of(&unit(request, part, odx))
	}

	fn named(number: Option<u8>, name: &str) -> Option<Named> {
		Some(Named {
			number,
			name: Some(name.into()),
		})
	}

	#[test]
	fn an_odis_run_names_and_numbers_units_from_odis_alone() {
		let here = tempfile::tempdir().unwrap();
		let stores = both(here.path());
		let odis = names(&stores, Source::Odis);
		assert_eq!(odis.describe().as_deref(), Some("the ODIS project SIM00"));
		assert_eq!(of(&odis, DASH), named(Some(0x17), "odis dash"));
		assert_eq!(of(&odis, ENGINE), named(Some(0x01), "odis engine"));
		// A name and no number: ODIS's table gives the link no address.
		assert_eq!(of(&odis, RADIO), named(None, "odis radio"));
		// A gearbox VCDS names and ODIS does not: nothing, never VCDS's name.
		assert_eq!(of(&odis, GEARBOX), None);
		assert_eq!(odis.name_of_number(0x02), None);
		assert_eq!(odis.name_of_number(0x17).as_deref(), Some("odis dash"));
		assert_eq!(odis.numbering(), [(0x01, "odis engine".to_string()), (0x17, "odis dash".to_string())]);
	}

	#[test]
	fn a_vcds_run_names_and_numbers_units_from_vcds_alone() {
		let here = tempfile::tempdir().unwrap();
		let stores = both(here.path());
		let vcds = names(&stores, Source::Vcds);
		assert_eq!(vcds.describe().as_deref(), Some("the VCDS installation"));
		assert_eq!(of(&vcds, ENGINE), named(Some(0x01), "vcds engine"));
		assert_eq!(of(&vcds, GEARBOX), named(Some(0x02), "vcds gearbox"));
		// The dash ODIS names: nothing on a VCDS run.
		assert_eq!(of(&vcds, DASH), None);
		assert_eq!(vcds.name_of_number(0x17), None);
		assert!(
			vcds.numbering().iter().all(|(_, name)| name.starts_with("vcds")),
			"{:?}",
			vcds.numbering()
		);
	}

	#[test]
	fn each_source_names_units_alone() {
		let here = tempfile::tempdir().unwrap();
		let stores = both(here.path());
		let odis_only = Stores {
			project: stores.project.clone(),
			vcds: crate::project::vcds_at(&here.path().join("no-vcds")),
		};
		let vcds_only = Stores {
			project: None,
			vcds: stores.vcds.clone(),
		};
		assert_eq!(of(&names(&odis_only, Source::Odis), DASH), named(Some(0x17), "odis dash"));
		assert_eq!(of(&names(&vcds_only, Source::Vcds), GEARBOX), named(Some(0x02), "vcds gearbox"));
		// Nothing set up: nothing named, and the line says what to run.
		let none = UnitNames::of_run(None);
		assert_eq!(of(&none, ENGINE), None);
		assert_eq!(none.unnamed().unwrap().why, "no label data on this machine");
	}

	#[test]
	fn a_project_read_before_unit_names_were_kept_says_to_read_it_again() {
		let here = tempfile::tempdir().unwrap();
		let project = Project {
			id: "SIM00".into(),
			dir: here.path().join("data").join("SIM00"),
		};
		std::fs::create_dir_all(&project.dir).unwrap();
		vag_data_db::record_language(&project.cache(), vag_data_db::ODIS, "/nowhere/SIM00", "eng").unwrap();
		let stores = Stores {
			project: Some(project),
			vcds: crate::project::vcds_at(&here.path().join("vcds")),
		};
		let unnamed = names(&stores, Source::Odis).unnamed().unwrap();
		assert_eq!(unnamed.why, "the ODIS project SIM00 was read before unit names were kept");
		assert_eq!(unnamed.hint.as_deref(), Some("use \"vagcan setup /nowhere/SIM00\" to read it again"));
	}

	#[test]
	fn a_list_is_parsed_whole_or_not_at_all() {
		let units = parse_list("--unit", "01, 713,70E").unwrap();
		assert_eq!(units.iter().map(|u| u.request).collect::<Vec<_>>(), vec![0x7E0, 0x713, 0x70E]);
		// One bad token fails the list — the caller is about to open the adapter on the
		// strength of this having parsed.
		assert!(parse_list("--unit", "01,zz").is_err());
		// Nothing at all is not an empty selection; it is a mistake.
		assert!(parse_list("--unit", "").is_err());
		assert!(parse_list("--unit", " , ").is_err());
	}

	#[test]
	fn a_number_nothing_pairs_with_an_id_is_refused_with_what_to_type() {
		// 4D: no source on this test's machine numbers it, and nothing installs it.
		let err = parse("--unit", "4D").unwrap_err();
		assert_eq!(err.to_string(), "--unit: no control unit is numbered 4D on this machine");
		assert_eq!(
			crate::report::hints(&err),
			[crate::report::Hint::Help("give its request id instead: `vagcan units` lists them".into())]
		);
		// A request id, and a typo, are the address layer's to judge.
		assert_eq!(parse("--unit", "713").unwrap().request, 0x713);
		assert!(parse("--unit", "zz").unwrap_err().to_string().starts_with("--unit: "));
	}

	#[test]
	fn the_numbering_installed_pairs_a_number_with_the_id_its_unit_answered_on() {
		let here = tempfile::tempdir().unwrap();
		let stores = both(here.path());
		let car = |units: Vec<UnitIdentity>| crate::units::Recorded { vin: "SIMVIN".into(), units };
		let [dash, engine, gearbox] = [DASH, ENGINE, GEARBOX].map(|(r, p, o)| unit(r, p, o));
		let cars = [car(vec![dash.clone(), engine.clone(), gearbox.clone()])];
		let pairs = |names: &UnitNames| -> Vec<(u8, u16)> { names.pairings(&cars).iter().map(|p| (p.number, p.request.unwrap())).collect() };
		assert_eq!(pairs(&names(&stores, Source::Odis)), [(0x17, 0x714), (0x01, 0x7E0)]);
		assert_eq!(pairs(&names(&stores, Source::Vcds)), [(0x01, 0x7E0), (0x02, 0x7E1)]);
		// Two records putting one number on two ids: neither is paired.
		let moved = UnitIdentity { request: 0x715, ..dash };
		let cars = [car(vec![moved]), car(vec![unit(0x714, None, Some("EV_SimDash"))])];
		assert!(names(&stores, Source::Odis).pairings(&cars).is_empty());
		// Or one id under two numbers: neither either.
		let cars = [
			car(vec![unit(0x714, None, Some("EV_SimDash"))]),
			car(vec![unit(0x714, None, Some("EV_SimEngine"))]),
		];
		assert!(names(&stores, Source::Odis).pairings(&cars).is_empty());
	}
}
