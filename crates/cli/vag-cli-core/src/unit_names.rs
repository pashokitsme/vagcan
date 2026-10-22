//! Which name and which number a control unit has in this run: the one place that says.
//!
//! **One source per run** (CLAUDE.md, "ODIS and VCDS are independent sources"), for unit names
//! and numbering as for everything else (owner, 2026-10-21). [`UnitNames`] is opened over the
//! run's one [`Run`] and answers from that source alone:
//!
//! - **ODIS**: the variant a unit's `F19E`/`F1A2` pick ([`crate::extracted::best_variants`]),
//!   the logical link that reaches its base variant — the link's long name — and the
//!   diagnostic address VW's table of logical links gives that link, as `setup` cached them
//!   ([`vag_data_labels::odis::Project::unit_names`]) — or, for a project read before they
//!   were kept, as the first run that needs them reads them ([`crate::odis_read`]);
//! - **VCDS**: the label file a unit's `F187` part number resolves to, and the address and name
//!   its header gives ([`vag_data_labels::LabelDb::unit_for_part`]).
//!
//! **One rule for every command**: a unit is named by what it said it is — its part number,
//! its `F19E`/`F1A2` — and nothing else; never by the number some other table gives its id.
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
	/// An ODIS project's unit names, never read, did not read now: why.
	NotRead(String),
	/// An ODIS project whose unit names were never read, and whose folder is not on this
	/// machine to read them from.
	Gone(std::path::PathBuf),
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
	///
	/// An ODIS project's names never read are read here, the first time (off an async worker:
	/// [`UnitNames::for_run_async`]).
	pub fn for_run() -> UnitNames {
		UnitNames::of_run(crate::source::run_here().ok().flatten())
	}

	/// [`UnitNames::for_run`] off the async runtime's worker threads: the first run on a
	/// project read before unit names were kept reads them, seconds of CPU.
	pub async fn for_run_async() -> UnitNames {
		match tokio::task::spawn_blocking(UnitNames::for_run).await {
			Ok(names) => names,
			Err(e) => UnitNames {
				run: None,
				held: Held::Unreadable(e.to_string()),
			},
		}
	}

	/// The names of one run's store, an ODIS project's read from its folder when they were
	/// never read ([`crate::odis_read`]).
	pub fn of_run(run: Option<Run>) -> UnitNames {
		if let Some(run) = run.as_ref().filter(|run| run.source == Source::Odis)
			&& let crate::odis_read::Ensured::Failed(why) = crate::odis_read::ensure(&run.store, crate::odis_read::Part::UnitNames)
		{
			return UnitNames {
				held: Held::NotRead(why),
				run: Some(run.clone()),
			};
		}
		UnitNames::as_held(run)
	}

	/// The names one run's store holds, nothing read to fill it — what a refusal reads of the
	/// other source, which this run does not read.
	fn as_held(run: Option<Run>) -> UnitNames {
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
						(true, Some(dir)) if !std::path::Path::new(&dir).is_dir() => Held::Gone(dir.into()),
						// The folder is there: the read failed, and says why, or has not been made.
						(true, Some(_)) => Held::NotRead(
							crate::odis_read::last_failure(&run.store, crate::odis_read::Part::UnitNames).unwrap_or_else(|| "they are not read yet".to_string()),
						),
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

	/// The other source's names, where it is set up beside this run's — read as it holds them,
	/// nothing read to fill it: for a line that says the other would name what this run does
	/// not. Never used to name anything.
	pub fn other(&self) -> Option<UnitNames> {
		let other = self.source()?.other();
		crate::source::held_here()
			.contains(&other)
			.then(|| crate::source::run_of_here(other))
			.flatten()
			.map(|run| UnitNames::as_held(Some(run)))
	}

	/// Say on stderr, as `units` does, when this run's names are broken rather than not set up.
	fn warn_broken(&self) {
		if let Some(unnamed) = self.unnamed().filter(|u| u.warning) {
			crate::say!("warning: the units are not named: {}", unnamed.why);
		}
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
			Held::NotRead(why) => Some(Unnamed {
				warning: true,
				why: format!("the unit names of {} did not read: {why}", what()),
				hint: None,
			}),
			Held::Gone(dir) => Some(Unnamed {
				warning: false,
				why: format!(
					"the unit names of {} were never read, and its folder {} is not on this machine",
					what(),
					crate::datadir::tilde(dir)
				),
				hint: Some("use \"vagcan setup <ODIS project>\" with the project's folder where it is now".to_string()),
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
///
/// Off the async runtime's worker threads ([`UnitNames::for_run_async`]); a command outside one
/// calls [`install_blocking`].
pub async fn install() -> UnitNames {
	let names = UnitNames::for_run_async().await;
	names.warn_broken();
	names.install(&crate::units::recorded_cars().unwrap_or_default());
	names
}

/// [`install`], for a command with no async runtime to keep free — the dash build, and the
/// firmware's build script through it.
pub fn install_blocking() -> UnitNames {
	let names = UnitNames::for_run();
	names.warn_broken();
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

/// Why `text` names no unit to address: a number this run's source does not give a unit, one
/// it gives and nothing paired with a request id, one whose names were never read — or simply
/// not a unit, in the address layer's words.
fn refusal(flag: &str, text: &str, why: String) -> anyhow::Error {
	let number = (text.len() < 3).then(|| u8::from_str_radix(text, 16).ok()).flatten();
	let Some(number) = number.filter(|n| vag_uds_client::address::request_for_short(*n).is_none()) else {
		return anyhow::anyhow!("{flag}: {why}");
	};
	let (this, other) = facts_here(number);
	let args: Vec<String> = std::env::args().skip(1).collect();
	refuse(flag, number, &this, other.as_ref(), &args)
}

/// What both sources say of `number` over this machine's record.
fn facts_here(number: u8) -> (Facts, Option<Facts>) {
	let cars = crate::units::recorded_cars().unwrap_or_default();
	let run = crate::source::run_here().ok().flatten();
	let names = UnitNames::as_held(run);
	let other = names.other().map(|other| Facts::of(&other, number, &cars));
	(Facts::of(&names, number, &cars), other)
}

/// The refusal of a `dash.toml` reference `<NN>:<rest>` whose number nothing pairs with a
/// request id — said as the build's error `why` is, with the request id to write instead where
/// the car's record has the unit, and the identification only where nothing is recorded.
pub fn dash_refusal(vin: &str, number: u8, rest: &str, why: &str) -> anyhow::Error {
	let (this, other) = facts_here(number);
	let args: Vec<String> = ["dev", "dash", "build", vin].iter().map(|a| a.to_string()).collect();
	dash_refuse(number, rest, why, &this, other.as_ref(), &args)
}

/// [`dash_refusal`] from what each source says.
fn dash_refuse(number: u8, rest: &str, why: &str, this: &Facts, other: Option<&Facts>, args: &[String]) -> anyhow::Error {
	use crate::report::Advice as _;
	let mut err = anyhow::anyhow!("{why}");
	// The id the car's record holds for the unit, where a source knows which recorded unit it is.
	if let Some(request) = this.request.or(other.and_then(|o| o.request)) {
		err = err.help(format!("refer to it by its request id: `{request:03X}:{rest}`"));
	} else if this.recorded == 0 {
		err = err.help("pair each unit's number with its id: `vagcan units --identify` with the car");
	}
	if let Some(note) = this.unpaired_note(number) {
		err = err.note(note);
	}
	if let Some(other) = other.filter(|other| other.request.is_some())
		&& let Some(source) = other.source
	{
		err = err.help(format!("or build as {source} numbers it: `{}`", retyped(args, source)));
	}
	err
}

/// What one source says of a unit number, for a refusal: whether it names it, whether a
/// recorded unit is it, and what it knows of the units recorded here.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Facts {
	source: Option<Source>,
	/// `the ODIS project SK37X`, when it can name units at all.
	describe: Option<String>,
	/// The folder of an ODIS project whose names were never read and cannot be.
	gone: Option<std::path::PathBuf>,
	/// Why its names did not read, where they did not.
	failed: Option<String>,
	/// What it calls the unit it numbers so.
	name: Option<String>,
	/// The request id of the recorded unit that is that unit, by this source.
	request: Option<u16>,
	/// Units recorded on this machine, and how many of them this source names nothing of.
	recorded: usize,
	unknown: usize,
}

impl Facts {
	fn of(names: &UnitNames, number: u8, cars: &[crate::units::Recorded]) -> Facts {
		let units: Vec<&UnitIdentity> = cars.iter().flat_map(|car| &car.units).collect();
		Facts {
			source: names.source(),
			describe: names.describe(),
			gone: match &names.held {
				Held::Gone(dir) => Some(dir.clone()),
				_ => None,
			},
			failed: match &names.held {
				Held::NotRead(why) => Some(why.clone()),
				_ => None,
			},
			name: names.name_of_number(number),
			request: names.pairings(cars).iter().find(|p| p.number == number).and_then(|p| p.request),
			recorded: units.len(),
			unknown: units.iter().filter(|u| names.of(u).is_none()).count(),
		}
	}

	/// Why no recorded unit is `number`, where units are recorded and the source numbers it:
	/// `the VCDS installation knows 1 of the 15 recorded units; none is unit 44`.
	fn unpaired_note(&self, number: u8) -> Option<String> {
		let what = self.describe.as_ref()?;
		self.name.as_ref()?;
		(self.recorded > 0).then(|| {
			format!(
				"{what} knows {} of the {} recorded units; none is unit {number:02X}",
				self.recorded - self.unknown,
				self.recorded
			)
		})
	}
}

/// The refusal [`refusal`] gives, from what each source says. Every `help:` line is a command
/// that works: the other source is offered only where it numbers the unit and has its id, the
/// identification only where no unit is recorded yet. `args` is the command line as typed,
/// after the program's name.
fn refuse(flag: &str, number: u8, this: &Facts, other: Option<&Facts>, args: &[String]) -> anyhow::Error {
	use crate::report::Advice as _;
	let by_id = "give its request id instead: `vagcan units` lists them";
	let mut err = match (&this.gone, &this.name, &this.describe) {
		(Some(dir), _, _) => anyhow::anyhow!("{flag}: unit {number:02X}: this run's unit names were never read")
			.note(format!("the ODIS project's folder {} is not on this machine", crate::datadir::tilde(dir)))
			.help(by_id)
			.help("or read the project where it is now: `vagcan setup <ODIS project>`"),
		(None, None, None) if this.failed.is_some() => anyhow::anyhow!("{flag}: unit {number:02X}: this run's unit names did not read")
			.note(this.failed.clone().unwrap_or_default())
			.help(by_id),
		(None, Some(name), Some(_)) => {
			let err = anyhow::anyhow!("{flag}: control unit {number:02X} ({name}) has no known request id");
			match this.unpaired_note(number) {
				None => err
					.help(by_id)
					.help("or pair each unit's number with its id: `vagcan units --identify` with the car"),
				Some(note) => err.note(note).help(by_id),
			}
		}
		(None, _, Some(what)) => anyhow::anyhow!("{flag}: {what} numbers no control unit {number:02X}").help(by_id),
		(None, _, None) => anyhow::anyhow!("{flag}: no control unit is numbered {number:02X} on this machine").help(by_id),
	};
	if let Some(other) = other.filter(|other| other.request.is_some())
		&& let Some(source) = other.source
	{
		err = err.help(format!("or read it as {source} numbers it: `{}`", retyped(args, source)));
	}
	err
}

/// The command line as typed, made to read `source`: `vagcan --source vcds faults --unit 44`.
pub fn retyped(args: &[String], source: Source) -> String {
	let mut kept = Vec::new();
	let mut rest = args.iter();
	while let Some(arg) = rest.next() {
		match arg.as_str() {
			"--source" => {
				rest.next();
			}
			flag if flag.starts_with("--source=") => {}
			_ => kept.push(arg.as_str()),
		}
	}
	format!("vagcan {} {}", source.flag(), kept.join(" "))
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

	/// D5: the `.lbl`/`.clb` files are not kept, so a VCDS store whose installation is gone
	/// is trusted as it is — never declared stale with nothing to rebuild from — and still
	/// names the units.
	#[test]
	fn a_vcds_store_whose_label_files_are_gone_still_names_units() {
		let here = tempfile::tempdir().unwrap();
		let stores = both(here.path());
		std::fs::remove_dir_all(here.path().join("Labels")).unwrap();
		assert_eq!(of(&names(&stores, Source::Vcds), GEARBOX), named(Some(0x02), "vcds gearbox"));
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
	fn a_project_whose_names_were_never_read_and_whose_folder_is_gone_says_what_is_missing() {
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
		assert_eq!(
			unnamed.why,
			"the unit names of the ODIS project SIM00 were never read, and its folder /nowhere/SIM00 is not on this machine"
		);
		assert_eq!(
			unnamed.hint.as_deref(),
			Some("use \"vagcan setup <ODIS project>\" with the project's folder where it is now")
		);
	}

	/// A project read before names were kept, its folder on this machine: the run reads them
	/// there, and says nothing to type. A folder that holds no project is a warning.
	#[test]
	fn a_project_whose_names_were_never_read_is_read_where_it_is() {
		let here = tempfile::tempdir().unwrap();
		let folder = here.path().join("SIM00-odis");
		std::fs::create_dir_all(&folder).unwrap();
		let project = Project {
			id: "SIM00".into(),
			dir: here.path().join("data").join("SIM00"),
		};
		std::fs::create_dir_all(&project.dir).unwrap();
		vag_data_db::record_language(&project.cache(), vag_data_db::ODIS, &folder.to_string_lossy(), "eng").unwrap();
		let stores = Stores {
			project: Some(project),
			vcds: crate::project::vcds_at(&here.path().join("vcds")),
		};
		// An empty folder is no project: the read was tried, and says why it did not read.
		let unnamed = names(&stores, Source::Odis).unnamed().unwrap();
		assert!(unnamed.warning, "{unnamed:?}");
		assert!(
			unnamed.why.starts_with("the unit names of the ODIS project SIM00 did not read: "),
			"{unnamed:?}"
		);
		assert_eq!(unnamed.hint, None, "nothing to type");
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

	/// What a source says of a number, for [`refuse`].
	fn facts(source: Source, name: Option<&str>, paired: bool, recorded: usize, unknown: usize) -> Facts {
		Facts {
			source: Some(source),
			describe: Some(match source {
				Source::Odis => "the ODIS project SIM00".into(),
				Source::Vcds => "the VCDS installation".into(),
			}),
			gone: None,
			failed: None,
			name: name.map(str::to_string),
			request: paired.then_some(0x714),
			recorded,
			unknown,
		}
	}

	fn refused(this: &Facts, other: Option<&Facts>, number: u8) -> (String, Vec<crate::report::Hint>) {
		let args: Vec<String> = ["--source", "x", "faults", "--unit"]
			.iter()
			.map(|a| a.to_string())
			.chain([format!("{number:02X}")])
			.collect();
		let err = refuse("--unit", number, this, other, &args);
		(err.to_string(), crate::report::hints(&err))
	}

	fn help(text: &str) -> crate::report::Hint {
		crate::report::Hint::Help(text.into())
	}

	fn note(text: &str) -> crate::report::Hint {
		crate::report::Hint::Note(text.into())
	}

	const BY_ID: &str = "give its request id instead: `vagcan units` lists them";

	/// The reviewers' four numbers, each way, on a machine with one car recorded (15 units):
	///
	/// - `44`: both number it; ODIS knows the recorded unit, VCDS not its part number.
	/// - `4B`: ODIS numbers it, VCDS does not; no recorded unit is it.
	/// - `61`: VCDS numbers it, ODIS does not; no recorded unit is it.
	/// - `4D`: neither numbers it.
	///
	/// No help line names a command that would fail: the other source is offered only where
	/// it numbers the unit and has its id, the identification only with nothing recorded.
	#[test]
	fn a_refused_number_offers_only_commands_that_work() {
		let odis = |name, paired| facts(Source::Odis, name, paired, 15, 0);
		let vcds = |name, paired| facts(Source::Vcds, name, paired, 15, 9);

		// 44 on a VCDS run: numbered, not paired; ODIS has it.
		assert_eq!(
			refused(
				&vcds(Some("J500 - Steering"), false),
				Some(&odis(Some("Steering Assistance"), true)),
				0x44
			),
			(
				"--unit: control unit 44 (J500 - Steering) has no known request id".into(),
				vec![
					note("the VCDS installation knows 6 of the 15 recorded units; none is unit 44"),
					help(BY_ID),
					help("or read it as ODIS numbers it: `vagcan --source odis faults --unit 44`"),
				]
			)
		);
		// 4B on an ODIS run: numbered, no recorded unit is it; VCDS numbers nothing there.
		assert_eq!(
			refused(&odis(Some("Multifunction Module"), false), Some(&vcds(None, false)), 0x4B),
			(
				"--unit: control unit 4B (Multifunction Module) has no known request id".into(),
				vec![
					note("the ODIS project SIM00 knows 15 of the 15 recorded units; none is unit 4B"),
					help(BY_ID)
				]
			)
		);
		// 4B on a VCDS run: not a number VCDS gives; ODIS has no id for it either.
		assert_eq!(
			refused(&vcds(None, false), Some(&odis(Some("Multifunction Module"), false)), 0x4B),
			("--unit: the VCDS installation numbers no control unit 4B".into(), vec![help(BY_ID)])
		);
		// 61 on an ODIS run: VCDS numbers it, but with no id: not offered.
		assert_eq!(
			refused(&odis(None, false), Some(&vcds(Some("Battery Regulation"), false)), 0x61),
			("--unit: the ODIS project SIM00 numbers no control unit 61".into(), vec![help(BY_ID)])
		);
		// 61 on a VCDS run, and 4D either way.
		assert_eq!(
			refused(&vcds(Some("Battery Regulation"), false), Some(&odis(None, false)), 0x61).1.len(),
			2
		);
		for (this, other) in [(odis(None, false), vcds(None, false)), (vcds(None, false), odis(None, false))] {
			let (error, hints) = refused(&this, Some(&other), 0x4D);
			assert!(error.ends_with("numbers no control unit 4D"), "{error}");
			assert_eq!(hints, [help(BY_ID)]);
		}
		// Nothing recorded yet: the identification pairs numbers with ids, so it is offered.
		let (_, hints) = refused(&facts(Source::Odis, Some("Steering Assistance"), false, 0, 0), None, 0x44);
		assert_eq!(
			hints,
			[
				help(BY_ID),
				help("or pair each unit's number with its id: `vagcan units --identify` with the car")
			]
		);
		// Names never read, the folder gone: said so, not "no such number".
		let gone = Facts {
			gone: Some("/nowhere/SIM00".into()),
			describe: None,
			..facts(Source::Odis, None, false, 15, 15)
		};
		let (error, hints) = refused(&gone, None, 0x44);
		assert_eq!(error, "--unit: unit 44: this run's unit names were never read");
		assert_eq!(hints[0], note("the ODIS project's folder /nowhere/SIM00 is not on this machine"));
		// Names whose read failed, the folder there: the reason, never "not on this machine".
		let failed = Facts {
			failed: Some("not an ODIS project: no AStringData.data".into()),
			describe: None,
			..facts(Source::Odis, None, false, 15, 15)
		};
		let (error, hints) = refused(&failed, None, 0x44);
		assert_eq!(error, "--unit: unit 44: this run's unit names did not read");
		assert_eq!(hints, [note("not an ODIS project: no AStringData.data"), help(BY_ID)]);
	}

	/// A `dash.toml` reference whose number nothing pairs: the request id to write, where the
	/// car's record has the unit by some source; never the identification when units are
	/// recorded.
	#[test]
	fn a_dash_reference_nothing_pairs_names_the_request_id_to_write() {
		let args: Vec<String> = ["dev", "dash", "build", "SIMVIN"].iter().map(|a| a.to_string()).collect();
		let why = "17:IDE00698: control unit 17 (J285) has no known request id";
		let refused = |this: &Facts, other: Option<&Facts>| {
			let err = dash_refuse(0x17, "IDE00698", why, this, other, &args);
			(err.to_string(), crate::report::hints(&err))
		};
		let vcds = facts(Source::Vcds, Some("J285"), false, 15, 14);
		let odis = facts(Source::Odis, Some("Dash Board"), true, 15, 0);
		assert_eq!(
			refused(&vcds, Some(&odis)),
			(
				why.to_string(),
				vec![
					help("refer to it by its request id: `714:IDE00698`"),
					note("the VCDS installation knows 1 of the 15 recorded units; none is unit 17"),
					help("or build as ODIS numbers it: `vagcan --source odis dev dash build SIMVIN`"),
				]
			)
		);
		// Nothing knows which recorded unit it is: no id to offer, and no identification either.
		let (_, hints) = refused(&vcds, Some(&facts(Source::Odis, None, false, 15, 15)));
		assert_eq!(hints, [note("the VCDS installation knows 1 of the 15 recorded units; none is unit 17")]);
		// Nothing recorded: the identification is what pairs it.
		let (_, hints) = refused(&facts(Source::Vcds, Some("J285"), false, 0, 0), None);
		assert_eq!(
			hints,
			[help("pair each unit's number with its id: `vagcan units --identify` with the car")]
		);
	}

	#[test]
	fn the_command_offered_reads_the_other_source_and_keeps_the_rest() {
		let args = |line: &str| line.split(' ').map(str::to_string).collect::<Vec<_>>();
		assert_eq!(retyped(&args("faults --unit 44"), Source::Vcds), "vagcan --source vcds faults --unit 44");
		assert_eq!(
			retyped(&args("-v --source odis sensors --unit 44"), Source::Vcds),
			"vagcan --source vcds -v sensors --unit 44"
		);
		assert_eq!(
			retyped(&args("--source=odis units --identify 44"), Source::Vcds),
			"vagcan --source vcds units --identify 44"
		);
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
