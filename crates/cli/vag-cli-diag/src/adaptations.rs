//! `vagcan adaptations <unit>`: one unit's adaptation channels and the values they hold
//! now, read with `0x22` and nothing else.
//!
//! **This is a sweep, and it is guarded as one** (CLAUDE.md, Safety): a unit's
//! adaptations are up to a couple of hundred identifiers read one after another — a fuzz
//! test of its diagnostic server as much as `units --identify <unit>` is. So, the way that
//! command is guarded:
//!
//! - **one named unit** per run ([`run`] takes one, or has one picked) — or `--all`, a
//!   sequence of single-unit runs each guarded on its own ([`all`]; owner, 2026-10-21);
//! - **a bounded list**: exactly the identifiers the project's data describes for this
//!   unit's variant ([`vag_cli_core::adaptation::identifiers`]) — never a range;
//! - **refused on a moving car** ([`safety::require_stationary`], "no answer" counted as
//!   moving), checked before the unit is identified and again right before the reads;
//! - **stopped the moment the unit changes** under them ([`anomaly::Monitor`] through
//!   [`scan::Guard`]), with the unit's part number, answered just before, re-read as a
//!   witness every [`anomaly::WITNESS_EVERY`] requests and once after the last;
//! - **never through the dash board**: the caller resolves a cable (`--slcan` makes the
//!   board's one), as `units --identify <unit>` does;
//! - **the allowlist** holds for every request: they go through [`AsyncUdsClient`], and
//!   the only ones made are `0x22`;
//! - **paced**: [`PACE`] between reads, one exchange at a time over the one bus.
//!
//! `--all` is a loop over [`one_unit`], each run whole: a fresh stationary check before
//! every unit, [`UNIT_PAUSE`] between units, only the units with adaptation data, and the
//! whole run ends at the first unit that changes under its reads.
//!
//! What it prints is a grouped result, summary first (CLAUDE.md, Command output): one line of
//! facts, a blank line, then a block per unit read — its heading, and a line per field. With
//! `--all` the blocks are printed once the run ends, or once it is stopped, with what was read
//! before it.

use std::time::Duration;

use anyhow::{Result, anyhow};
use vag_cli_core::adaptation::{self, Adaptation};
use vag_cli_core::extracted::Unsettled;
use vag_cli_core::marks::facts;
use vag_cli_core::plan::UnitIdentity;
use vag_cli_core::report::Advice as _;
use vag_cli_core::{note, out, say};
use vag_uds_can::UnitLink;
use vag_uds_client::AsyncUdsClient;
use vag_uds_client::address::UnitAddress;

use crate::render::plural;
use crate::{anomaly, car, plan, progress, safety, scan, units as walk};

/// Between one adaptation read and the next.
///
/// Not a property of a car: a pause that keeps a sweep from being a burst, the way
/// `units --identify <unit>` paces its block. Two hundred identifiers at this pace are
/// seconds, and the unit answers each before the next goes out anyway.
pub const PACE: Duration = Duration::from_millis(20);

/// The part number (`F187`): what every unit answers when it is there, so the one
/// identifier known good before the reads begin — the guard's witness.
const WITNESS: u16 = 0xF187;

/// Between one unit's reads and the next unit's, with `--all`: the units are read one
/// after another, never back to back.
pub const UNIT_PAUSE: Duration = Duration::from_millis(500);

/// What one unit's reads came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
	/// Its adaptations were read: the block to print.
	Read(Read),
	/// Nothing was asked of it beyond its identification.
	Skipped(Skip),
}

/// One unit's adaptations, read, ready to print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
	/// Its heading and a line per field ([`render`]).
	pub block: String,
	/// How many fields the block shows.
	pub fields: usize,
	/// What was asked of the unit and how it answered.
	pub stats: scan::ScanStats,
}

/// Why a unit's adaptations were not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
	/// It did not answer its identification.
	Silent,
	/// Nothing on this machine describes the adaptations of its variant.
	Undescribed,
	/// Its variant is not settled: the identifiers would be a family's union, never swept.
	Unsettled(Unsettled),
}

impl Skip {
	/// The refusal when this unit is the one asked for.
	fn error(self, unit: UnitAddress, reread: Option<String>) -> anyhow::Error {
		match (self, reread) {
			(Skip::Silent, _) => anyhow!("unit {} did not answer", named(unit)).help("`vagcan units` lists this car's units"),
			(Skip::Undescribed, Some(source)) => anyhow!("nothing describes the adaptations of unit {}", named(unit))
				.note("the ODIS project was read before adaptations were kept")
				.help(format!("read it again: `vagcan setup {source}`")),
			(Skip::Undescribed, None) => anyhow!("nothing describes the adaptations of unit {}", named(unit)),
			(Skip::Unsettled(why), _) => anyhow!("nothing describes the adaptations of unit {}", named(unit))
				.note(format!("the variant of unit {} is not settled ({})", named(unit), unsettled(&why)))
				.help(unsettled_help(&why)),
		}
	}

	/// The line on stderr when `--all` passes over this unit, with its hint under it.
	fn line(self, unit: UnitAddress, reread: Option<String>) -> String {
		match (self, reread) {
			(Skip::Silent, _) => format!(
				"skipped unit {}: it did not answer, before any unit was read\n  (use \"vagcan units\" to list this car's units)",
				named(unit)
			),
			(Skip::Undescribed, Some(source)) => format!(
				"skipped unit {}: nothing describes its adaptations\n  (use \"vagcan setup {source}\" to read them from the ODIS project)",
				named(unit)
			),
			(Skip::Undescribed, None) => format!("skipped unit {}: nothing describes its adaptations", named(unit)),
			(Skip::Unsettled(why), _) => format!("skipped unit {}: its variant is not settled ({})", named(unit), unsettled(&why)),
		}
	}
}

/// Why a variant is not settled, in a clause: `F1A2 001001 is none of the 17 variants of its
/// family`.
fn unsettled(why: &Unsettled) -> String {
	let n = format!("{} {}", why.candidates, plural(why.candidates, "variant"));
	match &why.version {
		Some(version) => format!("F1A2 {version} is none of the {n} of its family"),
		None => format!("it did not answer F1A2, and {n} of its family would fit"),
	}
}

/// What to do about an unsettled variant.
fn unsettled_help(why: &Unsettled) -> &'static str {
	match why.version {
		Some(_) => "a newer ODIS project or VCDS installation may describe it: `vagcan setup <dir>`",
		None => "see what it answers: `vagcan units --identify`",
	}
}

/// A path as a person reads it: under the home directory, `~/…` (CLAUDE.md, Command output).
fn home_short(path: &str) -> String {
	match std::env::var_os("HOME").map(std::path::PathBuf::from) {
		Some(home) if !home.as_os_str().is_empty() => match std::path::Path::new(path).strip_prefix(&home) {
			Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
			Ok(rest) => format!("~/{}", rest.display()),
			Err(_) => path.to_string(),
		},
		_ => path.to_string(),
	}
}

/// Where a unit's adaptation rows come from, given the car's VIN and what the unit said:
/// the project's data in a run, a synthetic cache in a test.
pub trait Data {
	/// Learn what can be learned about this unit offline — record it, read its VCDS
	/// rows — and give its adaptation rows.
	#[allow(async_fn_in_trait)] // static dispatch, as `UnitLink`
	async fn rows(&self, vin: Option<&str>, unit: &UnitIdentity) -> Result<Vec<Adaptation>, Unsettled>;

	/// The ODIS project to read again, when it was read before adaptations were kept: what
	/// would give a unit with no rows some.
	fn reread(&self) -> Option<String>;

	/// What this machine recorded of the car's units: by its VIN, or else the one
	/// record that covers the gateway's `listed` units. Nothing is asked of a unit.
	fn recorded(&self, vin: Option<&str>, listed: &[u16]) -> Vec<UnitIdentity>;

	/// How many adaptation rows a recorded unit has, from what is already on this machine;
	/// none for a unit whose variant is not settled.
	fn count(&self, vin: Option<&str>, unit: &UnitIdentity) -> usize;
}

/// The project's own data: what `setup` read, and what the VCDS installation gives the
/// first time a unit is seen — as `units --identify` and `watch` learn it.
pub struct Project {
	extracted: vag_cli_core::extracted::Extracted,
}

impl Project {
	/// This run's project, refused when its one source ([`Extracted::run_source`]) can
	/// describe no adaptation at all — said before anything is opened. An ODIS project read
	/// before adaptations were kept describes none, and a VCDS installation does not fill in
	/// for it: the two are never mixed (CLAUDE.md). A VCDS installation describes them per
	/// unit, read when the unit is first seen, so a run of one is never refused here.
	///
	/// An ODIS project read before adaptations were kept has them read here, from its folder,
	/// the first time ([`vag_cli_core::odis_read`]); only a folder that is gone is refused.
	///
	/// [`Extracted::run_source`]: vag_cli_core::extracted::Extracted::run_source
	pub async fn current() -> Result<Project> {
		use vag_cli_core::odis_read::{Ensured, Part};
		let stores = vag_cli_core::project::stores()?;
		// A source asked for and not set up is refused here, by name.
		let source = vag_cli_core::source::run_source(&stores)?;
		if source == Some(vag_cli_core::source::Source::Odis)
			&& let Some(project) = stores.project.clone()
		{
			match vag_cli_core::odis_read::ensure_async(project, Part::Adaptations).await {
				Ensured::Held => {}
				Ensured::Gone(folder) => {
					return Err(
						anyhow!("project {} describes no adaptations", stores.id())
							.note(format!(
								"its adaptation channels were never read, and its folder {} is not on this machine",
								home_short(&folder.to_string_lossy())
							))
							.help("read the project where it is now: `vagcan setup <ODIS project>`"),
					);
				}
				Ensured::Failed(why) => {
					return Err(anyhow!("project {} describes no adaptations", stores.id()).note(format!("its adaptation channels did not read: {why}")));
				}
			}
		}
		let extracted = vag_cli_core::extracted::open(&stores);
		match extracted.run_source() {
			Some(vag_cli_core::source::Source::Odis) => {
				// Read, and none found: reading it again would find none again.
				if !extracted.describes_adaptations() {
					return Err(anyhow!("this ODIS project describes no adaptations"));
				}
				Ok(Project { extracted })
			}
			Some(_) => Ok(Project { extracted }),
			None => Err(anyhow!("no ODIS project or VCDS installation is read").help("read one: `vagcan setup <dir>`")),
		}
	}
}

impl Data for Project {
	async fn rows(&self, vin: Option<&str>, unit: &UnitIdentity) -> Result<Vec<Adaptation>, Unsettled> {
		walk::record_for(vin, std::slice::from_ref(unit));
		// VCDS's rows of a unit are read the first time it is seen; a run of ODIS has no use
		// for them.
		if self.extracted.run_source() == Some(vag_cli_core::source::Source::Vcds) {
			vag_cli_core::registry::ensure_async(vec![unit.clone()]).await;
		}
		// Opened again: the read above may have written this unit's rows.
		let extracted = match vag_cli_core::project::stores() {
			Ok(stores) => vag_cli_core::extracted::open(&stores),
			Err(_) => self.extracted.clone(),
		};
		extracted.for_car(vin).adaptations(unit.odx_name.as_deref(), unit.odx_version.as_deref())
	}

	fn reread(&self) -> Option<String> {
		self.extracted.odis_unread_adaptations().first().map(|dir| home_short(dir))
	}

	fn recorded(&self, vin: Option<&str>, listed: &[u16]) -> Vec<UnitIdentity> {
		match vin {
			Some(vin) => walk::recorded(vin).ok().flatten().unwrap_or_default(),
			None => {
				let cars = walk::recorded_cars().unwrap_or_default();
				car::record_of_these(listed, &cars).map(|car| car.units.clone()).unwrap_or_default()
			}
		}
	}

	fn count(&self, vin: Option<&str>, unit: &UnitIdentity) -> usize {
		self
			.extracted
			.clone()
			.for_car(vin)
			.adaptations(unit.odx_name.as_deref(), unit.odx_version.as_deref())
			.map_or(0, |rows| rows.len())
	}
}

/// Which units a run reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Which {
	/// The one named.
	One(String),
	/// One picked from the car's units that have adaptation data.
	Pick,
	/// Every one of those, one after another.
	All,
}

/// `vagcan adaptations [<unit> | --all]`: one unit's; with none named, the one picked
/// from this car's units that have any; with `--all`, each of those in turn.
///
/// Everything that can be checked without the car is checked before `open`: the unit's
/// spelling, the project's data, and whether there is anybody to pick with.
pub async fn run<L: UnitLink>(open: impl AsyncFnOnce() -> Result<L>, which: Which) -> Result<()> {
	vag_cli_core::unit_names::install().await;
	let unit = match &which {
		Which::One(text) => Some(car::parse_unit("<UNIT>", text)?),
		Which::Pick | Which::All => None,
	};
	if which == Which::Pick && !vag_cli_core::ui::can_ask() {
		return Err(no_unit());
	}
	let data = Project::current().await?;
	let link = open().await?;
	match (which, unit) {
		(Which::All, _) => {
			all(link, &data, UNIT_PAUSE).await?;
		}
		(_, Some(unit)) => {
			single(link, unit, &data).await?;
		}
		(_, None) => {
			let mut console = vag_cli_core::ui::Console::new("vagcan adaptations <unit>");
			if let (link, Some(unit)) = pick(link, &data, &mut console).await? {
				single(link, unit, &data).await?;
			}
		}
	}
	Ok(())
}

/// The one unit asked for, read and printed: its summary line, then its block. A unit that
/// was not read is a refusal that says why.
pub async fn single<L: UnitLink>(link: L, unit: UnitAddress, data: &impl Data) -> Result<L> {
	let again = format!("`vagcan adaptations {}`", unit.label());
	let (link, outcome) = one_unit(link, unit, data, &again).await?;
	match outcome {
		Outcome::Read(read) => {
			let head = format!("{} {} of unit {}", read.fields, plural(read.fields, "adaptation"), named(unit));
			let reads = [read];
			print(facts(std::iter::once(head).chain(answered(&reads))), &reads);
			Ok(link)
		}
		Outcome::Skipped(skip) => Err(skip.error(unit, data.reread())),
	}
}

/// Every unit of this car that has adaptation data, read one after another — each through
/// [`one_unit`] whole, with its own stationary check, and [`UNIT_PAUSE`] between them.
///
/// The first unit refused or stopped ends the run: the units after it are not read, what was
/// read before it is printed, and the summary says where the run stopped and how many units
/// were left. **A unit that does not answer its identification ends it too, once any unit has
/// been read in this run**: a unit falling silent during a run of sweeps is what the guard
/// stops on (CLAUDE.md, Safety), and it says so with a STOPPED notice. Before anything has
/// been read, such a unit is passed over with a line that says so.
pub async fn all<L: UnitLink>(link: L, data: &impl Data, pause: Duration) -> Result<L> {
	let (mut link, vin, (choices, unrecorded)) = candidates(link, data).await;
	if unrecorded > 0 {
		say!("{}", not_offered(unrecorded));
	}
	if choices.is_empty() {
		return Err(nothing_to_offer());
	}
	note!(
		"{}",
		facts([
			format!("car {}", vin.as_deref().unwrap_or("with no VIN")),
			format!("{} {} with adaptations", choices.len(), plural(choices.len(), "unit")),
		])
	);
	let total = choices.len();
	let (mut reads, mut skipped) = (Vec::new(), 0usize);
	let summary = |reads: &[Read], skipped: usize, stopped: Option<(UnitAddress, usize)>| {
		let fields: usize = reads.iter().map(|r| r.fields).sum();
		facts(
			[
				format!("{} of {total} {} read", reads.len(), plural(total, "unit")),
				format!("{fields} {}", plural(fields, "adaptation")),
			]
			.into_iter()
			.chain(answered(reads))
			.chain([
				if skipped > 0 { format!("{skipped} skipped") } else { String::new() },
				match stopped {
					Some((unit, left)) => format!("stopped at unit {}, {left} not read", named(unit)),
					None => String::new(),
				},
			]),
		)
	};
	for (at, choice) in choices.iter().enumerate() {
		if at > 0 {
			tokio::time::sleep(pause).await;
		}
		match one_unit(link, choice.unit, data, "`vagcan adaptations --all`").await {
			Ok((back, outcome)) => {
				link = back;
				match outcome {
					Outcome::Read(read) => reads.push(read),
					Outcome::Skipped(Skip::Silent) if !reads.is_empty() => {
						print(summary(&reads, skipped, Some((choice.unit, total - at - 1))), &reads);
						// At level 0: the one notice no run may lose.
						say!("{}", silent_notice(choice.unit, reads.len()));
						return Err(
							anyhow!(
								"the run was stopped: unit {} did not answer after {} {} had been read",
								named(choice.unit),
								reads.len(),
								plural(reads.len(), "unit")
							)
							.help("see whether it answers: `vagcan units --identify`"),
						);
					}
					Outcome::Skipped(skip) => {
						skipped += 1;
						say!("{}", skip.line(choice.unit, data.reread()));
					}
				}
			}
			Err(e) => {
				print(summary(&reads, skipped, Some((choice.unit, total - at - 1))), &reads);
				return Err(e);
			}
		}
	}
	print(summary(&reads, skipped, None), &reads);
	Ok(link)
}

/// What `--all` says when a unit falls silent part way through the run.
fn silent_notice(unit: UnitAddress, read: usize) -> String {
	format!(
		"\n\
		 STOPPED: control unit {} did not answer its identification, after {read} {} had been read in this run.\n\
		 The run has ended here: a unit that falls silent during a run of sweeps may have stopped\n\
		 doing its job in the car. Do not clear its faults; the freeze frame is the evidence.\n\
		 \x20 (use \"vagcan units --identify\" to see whether it answers)",
		named(unit),
		plural(read, "unit")
	)
}

/// How the reads were answered, as facts for a summary line: refusals and silences only when
/// there were some, and nothing when nothing was asked.
fn answered(reads: &[Read]) -> Vec<String> {
	let sum = |f: fn(&scan::ScanStats) -> usize| reads.iter().map(|r| f(&r.stats)).sum::<usize>();
	let (asked, hits, refused, failed) = (sum(|s| s.asked), sum(|s| s.hits), sum(|s| s.refused), sum(|s| s.failed));
	if asked == 0 {
		return Vec::new();
	}
	let mut facts = vec![format!("{hits} of {asked} {} answered", plural(asked, "identifier"))];
	if refused > 0 {
		facts.push(format!("{refused} refused"));
	}
	if failed > 0 {
		facts.push(format!("{failed} unanswered"));
	}
	facts
}

/// The summary line, a blank line, and the blocks with a blank line between them.
fn print(summary: String, reads: &[Read]) {
	let blocks: Vec<&str> = reads.iter().map(|r| r.block.as_str()).collect();
	match blocks.is_empty() {
		true => out!("{summary}\n"),
		false => out!("{summary}\n\n{}", blocks.join("\n")),
	}
}

/// A unit as a sentence names it: its number and request id, or the id alone.
fn named(unit: UnitAddress) -> String {
	match unit.label() {
		label if label == format!("{:03X}", unit.request) => label,
		label => format!("{label} ({:03X})", unit.request),
	}
}

/// No unit named and nobody to pick one: what to type.
pub fn no_unit() -> anyhow::Error {
	anyhow!("no unit named, and no terminal to pick one at")
		.help("name the unit: `vagcan adaptations 09`")
		.help("`vagcan units` lists this car's units")
}

/// Read one unit's adaptations, guarded (see the module docs), into the block to print. The
/// link comes back for the next unit. `again` is the command a refusal says to run again.
pub async fn one_unit<L: UnitLink>(link: L, unit: UnitAddress, data: &impl Data, again: &str) -> Result<(L, Outcome)> {
	let link = stationary(link, again).await?;
	let (link, vin) = walk::read_vin(link).await;
	// The four identifiers `units --identify` asks every unit for, of this one: not a
	// sweep, and what picks its variant.
	let mut spinner = progress::Line::new();
	let (link, found) = walk::identify_listed(link, &[unit.request], &mut spinner).await;
	spinner.finish();
	let Some(identity) = found.into_iter().find(|u| u.request == unit.request && u.part_number.is_some()) else {
		return Ok((link, Outcome::Skipped(Skip::Silent)));
	};
	note!(
		"{}",
		facts([
			format!("unit {}", named(unit)),
			format!("F19E {}", identity.odx_name.as_deref().unwrap_or("-")),
			format!("F1A2 {}", identity.odx_version.as_deref().unwrap_or("-")),
		])
	);
	let rows = match data.rows(vin.as_deref(), &identity).await {
		Ok(rows) => rows,
		Err(why) => return Ok((link, Outcome::Skipped(Skip::Unsettled(why)))),
	};
	if rows.is_empty() {
		return Ok((link, Outcome::Skipped(Skip::Undescribed)));
	}
	say_sources(&rows);
	let dids = adaptation::identifiers(&rows);

	// Again, right before the reads: the registry read above can take minutes.
	let link = stationary(link, again).await?;
	let mut uds = AsyncUdsClient::new(link.to_unit(
		vag_uds_transport::CanId::Standard(unit.request),
		vag_uds_transport::CanId::Standard(unit.response),
	));
	let mut monitor = anomaly::Monitor::new(unit.request);
	// The identification just answered; the part number did by the rule above.
	monitor.seed(WITNESS);
	let mut answers: Vec<(u16, Vec<u8>)> = Vec::new();
	let spans: Vec<std::ops::RangeInclusive<u16>> = dids.iter().map(|&did| did..=did).collect();
	let stats = {
		let mut guard = scan::Guard {
			witness: Some(WITNESS),
			monitor: &mut monitor,
		};
		let stats = scan::scan_dids(&mut uds, &spans, PACE, 0, &mut guard, |hit| {
			answers.push((hit.did, hit.data.clone()));
			Ok(())
		})
		.await?;
		if guard.monitor.halted().is_none() {
			guard.confirm(&mut uds).await;
		}
		stats
	};
	let link = L::release(uds.into_transport());
	if let Some(halt) = monitor.halted() {
		// At level 0, through the screen lock: the one notice no run may lose.
		say!("{}", halt.report().trim_end());
		return Err(
			anyhow!("the read was stopped: control unit {} changed while it was being read", halt.unit())
				.help("see whether it answers: `vagcan units --identify`"),
		);
	}
	let read = Read {
		block: render(&heading(unit, &identity), &rows, &answers),
		fields: rows.len(),
		stats,
	};
	Ok((link, Outcome::Read(read)))
}

/// Refuse a moving car, or one that does not say whether it moves:
/// [`safety::require_stationary`] says which, and this says what it costs — no adaptations
/// read — and what to type.
async fn stationary<L: UnitLink>(link: L, again: &str) -> Result<L> {
	match safety::require_stationary(link).await {
		Ok(link) => Ok(link),
		Err((_, safety::NotStationary::Moving(kmh))) => Err(
			anyhow!("the car is moving ({kmh} km/h)")
				.note("adaptations are read only on a parked car")
				.help(format!("stop the car and run {again} again")),
		),
		Err((_, safety::NotStationary::Unknown)) => Err(
			anyhow!("cannot tell whether the car is moving: the engine did not report road speed")
				.note("adaptations are read only on a parked car, and no answer counts as moving")
				.help(format!("switch the ignition on and run {again} again")),
		),
	}
}

/// At `-v`: which source and variants gave the rows, and how many each. One source, the
/// run's: the variants are its own.
fn say_sources(rows: &[Adaptation]) {
	let mut by: std::collections::BTreeMap<(&str, &str), usize> = std::collections::BTreeMap::new();
	for row in rows {
		*by.entry((row.source, row.variant.as_str())).or_default() += 1;
	}
	for ((source, variant), n) in by {
		note!("{}", facts([format!("source {source} {variant}"), format!("{n} {}", plural(n, "field"))]));
	}
}

/// The heading line of one unit's block: its number and request id, what it calls itself
/// and its part number — `09 70E  BODY  5Q0937084CQ`, as `faults` heads a unit.
pub fn heading(unit: UnitAddress, identity: &UnitIdentity) -> String {
	let number = vag_uds_client::address::short_number(unit.request)
		.map(|n| format!("{n:02X} "))
		.unwrap_or_default();
	let mut line = format!("{number}{:03X}", unit.request);
	for part in [identity.component.as_deref(), identity.part_number.as_deref()].into_iter().flatten() {
		let part = part.trim();
		if !part.is_empty() {
			line.push_str("  ");
			line.push_str(part);
		}
	}
	line
}

/// Columns a block is laid out in: readable at 80 (CLAUDE.md, Command output).
const WIDTH: usize = 80;
/// The widest value kept on its field's line; a longer one goes under the name.
const SHORT_VALUE: usize = 24;
/// `  0600  `: the indent and the DID before a name.
const LEAD: usize = 8;

/// One unit's block: the heading, then a line per field — its DID, its name wrapped under
/// itself, and its value (with its unit) in a column when short, on a `= ` line under the
/// name when not. A DID that was not answered says so on every field of it.
pub fn render(heading: &str, rows: &[Adaptation], answers: &[(u16, Vec<u8>)]) -> String {
	use vag_cli_core::table::wrap;
	let width = |text: &str| text.chars().count();
	let names = rows.iter().map(|r| width(&r.name)).max().unwrap_or(0);
	let name_width = names.clamp(1, WIDTH - LEAD - 2 - SHORT_VALUE);
	let mut out = format!("{heading}\n");
	for row in rows {
		let value = match answers.iter().find(|(did, _)| *did == row.did) {
			Some((_, data)) => row.shown(data),
			None => "(not answered)".to_string(),
		};
		let mut name = wrap(&row.name, name_width);
		if name.is_empty() {
			name.push(String::new());
		}
		let short = width(&value) <= SHORT_VALUE;
		for (at, line) in name.iter().enumerate() {
			let did = if at == 0 { format!("{:04X}", row.did) } else { String::new() };
			let text = match (at, short) {
				(0, true) => format!("  {did:<4}  {line:<name_width$}  {value}"),
				_ => format!("  {did:<4}  {line}"),
			};
			out.push_str(text.trim_end());
			out.push('\n');
		}
		if !short {
			for (at, line) in wrap(&value, WIDTH - LEAD - 2).iter().enumerate() {
				let mark = if at == 0 { "= " } else { "  " };
				out.push_str(&format!("{:LEAD$}{mark}{line}\n", ""));
			}
		}
	}
	out
}

/// With no unit named: the car's units that have adaptation data, to pick one. `None`
/// when nothing was picked.
pub async fn pick<L: UnitLink>(link: L, data: &impl Data, asker: &mut impl vag_cli_core::ui::menu::Asker) -> Result<(L, Option<UnitAddress>)> {
	let (link, _, choices) = candidates(link, data).await;
	let picked = choose(&choices, asker)?;
	Ok((link, picked))
}

/// The car's units that have adaptation data, in the whole-car walk's order, and how many
/// listed units were left out for want of a record.
///
/// Which units: the gateway's list (one read), joined to what this machine recorded of
/// the car ([`Data::recorded`]). No unit is asked anything for this — the VIN off the
/// engine and the gateway's list are the two reads; a unit not recorded is counted and
/// left out, and `units --identify` records it.
async fn candidates<L: UnitLink>(link: L, data: &impl Data) -> (L, Option<String>, (Vec<Choice>, usize)) {
	let (link, vin) = walk::read_vin(link).await;
	let mut spinner = progress::Line::new();
	let (link, listed) = walk::installed(link, &mut spinner).await;
	spinner.finish();
	let listed = listed.unwrap_or_default();
	let record = data.recorded(vin.as_deref(), &listed);
	let found = choices(&walk::to_identify(&listed, &[plan::ENGINE]), &record, |unit| {
		data.count(vin.as_deref(), unit)
	});
	(link, vin, found)
}

/// Nothing to pick or to read with `--all`: what to type.
pub fn nothing_to_offer() -> anyhow::Error {
	anyhow!("no recorded unit of this car has adaptations")
		.help("record the car's units: `vagcan units --identify`")
		.help("or name one: `vagcan adaptations 09`")
}

/// The line for listed units that are not offered, with its hint under it.
fn not_offered(unrecorded: usize) -> String {
	let (is, them) = if unrecorded == 1 { ("is", "it") } else { ("are", "them") };
	format!(
		"{unrecorded} {} of this car {is} not recorded on this machine\n  (use \"vagcan units --identify\" to record {them})",
		plural(unrecorded, "unit")
	)
}

/// One unit to pick: its address, what it calls itself, and how many adaptations it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
	pub unit: UnitAddress,
	pub name: String,
	pub adaptations: usize,
}

/// The units of `wanted` (in its order) that `record` knows and that have adaptations;
/// and how many were left out for want of a record.
pub fn choices(wanted: &[u16], record: &[UnitIdentity], count: impl Fn(&UnitIdentity) -> usize) -> (Vec<Choice>, usize) {
	let mut seen = Vec::new();
	let (mut out, mut unrecorded) = (Vec::new(), 0usize);
	for &request in wanted {
		if seen.contains(&request) {
			continue;
		}
		seen.push(request);
		let Some(unit) = UnitAddress::from_request(request) else { continue };
		let Some(identity) = record.iter().find(|u| u.request == request) else {
			unrecorded += 1;
			continue;
		};
		let n = count(identity);
		if n > 0 {
			out.push(Choice {
				unit,
				name: identity.component.clone().unwrap_or_default(),
				adaptations: n,
			});
		}
	}
	(out, unrecorded)
}

/// Offer `choices` and return the one picked.
pub fn choose(choices: &(Vec<Choice>, usize), asker: &mut impl vag_cli_core::ui::menu::Asker) -> Result<Option<UnitAddress>> {
	let (choices, unrecorded) = choices;
	if *unrecorded > 0 {
		asker.say(&not_offered(*unrecorded))?;
	}
	if choices.is_empty() {
		return Err(nothing_to_offer());
	}
	let labels: Vec<(String, String)> = choices
		.iter()
		.map(|c| {
			(
				format!(
					"{} {:03X}",
					car::unit_number(vag_uds_client::address::short_number(c.unit.request)),
					c.unit.request
				),
				facts([c.name.clone(), format!("{} {}", c.adaptations, plural(c.adaptations, "adaptation"))]),
			)
		})
		.collect();
	let items: Vec<vag_cli_core::ui::menu::Item<'_>> = labels
		.iter()
		.map(|(label, detail)| vag_cli_core::ui::menu::Item { label, detail })
		.collect();
	Ok(asker.ask("Which unit's adaptations?", &items, 0)?.map(|at| choices[at].unit))
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::collections::BTreeMap;
	use std::sync::{Arc, Mutex};
	use vag_cli_core::marks::SEP;
	use vag_cli_core::verbosity::{Capture, WIRE};
	use vag_uds_transport::{AsyncIsoTpTransport, CanId, TransportError};

	// The car is made up: identifiers, part numbers and values that belong to no car.

	const ENGINE: u16 = 0x7E0;
	/// What a refusal of one unit's read says to run again.
	const AGAIN: &str = "`vagcan adaptations 09`";
	const UNIT: u16 = 0x70E;
	const SPEED: u16 = 0xF40D;

	#[derive(Default)]
	struct Script {
		records: BTreeMap<(u16, u16), Vec<u8>>,
		/// Every request, by unit, in order.
		sent: Vec<(u16, Vec<u8>)>,
		/// After this many requests to it, a unit says nothing more.
		dies_after: BTreeMap<u16, usize>,
		/// Once this unit has answered this identifier, the car moves at this speed.
		moves_on: Option<((u16, u16), u8)>,
	}

	#[derive(Clone)]
	struct Car(Arc<Mutex<Script>>);

	struct Channel {
		car: Car,
		request: u16,
		pdu: Option<Vec<u8>>,
	}

	impl UnitLink for Car {
		type Channel = Channel;
		fn to_unit(self, request: CanId, _: CanId) -> Channel {
			let CanId::Standard(request) = request else { panic!("extended id") };
			Channel {
				car: self,
				request,
				pdu: None,
			}
		}
		fn release(channel: Channel) -> Car {
			channel.car
		}
	}

	impl AsyncIsoTpTransport for Channel {
		async fn send(&mut self, pdu: &[u8]) -> Result<(), TransportError> {
			self.car.0.lock().unwrap().sent.push((self.request, pdu.to_vec()));
			self.pdu = Some(pdu.to_vec());
			Ok(())
		}

		async fn recv(&mut self, _: Duration) -> Result<Vec<u8>, TransportError> {
			let pdu = self.pdu.take().ok_or(TransportError::Timeout)?;
			let mut script = self.car.0.lock().unwrap();
			if let Some(after) = script.dies_after.get(&self.request)
				&& script.sent.iter().filter(|(unit, _)| *unit == self.request).count() > *after
			{
				return Err(TransportError::Timeout);
			}
			if let Some(((unit, did), kmh)) = script.moves_on
				&& unit == self.request
				&& pdu[..] == [0x22, (did >> 8) as u8, did as u8]
			{
				script.records.insert((ENGINE, SPEED), vec![kmh]);
			}
			match pdu.as_slice() {
				[0x22, hi, lo] => {
					let did = u16::from_be_bytes([*hi, *lo]);
					match script.records.get(&(self.request, did)) {
						Some(data) => Ok([&[0x62, *hi, *lo][..], data].concat()),
						None if script.records.keys().any(|(unit, _)| *unit == self.request) => Ok(vec![0x7F, 0x22, 0x31]),
						None => Err(TransportError::Timeout),
					}
				}
				[sid, ..] => Ok(vec![0x7F, *sid, 0x11]),
				[] => Err(TransportError::Timeout),
			}
		}
	}

	/// A car standing still at `kmh`, whose unit [`UNIT`] answers its identification and
	/// three adaptation identifiers.
	fn car(kmh: Option<u8>) -> Car {
		let mut s = Script::default();
		if let Some(kmh) = kmh {
			s.records.insert((ENGINE, SPEED), vec![kmh]);
		}
		s.records.insert((ENGINE, 0xF190), b"TESTVIN0000000009".to_vec());
		s.records.insert((UNIT, 0xF187), b"PART-0001 ".to_vec());
		s.records.insert((UNIT, 0xF197), b"TEST UNIT".to_vec());
		s.records.insert((UNIT, 0xF19E), b"EV_TestUnit\0".to_vec());
		s.records.insert((UNIT, 0xF1A2), b"001001".to_vec());
		s.records.insert((UNIT, 0x0600), vec![0x01, 0x2A]);
		s.records.insert((UNIT, 0x0610), vec![0x05]);
		Car(Arc::new(Mutex::new(s)))
	}

	fn field(did: u16, bit_offset: u32, name: &str) -> Adaptation {
		let reading = vag_data_labels::odis::Reading {
			did,
			name: name.into(),
			unit: Some("s".into()),
			bit_offset,
			bit_length: 8,
			signed: false,
			big_endian: true,
			scaling: vag_data_labels::catalog::Scaling::Linear(vag_data_labels::measure::LinearScale { factor: 1.0, offset: 0.0 }),
			text_id: None,
		};
		Adaptation::from_odis(&reading, "EV_TestUnit_001")
	}

	/// Data that describes 0x0600 (two fields), 0x0610 and 0x0620 for the unit.
	struct Described(Vec<Adaptation>);

	impl Data for Described {
		async fn rows(&self, vin: Option<&str>, unit: &UnitIdentity) -> Result<Vec<Adaptation>, Unsettled> {
			assert_eq!(vin, Some("TESTVIN0000000009"), "the car's VIN joins the rows");
			assert_eq!(unit.odx_name.as_deref(), Some("EV_TestUnit"));
			Ok(self.0.clone())
		}
		fn reread(&self) -> Option<String> {
			None
		}
		fn recorded(&self, _: Option<&str>, _: &[u16]) -> Vec<UnitIdentity> {
			Vec::new()
		}
		fn count(&self, _: Option<&str>, _: &UnitIdentity) -> usize {
			self.0.len()
		}
	}

	fn described() -> Described {
		Described(vec![
			field(0x0600, 0, "first"),
			field(0x0600, 8, "second"),
			field(0x0610, 0, "third"),
			field(0x0620, 0, "absent"),
		])
	}

	fn unit() -> UnitAddress {
		UnitAddress::from_request(UNIT).unwrap()
	}

	fn sent(car: &Car) -> Vec<(u16, Vec<u8>)> {
		car.0.lock().unwrap().sent.clone()
	}

	/// The adaptation identifiers asked of [`UNIT`], in order.
	fn adaptation_reads(car: &Car) -> Vec<u16> {
		sent(car)
			.into_iter()
			.filter(|(unit, pdu)| *unit == UNIT && pdu[0] == 0x22)
			.map(|(_, pdu)| u16::from_be_bytes([pdu[1], pdu[2]]))
			.filter(|did| !(0xF100..=0xF1FF).contains(did))
			.collect()
	}

	#[tokio::test]
	async fn a_standing_car_has_exactly_the_described_identifiers_read_and_shown() {
		let car = car(Some(0));
		let capture = Capture::start(0);
		single(car.clone(), unit(), &described()).await.unwrap();
		let said = capture.finish();
		// The bounded list: each described DID once, nothing in between, nothing else.
		assert_eq!(adaptation_reads(&car), [0x0600, 0x0610, 0x0620]);
		// Summary first, then the unit's block (CLAUDE.md, Command output).
		assert_eq!(
			said.stdout,
			format!(
				"4 adaptations of unit 70E{SEP}2 of 3 identifiers answered{SEP}1 refused\n\
				 \n\
				 70E  TEST UNIT  PART-0001\n\
				 \x20 0600  first   1 s\n\
				 \x20 0600  second  42 s\n\
				 \x20 0610  third   5 s\n\
				 \x20 0620  absent  (not answered)\n"
			)
		);
		assert_eq!(said.stderr, "", "data on stdout, nothing else at level 0");
		// The witness re-read once, after the last identifier.
		let after_last = sent(&car)
			.into_iter()
			.skip_while(|(_, pdu)| pdu[..] != [0x22, 0x06, 0x20])
			.skip(1)
			.collect::<Vec<_>>();
		assert_eq!(after_last, [(UNIT, vec![0x22, 0xF1, 0x87])]);
	}

	#[tokio::test]
	async fn every_request_is_a_read() {
		let car = car(Some(0));
		one_unit(car.clone(), unit(), &described(), AGAIN).await.unwrap();
		let all = sent(&car);
		assert!(!all.is_empty());
		for (unit, pdu) in all {
			assert_eq!(pdu.first(), Some(&0x22), "{unit:03X} was sent {pdu:02X?}");
		}
	}

	#[tokio::test]
	async fn a_moving_car_is_refused_before_the_unit_is_asked_anything() {
		for (kmh, why) in [(Some(30), "the car is moving (30 km/h)"), (None, "cannot tell")] {
			let car = car(kmh);
			let refused = one_unit(car.clone(), unit(), &described(), AGAIN).await.err().expect("refused");
			let said = vag_cli_core::report::unlabelled(&refused);
			assert!(
				said.contains(why) && said.contains("help:") && said.contains("`vagcan adaptations 09` again"),
				"{said}"
			);
			// Road speed off the engine, and nothing at all of the unit.
			assert_eq!(sent(&car), [(ENGINE, vec![0x22, 0xF4, 0x0D])], "{kmh:?}");
		}
	}

	#[tokio::test]
	async fn a_car_that_starts_moving_before_the_reads_is_refused_before_them() {
		struct Moves(Car);
		impl Data for Moves {
			async fn rows(&self, _: Option<&str>, _: &UnitIdentity) -> Result<Vec<Adaptation>, Unsettled> {
				self.0.0.lock().unwrap().records.insert((ENGINE, SPEED), vec![12]);
				Ok(described().0)
			}
			fn reread(&self) -> Option<String> {
				None
			}
			fn recorded(&self, _: Option<&str>, _: &[u16]) -> Vec<UnitIdentity> {
				Vec::new()
			}
			fn count(&self, _: Option<&str>, _: &UnitIdentity) -> usize {
				0
			}
		}
		let car = car(Some(0));
		let refused = one_unit(car.clone(), unit(), &Moves(car.clone()), AGAIN).await.err().expect("refused");
		assert!(format!("{refused:#}").contains("12 km/h"));
		assert!(adaptation_reads(&car).is_empty(), "{:?}", adaptation_reads(&car));
	}

	#[tokio::test]
	async fn a_unit_that_falls_silent_under_the_reads_stops_them() {
		// Thirty described identifiers; the unit answers its identification and two
		// reads, then nothing.
		let rows = Described((0..30u16).map(|n| field(0x0700 + n, 0, "n")).collect());
		let car = car(Some(0));
		car.0.lock().unwrap().dies_after.insert(UNIT, 4 + 2);
		let capture = Capture::start(0);
		let stopped = one_unit(car.clone(), unit(), &rows, AGAIN).await.err().expect("stopped");
		let said = capture.finish();
		assert!(format!("{stopped:#}").contains("was stopped"), "{stopped:#}");
		assert!(said.stderr.contains("STOPPED"), "{}", said.stderr);
		assert!(said.stdout.is_empty(), "no table for a stopped read: {}", said.stdout);
		// Two answered, then the run of silences the monitor waits for — and no more.
		assert_eq!(adaptation_reads(&car).len(), 2 + anomaly::QUIET_RUN);
	}

	#[tokio::test]
	async fn the_witness_is_re_read_while_the_reads_go_on() {
		let n = anomaly::WITNESS_EVERY as u16 + 1;
		let rows = Described((0..n).map(|k| field(0x0800 + k, 0, "n")).collect());
		let car = car(Some(0));
		one_unit(car.clone(), unit(), &rows, AGAIN).await.unwrap();
		let unit_reads: Vec<Vec<u8>> = sent(&car).into_iter().filter(|(u, _)| *u == UNIT).map(|(_, p)| p).collect();
		let witness = unit_reads.iter().filter(|p| p[..] == [0x22, 0xF1, 0x87]).count();
		// Identification, the one at the cadence, the one after the last.
		assert_eq!(witness, 3);
	}

	#[tokio::test]
	async fn a_unit_that_does_not_answer_or_has_nothing_described_is_not_read() {
		let car = car(Some(0));
		car.0.lock().unwrap().records.retain(|(u, _), _| *u != UNIT);
		let (_, outcome) = one_unit(car.clone(), unit(), &described(), AGAIN).await.unwrap();
		assert_eq!(outcome, Outcome::Skipped(Skip::Silent));
		assert!(adaptation_reads(&car).is_empty());

		let car = self::car(Some(0));
		let (_, outcome) = one_unit(car.clone(), unit(), &Described(Vec::new()), AGAIN).await.unwrap();
		assert_eq!(outcome, Outcome::Skipped(Skip::Undescribed));
		assert!(adaptation_reads(&car).is_empty());
	}

	/// The unit asked for and not read: a refusal in cargo's shape, and nothing on stdout.
	#[tokio::test]
	async fn the_unit_asked_for_and_not_read_is_an_error_that_says_why() {
		let silent = car(Some(0));
		silent.0.lock().unwrap().records.retain(|(u, _), _| *u != UNIT);
		let capture = Capture::start(0);
		let refused = single(silent, unit(), &described()).await.err().expect("refused");
		assert_eq!(
			vag_cli_core::report::unlabelled(&refused),
			"unit 70E did not answer\n\nhelp: `vagcan units` lists this car's units"
		);
		assert_eq!(capture.finish().stdout, "");

		struct Stale;
		impl Data for Stale {
			async fn rows(&self, _: Option<&str>, _: &UnitIdentity) -> Result<Vec<Adaptation>, Unsettled> {
				Ok(Vec::new())
			}
			fn reread(&self) -> Option<String> {
				Some("~/odis".into())
			}
			fn recorded(&self, _: Option<&str>, _: &[u16]) -> Vec<UnitIdentity> {
				Vec::new()
			}
			fn count(&self, _: Option<&str>, _: &UnitIdentity) -> usize {
				0
			}
		}
		let refused = single(car(Some(0)), unit(), &Stale).await.err().expect("refused");
		assert_eq!(
			vag_cli_core::report::unlabelled(&refused),
			"nothing describes the adaptations of unit 70E\n\
			 \n\
			 note: the ODIS project was read before adaptations were kept\n\
			 help: read it again: `vagcan setup ~/odis`"
		);
	}

	#[tokio::test]
	async fn stdout_is_the_same_at_every_level() {
		let mut runs = Vec::new();
		for level in 0..=WIRE {
			let capture = Capture::start(level);
			single(car(Some(0)), unit(), &described()).await.unwrap();
			runs.push(capture.finish());
		}
		vag_cli_core::verbosity::assert_split("adaptations", &runs);
		assert!(
			runs[1].stderr.contains(&format!("source odis EV_TestUnit_001{SEP}4 fields")),
			"{}",
			runs[1].stderr
		);
		assert!(
			runs[1].stderr.contains(&format!("unit 70E{SEP}F19E EV_TestUnit{SEP}F1A2 001001")),
			"{}",
			runs[1].stderr
		);
	}

	// --- `--all` ---

	const GATEWAY: u16 = 0x710;
	const SECOND: u16 = 0x713;
	const THIRD: u16 = 0x714;

	/// Rows per unit, every unit recorded as answering its own `EV_<id>`.
	struct PerUnit(BTreeMap<u16, Vec<Adaptation>>);

	impl Data for PerUnit {
		async fn rows(&self, _: Option<&str>, unit: &UnitIdentity) -> Result<Vec<Adaptation>, Unsettled> {
			Ok(self.0.get(&unit.request).cloned().unwrap_or_default())
		}
		fn reread(&self) -> Option<String> {
			None
		}
		fn recorded(&self, vin: Option<&str>, listed: &[u16]) -> Vec<UnitIdentity> {
			assert_eq!(vin, Some("TESTVIN0000000009"));
			assert_eq!(listed, [UNIT, SECOND, THIRD], "the gateway's list");
			self
				.0
				.keys()
				.map(|&request| UnitIdentity {
					request,
					part_number: Some("PART".into()),
					odx_name: Some(format!("EV_{request:03X}")),
					odx_version: None,
					component: Some(format!("UNIT {request:03X}")),
				})
				.collect()
		}
		fn count(&self, _: Option<&str>, unit: &UnitIdentity) -> usize {
			self.0.get(&unit.request).map_or(0, Vec::len)
		}
	}

	/// The car with two more units, listed by the gateway, each answering its
	/// identification and one adaptation; `0x7E1` is recorded with nothing described.
	fn three_units() -> (Car, PerUnit) {
		let car = car(Some(0));
		{
			let mut s = car.0.lock().unwrap();
			let mut list = vec![0u8; 32];
			for unit in [UNIT, SECOND, THIRD] {
				let n = (unit - 0x700) as usize;
				list[n / 8] |= 1 << (n % 8);
			}
			s.records.insert((GATEWAY, vag_uds_client::gateway::INSTALLATION_LIST), list);
			for unit in [SECOND, THIRD] {
				s.records.insert((unit, 0xF187), b"PART".to_vec());
				s.records.insert((unit, 0x0900), vec![unit as u8]);
			}
		}
		let mut rows = BTreeMap::new();
		rows.insert(UNIT, vec![field(0x0600, 0, "first")]);
		rows.insert(SECOND, vec![field(0x0900, 0, "second's")]);
		rows.insert(THIRD, vec![field(0x0900, 0, "third's")]);
		rows.insert(0x7E1, Vec::new());
		(car, PerUnit(rows))
	}

	/// Every request one unit was sent.
	fn sent_to(car: &Car, unit: u16) -> Vec<Vec<u8>> {
		sent(car).into_iter().filter(|(u, _)| *u == unit).map(|(_, pdu)| pdu).collect()
	}

	/// How many road-speed reads went to the engine.
	fn speed_checks(car: &Car) -> usize {
		sent_to(car, ENGINE).iter().filter(|pdu| pdu[..] == [0x22, 0xF4, 0x0D]).count()
	}

	#[tokio::test]
	async fn all_reads_each_unit_with_data_one_after_another_each_checked_on_its_own() {
		let (car, data) = three_units();
		let capture = Capture::start(0);
		all(car.clone(), &data, Duration::ZERO).await.unwrap();
		let said = capture.finish();
		// The summary first, then each unit's block under its heading, in the walk's order;
		// 7E1 has no rows.
		let headers: Vec<&str> = said.stdout.lines().filter(|l| !l.starts_with(' ') && !l.is_empty()).collect();
		assert_eq!(
			headers,
			[
				format!("3 of 3 units read{SEP}3 adaptations{SEP}3 of 3 identifiers answered").as_str(),
				"70E  TEST UNIT  PART-0001",
				"713  PART",
				"714  PART"
			],
			"{}",
			said.stdout
		);
		assert!(said.stdout.contains("\n\n713  PART\n"), "a blank line between blocks: {}", said.stdout);
		assert!(said.stdout.contains("second's"), "{}", said.stdout);
		assert!(sent_to(&car, 0x7E1).is_empty(), "a unit with no rows is not asked");
		// Two stationary checks per unit: before it is identified and before its reads.
		assert_eq!(speed_checks(&car), 2 * 3);
		for (unit, pdu) in sent(&car) {
			assert_eq!(pdu.first(), Some(&0x22), "{unit:03X} was sent {pdu:02X?}");
		}
	}

	#[tokio::test]
	async fn all_is_refused_at_the_unit_before_which_the_car_started_moving() {
		let (car, data) = three_units();
		// The car moves once the first unit's adaptation has been read.
		car.0.lock().unwrap().moves_on = Some(((UNIT, 0x0600), 20));
		let capture = Capture::start(0);
		let refused = all(car.clone(), &data, Duration::ZERO).await.err().expect("refused");
		let said = capture.finish();
		assert!(refused.to_string().starts_with("the car is moving (20 km/h)"), "{refused}");
		// What was read before is printed, under a summary that says where the run stopped.
		assert!(
			said.stdout.starts_with(&format!(
				"1 of 3 units read{SEP}1 adaptation{SEP}1 of 1 identifier answered{SEP}stopped at unit 713, 1 not read\n\n"
			)),
			"{}",
			said.stdout
		);
		// The first unit was read and shown; the second and third were asked nothing.
		assert!(said.stdout.contains("first"), "{}", said.stdout);
		assert!(sent_to(&car, SECOND).is_empty() && sent_to(&car, THIRD).is_empty());
	}

	#[tokio::test]
	async fn all_stops_at_the_first_unit_that_falls_silent_and_leaves_the_rest_unread() {
		let (car, mut data) = three_units();
		// The second unit has thirty adaptations and answers only its identification.
		data.0.insert(SECOND, (0..30u16).map(|n| field(0x0A00 + n, 0, "n")).collect());
		car.0.lock().unwrap().dies_after.insert(SECOND, 1 + 4 + 1);
		let capture = Capture::start(0);
		let stopped = all(car.clone(), &data, Duration::ZERO).await.err().expect("stopped");
		let said = capture.finish();
		assert!(stopped.to_string().contains("was stopped"), "{stopped}");
		assert!(said.stdout.contains("stopped at unit 713, 1 not read"), "{}", said.stdout);
		assert!(
			said.stderr.contains("STOPPED: control unit 713 (713)"),
			"the notice names it: {}",
			said.stderr
		);
		assert!(sent_to(&car, THIRD).is_empty(), "the third unit was read: {:02X?}", sent_to(&car, THIRD));
	}

	/// A unit offered that does not answer its identification, once a unit has been read in
	/// the run: the run ends there, with a STOPPED notice, and the units after it are not
	/// read (CLAUDE.md, Safety: stop on the first silent unit).
	#[tokio::test]
	async fn all_stops_at_a_unit_that_does_not_answer_once_one_has_been_read() {
		let (car, data) = three_units();
		car.0.lock().unwrap().records.retain(|(u, _), _| *u != SECOND);
		let capture = Capture::start(0);
		let stopped = all(car.clone(), &data, Duration::ZERO).await.err().expect("stopped");
		let said = capture.finish();
		assert_eq!(
			vag_cli_core::report::unlabelled(&stopped),
			"the run was stopped: unit 713 did not answer after 1 unit had been read\n\nhelp: see whether it answers: `vagcan units --identify`"
		);
		assert!(
			said
				.stderr
				.contains("STOPPED: control unit 713 did not answer its identification, after 1 unit had been read in this run."),
			"{}",
			said.stderr
		);
		assert!(
			said.stdout.starts_with(&format!(
				"1 of 3 units read{SEP}1 adaptation{SEP}1 of 1 identifier answered{SEP}stopped at unit 713, 1 not read\n\n"
			)),
			"{}",
			said.stdout
		);
		assert!(sent_to(&car, THIRD).is_empty(), "the third unit was asked: {:02X?}", sent_to(&car, THIRD));
	}

	/// The same before anything has been read: passed over with a line that says so, and
	/// counted in the summary; the run goes on.
	#[tokio::test]
	async fn all_passes_over_a_unit_that_does_not_answer_before_any_was_read() {
		let (car, data) = three_units();
		car.0.lock().unwrap().records.retain(|(u, _), _| *u != UNIT);
		let capture = Capture::start(0);
		all(car.clone(), &data, Duration::ZERO).await.unwrap();
		let said = capture.finish();
		assert!(
			said
				.stderr
				.contains("skipped unit 70E: it did not answer, before any unit was read\n  (use \"vagcan units\" to list this car's units)"),
			"{}",
			said.stderr
		);
		assert!(
			said.stdout.starts_with(&format!(
				"2 of 3 units read{SEP}2 adaptations{SEP}2 of 2 identifiers answered{SEP}1 skipped\n"
			)),
			"{}",
			said.stdout
		);
		assert!(said.stdout.contains("third's"), "the run went on: {}", said.stdout);
	}

	/// A refusal part way through `--all` says to run `--all` again, not the one unit.
	#[tokio::test]
	async fn an_all_refusal_says_to_run_all_again() {
		let (car, data) = three_units();
		car.0.lock().unwrap().moves_on = Some(((UNIT, 0x0600), 20));
		let capture = Capture::start(0);
		let refused = all(car, &data, Duration::ZERO).await.err().expect("refused");
		capture.finish();
		assert!(
			vag_cli_core::report::unlabelled(&refused).ends_with("help: stop the car and run `vagcan adaptations --all` again"),
			"{refused}"
		);
	}

	/// A unit whose variant is not settled is refused, and nothing beyond its
	/// identification is asked of it.
	#[tokio::test]
	async fn a_unit_whose_variant_is_not_settled_is_refused_and_not_swept() {
		struct Unsure;
		impl Data for Unsure {
			async fn rows(&self, _: Option<&str>, _: &UnitIdentity) -> Result<Vec<Adaptation>, Unsettled> {
				Err(Unsettled {
					version: Some("001001".into()),
					candidates: 17,
				})
			}
			fn reread(&self) -> Option<String> {
				None
			}
			fn recorded(&self, _: Option<&str>, _: &[u16]) -> Vec<UnitIdentity> {
				Vec::new()
			}
			fn count(&self, _: Option<&str>, _: &UnitIdentity) -> usize {
				0
			}
		}
		let car = car(Some(0));
		let refused = single(car.clone(), unit(), &Unsure).await.err().expect("refused");
		assert_eq!(
			vag_cli_core::report::unlabelled(&refused),
			"nothing describes the adaptations of unit 70E\n\
			 \n\
			 note: the variant of unit 70E is not settled (F1A2 001001 is none of the 17 variants of its family)\n\
			 help: a newer ODIS project or VCDS installation may describe it: `vagcan setup <dir>`"
		);
		assert!(adaptation_reads(&car).is_empty(), "{:?}", adaptation_reads(&car));
	}

	#[tokio::test]
	async fn all_with_nothing_to_read_says_what_to_type() {
		let (car, _) = three_units();
		let none = PerUnit(BTreeMap::from([(UNIT, Vec::new()), (SECOND, Vec::new()), (THIRD, Vec::new())]));
		let capture = Capture::start(0);
		let refused = all(car, &none, Duration::ZERO).await.err().expect("nothing");
		let said = capture.finish();
		assert_eq!(
			vag_cli_core::report::unlabelled(&refused),
			vag_cli_core::report::unlabelled(&nothing_to_offer())
		);
		assert_eq!(refused.to_string(), "no recorded unit of this car has adaptations");
		assert_eq!(said.stdout, "", "nothing read, nothing printed");
		// The engine, the gearbox and the gateway are walked and not in this record.
		assert!(
			said
				.stderr
				.contains("3 units of this car are not recorded on this machine\n  (use \"vagcan units --identify\" to record them)"),
			"{}",
			said.stderr
		);
	}

	#[test]
	fn the_picker_offers_recorded_units_with_adaptations_in_the_walk_s_order() {
		let identity = |request: u16, component: &str| UnitIdentity {
			request,
			part_number: Some("P".into()),
			odx_name: Some(format!("EV_{request:03X}")),
			odx_version: None,
			component: Some(component.into()),
		};
		let record = [identity(0x7E0, "ENGINE"), identity(0x70E, "BODY"), identity(0x713, "BRAKES")];
		let count = |u: &UnitIdentity| match u.request {
			0x70E => 12,
			0x713 => 1,
			_ => 0,
		};
		// 0x7E0 has none; 0x746 is listed and not recorded.
		let picked = choices(&[0x7E0, 0x713, 0x70E, 0x746], &record, count);
		assert_eq!(picked.1, 1);
		assert_eq!(
			picked.0.iter().map(|c| (c.unit.request, c.adaptations)).collect::<Vec<_>>(),
			[(0x713, 1), (0x70E, 12)]
		);

		let mut asker = vag_cli_core::ui::menu::Scripted::new(vec![vag_cli_core::ui::menu::Answer::Pick(1)]);
		assert_eq!(choose(&picked, &mut asker).unwrap(), Some(UnitAddress::from_request(0x70E).unwrap()));
		let menu = asker.last_menu();
		assert!(menu.contains("713") && menu.contains(&format!("BRAKES{SEP}1 adaptation")), "{menu}");
		assert!(menu.contains("70E") && menu.contains(&format!("BODY{SEP}12 adaptations")), "{menu}");
		assert!(
			asker
				.all_said()
				.contains("1 unit of this car is not recorded on this machine\n  (use \"vagcan units --identify\" to record it)"),
			"{}",
			asker.all_said()
		);
		// Quitting picks nothing; nothing to offer is a refusal that says what to type.
		let mut quits = vag_cli_core::ui::menu::Scripted::new(vec![vag_cli_core::ui::menu::Answer::Quit]);
		assert_eq!(choose(&picked, &mut quits).unwrap(), None);
		let none = choose(&(Vec::new(), 0), &mut vag_cli_core::ui::menu::Scripted::new(vec![])).unwrap_err();
		assert!(
			vag_cli_core::report::unlabelled(&none).contains("help: record the car's units: `vagcan units --identify`"),
			"{none}"
		);
	}

	#[test]
	fn a_long_name_wraps_under_itself_and_a_long_value_goes_under_the_name() {
		let long = "A name long enough that it cannot sit on one line beside its value and has to wrap under itself";
		let rows = [field(0x0600, 0, long), field(0x0610, 0, "short")];
		let mut wide = rows[1].clone();
		wide.form = vag_cli_core::adaptation::Form::Bytes;
		wide.bit_length = None;
		wide.name = "bytes".into();
		wide.did = 0x0620;
		let text = render(
			"70E  BODY  PART-0001",
			&[rows[0].clone(), rows[1].clone(), wide],
			&[(0x0600, vec![7]), (0x0610, vec![1]), (0x0620, (0u8..30).collect())],
		);
		assert_eq!(
			text,
			"70E  BODY  PART-0001\n\
			 \x20 0600  A name long enough that it cannot sit on one    7 s\n\
			 \x20       line beside its value and has to wrap under\n\
			 \x20       itself\n\
			 \x20 0610  short                                           1 s\n\
			 \x20 0620  bytes\n\
			 \x20       = 00 01 02 03 04 05 06 07 08 09 0A 0B 0C 0D 0E 0F 10 11 12 13 14 15 16\n\
			 \x20         17 18 19 1A 1B 1C 1D\n"
		);
		for line in text.lines() {
			assert!(line.chars().count() <= 80, "{line}");
		}
	}

	#[test]
	fn no_unit_and_no_terminal_says_the_command_with_a_unit() {
		let said = vag_cli_core::report::unlabelled(&no_unit());
		assert!(said.contains("\n\nhelp: name the unit: `vagcan adaptations 09`"), "{said}");
	}
}
