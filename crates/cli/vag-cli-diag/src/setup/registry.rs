//! Step 5: the car's channels, out of the VCDS install's measurement registry.
//!
//! The reading is [`vag_cli_core::registry`]'s — `watch`, `measure` and `units --identify`
//! read the same way for the car in front of them, and `dev dash build` offline — and this is
//! `setup`'s side of it: which units (every car recorded on this machine,
//! `~/.vagcan/cars/<VIN>/units.json`, written by those three commands), a spinner that counts
//! them down, a `-v` line per unit as it is done, and what the read came to as a [`Step`] of
//! the closing table.
//!
//! What the step could not read, it says at `-v`: a table that did not open, a row that
//! did not read, a list that lost its first row. The row is `partial` then: a number on its
//! own reads as everything there was.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;

use super::{State, Step};
use crate::progress::Spinner;
use crate::registry::{Progress, Read};

pub(super) const NAME: &str = "registry";

/// Why the step is pending with no car recorded, at `-v`: what reads the channels
/// — said as what happens, not as a command for the reader to run.
pub(super) const PENDING_WHY: &str =
	"no car recorded on this machine; `watch`, `measure` and `units --identify` read its units' channels when they meet the car";

/// The `(F19E, F1A2)` of every unit of every car recorded on this machine,
/// each once, in order. A unit that did not say what it is has no file to find.
pub(super) fn recorded_units() -> Result<BTreeSet<(String, String)>> {
	let mut out = BTreeSet::new();
	for car in crate::units::recorded_cars()? {
		for unit in car.units {
			if let Some(odx) = unit.odx_name {
				out.insert((odx, unit.odx_version.unwrap_or_default()));
			}
		}
	}
	Ok(out)
}

/// The step's progress: a spinner while a table opens and while the units'
/// lists are found — all at once, counted on the spinner as each unit is done,
/// with the elapsed time once it runs long — and then a line per unit under the
/// header, numbered, in order.
struct Said {
	spinner: Option<Spinner>,
	/// Which unit's line is next, and of how many.
	at: usize,
	of: usize,
	/// At `-vv`, what the unit's list was looked up by, said under its `read [n/N]` line.
	looked_up: Option<String>,
}

impl Progress for Said {
	fn waiting(&mut self, what: &str) {
		vag_cli_core::wire!("    {what}");
		// Cleared before the next one draws, or two lines spin at once.
		self.spinner = None;
		self.spinner = Some(Spinner::new(what));
	}

	fn unit(&mut self, at: usize, of: usize, _name: &str) {
		(self.at, self.of) = (at, of);
	}

	fn looked_up(&mut self, _odx: &str, version: &str, project: Option<&str>, _outcome: &str) {
		// The name and the outcome are the `read [n/N]` line's; only what it lacks is kept.
		let by = [format!("F1A2 {version}"), project.map(|p| format!("car project {p}")).unwrap_or_default()];
		self.looked_up = Some(vag_cli_core::marks::facts(by));
	}

	fn reporter(&self) -> Option<crate::progress::Reporter> {
		self.spinner.as_ref().map(Spinner::reporter)
	}

	fn done(&mut self, line: &str) {
		self.spinner = None;
		vag_cli_core::verbosity::say(vag_cli_core::verbosity::NOTES, &format!("    read [{}/{}] {line}", self.at + 1, self.of));
		if let Some(by) = self.looked_up.take() {
			vag_cli_core::verbosity::say(vag_cli_core::verbosity::WIRE, &format!("      looked up by {by}"));
		}
	}
}

/// Read the registry for `units` out of the install at `root` and write their
/// channels into the VCDS store's cache.
///
/// The read runs on the blocking pool, not on the main task: it is minutes of
/// CPU when a key has to be searched for, and made on the task inside `main`'s
/// `select!` it would sit between Ctrl-C and the branch that acts on it — the
/// signal arrives, and nothing can run until the read is over (found in review,
/// 2026-09-28, on `dev dash build`'s copy of the same shape). Every key found
/// is saved after its table, or after the units' lists, so a run ended this way
/// leaves the next one less to search for.
pub(super) async fn registry_channels(root: &Path, stores: &crate::project::Stores, units: &BTreeSet<(String, String)>) -> Result<Step> {
	let project = &stores.vcds;
	if units.is_empty() {
		// The label step has just made this installation's source, so it is there; a
		// `None` would be a source dropped from another terminal meanwhile, and then
		// there is nothing to say about rows.
		let removed = crate::registry::clear(root, project)?.unwrap_or_default();
		// Where the installation is, written down now: the first command with a
		// car reads the channels of the car's units from it.
		crate::registry::record_installation(project, root)?;
		return Ok(Step::new(vag_data_db::VCDS, NAME, State::Pending, super::NEXT_WITH_THE_CAR).because(format!("{PENDING_WHY}{removed}")));
	}
	vag_cli_core::verbosity::say(
		vag_cli_core::verbosity::NOTES,
		&format!(
			"    reading the registry for {} {} of the cars recorded here",
			units.len(),
			crate::render::plural(units.len(), "unit")
		),
	);
	let (root, stores, units) = (root.to_path_buf(), stores.clone(), units.clone());
	let outcome = tokio::task::spawn_blocking(move || {
		let mut said = Said {
			spinner: None,
			at: 0,
			of: 0,
			looked_up: None,
		};
		crate::registry::read(&root, &stores, &units, &mut said)
	})
	.await
	.map_err(|e| anyhow::anyhow!("the registry read did not finish: {e}"))??;
	Ok(match outcome {
		Read::NoRegistry { why, removed } => {
			Step::new(vag_data_db::VCDS, NAME, State::Missing, "RM.rod did not open").because(format!("the registry did not open: {why}{removed}"))
		}
		Read::Done(summary) => {
			// One short line at level 0; the rest of what it came to at `-v`, under it.
			let more = summary.more();
			let mut why: Vec<String> = Vec::new();
			if !more.is_empty() {
				why.push(more.join(vag_cli_core::marks::SEP));
			}
			why.extend(summary.notes.iter().cloned());
			let state = match summary.notes.is_empty() {
				true => State::Wrote,
				false => State::Partial,
			};
			let step = Step::new(vag_data_db::VCDS, NAME, state, summary.headline()).at(summary.cache);
			match why.is_empty() {
				true => step,
				false => step.because(why.join("\n")),
			}
		}
	})
}

/// The step when the cars' records could not be read: nothing was read. The row
/// says what did not read — the file, or the error that names it — and its
/// `help:` line, at level 0, what to do and the command that reads the
/// installation at `root` again; a record's parse error is at `-v`.
pub(super) fn failed(e: &anyhow::Error, root: &Path) -> Step {
	let again = crate::missing::shell_word(&root.display().to_string());
	match vag_cli_core::report::find::<crate::units::NotARecord>(e) {
		Some(bad) => Step::new(
			vag_data_db::VCDS,
			NAME,
			State::Failed,
			format!("not a record this tool wrote: {}", bad.path.display()),
		)
		.because(bad.cause.clone())
		.fix(format!("help: move {} aside, then run `vagcan setup {again}`", bad.path.display())),
		None => {
			let cars = crate::datadir::cars_dir().map_or_else(|_| "~/.vagcan/cars".to_string(), |dir| dir.display().to_string());
			Step::new(vag_data_db::VCDS, NAME, State::Failed, format!("the cars' records did not read: {e:#}")).fix(format!(
				"help: make {cars} and the records in it readable, then run `vagcan setup {again}`"
			))
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn with_no_car_recorded_the_step_is_pending_names_the_live_commands_and_records_the_installation() {
		// Spec 4: the units come from the record `watch`, `measure` and
		// `units --identify` write, so the reader is told when the channels come —
		// the first time one of them meets the car — and is asked to run nothing.
		// And the installation is written down even now, so that first car command
		// knows where to read the channels from.
		let dir = tempfile::tempdir().unwrap();
		let root = dir.path().join("vcds");
		std::fs::create_dir_all(root.join("UDS_EV")).unwrap();
		let project = crate::project::Project {
			id: "TEST".into(),
			dir: dir.path().join("TEST"),
		};
		std::fs::create_dir_all(&project.dir).unwrap();
		let stores = crate::project::Stores {
			project: None,
			vcds: project.clone(),
		};
		let step = registry_channels(&root, &stores, &BTreeSet::new()).await.unwrap();
		assert_eq!(step.state, State::Pending, "{step:?}");
		assert_eq!((step.source, step.name), ("vcds", "registry"));
		// The row says when the channels come; `-v` says which commands read them.
		// Neither tells the reader to run anything: the tool learns the car as a
		// side effect of the car commands.
		assert_eq!(step.detail, "read when a car command first meets the car");
		let why = step.why.as_deref().expect("a reason at -v");
		assert!(why.starts_with("no car recorded on this machine"), "{why}");
		assert!(
			why.contains("`watch`, `measure` and `units --identify` read its units' channels when they meet the car"),
			"{why}"
		);
		for said in [step.detail.as_str(), why] {
			assert!(!said.contains("run "), "an instruction to run a command: {said}");
		}
		assert!(step.fix.is_none(), "nothing to type: {step:?}");
		assert!(!why.contains("survey"), "no other instruction: {why}");
		assert!(
			!why.contains("connected"),
			"recorded, not connected: a car connected with `info` alone is not on record: {why}"
		);
		let log = crate::registry::Log::load(&project).unwrap().expect("the installation is recorded");
		assert_eq!(log.installation, root.canonicalize().unwrap());
		assert!(log.units.is_empty());
	}
}
