//! Which one source a run reads: an ODIS project, or the machine's VCDS installation.
//!
//! The two are independent sources, not companions (owner, 2026-10-21: «vcds и odis – не
//! компаньоны. они независимы друг от друга и должны уметь работать самостоятельно»). Each
//! describes a car on its own, so a run names everything — channels, scalings, fault text,
//! freeze frames, adaptations — from **one** of them, and nothing of the other fills a gap
//! in it. What the run's source does not describe is shown raw, or as its number.
//!
//! **[`run_source`] is the one place that decides.** In order:
//!
//! 1. `--source odis|vcds` ([`select`]), for one command;
//! 2. `VAGCAN_SOURCE` ([`SOURCE_ENV`]), for a shell — and for the firmware's build script,
//!    which has no flag;
//! 3. `[source] use = "odis" | "vcds"` in `config.toml`;
//! 4. otherwise ODIS where the run's project holds an ODIS source, else VCDS where the
//!    machine's store holds an installation, else none.
//!
//! A source asked for in any of the first three ways and not set up is an error that names
//! what asked and what to type; it is never quietly replaced by the other.
//!
//! **One accessor holds the run's data: [`Run`].** It carries one store — the ODIS project,
//! or the VCDS store — and nothing else, so a reader handed it cannot reach the other
//! source's rows. [`crate::extracted::Extracted`] is built over one, and so is everything
//! that lists a run's inputs.

use std::path::PathBuf;

use anyhow::Result;

use crate::project::{Project, Stores};
use crate::report::Advice as _;

/// One of the two kinds of source a run can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, clap::ValueEnum)]
pub enum Source {
	/// An extracted ODIS-Service project.
	Odis,
	/// A VCDS installation.
	Vcds,
}

impl Source {
	/// Both, in the order the default asks for them.
	pub const ALL: [Source; 2] = [Source::Odis, Source::Vcds];

	/// The cache's word for this kind of source (`vag_data_db::ODIS`, `VCDS`).
	pub fn kind(self) -> &'static str {
		match self {
			Source::Odis => vag_data_db::ODIS,
			Source::Vcds => vag_data_db::VCDS,
		}
	}

	/// The source a cache kind names, `None` for any other word.
	pub fn of_kind(kind: &str) -> Option<Source> {
		Source::ALL.into_iter().find(|s| s.kind() == kind)
	}

	/// What a person types for it, `odis` or `vcds` — also its spelling in `config.toml`.
	pub fn word(self) -> &'static str {
		self.kind()
	}

	/// A value as typed (`odis`, `VCDS`), or `None` for anything else.
	pub fn parse(text: &str) -> Option<Source> {
		Source::of_kind(&text.trim().to_ascii_lowercase())
	}

	/// Its name in a sentence: `ODIS`, `VCDS`.
	pub fn name(self) -> &'static str {
		match self {
			Source::Odis => "ODIS",
			Source::Vcds => "VCDS",
		}
	}

	/// The possessive a line names a variant's rows with: `ODIS's EV_SimEngine_001`.
	pub fn possessive(self) -> &'static str {
		match self {
			Source::Odis => "ODIS's",
			Source::Vcds => "VCDS's",
		}
	}

	/// What one of this kind is called: `an ODIS project`, `a VCDS installation`.
	pub fn what(self) -> &'static str {
		match self {
			Source::Odis => "an ODIS project",
			Source::Vcds => "a VCDS installation",
		}
	}

	/// What `setup` is given for one: `<ODIS project>`, `<VCDS installation>`.
	pub fn placeholder(self) -> &'static str {
		match self {
			Source::Odis => "<ODIS project>",
			Source::Vcds => "<VCDS installation>",
		}
	}

	/// The other one.
	pub fn other(self) -> Source {
		match self {
			Source::Odis => Source::Vcds,
			Source::Vcds => Source::Odis,
		}
	}

	/// The `help:` line that sets one up.
	pub fn setup_help(self) -> &'static str {
		match self {
			Source::Odis => "read one: `vagcan setup <ODIS project>`",
			Source::Vcds => "read one: `vagcan setup <VCDS installation>` (with no path, `vagcan setup` can download one)",
		}
	}

	/// The flag that picks it for one command: `--source vcds`.
	pub fn flag(self) -> String {
		format!("--source {}", self.word())
	}
}

impl std::fmt::Display for Source {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(self.name())
	}
}

/// The environment variable that picks the source for a shell, and for the firmware's build.
pub const SOURCE_ENV: &str = "VAGCAN_SOURCE";

/// `--source`'s answer, for the length of the process — a global for the reason
/// `crate::project::select` is one: a dozen leaves consult it and none takes it as an argument.
static SELECTED: std::sync::OnceLock<Source> = std::sync::OnceLock::new();

/// Remember what `--source` said, before any command runs.
pub fn select(source: Source) {
	let _ = SELECTED.set(source);
}

/// A source asked for, and what asked: `--source vcds`, `VAGCAN_SOURCE=vcds`, the setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
	pub source: Source,
	/// What asked, as the person wrote it.
	pub by: String,
	/// Where it was asked — which is where another source is asked for instead.
	pub origin: Origin,
}

/// Where a source was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
	Flag,
	Env,
	Config,
}

impl Asked {
	/// The `help:` line that asks for `other` where this was asked: the flag for a flag, the
	/// variable for the variable, the setting for the setting.
	pub fn switch_to(&self, other: Source) -> String {
		match self.origin {
			Origin::Flag => format!("or read {} instead: `{}`", other.what(), other.flag()),
			Origin::Env => format!("or read {} instead: `{SOURCE_ENV}={}`", other.what(), other.word()),
			Origin::Config => format!(
				"or read {} instead: set `[source] use = \"{}\"` in ~/.vagcan/config.toml",
				other.what(),
				other.word()
			),
		}
	}
}

/// What this run asks for, if anything does: the flag, the environment, `config.toml`.
///
/// A value that names no source is an error, wherever it was written: read as unset it
/// would pick the default — the other source, perhaps — with nothing said.
pub fn asked() -> Result<Option<Asked>> {
	let env = std::env::var(SOURCE_ENV).ok();
	let configured = crate::config::source(&crate::config::load());
	asked_in(SELECTED.get().copied(), env.as_deref(), configured.as_deref())
}

/// The rule behind [`asked`], with every input passed in. An empty value is unset.
pub fn asked_in(flag: Option<Source>, env: Option<&str>, configured: Option<&str>) -> Result<Option<Asked>> {
	if let Some(source) = flag {
		return Ok(Some(Asked {
			source,
			by: source.flag(),
			origin: Origin::Flag,
		}));
	}
	for (value, by, origin) in [
		(env, format!("{SOURCE_ENV}={}", env.unwrap_or_default().trim()), Origin::Env),
		(
			configured,
			format!("[source] use = {:?} in config.toml", configured.unwrap_or_default().trim()),
			Origin::Config,
		),
	] {
		let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
			continue;
		};
		let Some(source) = Source::parse(value) else {
			return Err(anyhow::anyhow!("{by}: not a source").help("name one: `odis` or `vcds`"));
		};
		return Ok(Some(Asked { source, by, origin }));
	}
	Ok(None)
}

/// Which sources are set up for this run: the project holds an ODIS source, the VCDS
/// store an installation. A store whose cache will not answer is none of them.
pub fn held(stores: &Stores) -> Vec<Source> {
	survey(stores).held
}

/// [`held`] over this run's stores — which sources a line may offer the other of. Source rows
/// only: nothing of either source's data is read.
pub fn held_here() -> Vec<Source> {
	crate::project::stores_or_vcds().map(|stores| held(&stores)).unwrap_or_default()
}

/// What the stores hold, and which of them would not say.
#[derive(Debug, Default)]
pub struct Survey {
	/// The sources set up.
	pub held: Vec<Source>,
	/// A store whose cache did not answer — an older vagcan's, or one a running `setup`
	/// holds — with what it is called and why: `(source, "the ODIS project SK37X", why)`.
	pub unreadable: Vec<(Source, String, String)>,
}

/// [`held`], with the stores that did not answer kept apart rather than counted as none.
pub fn survey(stores: &Stores) -> Survey {
	let mut out = Survey::default();
	let stores_of = [(Source::Odis, stores.project.as_ref()), (Source::Vcds, Some(&stores.vcds))];
	for (source, store) in stores_of {
		let Some(store) = store else { continue };
		match crate::project::has_source(store, source.kind()) {
			Ok(true) => out.held.push(source),
			Ok(false) => {}
			Err(e) => {
				let run = Run {
					source,
					store: store.clone(),
				};
				out.unreadable.push((source, run.describe(), format!("{e:#}")));
			}
		}
	}
	out
}

/// **The one decision**: the source this run reads, given its stores. `None` with neither
/// set up and none asked for — the ordinary state of a machine `setup` has not run on.
pub fn run_source(stores: &Stores) -> Result<Option<Source>> {
	decide_over(asked()?, &survey(stores))
}

/// [`decide`], refusing a store that did not answer where it would have been the one read:
/// the source asked for, or the default's first choice. Read as holding nothing, it would
/// hand the run to the other source — a mix of intent, said in one note at most.
pub fn decide_over(asked: Option<Asked>, survey: &Survey) -> Result<Option<Source>> {
	let order: Vec<Source> = match &asked {
		Some(asked) => vec![asked.source],
		None => Source::ALL.to_vec(),
	};
	for source in order {
		if survey.held.contains(&source) {
			break;
		}
		if let Some((_, what, why)) = survey.unreadable.iter().find(|(s, ..)| *s == source) {
			let mut err = anyhow::anyhow!("{what} did not read: {why}").help(format!("read it again: `vagcan setup --refresh {}`", source.placeholder()));
			// The other source only where it is set up, asked for the way this one was.
			let other = source.other();
			if survey.held.contains(&other) {
				err = err.help(match &asked {
					Some(asked) => asked.switch_to(other),
					None => format!("or read {} for this run: `{}`", other.what(), other.flag()),
				});
			}
			return Err(err);
		}
	}
	decide(asked, &survey.held)
}

/// The rule behind [`run_source`], with what was asked and what is set up passed in.
pub fn decide(asked: Option<Asked>, held: &[Source]) -> Result<Option<Source>> {
	let Some(asked) = asked else {
		return Ok(Source::ALL.into_iter().find(|s| held.contains(s)));
	};
	if held.contains(&asked.source) {
		return Ok(Some(asked.source));
	}
	let none = match asked.source {
		Source::Odis => "no ODIS project is set up",
		Source::Vcds => "no VCDS installation is set up",
	};
	let mut err = anyhow::anyhow!("{}: {none}", asked.by).help(asked.source.setup_help());
	let other = asked.source.other();
	if held.contains(&other) {
		err = err.help(asked.switch_to(other));
	}
	Err(err)
}

/// `--source <word> ` for a command a line tells the reader to type, when `source` is not the
/// one a bare command would read over these stores — so that typing it reads the same source
/// again. Empty when it is.
pub fn flag_for(stores: &Stores, source: Source) -> String {
	match decide(None, &held(stores)) {
		Ok(Some(default)) if default == source => String::new(),
		_ => format!("{} ", source.flag()),
	}
}

/// The one store a run reads, and which source it holds — the single accessor every reader
/// of label data is handed, so none of them can reach the other source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
	pub source: Source,
	/// The ODIS project, or the machine's VCDS store.
	pub store: Project,
}

impl Run {
	/// The run's store for `source` out of `stores`, or `None` where it has none: ODIS with
	/// no project.
	pub fn of(stores: &Stores, source: Source) -> Option<Run> {
		let store = match source {
			Source::Odis => stores.project.clone()?,
			Source::Vcds => stores.vcds.clone(),
		};
		Some(Run { source, store })
	}

	/// The cache the run's rows are in.
	pub fn cache(&self) -> PathBuf {
		self.store.cache()
	}

	/// Every file of the store the run's rows are read from: the cache, and for VCDS the
	/// names and the registry log — what a build that must be redone when they change lists.
	pub fn files(&self) -> Vec<PathBuf> {
		let mut files = vec![self.cache()];
		if self.source == Source::Vcds {
			files.extend([self.store.names(), self.store.registry_log()]);
		}
		files
	}

	/// The source in a sentence: `the ODIS project SK37X`, `the VCDS installation`.
	pub fn describe(&self) -> String {
		match self.source {
			Source::Odis => format!("the ODIS project {}", self.store.id),
			Source::Vcds => "the VCDS installation".to_string(),
		}
	}
}

/// This run's source over this run's stores, or `None` with nothing set up. An error for a
/// source asked for and not set up.
pub fn run(stores: &Stores) -> Result<Option<Run>> {
	Ok(run_source(stores)?.and_then(|source| Run::of(stores, source)))
}

/// [`run`] over this run's stores, the project or the VCDS store alone: `None` with nothing
/// set up. An error for a source asked for and not set up.
pub fn run_here() -> Result<Option<Run>> {
	match crate::project::stores_or_vcds() {
		Some(stores) => run(&stores),
		None => Ok(None),
	}
}

/// Refuse what would make a run read the wrong source, before a command opens anything: a
/// source asked for and not set up, a store that does not answer, and a project named that is
/// not there — read without it, the run would quietly be a VCDS run. Nothing set up at all is
/// the command's own to say, in `missing`'s words, so that it can offer `setup`.
pub fn check() -> Result<()> {
	match crate::project::stores() {
		Ok(stores) => run_source(&stores).map(|_| ()),
		Err(e) if crate::report::find::<crate::missing::NoLabelData>(&e).is_some() => Ok(()),
		Err(e) => Err(e),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn asked(source: Source) -> Option<Asked> {
		Some(Asked {
			source,
			by: source.flag(),
			origin: Origin::Flag,
		})
	}

	#[test]
	fn the_default_is_odis_where_there_is_one_and_vcds_otherwise() {
		assert_eq!(decide(None, &[Source::Odis, Source::Vcds]).unwrap(), Some(Source::Odis));
		assert_eq!(decide(None, &[Source::Odis]).unwrap(), Some(Source::Odis));
		assert_eq!(decide(None, &[Source::Vcds]).unwrap(), Some(Source::Vcds));
		assert_eq!(decide(None, &[]).unwrap(), None);
	}

	#[test]
	fn a_source_asked_for_is_the_one_read_and_never_replaced_by_the_other() {
		for held in [&[Source::Odis, Source::Vcds][..], &[Source::Vcds]] {
			assert_eq!(decide(asked(Source::Vcds), held).unwrap(), Some(Source::Vcds));
		}
		assert_eq!(decide(asked(Source::Odis), &[Source::Odis, Source::Vcds]).unwrap(), Some(Source::Odis));
		// Asked for and not set up: an error naming what asked, and what to type.
		let err = decide(asked(Source::Odis), &[Source::Vcds]).unwrap_err();
		assert_eq!(err.to_string(), "--source odis: no ODIS project is set up");
		let hints = crate::report::find::<crate::report::Hinted>(&err).unwrap().hints().to_vec();
		assert_eq!(
			hints,
			[
				crate::report::Hint::Help("read one: `vagcan setup <ODIS project>`".into()),
				crate::report::Hint::Help("or read a VCDS installation instead: `--source vcds`".into()),
			]
		);
		let err = decide(asked(Source::Vcds), &[]).unwrap_err();
		assert_eq!(err.to_string(), "--source vcds: no VCDS installation is set up");
		let hints = crate::report::find::<crate::report::Hinted>(&err).unwrap().hints().to_vec();
		assert_eq!(hints.len(), 1, "no other source to offer: {hints:?}");
	}

	#[test]
	fn the_flag_outranks_the_environment_which_outranks_the_setting() {
		let source = |a: Result<Option<Asked>>| a.unwrap().map(|a| (a.source, a.by));
		assert_eq!(source(asked_in(None, None, None)), None);
		assert_eq!(
			source(asked_in(None, None, Some("vcds"))),
			Some((Source::Vcds, "[source] use = \"vcds\" in config.toml".into()))
		);
		assert_eq!(
			source(asked_in(None, Some("ODIS"), Some("vcds"))),
			Some((Source::Odis, "VAGCAN_SOURCE=ODIS".into()))
		);
		assert_eq!(
			source(asked_in(Some(Source::Vcds), Some("odis"), Some("odis"))),
			Some((Source::Vcds, "--source vcds".into()))
		);
		// Empty is unset, all the way down.
		assert_eq!(source(asked_in(None, Some(" "), Some(""))), None);
	}

	#[test]
	fn a_store_that_does_not_answer_is_refused_where_it_would_be_read_never_handed_to_the_other() {
		let unreadable = |source: Source| Survey {
			held: vec![source.other()],
			unreadable: vec![(source, format!("the {source} store"), "file is not a database".into())],
		};
		// The default's first choice did not answer: an error, not a VCDS run.
		let err = decide_over(None, &unreadable(Source::Odis)).unwrap_err();
		assert_eq!(err.to_string(), "the ODIS store did not read: file is not a database");
		let hints = crate::report::hints(&err);
		assert_eq!(
			hints,
			[
				crate::report::Hint::Help("read it again: `vagcan setup --refresh <ODIS project>`".into()),
				crate::report::Hint::Help("or read a VCDS installation for this run: `--source vcds`".into()),
			]
		);
		// Asked for by name: the same refusal, not "not set up".
		assert!(decide_over(asked(Source::Odis), &unreadable(Source::Odis)).is_err());
		// The other one asked for, or the default reaching a held source first: no matter.
		assert_eq!(decide_over(asked(Source::Vcds), &unreadable(Source::Odis)).unwrap(), Some(Source::Vcds));
		assert_eq!(decide_over(None, &unreadable(Source::Vcds)).unwrap(), Some(Source::Odis));
	}

	#[test]
	fn the_other_source_is_offered_only_where_it_is_set_up_and_where_this_one_was_asked() {
		// A store that does not answer, and nothing else set up: no `--source vcds` to offer.
		let alone = Survey {
			held: Vec::new(),
			unreadable: vec![(Source::Odis, "the ODIS project SK37X".into(), "locked".into())],
		};
		let hints = crate::report::hints(&decide_over(None, &alone).unwrap_err());
		assert_eq!(hints.len(), 1, "{hints:?}");
		// Asked in config.toml or the environment: the switch is that setting, not a flag.
		let config = asked_in(None, None, Some("odis")).unwrap();
		let hints = crate::report::hints(&decide(config, &[Source::Vcds]).unwrap_err());
		assert_eq!(
			hints.last(),
			Some(&crate::report::Hint::Help(
				"or read a VCDS installation instead: set `[source] use = \"vcds\"` in ~/.vagcan/config.toml".into()
			))
		);
		let env = asked_in(None, Some("vcds"), None).unwrap();
		let hints = crate::report::hints(&decide(env, &[Source::Odis]).unwrap_err());
		assert_eq!(
			hints.last(),
			Some(&crate::report::Hint::Help("or read an ODIS project instead: `VAGCAN_SOURCE=odis`".into()))
		);
	}

	#[test]
	fn a_command_a_line_names_carries_the_runs_source_where_it_is_not_the_default() {
		let here = tempfile::tempdir().unwrap();
		let stores = both(here.path());
		assert_eq!(flag_for(&stores, Source::Odis), "", "the default needs no flag");
		assert_eq!(flag_for(&stores, Source::Vcds), "--source vcds ");
		let vcds_only = Stores {
			project: None,
			vcds: stores.vcds.clone(),
		};
		assert_eq!(flag_for(&vcds_only, Source::Vcds), "", "VCDS alone is the default");
	}

	#[test]
	fn a_setting_that_is_not_a_string_names_no_source() {
		let doc: toml_edit::DocumentMut = "[source]\nuse = true\n".parse().unwrap();
		let value = crate::config::source(&doc);
		assert_eq!(value.as_deref(), Some("true"));
		assert!(asked_in(None, None, value.as_deref()).is_err());
		let unset: toml_edit::DocumentMut = "[faults]\nlanguage = \"de\"\n".parse().unwrap();
		assert_eq!(crate::config::source(&unset), None);
	}

	#[test]
	fn a_value_that_names_no_source_is_refused_where_it_was_written() {
		let err = asked_in(None, None, Some("vcsd")).unwrap_err();
		assert_eq!(err.to_string(), "[source] use = \"vcsd\" in config.toml: not a source");
		let err = asked_in(None, Some("both"), Some("vcds")).unwrap_err();
		assert_eq!(err.to_string(), "VAGCAN_SOURCE=both: not a source");
	}

	/// Both sources set up, each marked with its own word in every name, id and wording it
	/// holds: an ODIS project under `data/SK37X/`, the VCDS store under `vcds/`. Invented.
	fn both(here: &std::path::Path) -> Stores {
		use vag_data_labels::catalog::Scaling;
		use vag_data_labels::measure::LinearScale;
		let reading = |did: u16, name: &str, id: &str| vag_data_labels::odis::Reading {
			did,
			name: name.to_string(),
			unit: None,
			bit_offset: 0,
			bit_length: 8,
			signed: false,
			big_endian: true,
			scaling: Scaling::Linear(LinearScale { factor: 1.0, offset: 0.0 }),
			text_id: Some(id.to_string()),
		};
		let project = Project {
			id: "SK37X".into(),
			dir: here.join("data").join("SK37X"),
		};
		let vcds = crate::project::vcds_at(&here.join("vcds"));
		for dir in [&project.dir, &vcds.dir] {
			std::fs::create_dir_all(dir).unwrap();
		}
		vag_data_db::put_readings(
			&project.cache(),
			"/nowhere/SK37X",
			"EV_Unit_001",
			&[reading(0x1000, "odis channel", "IDE00001")],
		)
		.unwrap();
		vag_data_db::put_all_adaptations(
			&project.cache(),
			"/nowhere/SK37X",
			[("EV_Unit_001", &[reading(0x0600, "odis adaptation", "IDE00002")][..])],
		)
		.unwrap();
		vag_data_db::record_language(&vcds.cache(), vag_data_db::VCDS, "/nowhere/vcds/Labels", "eng").unwrap();
		vag_data_db::put_all_vcds_readings(
			&vcds.cache(),
			"/nowhere/vcds/Labels",
			[(
				"EV_Unit",
				&[
					reading(0x2000, "vcds channel", "900001"),
					reading(0x1000, "vcds at the shared field", "900002"),
				][..],
			)],
		)
		.unwrap();
		// VCDS's wording, for its own ids and for one of ODIS's: an ODIS run reads none of it.
		std::fs::write(vcds.names(), r#"{"900001": "vcds wording", "IDE00001": "vcds wording for an odis id"}"#).unwrap();
		Stores {
			project: Some(project),
			vcds,
		}
	}

	/// **The guard.** Every reader a run hands label data to is handed one [`Run`], and a run
	/// over the two sources shows nothing of the other: no channel, field, name, wording, text
	/// id or input file. A reader that reached both stores in one run fails here.
	#[test]
	fn no_reader_of_a_run_sees_the_other_source() {
		let here = tempfile::tempdir().unwrap();
		let stores = both(here.path());
		assert_eq!(held(&stores), [Source::Odis, Source::Vcds]);
		assert_eq!(decide(None, &held(&stores)).unwrap(), Some(Source::Odis), "the default with both");
		for source in Source::ALL {
			let (mine, theirs) = (source.word(), source.other().word());
			let run = Run::of(&stores, source).unwrap();
			// The input files a build or a catalogue lists are the run's store's alone.
			assert!(run.files().iter().all(|f| f.starts_with(&run.store.dir)), "{source}: {:?}", run.files());

			let x = crate::extracted::open_run(&run);
			assert_eq!(x.run_source(), Some(source));
			let channels: Vec<String> = x
				.for_unit(Some("EV_Unit"), Some("001001"))
				.into_iter()
				.map(|d| d.name.to_string())
				.collect();
			assert!(!channels.is_empty(), "{source}: its own channels");
			assert!(channels.iter().all(|n| n.starts_with(mine)), "{source}: {channels:?}");
			let adaptations: Vec<String> = x
				.adaptations(Some("EV_Unit"), Some("001001"))
				.unwrap()
				.into_iter()
				.map(|a| a.name)
				.collect();
			assert!(adaptations.iter().all(|n| !n.contains(theirs)), "{source}: {adaptations:?}");
			// Wording: VCDS's text table on a VCDS run only, never over an ODIS row's id.
			assert_eq!(x.names().is_empty(), source == Source::Odis, "{source}");
			if source == Source::Odis {
				assert_eq!(x.name_of(Some("IDE00001")), None, "VCDS's wording named an ODIS row");
			}
			// The text ids `--did` is checked against, and the glossary's seed.
			let known = crate::plan::known_text_ids_in(Some(&run)).unwrap();
			let expected: std::collections::BTreeSet<String> = match source {
				Source::Odis => ["IDE00001"].map(String::from).into(),
				Source::Vcds => ["900001", "900002"].map(String::from).into(),
			};
			assert_eq!(known, expected, "{source}");
			let csv = here.path().join(format!("names-{mine}.csv"));
			crate::glossary::seed_into(Some(&run), csv.clone()).unwrap();
			let seeded = std::fs::read_to_string(&csv).unwrap();
			assert!(!seeded.contains(theirs), "{source}'s seed holds the other source's words: {seeded}");
			for id in &expected {
				assert!(seeded.contains(id.as_str()), "{source}: {id} missing from {seeded}");
			}
		}
	}

	#[test]
	fn each_source_is_set_up_alone_or_beside_the_other() {
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
		assert_eq!(held(&odis_only), [Source::Odis]);
		assert_eq!(held(&vcds_only), [Source::Vcds]);
		// Every selector value over every setup: the one asked for where it is there, an error
		// where it is not, the default otherwise.
		for (stores, odis, vcds, default) in [
			(&odis_only, true, false, Some(Source::Odis)),
			(&vcds_only, false, true, Some(Source::Vcds)),
			(&stores, true, true, Some(Source::Odis)),
		] {
			let held = held(stores);
			assert_eq!(decide(None, &held).unwrap(), default);
			assert_eq!(decide(asked(Source::Odis), &held).ok().flatten(), odis.then_some(Source::Odis));
			assert_eq!(decide(asked(Source::Vcds), &held).ok().flatten(), vcds.then_some(Source::Vcds));
		}
	}

	#[test]
	fn a_run_holds_one_store_and_its_files_only() {
		let project = Project {
			id: "SK37X".into(),
			dir: PathBuf::from("/x/data/SK37X"),
		};
		let vcds = crate::project::vcds_at(std::path::Path::new("/x/vcds"));
		let stores = Stores {
			project: Some(project.clone()),
			vcds: vcds.clone(),
		};
		let odis = Run::of(&stores, Source::Odis).unwrap();
		assert_eq!(odis.files(), [project.cache()]);
		assert_eq!(odis.describe(), "the ODIS project SK37X");
		let run = Run::of(&stores, Source::Vcds).unwrap();
		assert_eq!(run.files(), [vcds.cache(), vcds.names(), vcds.registry_log()]);
		assert!(Run::of(&Stores { project: None, vcds }, Source::Odis).is_none());
	}
}
