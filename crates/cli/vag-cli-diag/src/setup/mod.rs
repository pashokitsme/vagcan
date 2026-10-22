//! `vagcan setup` — the one command that makes this tool usable.
//!
//! Everything this tool knows about a car's control units comes from somebody
//! else's data, and none of it may be redistributed. So it is not in this
//! repository and never will be, and the price of that is a step somebody has to
//! run once. This is that step.
//!
//! **Two sources now, so it is a choice rather than an argument.**
//! [`source::choose`] asks which — a VCDS installation, an extracted
//! ODIS-Service project, or a download — and `setup <PATH>` still skips the menu
//! entirely, because the folder itself says which of the two it is.
//!
//! An ODIS project lands in a project under `~/.vagcan/data/<id>/`, named as it names
//! itself; a VCDS installation lands in the machine's one VCDS store, `~/.vagcan/vcds/`,
//! with no name asked: it serves every car ([`crate::project`]). A second VCDS
//! installation replaces the first. What each branch writes:
//!
//! | source | what it gives | where it lands |
//! |---|---|---|
//! | VCDS | the `.rod` files and the fault text, raw | `~/.vagcan/rod/`, shared |
//! | VCDS | the label files, parsed | `vcds/cache.sqlite` |
//! | VCDS | every name in `TTTEXT.ROD`, by text id | `vcds/names.json` |
//! | VCDS | the `IDE`/`MAS` id each of those texts names | `vcds/odx-ids.json` |
//! | VCDS | `.rod` section keys | `vcds/rod-keys.json` |
//! | ODIS | every variant's channels, by identifier, **with scalings** | `data/<id>/cache.sqlite` |
//! | ODIS | every `(text id, name)` pair in the project | `data/<id>/names-odis.json` |
//!
//! **What it says is in the shape of the version-control tool's status, on stderr**: a
//! heading naming what is read, an indented line per step with its verb in the past
//! (`copied`, `parsed`, `failed to read`), one `Ready:` line, and a hint in parentheses
//! under it. `-v` adds each step's time and path, and its reason when it fell short,
//! indented under the step; `-vv` the stages and the files.
//!
//! The copy is what lets every car command run without the installation: fault
//! naming reads `.rod` files straight off disk at run time, so those are taken
//! from the install. The `.lbl`/`.clb` files are **not** copied (D4) — they are
//! read once, here, into `cache.sqlite`, and that cache is what survives of
//! them. The consequence is D5, honoured by every reader of the store's cache: a
//! cache whose label files are gone is trusted rather than declared stale, or
//! every run after somebody moved their installation would try to rebuild from
//! nothing. The installation is still worth keeping: step 5 and the first
//! command with a car read a unit's channels from it, and a unit met later —
//! swapped, updated, asleep the first time — is read from it then.
//!
//! **Offline.** No adapter is opened and no car is addressed.
//!
//! ## `vagcan setup` with no terminal now exits 1, and that is deliberate
//!
//! It used to print "Nothing downloaded…" and exit **0**: `confirm_download`
//! answered no on a redirected stdin, and a declined offer is a decision
//! honoured, which is a success. That reasoning does not survive the menu.
//! There is no default source any more — with no path and no terminal there is
//! nothing to decide *with*, and exit 0 would tell a script that setup
//! succeeded when nothing was set up. The next command then fails with "no car
//! has been set up yet", one command away from the thing that caused it.
//!
//! So the refusal is an error, and it names the command line that needs no menu
//! ([`crate::ui::menu`]'s `menu_needs_a_terminal`). A script doing `vagcan setup || …`
//! changes behaviour on upgrade, and changes it to the branch it should always
//! have taken. `setup <PATH>` is unaffected and still works under `</dev/null`.
//!
//! ## Running it twice
//!
//! Each VCDS step is skipped when what it would write is already newer than what
//! it would read, and `--refresh` forces the lot. That is [`crate::labels`]'s
//! rule — a cache is trusted only while it is newer than the label files it came
//! from — applied to the other artefacts rather than a second rule invented for
//! them. It matters because recovering a `.rod` section's key is minutes of CPU.
//! The names are the exception: they are read every run, in seconds, and
//! written only when they come out different (see `names`).
//!
//! A project cache an older vagcan wrote, or a file there that is not a database, is
//! refused by every reader and writer; a run without `--refresh` stops on it before
//! reading anything. `--refresh` removes it first, the run builds today's from the
//! sources it reads, and every other source the old cache held is named with the
//! command that reads it again ([`prepare_cache`]).

mod registry;
pub mod source;
pub mod sources;
pub mod vendor;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::render::plural;

// The installation's layout — where the ODX files are, what the text table is
// called per language build, which code page it is in — is `core`'s now, because
// the registry read that needs it runs from `watch` and `measure` as well as
// from here — and which build a directory holds, and in what language, is asked
// by `faults` of the pool too. Same names, so every use below reads as it did.
pub(crate) use crate::registry::CODES_FILES;
use crate::registry::{ODX_DIR, TEXT_TABLES, build_of, codes_language, text_page};

/// Label files-wide `.rod` files whose keys every car needs.
///
/// `RD.rod` is the fault registry — the hop from a unit's own fault number to
/// the code that names it (`.archive/research/labels/fault-naming-hop.md`) — and `MUX.rod`
/// carries the shared multiplexer tables. Both are one file for the whole
/// label files, so recovering their keys once serves every vehicle.
///
/// Per-unit files are deliberately not swept. There are over sixteen thousand
/// of them, a section of a file not under its own name costs a search of minutes, and which
/// handful a given car needs is a question only that car can answer — it names
/// its own file in identifier `F19E`.
const SHARED_ROD_FILES: &[&str] = &["RD.rod", "MUX.rod"];

/// How many threads copy the raw files: see [`copy_label_files`].
const COPY_WORKERS: usize = 4;

pub struct Options<'a> {
	/// The VCDS installation root. Without one, an installation is offered for
	/// download and the run continues into the same parse.
	pub dir: Option<&'a str>,
	/// Redo every step, whatever is already on disk.
	pub refresh: bool,
	/// Where the archives are served from. A parameter so the download path is
	/// testable against a local file rather than the network.
	pub archive_base: &'a str,
	/// Fetch an installation and read that, without asking which source.
	///
	/// **The download's only route in used to be the menu**, which made it
	/// unreachable to a caller that had already asked its own question — and
	/// [`crate::rescue`] is exactly that caller: it has the person's `yes` in
	/// hand and a menu in front of them would be the same question twice. It
	/// is the same answer as picking "Download VCDS" off [`source::choose`],
	/// and goes down the same path from there, so the fetch, the reuse of an
	/// installation already unpacked, and the parse stay one path rather than
	/// two.
	///
	/// Ignored when `dir` is given: a path on the command line is a statement
	/// about what to read, and nothing here talks it out of it.
	pub download: bool,
}

/// Everything one `setup` run decided before it started reading anything.
///
/// **The order in here is the whole point (D7).** `source::project_id` answers
/// from the folder name, because it runs before anything is opened and the
/// folder is all there is. An ODIS project names itself inside, in
/// `index.xml`'s `<SHORT-NAME>`, and that survives what an unzip does to a
/// folder: `SK37X (1)` on disk is still `SK37X` in there. So the project is
/// **opened first**, its own name preferred, and only then is a directory
/// created — otherwise one car's data lands in a store called `SK-37X-copy`
/// while the project inside it calls itself `SK37X`, which is the two-directory
/// failure `datadir::existing_folder` was written to undo for cars.
struct Chosen {
	source: source::Source,
	/// The VCDS installation read beside the ODIS project, when the recommended
	/// row was picked and one was actually given. Always a VCDS source by the
	/// time it gets here — a download has already been fetched.
	names: Option<source::Source>,
	/// The ODIS project this run reads into, named as it names itself. `None` for a
	/// VCDS installation alone: that lands in the VCDS store, and is not named.
	project: Option<crate::project::Project>,
	/// The machine's VCDS store, where a VCDS installation lands.
	vcds: crate::project::Project,
	/// The opened ODIS project, when that is what this is. Opened before the
	/// directory was created, and kept rather than reopened: it holds 88 MB of
	/// inflated string pools.
	odis: Option<vag_data_labels::odis::Project>,
	/// What landing on `project` means for the source: the closing line says it.
	landing: source::Landing,
}

/// Ask what to read, work out what to call it, and open the store.
fn choose(io: &mut impl crate::ui::menu::Asker, dialog: &mut impl source::Dialog, opts: &Options<'_>) -> Result<Option<Chosen>> {
	// A download already asked for is not a question to ask again — see
	// `Options::download`. Everything after this line is the menu's own path.
	let chosen = match (opts.dir, opts.download) {
		(None, true) => source::Choice::download(),
		_ => match source::choose(io, dialog, opts.dir)? {
			Some(chosen) => chosen,
			None => return Ok(None),
		},
	};
	let source = fetched(chosen.source, opts)?;
	// The VCDS half of the recommended row. Resolved the same way, because a
	// download is a download whichever question asked for it.
	let names = chosen.names.map(|from| fetched(from, opts)).transpose()?;

	let vcds = crate::project::vcds()?;
	// A VCDS installation alone asks nothing: it lands in the machine's one store.
	let source::Source::Odis { dir } = &source else {
		let landing = match &source {
			source::Source::Vcds { dir } => vcds_landing(&vcds, dir),
			_ => source::Landing::New,
		};
		return Ok(Some(Chosen {
			source,
			names,
			project: None,
			vcds,
			odis: None,
			landing,
		}));
	};
	let existing = crate::project::list()?;
	let rereads = rereads(&source, &existing)?;
	let named = source::project_id(io, crate::ui::can_ask(), &source, &existing)?;
	// Opened before a directory exists, so its own name can win.
	let odis = open_odis(dir)?;
	let id = prefer_its_own_name(odis.id(), &named.id);
	let project = crate::project::open_or_create(&id)?;
	let renamed = (id != named.id).then_some(named.id.as_str());
	let (level, line) = project_line(&project, named.unasked, renamed);
	vag_cli_core::verbosity::say(level, &line);
	Ok(Some(Chosen {
		landing: source::landing(&id, &existing, &rereads),
		source,
		names,
		project: Some(project),
		vcds,
		odis: Some(odis),
	}))
}

/// What reading the VCDS installation at `dir` means for the store: a reread when the
/// store holds this one, a replacement when it holds another, new otherwise.
fn vcds_landing(store: &crate::project::Project, dir: &Path) -> source::Landing {
	if crate::project::records_source(store, vag_data_db::VCDS, &dir.display().to_string()) {
		return source::Landing::Rereads;
	}
	match crate::project::holds_vcds(store) {
		true => source::Landing::Replaces,
		false => source::Landing::New,
	}
}

/// The run's one line about its project: the name, how it came to be that name when
/// that is not the plain answer, and the directory last.
///
/// Level 0 when the name was taken with nobody asked — a directory created under a
/// name that appears nowhere is found later with no idea how it got there — and
/// `-v` otherwise, where the closing line has named the project already.
fn project_line(project: &crate::project::Project, unasked: bool, renamed: Option<&str>) -> (u8, String) {
	let how = match (unasked, renamed) {
		(true, _) => " (no terminal: default taken)".to_string(),
		(false, Some(folder)) => format!(" (its index.xml; the folder is `{folder}`)"),
		(false, None) => String::new(),
	};
	let level = if unasked { 0 } else { vag_cli_core::verbosity::NOTES };
	(
		level,
		format!("{STEP}into the project {}{how} at {}", project.id, home_short(&project.dir)),
	)
}

/// The projects in `existing` that have read the ODIS project `source` before.
fn rereads(source: &source::Source, existing: &[String]) -> Result<Vec<String>> {
	let (kind, dir) = match source {
		source::Source::Odis { dir } => (vag_data_db::ODIS, dir),
		source::Source::Vcds { .. } | source::Source::DownloadVcds => return Ok(Vec::new()),
	};
	let projects = crate::datadir::projects_dir()?;
	let dir = dir.display().to_string();
	Ok(
		existing
			.iter()
			.filter(|id| {
				let project = crate::project::Project {
					id: (*id).clone(),
					dir: projects.join(id),
				};
				crate::project::records_source(&project, kind, &dir)
			})
			.cloned()
			.collect(),
	)
}

/// A download resolved into the installation it fetched.
///
/// The download is not a source, it is how one is obtained. Picking it from a
/// menu *is* the consent, and one decision is one question. Both menus can ask
/// for it, and so can [`Options::download`], so all three go through here.
fn fetched(source: source::Source, opts: &Options<'_>) -> Result<source::Source> {
	match source {
		source::Source::DownloadVcds => Ok(source::Source::Vcds {
			dir: vendor::fetch(opts.archive_base)?,
		}),
		named => Ok(named),
	}
}

/// Run one stage of a step, and say at `-vv` how long it took.
///
/// One `timing` line per stage on stderr, at `-vv`. This is the instrument the
/// 2026-09-10 performance work was done with (it was `VAGCAN_TIMING=1` until `-v`
/// took it over), kept because "which stage got slow" is the first question the
/// next regression asks, and an `eprintln!` somebody has to re-add is not an
/// instrument. A step's own time is on its `-v` line ([`Step::took`]).
fn timed<T>(stage: &str, work: impl FnOnce() -> T) -> T {
	let started = std::time::Instant::now();
	let out = work();
	status(vag_cli_core::verbosity::WIRE, "Timing", &timing(stage, started.elapsed()));
	out
}

/// One of `setup`'s own detail lines at `level` — `-v`, `-vv` — indented under the steps.
fn status(level: u8, verb: &str, text: &str) {
	vag_cli_core::verbosity::say(level, &format!("{DETAIL}{} {text}", verb.to_lowercase()));
}

/// Run one step, and keep how long it took on the step itself.
fn timed_step(work: impl FnOnce() -> Result<Step>) -> Result<Step> {
	let started = std::time::Instant::now();
	vag_cli_core::verbosity::keep_details();
	let step = work();
	let details = vag_cli_core::verbosity::kept_details();
	let mut step = step?;
	step.took = Some(started.elapsed());
	step.details = details;
	Ok(step)
}

/// Run work that makes several steps, its `-v` and `-vv` lines kept for the first of them.
fn kept_for_first(work: impl FnOnce() -> Result<Vec<Step>>) -> Result<Vec<Step>> {
	vag_cli_core::verbosity::keep_details();
	let steps = work();
	let details = vag_cli_core::verbosity::kept_details();
	let mut steps = steps?;
	if let Some(first) = steps.first_mut() {
		first.details.splice(0..0, details);
	}
	Ok(steps)
}

/// Seconds as a step's `-v` line gives them: `14.4 s`.
fn seconds(took: std::time::Duration) -> String {
	format!("{:.1} s", took.as_secs_f64())
}

/// A `Timing` line's clause: the stage, then seconds.
fn timing(stage: &str, took: std::time::Duration) -> String {
	format!("{stage}: {}", seconds(took))
}

/// Open an ODIS project and say at `-v` which one it turned out to be.
fn open_odis(dir: &Path) -> Result<vag_data_labels::odis::Project> {
	let started = std::time::Instant::now();
	let project = vag_data_labels::odis::Project::open(dir).with_context(|| format!("reading the ODIS project at {}", dir.display()))?;
	status(
		vag_cli_core::verbosity::NOTES,
		"Opened",
		&opened_odis(dir, started.elapsed(), project.pools().len(), project.version()),
	);
	Ok(project)
}

/// The `-v` `Opened` line's clause for an ODIS project: what it is, then how long its
/// string pools took and where, as a step's `-v` line gives them.
fn opened_odis(dir: &Path, took: std::time::Duration, pools: usize, version: Option<&str>) -> String {
	format!(
		"the ODIS project, version {}, {pools} {} ({}; {})",
		version.unwrap_or("unknown"),
		plural(pools, "pool"),
		seconds(took),
		dir.display()
	)
}

/// The name the project gives itself, where it can be a directory name.
///
/// Falls back to what the folder was called when `<SHORT-NAME>` holds something
/// a directory cannot be named: nothing is sanitised into shape here, because a
/// mangled name files a car where no later run would look for it.
///
/// **The landing is decided on the name this returns.** Swapping the name here can
/// turn "new" into "added" — the ordinary re-run: a second download unzips to
/// `SK37X (1)`, which cleans to a name no project has, and then `<SHORT-NAME>`
/// files it into the `SK37X` that has been there all along. Design §5 makes that a
/// merge, so the closing line's landing is asked of the name filed under
/// ([`choose`]), and the `project` line names the folder it was not filed under
/// ([`project_line`]).
fn prefer_its_own_name(named: &str, asked: &str) -> String {
	if named == asked {
		return asked.to_string();
	}
	crate::project::folder_name(named).unwrap_or_else(|_| asked.to_string())
}

/// The file this installation uses for a job, by name or by asking.
///
/// `known` is tried first, so an English or Russian install asks nothing. Only
/// when none of them is there does this offer the directory's own `suffix`
/// files: the file is almost certainly present under a name from a build
/// nobody here has seen, and "your installation is broken" would be the wrong
/// thing to tell somebody looking straight at it.
///
/// Nothing is asked when stdin is not a terminal — a script gets `None` and the
/// step reports what it could not find, rather than blocking on a prompt no one
/// is there to answer.
fn locate(dir: &Path, known: &[&str], what: &str, suffix: &str) -> Result<Option<PathBuf>> {
	for name in known {
		let candidate = dir.join(name);
		if candidate.is_file() {
			return Ok(Some(candidate));
		}
	}
	// Nothing of that kind is there at all, so there is nothing to ask: a list
	// with no choices is an error, and it stopped setup at its first step on an
	// installation that simply ships no fault text.
	if !crate::ui::can_ask() || !holds_any(dir, suffix) {
		return Ok(None);
	}
	// The question's own line: what is asked for, and why it is asked at all.
	vag_cli_core::say!("none of {known:?} is in {} — pick the {what}:", dir.display());
	let mut chooser = crate::ui::Console::new(format!(
		"re-run `vagcan setup` from a terminal, or point it at a build\n\
         whose {what} is one of {known:?}"
	));
	crate::ui::picker::pick_path(&mut chooser, dir, &[crate::ui::picker::Level::files(what).ending(suffix)])
}

/// Whether `dir` holds any file ending in `suffix`, case aside — what a picker
/// for that kind of file would have to offer.
fn holds_any(dir: &Path, suffix: &str) -> bool {
	let suffix = suffix.to_ascii_lowercase();
	std::fs::read_dir(dir).is_ok_and(|entries| {
		entries
			.flatten()
			.any(|e| e.path().is_file() && e.file_name().to_string_lossy().to_ascii_lowercase().ends_with(&suffix))
	})
}

/// Whether this text table's key is already in the cache, so no search is due.
fn keyed_already(source: &Path, project: &crate::project::Project) -> Result<bool> {
	let cache = project.rod_keys();
	let Ok(text) = std::fs::read_to_string(&cache) else { return Ok(false) };
	let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
		return Ok(false);
	};
	let name = source.file_name().unwrap_or_default().to_string_lossy();
	Ok(
		json
			.as_object()
			.is_some_and(|m| m.keys().any(|k| k.starts_with(name.as_ref()) && k.ends_with("TXT"))),
	)
}

/// Clear `target` when what is about to be written is a different build.
///
/// The copy is freshness-gated per file, which makes a second run of the same
/// installation nearly free — and makes a run of a *different* one a disaster:
/// nothing is ever removed, so the Russian build lands on top of the English
/// one and the two mix. The label loader then reads whichever directory happens
/// to be flat, so a reader who asked for Russian keeps getting English names
/// beside Russian fault text, with nothing anywhere saying so.
///
/// Detected from the files themselves rather than from a marker, and only the
/// mismatch clears: an unchanged installation still costs a second to confirm.
/// `--refresh` clears unconditionally, which is what it is for.
///
/// Returns the build it cleared away when it was another one, for the copy's row:
/// a shared pool emptied under every project is not a thing to say at `-v` only.
fn replace_if_another_build(root: &Path, target: &Path, refresh: bool) -> Result<Option<&'static str>> {
	if !target.exists() {
		return Ok(None);
	}
	let (here, incoming) = (build_of(target), build_of(root));
	let mismatch = matches!((here, incoming), (Some(a), Some(b)) if a != b);
	if !mismatch && !refresh {
		return Ok(None);
	}
	if mismatch {
		// Why, at `-v`: layering the two would leave names from one build and fault
		// text from the other, with nothing to say which was which.
		status(
			vag_cli_core::verbosity::NOTES,
			"Removed",
			&format!(
				"{}: it held the build that ships {}, this one ships {}",
				target.display(),
				here.unwrap_or("?"),
				incoming.unwrap_or("?")
			),
		);
	}
	std::fs::remove_dir_all(target).with_context(|| format!("clearing {}", target.display()))?;
	Ok(if mismatch { here } else { None })
}

/// How a step ended, which picks its status line's verb ([`Step::verb`]).
///
/// A step that had nothing to do is worth a line as much as one that ran: somebody
/// who expected minutes and got seconds needs to see why, or they will assume it
/// failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
	Wrote,
	/// Nothing written: what is on disk reads as this run's read does.
	Current,
	/// Wrote something, but not all of it. Distinct from `Wrote` because a
	/// number on its own reads as a total: "124,294 names" looks complete until
	/// you know the table held 195,910. Distinct from `Missing` because what it
	/// did write is real and usable.
	Partial,
	/// Wrote nothing: the source lacks it.
	Missing,
	/// Not done yet, for a reason that is a step still to take rather than
	/// something the source lacks: the registry's channels wait for a car to
	/// have been connected. Not a gap — "with gaps" and "a newer VCDS may have
	/// it" would send the reader to the wrong place — and the detail says when it
	/// is done, with nothing for the reader to run.
	Pending,
	/// Did not complete, for a reason on this machine rather than in the
	/// source: a record of a car's units that does not read. A gap in the
	/// report, never an abort of the run — the steps before it wrote real
	/// files, and a run that stopped without its report would leave them
	/// unmentioned (found in review, 2026-09-28). The detail names the file;
	/// what to do about it closes the run ([`Step::fix`]).
	Failed,
}

impl State {
	/// Whether the run is short of something because of it.
	fn gap(self) -> bool {
		matches!(self, State::Partial | State::Missing | State::Failed)
	}
}

/// What one step of the run did: its status line, and what `-v` adds.
#[derive(Debug)]
struct Step {
	/// Which source the step read: [`vag_data_db::VCDS`] or [`vag_data_db::ODIS`].
	source: &'static str,
	/// The step, in a word: `copy`, `labels`, `names`, `keys`, `registry`, `channels`, `faults`.
	name: &'static str,
	state: State,
	/// The status line's clause, one line: the counts, or a gap's reason in a few words.
	detail: String,
	/// The whole reason, at `-v`, for a step short of something.
	why: Option<String>,
	/// What it wrote into, at `-v`.
	path: Option<PathBuf>,
	/// How long it took, at `-v`, on the same line as its path.
	took: Option<std::time::Duration>,
	/// The `-v` and `-vv` lines said while it ran, said under its line once it is done
	/// ([`vag_cli_core::verbosity::keep_details`]).
	details: Vec<(u8, String)>,
	/// What to type about it, at level 0 under its line: a `help: …` line, for a
	/// step whose remedy is a command.
	fix: Option<String>,
}

impl Step {
	fn new(source: &'static str, name: &'static str, state: State, detail: impl Into<String>) -> Step {
		Step {
			source,
			name,
			state,
			detail: detail.into(),
			why: None,
			path: None,
			took: None,
			details: Vec::new(),
			fix: None,
		}
	}

	fn fix(mut self, fix: impl Into<String>) -> Step {
		self.fix = Some(fix.into());
		self
	}

	fn at(mut self, path: impl Into<PathBuf>) -> Step {
		self.path = Some(path.into());
		self
	}

	fn because(mut self, why: impl Into<String>) -> Step {
		self.why = Some(why.into());
		self
	}
}

/// The command to type when there is no terminal to ask at — what `setup` and
/// the offer to run it print on a pipe. ODIS first, as `missing` orders them:
/// it is the source that brings scalings.
pub const WITHOUT_A_TERMINAL: &str = "vagcan setup <ODIS project or VCDS installation>";

pub async fn run(opts: Options<'_>) -> Result<()> {
	let mut io = crate::ui::Console::new(WITHOUT_A_TERMINAL);
	// **The system folder panel is opened from this thread, and this thread is
	// the main one.** `main` is `#[tokio::main]` — `block_on` around the whole
	// of `main`'s future, which it runs on the calling thread — and the `setup`
	// arm awaits this future on that same thread, so nothing has moved off the
	// main thread by the time `native_folder` runs. macOS requires exactly that
	// of `NSOpenPanel`; see `source::native_folder`. Do not wrap this call in
	// `spawn_blocking`, and do not `spawn` it: the one thing that leaves the
	// main thread is step 5's registry read, inside `registry_channels`.
	run_with(&mut io, &mut source::native_folder, opts).await
}

/// [`run`] inside another command — [`crate::rescue`]'s, which fetches label data
/// for a command that stopped short of it and then runs that command again. Its status
/// lines are on stderr, as every run's are: stdout is the data of the command asked
/// for, and `vagcan faults > out.txt` must hold the faults and nothing of `setup`.
pub(crate) async fn run_inside(opts: Options<'_>) -> Result<()> {
	let mut io = crate::ui::Console::new(WITHOUT_A_TERMINAL);
	// On the main thread, for the reason [`run`] gives.
	run_with(&mut io, &mut source::native_folder, opts).await
}

/// The rule behind [`run`], with the asking behind [`crate::ui::menu::Asker`] and
/// [`source::Dialog`] so the flow is testable without a terminal — and without a
/// window, which CI has even less of.
async fn run_with(io: &mut impl crate::ui::menu::Asker, dialog: &mut impl source::Dialog, opts: Options<'_>) -> Result<()> {
	let started = std::time::Instant::now();
	// A migration this run makes, and what `-v` says while the source is chosen, are said
	// under the heading, not before it.
	crate::project::hold_migration_notes();
	vag_cli_core::verbosity::keep_details();
	let chosen = choose(io, dialog, &opts);
	let before = vag_cli_core::verbosity::kept_details();
	let Some(mut chosen) = chosen? else { return Ok(()) };
	let reads = this_run_reads(&chosen);
	// The caches this run writes: the ODIS project's, the VCDS store's, or both.
	let vcds_dir = match (&chosen.source, &chosen.names) {
		(source::Source::Vcds { dir }, _) | (_, Some(source::Source::Vcds { dir })) => Some(dir.clone()),
		_ => None,
	};
	let mut removed: Vec<String> = Vec::new();
	let mut caches: Vec<PathBuf> = chosen.project.iter().map(|p| p.cache()).collect();
	if vcds_dir.is_some() {
		caches.push(chosen.vcds.cache());
	}
	for cache in caches {
		if let Some(said) = prepare_cache(&cache, opts.refresh, &reads)? {
			// Its first line, as it happens; in full at the close, where it is read.
			vag_cli_core::note!("{}", said.lines().next().unwrap_or_default());
			removed.push(said);
		}
	}

	// **The pair's order: the VCDS installation's files, the ODIS project, then
	// the VCDS registry.** The registry step asks whether this run's ODIS rows
	// describe a unit the installation has nothing for, so it comes after them; the
	// files come first so the pair reads the way it always has. The names do not
	// care: the text table's read replaces `names.json`, `read_odis` merges into
	// `names-odis.json`.
	let store = chosen.vcds.clone();
	vag_cli_core::say!("{}", heading(&chosen));
	for note in crate::project::take_migration_notes() {
		match note.starts_with("warning:") {
			true => vag_cli_core::say!("{note}"),
			false => vag_cli_core::say!("{STEP}{note}"),
		}
	}
	for (level, text) in before {
		vag_cli_core::verbosity::say(level, &format!("{DETAIL}{}", tilde_in(text.trim_start())));
	}
	let mut said = Said::default();
	let mut steps = Vec::new();
	if let Some(dir) = &vcds_dir {
		let (dir, store, refresh) = (dir.clone(), store.clone(), opts.refresh);
		let files = off_the_main_task(move || read_vcds_files(&dir, &store, refresh)).await?;
		said.steps(&files);
		steps.extend(files);
	}
	if let source::Source::Odis { dir } = &chosen.source {
		let odis = chosen.odis.take().expect("an ODIS source opens its project in `choose`");
		let project = chosen.project.clone().expect("an ODIS source has its project by `choose`");
		let dir = dir.clone();
		let read = off_the_main_task(move || kept_for_first(|| read_odis(&odis, &dir, &project))).await?;
		said.steps(&read);
		steps.extend(read);
	}
	// What the registry read and the closing line ask of: this run's project, or the one
	// the next command reads beside the VCDS store.
	let project = match &chosen.project {
		Some(project) => Some(project.clone()),
		None => crate::project::selected().ok().flatten(),
	};
	let stores = crate::project::Stores { project, vcds: store };
	if let Some(dir) = &vcds_dir {
		vag_cli_core::verbosity::keep_details();
		let step = read_vcds_registry(dir, &stores).await;
		let details = vag_cli_core::verbosity::kept_details();
		let mut step = step?;
		step.details.splice(0..0, details);
		said.steps(std::slice::from_ref(&step));
		steps.push(step);
	}

	// Written down so a later command needs no flag. Not a preference — the
	// answer to "which car did I just set up", which is the one a bare
	// `vagcan faults` has to be able to reach. A VCDS installation names no project.
	if let Some(project) = &chosen.project {
		crate::project::remember(&project.id)?;
	}
	// Each source this run read, asked of that source alone: the two are independent, and
	// a VCDS run must not close on scalings an ODIS project beside it holds (CLAUDE.md).
	use vag_cli_core::source::Source;
	let pool = crate::project::rod_pool()?;
	let ready = |source: Source| -> Result<Ready> {
		Ok(Ready {
			scalings: !crate::extracted::open_from(&stores, Some(source)).is_empty(),
			fault_labels: fault_text_available(&pool, source, &stores)?,
		})
	};
	let read_odis = matches!(chosen.source, source::Source::Odis { .. });
	let read: Vec<Source> = [(read_odis, Source::Odis), (vcds_dir.is_some(), Source::Vcds)]
		.into_iter()
		.filter_map(|(read, source)| read.then_some(source))
		.collect();
	let first = ready(if read_odis { Source::Odis } else { Source::Vcds })?;
	let closing = Closing {
		name: chosen
			.project
			.as_ref()
			.filter(|_| read_odis)
			.map_or(crate::project::VCDS_STORE, |p| p.id.as_str()),
		landing: chosen.landing,
		scalings: first.scalings,
		fault_labels: first.fault_labels,
		also: match read_odis && vcds_dir.is_some() {
			true => Some(ready(Source::Vcds)?),
			false => None,
		},
		reads_instead: reads_instead(&stores, &read),
		took: started.elapsed(),
	};
	said.close(&steps, &closing);
	// Last, where it is read: a removed cache leaves sources to read again.
	for said in removed {
		vag_cli_core::say!("{said}");
	}
	Ok(())
}
/// The sources a run reads, as the cache records them: `(kind, dir)`, the ODIS project's
/// folder and the VCDS installation's label directory ([`crate::labels::label_dir_under`]).
fn this_run_reads(chosen: &Chosen) -> Vec<(&'static str, String)> {
	let mut reads = Vec::new();
	for source in std::iter::once(&chosen.source).chain(&chosen.names) {
		match source {
			source::Source::Odis { dir } => reads.push((vag_data_db::ODIS, dir.display().to_string())),
			source::Source::Vcds { dir } => {
				if let Ok(labels) = crate::labels::label_dir_under(dir) {
					reads.push((vag_data_db::VCDS, labels.display().to_string()));
				}
			}
			source::Source::DownloadVcds => {}
		}
	}
	reads
}

/// The line that heads the sources a removed cache held and this run does not read.
const READ_AGAIN: &str = "It also held what these were read from, which this run does not read — read each again:";

/// The project's cache, made ready for this run to write into.
///
/// A cache every writer would refuse — an older vagcan's, or a file that is not a database
/// — stops a run without `--refresh` here, naming `--refresh`, before minutes of reading
/// meet the same refusal at the first write; the file is not touched. With `--refresh` it
/// is removed, and what is said comes back: the run rebuilds only what it reads (`reads`,
/// as [`this_run_reads`] spells it), so every other source the old cache held is named
/// with the `vagcan setup --refresh <dir>` that reads it again, or as gone from this
/// machine. `None` when the cache is current or not there.
fn prepare_cache(cache: &Path, refresh: bool, reads: &[(&str, String)]) -> Result<Option<String>> {
	let refused = match vag_data_db::check(cache) {
		Ok(()) => return Ok(None),
		Err(refused) => refused,
	};
	let what = match &refused {
		_ if !refresh => anyhow::bail!("{refused}"),
		vag_data_db::Error::OlderCache { .. } => "was written by an older vagcan",
		vag_data_db::Error::NotACache { .. } => "is not a database",
		_ => anyhow::bail!("checking {}: {refused}", cache.display()),
	};
	let held = vag_data_db::remove_if_older(cache)
		.map_err(|e| anyhow::anyhow!("clearing {}: {e}", cache.display()))?
		.unwrap_or_default();
	// A relative `dir` an older vagcan recorded matches nothing: resolving it here would
	// be a guess about where that run was.
	let read = |kind: &str, dir: &str| {
		Path::new(dir).is_absolute()
			&& reads
				.iter()
				.any(|(k, d)| *k == kind && vag_data_db::normalise_dir(d) == vag_data_db::normalise_dir(dir))
	};
	let mut again: Vec<String> = Vec::new();
	for (kind, dir) in &held {
		if read(kind, dir) {
			continue;
		}
		let line = read_again(kind, dir);
		if !again.contains(&line) {
			again.push(line);
		}
	}
	let mut said = format!("{} {what}: removed, and rebuilt from what this run reads.", cache.display());
	if !again.is_empty() {
		said.push_str(&format!("\n{READ_AGAIN}\n{}", again.join("\n")));
	}
	Ok(Some(said))
}

/// The line that says how to read one source of a removed cache again: the command, or
/// why there is none.
///
/// A relative `dir` — recorded by an older vagcan, true in the directory it ran in — is
/// never looked for from this process's working directory: in a checkout it would name
/// a folder there and suggest `setup --refresh vendor/…`, which is the guess this tool
/// does not make (`CLAUDE.md`). It is named as a path nothing resolves.
fn read_again(kind: &str, dir: &str) -> String {
	let at = Path::new(dir);
	if !at.is_absolute() {
		return format!("  {dir} — a relative path an older vagcan recorded; nothing resolves it");
	}
	match kind {
		vag_data_db::ODIS if at.is_dir() => format!("  vagcan setup --refresh {}", crate::missing::shell_word(dir)),
		vag_data_db::ODIS => format!("  {dir} — an ODIS project, not on this machine any more"),
		vag_data_db::VCDS => match crate::registry::installation_holding(at) {
			Some(root) => format!("  vagcan setup --refresh {}", crate::missing::shell_word(&root.display().to_string())),
			None => format!(
				"  {dir} — a VCDS installation's labels, not on this machine any more \
				 (`vagcan setup --refresh` with no folder offers to download one)"
			),
		},
		other => format!("  {dir} — a {other} source"),
	}
}

/// Whether `vagcan faults` will be able to name a code from `source` once this run is done:
/// VCDS's fault labels in the shared pool, or fault rows in the project's cache — the one
/// asked, never the other standing in for it.
///
/// Asked of the project this run wrote, not of `OdisFaults::open`, which
/// resolves the *current* project (`--project`, `VAGCAN_PROJECT`, config.toml)
/// and answered for the first car while a second one was being set up. And an
/// error is an error: a cache this run has just written that will not answer
/// is not "no fault text". Only a cache that is not there at all is.
fn fault_text_available(pool: &Path, source: vag_cli_core::source::Source, stores: &crate::project::Stores) -> Result<bool> {
	if source == vag_cli_core::source::Source::Vcds {
		return Ok(crate::faultnames::has_fault_labels(pool));
	}
	let Some(cache) = stores.cache_of(vag_data_db::ODIS).filter(|cache| cache.is_file()) else {
		return Ok(false);
	};
	let (_, codes) = vag_data_db::fault_counts(&cache).map_err(|e| anyhow::anyhow!("counting the fault texts in {}: {e}", cache.display()))?;
	Ok(codes > 0)
}

/// Run one of `setup`'s steps on the blocking pool, so the main task — inside
/// `main`'s `select!` with Ctrl-C — is free to act on the signal. Made on the
/// main task, minutes of key search or label parsing sat between the signal and
/// the branch that acts on it (found in review, 2026-09-28: steps 1–4 lost it,
/// since 2026-09-14; step 5 was fixed first). Nothing that needs the main thread
/// runs here: the folder panel is [`choose`]'s, before any step. A step that
/// asks a question asks it from this thread, which is the same terminal.
async fn off_the_main_task<T: Send + 'static>(step: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
	tokio::task::spawn_blocking(step)
		.await
		.map_err(|e| anyhow::anyhow!("the step did not finish: {e}"))?
}

/// Steps 1 to 4: the installation's files — the pool, the label files, the
/// names, the keys — into the VCDS store. Apart from step 5 so the pair can read the
/// ODIS project between them (see [`run_with`]).
fn read_vcds_files(root: &Path, store: &crate::project::Project, refresh: bool) -> Result<Vec<Step>> {
	let pool = crate::project::rod_pool()?;
	let replaced = replace_if_another_build(root, &pool, refresh)?;
	std::fs::create_dir_all(&pool).with_context(|| format!("creating {}", pool.display()))?;
	std::fs::create_dir_all(&store.dir).with_context(|| format!("creating {}", store.dir.display()))?;

	// The copy runs first and the derivations then read from it, so afterwards
	// `~/.vagcan` is the one set of raw files the car commands read. The
	// installation is still kept: the channels of a unit met later are read
	// from it (step 5, and `registry::ensure` at the car).
	// The fault text is looked for once, in step [1/5], and handed on: finding
	// it can mean asking the person which file it is, and one run must ask
	// that once.
	let started = std::time::Instant::now();
	vag_cli_core::verbosity::keep_details();
	let copy = copy_label_files(root, &pool, refresh);
	let details = vag_cli_core::verbosity::kept_details();
	let (mut copied, codes) = copy?;
	copied.took = Some(started.elapsed());
	copied.details = details;
	if let Some(build) = replaced {
		copied.detail.push_str(&format!("; replaced the {build} build"));
	}
	Ok(vec![
		copied,
		timed_step(|| label_cache(root, codes.as_deref(), store, refresh))?,
		timed_step(|| names(&pool, store))?,
		timed_step(|| rod_keys(&pool, store))?,
	])
}

/// Step 5, and the installation recorded as the VCDS store's source.
///
/// Async for one reason: the read is minutes of CPU, and made on the main task it
/// would sit between Ctrl-C and the `select!` that acts on it (see
/// [`registry::registry_channels`]). Nothing else in `setup` moves off the main
/// thread — the folder panel needs it (see [`run`]).
async fn read_vcds_registry(root: &Path, stores: &crate::project::Stores) -> Result<Step> {
	// The records of the cars on this machine, before anything is announced:
	// one that does not read is a step that did not complete, never an abort
	// with the steps before it unreported. The installation is still written
	// down when it is another than the one logged — this run's, so that the
	// next `setup`, or the first command with a car, reads from the right one
	// — and what was tried with the same one is kept: nothing was read here,
	// and a log wiped clean made every unit count as unread (found in review).
	let started = std::time::Instant::now();
	let mut step = match registry::recorded_units() {
		Ok(units) => registry::registry_channels(root, stores, &units).await?,
		Err(e) => {
			crate::registry::note_installation(&stores.vcds, root)?;
			registry::failed(&e, root)
		}
	};
	step.took = Some(started.elapsed());
	// From the installation itself, not the pool: see `registry`.
	crate::project::record_source(
		&stores.vcds,
		crate::project::SourceEntry {
			kind: vag_data_db::VCDS,
			path: root.display().to_string(),
			version: build_of(root).map(str::to_string),
			detail: None,
		},
	)?;
	Ok(step)
}
/// What one half of the ODIS read could not take, told apart by whose fault it
/// is.
///
/// **A refused type is not a broken file**: it is on
/// [`vag_data_labels::odis::loaders::REFUSED`], the permanent never-parsed
/// list, and this tool declining to read one says nothing about the project.
/// Anything else is a file that would not read. Both are counted so that a
/// project which gave up half of itself cannot look like one that had nothing
/// to give.
#[derive(Debug, Default)]
struct Skipped {
	refused: usize,
	unreadable: usize,
}

impl Skipped {
	fn count(&mut self, e: &vag_data_labels::odis::Error) {
		match e {
			vag_data_labels::odis::Error::Refused(_) => self.refused += 1,
			_ => self.unreadable += 1,
		}
	}

	/// The clause for the step's detail line, or nothing when nothing was
	/// skipped — a run that skipped nothing should not have to say so.
	fn note(&self) -> String {
		match self.refused + self.unreadable {
			0 => String::new(),
			_ => format!(", {} refused, {} unreadable", self.refused, self.unreadable),
		}
	}
}

/// The ODIS branch: every variant's channels into `cache.sqlite`, every name it
/// knows into `names-odis.json`.
///
/// **A variant that will not read costs itself and nothing else.** A project
/// describes hundreds of control units, and one whose measurement chain reaches
/// a refused type ([`vag_data_labels::odis::Error::Refused`] — a flash job, an access
/// key) or a shape this reader has no loader for must not cost the other
/// hundreds. The count of what was skipped is reported rather than hidden.
fn read_odis(odis: &vag_data_labels::odis::Project, dir: &Path, project: &crate::project::Project) -> Result<Vec<Step>> {
	let source = dir.display().to_string();
	let started = std::time::Instant::now();

	let variants = timed("odis variants", || odis.variants()).with_context(|| format!("listing the variants of {}", dir.display()))?;
	// The language the project declares for itself, on its source row — the
	// fault texts carry none of their own (`vag_data_labels::odis::Project::language`).
	if let Some(language) = odis.language() {
		vag_data_db::record_language(&project.cache(), vag_data_db::ODIS, &source, language)
			.map_err(|e| anyhow::anyhow!("recording the language of {} in {}: {e}", dir.display(), project.cache().display()))?;
	}
	// Every variant's fault table and measurement chain are walked on rayon's
	// pool, one variant per task, and the results come back **in variant
	// order** — `collect` on an indexed parallel iterator keeps it — so the rows
	// land in the cache in the same order and with the same ids a one-at-a-time
	// walk gave them. The two halves are read separately and fail separately: a
	// variant whose fault table is a refused type still has its channels read,
	// and the other way round. The line is fed from the workers through a
	// `Reporter`; its own thread keeps the spinner moving whether or not a
	// variant has just finished.
	type Walked = (
		Result<Vec<vag_data_labels::odis::Fault>, vag_data_labels::odis::Error>,
		Result<Vec<vag_data_labels::odis::Reading>, vag_data_labels::odis::Error>,
		Result<Vec<vag_data_labels::odis::Reading>, vag_data_labels::odis::Error>,
		Result<Vec<vag_data_labels::odis::EnvironmentRecord>, vag_data_labels::odis::Error>,
	);
	let walked: Vec<Walked> = {
		use rayon::prelude::*;
		let progress = crate::progress::Line::new();
		let reporter = progress.reporter();
		let done = std::sync::atomic::AtomicUsize::new(0);
		timed("odis readings (walk)", || {
			variants
				.par_iter()
				.map(|variant| {
					let faults = odis.faults(variant);
					let readings = odis.readings(variant);
					// The adaptation read: the same chain from the other `0x22` service.
					let adaptations = odis.adaptations(variant);
					// The fault records' layouts: what `faults` decodes a freeze frame by.
					let records = odis.extended_records(variant);
					let finished = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
					reporter.set(&format!("{finished} of {} — {}", variants.len(), variant.name));
					(faults, readings, adaptations, records)
				})
				.collect()
		})
	};
	let (mut with_channels, mut with_faults) = (0usize, 0usize);
	// One tally per half, for the reason above.
	let (mut chain_skipped, mut fault_skipped) = (Skipped::default(), Skipped::default());
	let mut channel_rows: Vec<(&str, &[vag_data_labels::odis::Reading])> = Vec::new();
	let mut fault_rows: Vec<(&str, &[vag_data_labels::odis::Fault])> = Vec::new();
	let (mut adaptation_rows, mut with_adaptations, mut adaptation_skipped): (Vec<(&str, &[vag_data_labels::odis::Reading])>, usize, Skipped) =
		(Vec::new(), 0, Skipped::default());
	let mut frame_rows: Vec<(&str, &[vag_data_labels::odis::EnvironmentRecord])> = Vec::new();
	let mut frame_skipped = Skipped::default();
	for (variant, (faults, readings, adaptations, records)) in variants.iter().zip(&walked) {
		match records {
			Ok(records) if !records.is_empty() => frame_rows.push((variant.name.as_str(), records.as_slice())),
			Ok(_) => {}
			Err(e) => frame_skipped.count(e),
		}
		match adaptations {
			Err(e) => adaptation_skipped.count(e),
			Ok(rows) if rows.is_empty() => {}
			Ok(rows) => {
				adaptation_rows.push((variant.name.as_str(), rows.as_slice()));
				with_adaptations += 1;
			}
		}
		match faults {
			Ok(faults) if !faults.is_empty() => {
				fault_rows.push((variant.name.as_str(), faults.as_slice()));
				with_faults += 1;
			}
			// A variant that declares no fault-code property is a fact about
			// the unit, not a failure.
			Ok(_) => {}
			// Counted, not swallowed. `if let Ok(..)` here made a project
			// whose fault tables all refused indistinguishable from one that
			// carries no fault text at all.
			Err(e) => fault_skipped.count(e),
		}
		match readings {
			// The refusal list is enforced by the parser and honoured here: a
			// refused type is a file this tool declines to read, not a broken
			// one, so the variant is skipped and the rest of the project stands.
			Err(e) => chain_skipped.count(e),
			Ok(readings) if readings.is_empty() => {}
			Ok(readings) => {
				channel_rows.push((variant.name.as_str(), readings.as_slice()));
				with_channels += 1;
			}
		}
	}
	// Both batches replace everything this source wrote before, not only the
	// variants in them: a variant that read last time and not this time must not
	// keep last time's rows.
	let faults_started = std::time::Instant::now();
	let codes = timed("odis faults (sqlite)", || {
		vag_data_db::put_all_faults(&project.cache(), &source, fault_rows)
	})
	.map_err(|e| anyhow::anyhow!("writing the fault codes to {}: {e}", project.cache().display()))?;
	let frames = timed("odis freeze frames (sqlite)", || {
		vag_data_db::put_all_odis_freeze_frames(&project.cache(), &source, frame_rows)
	})
	.map_err(|e| anyhow::anyhow!("writing the freeze-frame layouts to {}: {e}", project.cache().display()))?;
	let faults_took = faults_started.elapsed();
	let channels = timed("odis readings (sqlite)", || {
		vag_data_db::put_all_readings(&project.cache(), &source, channel_rows)
	})
	.map_err(|e| anyhow::anyhow!("writing the channels to {}: {e}", project.cache().display()))?;
	let adaptations_started = std::time::Instant::now();
	let adapted = timed("odis adaptations (sqlite)", || {
		vag_data_db::put_all_adaptations(&project.cache(), &source, adaptation_rows)
	})
	.map_err(|e| anyhow::anyhow!("writing the adaptation channels to {}: {e}", project.cache().display()))?;
	let mut adaptation_step = Step::new(
		vag_data_db::ODIS,
		"adaptations",
		State::Wrote,
		format!(
			"{adapted} {}, {with_adaptations} {}{}",
			plural(adapted, "channel"),
			plural(with_adaptations, "variant"),
			adaptation_skipped.note()
		),
	)
	.at(project.cache());
	adaptation_step.took = Some(adaptations_started.elapsed());
	// What the project calls each unit, and the number it gives it: `units`, `faults --unit`
	// and the rest name and number units from these on an ODIS run, never from VCDS's.
	let named_started = std::time::Instant::now();
	// A link that does not read costs its own unit's name, never the setup: it is counted in
	// the step's line, and the read is recorded all the same, so no run reads it again.
	let named = timed("odis unit names", || odis.unit_names());
	let (unit_names, unreadable) = match named {
		Ok(named) => {
			let unreadable = match named.skipped {
				0 => String::new(),
				1 => ", 1 link unreadable".to_string(),
				n => format!(", {n} links unreadable"),
			};
			(named.units, unreadable)
		}
		Err(e) => (Vec::new(), format!(", none read: {e}")),
	};
	timed("odis unit names (sqlite)", || {
		vag_data_db::put_unit_names(&project.cache(), &source, &unit_names)
	})
	.map_err(|e| anyhow::anyhow!("writing the unit names to {}: {e}", project.cache().display()))?;
	let mut links: Vec<(&str, Option<u32>)> = unit_names.iter().map(|u| (u.link.as_str(), u.address)).collect();
	links.sort_unstable();
	links.dedup();
	let numbered = links.iter().filter(|(_, address)| address.is_some()).count();
	let mut named_step = Step::new(
		vag_data_db::ODIS,
		"unit names",
		State::Wrote,
		format!(
			"{} {} for {} {}, {numbered} numbered{unreadable}",
			links.len(),
			plural(links.len(), "unit name"),
			unit_names.len(),
			plural(unit_names.len(), "variant"),
		),
	)
	.at(project.cache());
	named_step.took = Some(named_started.elapsed());
	let mut units = Step::new(
		vag_data_db::ODIS,
		"channels",
		State::Wrote,
		format!(
			"{channels} {}, {with_channels} of {} {}{}",
			plural(channels, "channel"),
			variants.len(),
			plural(variants.len(), "variant"),
			chain_skipped.note()
		),
	)
	.at(project.cache());
	// The walk reads both halves at once; it is the channels' time, and the faults'
	// is their own write.
	units.took = Some(
		started
			.elapsed()
			.saturating_sub(faults_took)
			.saturating_sub(adaptation_step.took.unwrap_or_default())
			.saturating_sub(named_step.took.unwrap_or_default()),
	);
	let mut faults = Step::new(
		vag_data_db::ODIS,
		"faults",
		State::Wrote,
		format!(
			"{codes} fault {} of {with_faults} {}{} and {frames} freeze-frame {}{}",
			plural(codes, "code"),
			plural(with_faults, "variant"),
			fault_skipped.note(),
			plural(frames, "field"),
			frame_skipped.note()
		),
	)
	.at(project.cache());
	faults.took = Some(faults_took);

	let names = timed_step(|| {
		let names = {
			let _spinner = crate::progress::Spinner::new("reading every object in the project".to_string());
			timed("odis names (walk)", || odis.names()).with_context(|| format!("reading the names of {}", dir.display()))?
		};
		let merged = timed("odis names (merge + write)", || merge_names(&project.odis_names(), names))?;
		Ok(
			Step::new(
				vag_data_db::ODIS,
				"names",
				State::Wrote,
				format!("{merged} ODIS {}", plural(merged, "name")),
			)
			.at(project.odis_names()),
		)
	})?;

	crate::project::record_source(
		project,
		crate::project::SourceEntry {
			kind: vag_data_db::ODIS,
			path: source,
			version: odis.version().map(str::to_string),
			detail: Some(format!("{} variants, {} pools", variants.len(), odis.pools().len())),
		},
	)?;
	Ok(vec![units, adaptation_step, faults, named_step, names])
}

/// Fold an ODIS project's pooled names into `names-odis.json`.
///
/// **Its own file, not `names.json`, and that separation is the whole point.**
/// Both name VW's text ids — this file by the `IDE`/`MAS` id itself,
/// `names.json` by the VCDS record whose tail names it (`odx-ids.json`) — but
/// they are not interchangeable wording. An ODIS *reading* carries the parameter's name in one ECU variant;
/// the pooled entry is the generic text for the id. Writing the second where a
/// reader expects the first cost real wording on the owner's car: 0 channels
/// gained a name, 340 got different and mostly worse wording, and the
/// physical and the logical wakeup counters both took the pooled counter's
/// name — two live channels labelled identically.
///
/// So this file is an index of what a text id means, which `vagcan dev vcds names`
/// searches, and `names.json` stays what a VCDS installation recovered.
///
/// **What is already there wins**, within this file: silently changing a name
/// under somebody who has been reading it is worse than not adding one.
fn merge_names(path: &Path, incoming: std::collections::BTreeMap<String, String>) -> Result<usize> {
	let mut names: std::collections::BTreeMap<String, String> = std::fs::read_to_string(path)
		.ok()
		.and_then(|text| serde_json::from_str(&text).ok())
		.unwrap_or_default();
	for (id, name) in incoming {
		names.entry(id).or_insert(name);
	}
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
	}
	std::fs::write(path, serde_json::to_string_pretty(&names)?).with_context(|| format!("writing {}", path.display()))?;
	Ok(names.len())
}

/// Step 1: copy the raw files into the shared pool, which every car command
/// reads instead of the installation. (The installation itself is still worth
/// keeping: step 5 and the first command with a car read a unit's channels
/// from it.)
///
/// **Flat, and only the files something reads at run time.** Fault naming and
/// ODX lookup search for a file *by name* (`dtc::find_named`,
/// `find_rod_by_odx_name`) and never by path, so the directory a `.rod` sat in
/// was never load-bearing — flattening loses nothing and makes one pool
/// shareable across every project.
///
/// **The `.lbl`/`.clb` files are not copied (D4).** They are read once, here,
/// into `cache.sqlite`, and that cache is what survives of them. The
/// consequence — a cache that can no longer be rebuilt without the installation
/// — is D5: every reader of the store's cache opens it as it is, never rebuilding it.
///
/// Idempotent and freshness-gated per file, the same rule the rest of setup
/// follows: a file is copied only when it is missing from the destination or
/// newer than what is there, and `--refresh` copies the lot.
/// Returns the step and **where the fault text was found**, because finding it
/// may have meant asking: [`locate`] opens a picker when the file is under a
/// name this tool does not know, and step [2/5] needs the same file to read
/// the build's language off its name. Looking twice asked twice.
fn copy_label_files(root: &Path, target: &Path, refresh: bool) -> Result<(Step, Option<PathBuf>)> {
	// What a copy cut short left behind, before anything is copied: one staging
	// file per kill, each under its pid, and nothing reads them.
	let stale = crate::datadir::remove_stale_copies(target);
	if stale > 0 {
		status(
			vag_cli_core::verbosity::NOTES,
			"Removed",
			&format!("{stale} staging {} of a copy cut short", plural(stale, "file")),
		);
	}
	let mut plan: Vec<(PathBuf, PathBuf)> = Vec::new();
	let odx = root.join(ODX_DIR);
	match odx.is_dir() {
		true => collect_rod_files(&odx, target, &mut plan)?,
		// A stripped or partial installation still yields whatever it has; a
		// missing input is reported, not fatal.
		false => status(
			vag_cli_core::verbosity::NOTES,
			"Skipped",
			&format!("the .rod files: no {ODX_DIR}/ in this installation"),
		),
	}
	// The fault text, under whichever name this language build gives it. Copied
	// under that same name: `faultnames` looks for the whole list too, so the
	// build stays recognisable rather than being flattened to the English one.
	let found = locate(root, CODES_FILES, "fault text file", ".dat")?;
	match &found {
		Some(codes) => {
			let name = codes.file_name().unwrap_or_default();
			plan.push((codes.clone(), target.join(name)));
		}
		None => status(
			vag_cli_core::verbosity::NOTES,
			"Skipped",
			"the fault text: none in this installation, so faults read as numbers",
		),
	}
	if plan.is_empty() {
		return Ok((
			Step::new(vag_data_db::VCDS, "copy", State::Missing, format!("no {ODX_DIR}/ and no fault text"))
				.because(format!("no {ODX_DIR}/ and no fault text under {}", root.display())),
			found,
		));
	}

	// On rayon's pool: sixteen thousand small files are almost all file-system
	// calls — a stat, a create, a clone, a rename — which the file system serves
	// side by side, and one at a time they were most of a cold setup once the
	// keys came from the file names. Every file lands under its own name, so the
	// order they land in changes nothing.
	use rayon::prelude::*;
	use std::sync::atomic::{AtomicUsize, Ordering};
	// Two files of one name in two directories land on one name in the flat
	// pool, and two threads writing one name would share its staging file — so
	// one is kept per name, compared as the file system compares names (APFS
	// ignores case). The newer source wins, the first listed on a tie. That is
	// what the loop this replaced left without `--refresh`, since it skipped a
	// source older than the copy already made; with `--refresh` it copied every
	// one and the last listed won. Now the newer one wins either way.
	let mut landing: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
	let mut keep = vec![true; plan.len()];
	let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
	for at in 0..plan.len() {
		let name = plan[at].1.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
		match landing.get(&name).copied() {
			None => {
				landing.insert(name, at);
			}
			Some(held) if modified(&plan[at].0) > modified(&plan[held].0) => {
				keep[held] = false;
				landing.insert(name, at);
			}
			Some(_) => keep[at] = false,
		}
	}
	let mut kept = keep.into_iter();
	plan.retain(|_| kept.next().unwrap_or(false));
	let total = plan.len();
	let (copied, skipped, done) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
	let bytes = std::sync::atomic::AtomicU64::new(0);
	let mut progress = crate::progress::Line::new();
	let reporter = progress.reporter();
	// Four workers, not one per core: the file system, not the CPU, is what the
	// copy waits on. Measured on the M4 (2026-10-20, cold, 16,577 files): one
	// worker 3.7–4.1 s, two 2.1–2.5 s, four 2.0–2.6 s, ten 2.5 s — and ten spent
	// 10–12 s of system time to get there where four spent 6.
	let workers = rayon::ThreadPoolBuilder::new()
		.num_threads(COPY_WORKERS)
		.build()
		.context("starting the copy's workers")?;
	workers.install(|| {
		plan.par_iter().try_for_each(|(src, dst)| -> Result<()> {
			let at = done.fetch_add(1, Ordering::Relaxed) + 1;
			if !refresh && is_newer(dst, src) {
				skipped.fetch_add(1, Ordering::Relaxed);
				return Ok(());
			}
			reporter.set(&format!("copying — {at} of {total}"));
			// Staged and renamed into place: a copy cut short by Ctrl-C must not
			// leave a truncated file under the final name, newer than its source and
			// so skipped as current by the next run (see `datadir::copy_file`).
			status(
				vag_cli_core::verbosity::WIRE,
				"Copying",
				&format!("{} to {}", src.display(), dst.display()),
			);
			crate::datadir::copy_file(src, dst)?;
			bytes.fetch_add(std::fs::metadata(dst).map_or(0, |m| m.len()), Ordering::Relaxed);
			copied.fetch_add(1, Ordering::Relaxed);
			Ok(())
		})
	})?;
	progress.finish();
	let (copied, skipped, bytes) = (copied.into_inner(), skipped.into_inner(), bytes.into_inner());
	// The pool is `.rod` files and, beside them, the one fault text file.
	let rods = plan
		.iter()
		.filter(|(_, dst)| dst.extension().is_some_and(|e| e.eq_ignore_ascii_case("rod")))
		.count();
	let what = match total > rods {
		true => format!("{rods} .rod {} and the fault text", plural(rods, "file")),
		false => format!("{rods} .rod {}", plural(rods, "file")),
	};
	let (state, detail) = match (copied, skipped) {
		(0, _) => (State::Current, format!("{what}, all up to date")),
		(_, 0) => (State::Wrote, format!("{what} ({})", size(bytes))),
		_ => (
			State::Wrote,
			format!("{copied} of {total} {} ({}), the rest all up to date", plural(total, "file"), size(bytes)),
		),
	};
	Ok((Step::new(vag_data_db::VCDS, "copy", state, detail).at(target), found))
}

/// A byte count the way a person reads one: `512 B`, `14.2 KB`, `122.4 MB`, `1.2 GB`.
fn size(bytes: u64) -> String {
	const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
	let mut value = bytes as f64;
	let mut unit = 0;
	while value >= 1000.0 && unit + 1 < UNITS.len() {
		value /= 1000.0;
		unit += 1;
	}
	match unit {
		0 => format!("{bytes} B"),
		_ => format!("{value:.1} {}", UNITS[unit]),
	}
}

/// Every `.rod` under `src`, wherever it sits, landing flat in `dst`.
///
/// The plan is built before anything is written, so the copy can report progress
/// against a known total and an unreadable directory fails before it has
/// half-copied a tree.
fn collect_rod_files(src: &Path, dst: &Path, plan: &mut Vec<(PathBuf, PathBuf)>) -> Result<()> {
	for entry in std::fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
		let path = entry?.path();
		if path.is_dir() {
			collect_rod_files(&path, dst, plan)?;
			continue;
		}
		let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
			continue;
		};
		if name.to_ascii_lowercase().ends_with(".rod") {
			plan.push((path.clone(), dst.join(name)));
		}
	}
	Ok(())
}

/// Step 2: parse the installation's label files into the project's cache.
///
/// **Reads the installation, not a copy of it**, because there is no longer a
/// copy: D4 drops the `.lbl`/`.clb` files and this cache is what survives of
/// them. This is the one moment they are ever read; the installation is still
/// kept, for the channels of units met later.
/// `codes` is the fault text file step [1/5] found, passed in rather than
/// looked for again — [`locate`] can ask the person which file it is, and one
/// setup run asks that once.
fn label_cache(root: &Path, codes: Option<&Path>, project: &crate::project::Project, refresh: bool) -> Result<Step> {
	// Current when the load left the cache file as it was: it was trusted, not rebuilt.
	let stamp = || std::fs::metadata(project.cache()).and_then(|m| m.modified()).ok();
	let before = stamp();
	let db = crate::labels::load_cached(root, &project.cache(), refresh)?;
	let current = before.is_some() && before == stamp();
	// This installation is the project's VCDS installation now: a second one replaces
	// the first in a project, and the first one's source goes with what it held. Said
	// here, by setup, and never by a build — a build is also how `dev vcds labels`
	// looked into another folder.
	let labels = crate::labels::label_dir_under(root)?;
	vag_data_db::keep_only_vcds(&project.cache(), &labels.to_string_lossy())
		.map_err(|e| anyhow::anyhow!("replacing the earlier VCDS installation in {}: {e}", project.cache().display()))?;
	// The language of this build's fault text, on the same source row the
	// label files were written under, so that `faults` can tell a VCDS source
	// in one language from an ODIS project in another.
	if let Some(language) = codes.and_then(|codes| codes.file_name().and_then(|n| n.to_str()).and_then(codes_language)) {
		vag_data_db::record_language(&project.cache(), vag_data_db::VCDS, &labels.to_string_lossy(), language)
			.map_err(|e| anyhow::anyhow!("recording the language of {} in {}: {e}", root.display(), project.cache().display()))?;
	}
	let counted = format!("{} {}", db.len(), plural(db.len(), "label file"));
	Ok(
		match current {
			true => Step::new(vag_data_db::VCDS, "labels", State::Current, format!("{counted}, all up to date")),
			false => Step::new(vag_data_db::VCDS, "labels", State::Wrote, counted),
		}
		.at(project.cache()),
	)
}

/// Step 3: read the names out of the global text table.
///
/// Two of the existing tools, chained the way `vagcan dev vcds`'s own help
/// documents: `rod --dump` writes the decrypted, inflated `[TXT]` section, and
/// the text table reader reads every record of it under its own key. The
/// intermediate file is this function's business and nobody else's, so it goes
/// in a scratch directory and is removed again.
///
/// One read, two files: `names.json`, text id → name, and `odx-ids.json`, text
/// id → the `IDE`/`MAS` id the text names (see [`crate::project::Project::odx_ids`]).
///
/// **Read on every run**, unlike the other steps. Skipping when the output was
/// newer than the table saved minutes while this was a dictionary search; the
/// read is seconds now, and the skip had a hole: the pool keeps the
/// installation's own file time, so a table from another build could pass for
/// the one read last time. A read that comes out the same writes nothing
/// ([`write_if_changed`]). The registry step does not lean on this file: it
/// names the `RM.rod` rows it reads from the installation's own text table,
/// the same install as the rows.
fn names(pool: &Path, project: &crate::project::Project) -> Result<Step> {
	// The text table is read out of the pool it was just copied into, so the
	// keys recovered from it match the bytes every later run will open.
	let odx = pool.to_path_buf();
	let (out, ids) = (project.names(), project.odx_ids());
	let Some(source) = locate(&odx, TEXT_TABLES, "measurement text table", ".rod")? else {
		return Ok(
			Step::new(vag_data_db::VCDS, "names", State::Missing, "no text table").because(format!("none of {TEXT_TABLES:?} is under {}", odx.display())),
		);
	};
	status(vag_cli_core::verbosity::WIRE, "Reading", &format!("the text table {}", source.display()));
	// How long it takes is the spinner's job to say, and it says it in elapsed
	// seconds rather than in a sentence nobody can act on.
	// Ask what opening it would cost before starting, because the two cases look
	// identical while running. A shifted text table has no anchor, so the only
	// route is sixty full-space searches — hours to days — and a spinner that
	// will not stop today is indistinguishable from one that stops in ninety
	// seconds. The Russian build ships exactly such a table.
	let name = source.file_name().unwrap_or_default().to_string_lossy().into_owned();
	let keyed = keyed_already(&source, project)?;
	status(
		vag_cli_core::verbosity::WIRE,
		"Reading",
		&format!("the text table's key: {}", if keyed { "cached" } else { "not cached" }),
	);
	if !keyed
		&& let Ok(bytes) = std::fs::read(&source)
		&& vag_data_labels::rod::key_cost(&bytes, "TXT") == Some(vag_data_labels::rod::KeyCost::AnchorSweep)
	{
		// Why, for the next reader of this code: `.archive/research/labels/tttext2.md`
		// §3.3. The person running `setup` is owed what it costs them and what to do.
		return Ok(
			Step::new(vag_data_db::VCDS, "names", State::Missing, format!("{name} opens only inside VCDS"))
				.because(NAMES_ONLY_IN_VCDS)
				.fix(READ_AN_ENGLISH_VCDS),
		);
	}
	let scratch = out.with_file_name("tttext-scratch");
	let _ = std::fs::remove_dir_all(&scratch);
	// `Classic`: a shifted table is reported, never swept for hours.
	let sections = crate::vcds::rod::open(
		&source.to_string_lossy(),
		vag_data_labels::rod::Search::Classic,
		Some(&project.rod_keys().to_string_lossy()),
		Some(&scratch.to_string_lossy()),
	)?;
	let text = scratch.join("TXT.bin");
	if !text.is_file() {
		let _ = std::fs::remove_dir_all(&scratch);
		let shut = sections.iter().find(|s| s.tag == "TXT").map_or("there is no such section", |s| {
			crate::vcds::rod::why_shut(s).unwrap_or("it opened, and nothing was written")
		});
		return Ok(
			Step::new(vag_data_db::VCDS, "names", State::Missing, format!("[TXT] of {name} did not open"))
				.because(format!("the [TXT] section of {} did not open: {shut}", source.display())),
		);
	}
	let table = crate::vcds::tttext::read_file(&text, text_page(&name));
	let _ = std::fs::remove_dir_all(&scratch);
	written_names(&table?, &source, &out, &ids)
}

/// What the names step leaves on disk, given what the text table read as.
///
/// Apart from [`names`] so the rule is testable without a `.rod` container to
/// open.
fn written_names(table: &vag_data_labels::tttext::Table, source: &Path, out: &Path, ids: &Path) -> Result<Step> {
	// A section with no record in it is not a table with no names: `{}` written
	// over the last good read would be a loss reported as "0 names".
	if table.texts.is_empty() {
		let example = match table.malformed.first() {
			Some((id, plain)) => format!(", e.g. {id:06} {plain:?}"),
			None => String::new(),
		};
		return Ok(
			Step::new(vag_data_db::VCDS, "names", State::Missing, "the text table read into no name").because(format!(
				"the [TXT] section of {} read into no name: {} {} of neither shape{example}, {} {} that were not records; \
			 the names already here are left as they were",
				source.display(),
				table.malformed.len(),
				plural(table.malformed.len(), "record"),
				table.not_records,
				plural(table.not_records, "line")
			)),
		);
	}
	let (names_text, count) = crate::vcds::tttext::names_json(table)?;
	let (ids_text, with_ids) = crate::vcds::tttext::odx_ids_json(table)?;
	let names_changed = write_if_changed(out, &names_text)?;
	let ids_changed = write_if_changed(ids, &ids_text)?;
	let detail = format!("{count} measurement {}, {with_ids} with an IDE or MAS id", plural(count, "name"));
	// Nothing is withheld for doubt any more — every record reads under its
	// own key — so the one way to fall short is a line of neither shape. The
	// 26.3 table has none; one here says the table or its key is not the one
	// this reader was built on, and a count on its own would hide that.
	if !table.malformed.is_empty() || table.not_records > 0 {
		let shown: Vec<String> = table.malformed.iter().take(3).map(|(id, plain)| format!("{id:06} {plain:?}")).collect();
		let example = match shown.is_empty() {
			true => String::new(),
			false => format!(" — e.g. {}", shown.join(", ")),
		};
		let left_out = table.malformed.len() + table.not_records;
		return Ok(
			Step::new(
				vag_data_db::VCDS,
				"names",
				State::Partial,
				format!("{detail}; {left_out} {} left out", plural(left_out, "record")),
			)
			.because(format!(
				"{} {} read as neither `<name>,` nor `<name>,<kind>,<value>`, and {} {} not records; \
				 both are left out rather than guessed at{example}",
				table.malformed.len(),
				plural(table.malformed.len(), "record"),
				table.not_records,
				if table.not_records == 1 { "line was" } else { "lines were" }
			))
			.at(out),
		);
	}
	// The one file the names step writes, at `-v`; `odx-ids.json` is beside it.
	Ok(
		match names_changed || ids_changed {
			true => Step::new(vag_data_db::VCDS, "names", State::Wrote, detail),
			false => Step::new(vag_data_db::VCDS, "names", State::Current, format!("{detail}, all up to date")),
		}
		.at(out),
	)
}

/// Write `text` to `path` unless the file holds exactly that already, and say
/// whether it wrote.
///
/// The names are read on every run, and a file rewritten with the same bytes
/// still looks new to whatever keys on its time — `dev dash build` counts
/// `names.json` among its inputs.
fn write_if_changed(path: &Path, text: &str) -> Result<bool> {
	if std::fs::read(path).is_ok_and(|held| held == text.as_bytes()) {
		return Ok(false);
	}
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
	}
	std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
	Ok(true)
}

/// Step 4: recover the keys of the `.rod` sections every car needs.
fn rod_keys(pool: &Path, project: &crate::project::Project) -> Result<Step> {
	let cache = project.rod_keys();
	let present: Vec<PathBuf> = SHARED_ROD_FILES.iter().map(|name| pool.join(name)).filter(|p| p.is_file()).collect();
	if present.is_empty() {
		return Ok(
			Step::new(
				vag_data_db::VCDS,
				"keys",
				State::Missing,
				format!("none of {}", SHARED_ROD_FILES.join(", ")),
			)
			.because(format!("none of {SHARED_ROD_FILES:?} is in {}", pool.display())),
		);
	}
	// What the keys file held before: the same after is a step with nothing to do.
	let before = std::fs::read(&cache).ok();
	let mut shut = Vec::new();
	for file in &present {
		for section in crate::vcds::rod::open(
			&file.to_string_lossy(),
			vag_data_labels::rod::Search::Classic,
			Some(&cache.to_string_lossy()),
			None,
		)? {
			let why = crate::vcds::rod::why_shut(&section);
			status(
				vag_cli_core::verbosity::WIRE,
				"Saving",
				&format!("[{}] in {}: {}", section.tag, file.display(), why.unwrap_or("open")),
			);
			if let Some(why) = why {
				shut.push(format!("[{}] in {} did not open: {why}", section.tag, file.display()));
			}
		}
	}
	let keys = std::fs::read_to_string(&cache)
		.ok()
		.and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
		.and_then(|v| v.as_object().map(|m| m.len()))
		.unwrap_or(0);
	// The cache holds only what a file's name does not give, so on an install
	// whose files carry their own names it holds nothing, and says why.
	let detail = match (keys, shut.is_empty()) {
		(0, true) => "no .rod keys: every section opens by its file name".to_string(),
		(n, _) => format!("{n} .rod {}", plural(n, "key")),
	};
	let unchanged = before.is_some() && before == std::fs::read(&cache).ok();
	Ok(match shut.is_empty() {
		true if unchanged && keys > 0 => Step::new(vag_data_db::VCDS, "keys", State::Current, format!("{detail}, all up to date")).at(cache),
		true => Step::new(vag_data_db::VCDS, "keys", State::Wrote, detail).at(cache),
		false => Step::new(
			vag_data_db::VCDS,
			"keys",
			State::Partial,
			format!("{detail}; {} {} shut", shut.len(), plural(shut.len(), "section")),
		)
		.because(shut.join("\n    "))
		.at(cache),
	})
}

/// What a text table only VCDS itself opens costs the reader — the `-v` half of
/// the names row that says it did not open.
const NAMES_ONLY_IN_VCDS: &str = "`vagcan watch` shows this installation's channels by reading id, not by name";

/// What to type about it, at level 0: an English VCDS instead — "instead", not
/// "too", since a second VCDS installation replaces the first in a project. The
/// English build is the one [`vendor`] fetches, and its text table opens.
const READ_AN_ENGLISH_VCDS: &str = "help: run `vagcan setup` with no path and pick \"Download VCDS\" (the English build; its names open)";

/// Whether `out` was written after `source` last changed.
///
/// The freshness rule the label cache already uses, applied to a file rather
/// than a directory. Anything unreadable counts as not fresh: redoing work is
/// cheap next to trusting a file that is not there.
fn is_newer(out: &Path, source: &Path) -> bool {
	match (std::fs::metadata(out), std::fs::metadata(source)) {
		(Ok(o), Ok(s)) => match (o.modified(), s.modified()) {
			(Ok(o), Ok(s)) => o >= s,
			_ => false,
		},
		_ => false,
	}
}

/// What `-v` says about scalings when the project has none.
///
/// A VCDS installation's label files carry names and no numbers
/// (`.archive/research/labels/rod-labels.md` §4.0c); its registry carries them
/// for the units of the cars this machine has met, and the registry row says
/// itself why it brought none — no car yet, or units the installation cannot
/// open. So this names only what that row cannot, and says nothing that would
/// send somebody whose units are already on record back to the car.
const SCALINGS_ARE_MEASURED: &str = "scalings: none yet; an ODIS project brings them for every unit it describes";

/// The same, when the registry step is still to come: what finishes it is a car
/// command meeting the car, not the other source — and nothing for the reader to
/// run for it.
const SCALINGS_WAIT_FOR_THE_CAR: &str = "scalings: none yet; an ODIS project brings them for every unit it describes, this \
                                         installation for the units it has files for, the first time a car command meets the car";

/// The pending registry row's detail: a reader told "not yet" sees what changes
/// that. Not a command to run — the tool reads the channels as a side effect of
/// the car commands, and never asks for a run made only to feed it.
const NEXT_WITH_THE_CAR: &str = "read when a car command first meets the car";

/// What the closing line says beside the steps.
struct Closing<'a> {
	/// What the run wrote into: the ODIS project's name, or the VCDS store's.
	name: &'a str,
	/// What landing on it meant for the source read.
	landing: source::Landing,
	/// Whether the stores now hold per-variant scalings — asked of them rather than of
	/// which branch ran, so a VCDS run beside a project that has them does not tell its
	/// reader they have none.
	scalings: bool,
	/// Whether `vagcan faults` will name a code: VCDS's fault labels, or the fault
	/// text an ODIS project carries.
	fault_labels: bool,
	/// The VCDS installation read in the same run as the ODIS project, and what it holds:
	/// a Ready line of its own, since the two are never one source.
	also: Option<Ready>,
	/// The source the car commands read when it is not one this run read — the default, or
	/// `[source] use` — said so a reader is not surprised the next command reads the other.
	reads_instead: Option<String>,
	/// How long the run took, from the question to the close.
	took: std::time::Duration,
}

/// What one source holds once `setup` has read it.
#[derive(Debug, Clone, Copy)]
struct Ready {
	scalings: bool,
	fault_labels: bool,
}

/// The hint when the car commands read a source this run did not read: which, and the flag
/// that reads the one just read.
fn reads_instead(stores: &crate::project::Stores, read: &[vag_cli_core::source::Source]) -> Option<String> {
	let just_read = read.first()?;
	// A setting that will refuse the next command — `[source] use = "odis"` with no ODIS
	// project — is said here, where it can be put right, not first at the car.
	let run = match vag_cli_core::source::run(stores) {
		Ok(run) => run?,
		Err(e) => return Some(format!("car commands will refuse: {e}; `{}` reads this one", just_read.flag())),
	};
	if read.contains(&run.source) {
		return None;
	}
	Some(format!("car commands read {}; `{}` reads this one", run.describe(), just_read.flag()))
}

/// One line of what `setup` says, at `level`, indentation and all.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
	level: u8,
	text: String,
}

impl Line {
	fn at(level: u8, text: impl Into<String>) -> Line {
		Line { level, text: text.into() }
	}

	fn say(&self) {
		vag_cli_core::verbosity::say(self.level, &self.text);
	}
}

/// How far a step line sits under the heading, and its `-v` detail under the step.
const STEP: &str = "  ";
const DETAIL: &str = "    ";

/// What a run has said so far: each step's lines as the step ends, and each `help:` once.
#[derive(Debug, Default)]
struct Said {
	helps: Vec<String>,
}

impl Said {
	/// Say these steps, now.
	fn steps(&mut self, steps: &[Step]) {
		for line in steps.iter().flat_map(|step| self.lines_of(step)) {
			line.say();
		}
	}

	/// One step's lines: its line, at `-v` its time, path and reason, and its `help:` the
	/// first time.
	fn lines_of(&mut self, step: &Step) -> Vec<Line> {
		let mut lines = step_lines(step);
		if let Some(help) = &step.fix
			&& !self.helps.contains(help)
		{
			self.helps.push(help.clone());
			lines.push(Line::at(0, help.clone()));
		}
		lines
	}

	/// The close, said: [`closing_lines`].
	fn close(&mut self, steps: &[Step], closing: &Closing<'_>) {
		for line in closing_lines(steps, closing) {
			line.say();
		}
	}
}

/// The run's heading: what it reads, from where — `Read the VCDS installation at ~/vcds-en`.
fn heading(chosen: &Chosen) -> String {
	let mut read = Vec::new();
	for source in std::iter::once(&chosen.source).chain(&chosen.names) {
		match source {
			source::Source::Odis { dir } => read.push(format!(
				"the ODIS project {} at {}",
				chosen.project.as_ref().map_or("", |p| p.id.as_str()),
				home_short(dir)
			)),
			source::Source::Vcds { dir } => read.push(format!("the VCDS installation at {}", home_short(dir))),
			source::Source::DownloadVcds => {}
		}
	}
	format!("Read {}", read.join(" and "))
}

/// A path as a person reads it: under the home directory, `~/…`.
fn home_short(path: &Path) -> String {
	crate::datadir::tilde(path)
}

/// A step's line — `  parsed 3035 label files` — and at `-v`, indented under it, how long
/// it took and what it wrote into, then its reason when it fell short. A level only adds
/// lines, so the line a quieter run printed is never changed.
fn step_lines(step: &Step) -> Vec<Line> {
	let mut lines = vec![Line::at(0, format!("{STEP}{}", step.clause()))];
	let extra: Vec<String> = [step.took.map(seconds), step.path.as_deref().map(home_short)]
		.into_iter()
		.flatten()
		.collect();
	if !extra.is_empty() {
		lines.push(Line::at(vag_cli_core::verbosity::NOTES, format!("{DETAIL}{}", extra.join(", "))));
	}
	if let Some(why) = &step.why {
		for line in why.lines() {
			lines.push(Line::at(vag_cli_core::verbosity::NOTES, format!("{DETAIL}{}", line.trim_start())));
		}
	}
	// What was said while it ran, under it, with `~` for the home directory. A line said
	// further in than a detail keeps the difference: it belongs to the line above it.
	for (level, text) in &step.details {
		let said = text.trim_start();
		let deeper = (text.len() - said.len()).saturating_sub(DETAIL.len());
		lines.push(Line::at(*level, format!("{DETAIL}{}{}", " ".repeat(deeper), tilde_in(said))));
	}
	lines
}

/// Every path under the home directory in a line, as `~/…`.
fn tilde_in(text: &str) -> String {
	match std::env::var("HOME") {
		Ok(home) if !home.is_empty() => text.replace(&format!("{home}/"), "~/"),
		_ => text.to_string(),
	}
}

/// The close: one `Ready:` line, what the run came to, and the hint under it.
fn closing_lines(steps: &[Step], closing: &Closing<'_>) -> Vec<Line> {
	let gaps = steps.iter().filter(|s| s.state.gap()).count();
	// What one source holds, for whom: `fault text and scalings for SK37X`. A second source
	// read in the same run is a second clause of the one result line — each said of itself.
	let holds = |ready: Ready, whom: &str| {
		let fault_text = if ready.fault_labels { "fault text" } else { "no fault text" };
		let scalings = if ready.scalings { "scalings" } else { "no scalings" };
		format!("{fault_text} and {scalings} for {whom}")
	};
	let ready = match gaps {
		0 => "Ready".to_string(),
		n => format!("Ready with {n} {}", plural(n, "gap")),
	};
	let whom = match closing.name == crate::project::VCDS_STORE {
		true => "every car".to_string(),
		false => closing.name.to_string(),
	};
	let first = Ready {
		scalings: closing.scalings,
		fault_labels: closing.fault_labels,
	};
	let result = match closing.also {
		None => holds(first, &whom),
		Some(also) => format!("{}; {}", holds(first, &whom), holds(also, "every car")),
	};
	let mut lines = vec![Line::at(0, format!("{ready}: {result}."))];
	let hint = |level: u8, text: &str| Line::at(level, format!("{STEP}({text})"));
	if let Some(instead) = &closing.reads_instead {
		lines.push(hint(0, instead));
	}
	match closing.landing {
		source::Landing::Replaces => lines.push(hint(0, "this installation replaces the one read before")),
		source::Landing::Adds => lines.push(hint(0, &format!("added to the project {}", closing.name))),
		source::Landing::New | source::Landing::Rereads => {}
	}
	// What to type next. The fault line promises names only when `vagcan faults` will print
	// them: it once announced labels copied in after an ODIS-only run that copied nothing.
	lines.push(hint(
		0,
		match closing.fault_labels || closing.also.is_some_and(|a| a.fault_labels) {
			true => "use \"vagcan faults\" with the car connected",
			false => "use \"vagcan faults\" with the car connected; its codes show as numbers",
		},
	));
	// Said only where it is true. An ODIS project declares a scaling per variant, so "none
	// yet" would be false there.
	if !closing.scalings && !closing.also.is_some_and(|a| a.scalings) {
		let pending = steps.iter().any(|s| s.state == State::Pending);
		lines.push(hint(
			vag_cli_core::verbosity::NOTES,
			if pending { SCALINGS_WAIT_FOR_THE_CAR } else { SCALINGS_ARE_MEASURED },
		));
	}
	lines.push(Line::at(
		vag_cli_core::verbosity::NOTES,
		format!("{STEP}(took {})", seconds(closing.took)),
	));
	lines
}

impl Step {
	/// The verb the step's line opens with, in the past: what it did, or `skipped` and
	/// `failed to`.
	fn verb(&self) -> &'static str {
		match self.state {
			State::Missing | State::Pending => "skipped",
			State::Failed => "failed to read",
			State::Wrote | State::Current | State::Partial => match (self.source, self.name) {
				(_, "copy") => "copied",
				(_, "labels") => "parsed",
				(vag_data_db::ODIS, "names") | (_, "keys") => "saved",
				_ => "read",
			},
		}
	}

	/// What the step is about, for a line that says it was skipped or failed.
	fn what(&self) -> &'static str {
		match (self.source, self.name) {
			(_, "copy") => "the .rod files",
			(_, "labels") => "the label files",
			(vag_data_db::ODIS, "names") => "the ODIS names",
			(_, "names") => "the measurement names",
			(_, "keys") => "the .rod keys",
			(_, "channels") => "the ODIS channels",
			(_, "faults") => "the ODIS fault codes",
			_ => "the channels",
		}
	}

	/// The line after the indent: the verb and the step's counts, or for one skipped or
	/// failed, what it is about and why.
	fn clause(&self) -> String {
		match self.state {
			State::Missing | State::Pending | State::Failed => {
				format!("{} {}: {}", self.verb(), self.what(), self.detail)
			}
			_ => format!("{} {}", self.verb(), self.detail),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_command_for_a_pipe_names_odis_first() {
		let odis = WITHOUT_A_TERMINAL.find("ODIS").expect(WITHOUT_A_TERMINAL);
		let vcds = WITHOUT_A_TERMINAL.find("VCDS").expect(WITHOUT_A_TERMINAL);
		assert!(odis < vcds, "{WITHOUT_A_TERMINAL}");
		assert!(WITHOUT_A_TERMINAL.starts_with("vagcan setup "), "{WITHOUT_A_TERMINAL}");
	}

	#[test]
	fn a_project_that_names_itself_beats_the_folder_it_was_unzipped_into() {
		// D7, and the failure it prevents: an unzip produces `SK37X (1)`, the
		// picker cleans that to `SK-37X-copy`, and the project inside still
		// calls itself `SK37X`. Filing it under the folder's name would put one
		// car in two stores — the two-directory bug `datadir::existing_folder`
		// was written to undo, arriving by a different door.
		assert_eq!(prefer_its_own_name("SK37X", "SK-37X-copy"), "SK37X");
		// Agreement keeps the name.
		assert_eq!(prefer_its_own_name("SK37X", "SK37X"), "SK37X");
		// A `<SHORT-NAME>` that could not be a directory falls back rather than
		// being sanitised: a mangled name files a car where nothing looks for it.
		assert_eq!(prefer_its_own_name("../escape", "SK37X"), "SK37X");
	}

	/// A project under test, where `project_line` reads its name and directory.
	fn project_at(id: &str) -> crate::project::Project {
		crate::project::Project {
			id: id.into(),
			dir: PathBuf::from(format!("/home/x/.vagcan/data/{id}")),
		}
	}

	#[test]
	fn the_project_is_one_line() {
		// `-v` once said the project three times in three formats. It is one line, with
		// how the name came about when that is not the plain answer.
		let project = project_at("SK37X");
		assert_eq!(
			project_line(&project, false, None),
			(
				vag_cli_core::verbosity::NOTES,
				"  into the project SK37X at /home/x/.vagcan/data/SK37X".to_string()
			)
		);
		// Filed under its index.xml's name: the folder it was not filed under is named.
		assert_eq!(
			project_line(&project, false, Some("SK-37X-copy")).1,
			"  into the project SK37X (its index.xml; the folder is `SK-37X-copy`) at /home/x/.vagcan/data/SK37X"
		);
		// With no terminal the default was taken unasked, and that is said at level 0:
		// a directory under a name that appears nowhere is a mystery found later.
		assert_eq!(project_line(&project, true, None).0, 0);
	}

	#[tokio::test]
	async fn a_run_that_could_not_ask_fails_rather_than_succeeding_at_nothing() {
		// The exit-code decision, pinned. With no path and no way to ask, there
		// is nothing to decide with — and `Ok(())` here would tell a script that
		// setup succeeded when nothing was set up, leaving the real failure to
		// surface one command later as "no car has been set up yet".
		//
		// Driven with an asker that has no answers rather than with a real
		// terminal check, so what is under test is that `run_with` propagates
		// the refusal instead of swallowing it.
		let mut io = crate::ui::menu::Scripted::new(vec![]);
		let outcome = run_with(
			&mut io,
			&mut source::no_dialog(),
			Options {
				dir: None,
				refresh: false,
				archive_base: vendor::ARCHIVE_BASE,
				download: false,
			},
		)
		.await;
		assert!(outcome.is_err(), "a run that asked nobody anything reported success");
	}

	#[test]
	fn a_name_swap_onto_a_project_that_exists_is_a_merge_after_all() {
		// The ordinary re-run, and the one case where the swap changes the
		// answer: a second download unzips to `SK37X (1)`, which cleans to a name
		// no project has, so `source::project_id` found it new. Then
		// `<SHORT-NAME>` files it into the `SK37X` that has been there all along.
		// Design §5 makes that a merge, and the closing line asks the landing of
		// the name filed under (`choose`).
		let existing = ["SK37X".to_string()];
		assert_eq!(prefer_its_own_name("SK37X", "SK-37X-1"), "SK37X");
		assert_eq!(source::landing("SK37X", &existing, &[]), source::Landing::Adds);
		// The same folder set up a second time lands on a project that has read
		// it: that is a reread, not a merge.
		assert_eq!(source::landing("SK37X", &existing, &existing), source::Landing::Rereads);
	}

	#[test]
	fn the_pair_reads_the_vcds_files_then_odis_then_the_vcds_registry() {
		// Both halves of the order were found in review (2026-09-28). The
		// registry step asks whether this run's ODIS rows describe a unit, so it
		// comes after them; step 2 trusts its label cache while the cache file is
		// newer than the label files, and the ODIS read writes that file, so the
		// files come before them. Spec §5 once put the wording first, while both
		// sources wrote `names.json`; the names no longer care.
		//
		// Asserted against the source rather than by running two multi-minute
		// reads: what is being pinned is an ordering decision, and a test that
		// needed a VCDS installation and an ODIS project could not run here at
		// all.
		let body = include_str!("mod.rs");
		let at = |needle: &str| body.find(needle).unwrap_or_else(|| panic!("{needle} is gone"));
		let files = at("off_the_main_task(move || read_vcds_files(&dir, &store, refresh)).await?");
		let odis = at("off_the_main_task(move || read_odis(&odis, &dir, &project)).await?");
		let registry = at("read_vcds_registry(dir, &stores).await?");
		assert!(files < odis, "the ODIS read moved ahead of the label files' freshness check");
		assert!(odis < registry, "the registry step moved ahead of the ODIS rows it asks about");
	}

	#[test]
	fn what_setup_says_at_v_is_one_line_each() {
		// Caught three times on real terminals when this was wrapped prose. A line is
		// one line, its path last, so its width is the terminal's business.
		let long = Path::new("/Users/somebody/Downloads/an-odis-project-with-a-long-name/SK37X");
		let took = std::time::Duration::from_millis(14_420);
		for said in [
			project_line(&project_at("SK37X"), false, Some("SK-37X-copy")).1,
			project_line(&project_at("SK37X"), true, None).1,
			opened_odis(long, took, 230, Some("2610.2.688")),
			timing("odis names (walk)", std::time::Duration::from_millis(32_590)),
		] {
			assert!(!said.contains('\n'), "{said:?}");
		}
		assert_eq!(
			opened_odis(long, took, 230, Some("2610.2.688")),
			format!("the ODIS project, version 2610.2.688, 230 pools (14.4 s; {})", long.display())
		);
		assert_eq!(
			opened_odis(long, took, 1, None),
			format!("the ODIS project, version unknown, 1 pool (14.4 s; {})", long.display())
		);
		// A stage inside a step, at -vv: the instrument the performance work used.
		assert_eq!(
			timing("odis names (walk)", std::time::Duration::from_millis(32_590)),
			"odis names (walk): 32.6 s"
		);
	}

	#[test]
	fn a_step_at_v_has_its_time_and_path_and_its_reason_under_it() {
		let mut step = Step::new(vag_data_db::VCDS, "keys", State::Partial, "2 .rod keys; 1 section shut")
			.because("[TXT] in RD.rod did not open")
			.at("/x/rod-keys.json");
		step.took = Some(std::time::Duration::from_millis(60));
		let lines: Vec<(u8, String)> = step_lines(&step).into_iter().map(|line| (line.level, line.text)).collect();
		let v = vag_cli_core::verbosity::NOTES;
		assert_eq!(
			lines,
			[
				(0, "  saved 2 .rod keys; 1 section shut".to_string()),
				(v, "    0.1 s, /x/rod-keys.json".to_string()),
				(v, "    [TXT] in RD.rod did not open".to_string())
			]
		);
	}

	#[test]
	fn odis_wording_is_kept_out_of_the_file_that_promises_vcds_wording() {
		// The regression this split exists for. `names.json` is what a reader
		// prefers over a channel's own name, so an ODIS run writing into it
		// replaced variant-specific wording with the pooled text for the id —
		// and collapsed two live channels onto one label.
		let here = TempDir::new("odisnames");
		let project = crate::project::Project {
			id: "SK37X".into(),
			dir: here.0.clone(),
		};
		assert_ne!(project.odis_names(), project.names(), "one file again");
		let incoming = [("MAS90001".to_string(), "Invented_Wakeup_Events_Counter".to_string())]
			.into_iter()
			.collect();
		merge_names(&project.odis_names(), incoming).unwrap();
		assert!(project.odis_names().is_file());
		assert!(!project.names().exists(), "an ODIS run wrote the file a VCDS run owns");
	}

	#[test]
	fn a_name_already_in_names_json_is_not_changed_under_somebody() {
		// The two sources agree about what a text id means — that is the whole
		// finding `.archive/research/labels/odis-crib.md` rests on — so where they do
		// not, the incumbent is what every earlier run has been reporting.
		let here = TempDir::new("names");
		let path = here.write("names.json", br#"{"000116": "Transmission Input Speed"}"#);
		let incoming = [
			("000116".to_string(), "Getriebe-Eingangsdrehzahl".to_string()),
			("000117".to_string(), "Motordrehzahl".to_string()),
		]
		.into_iter()
		.collect();
		assert_eq!(merge_names(&path, incoming).unwrap(), 2);
		let back: std::collections::BTreeMap<String, String> = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
		assert_eq!(
			back["000116"], "Transmission Input Speed",
			"it overwrote a name somebody was already reading"
		);
		assert_eq!(back["000117"], "Motordrehzahl");
	}

	/// A text table read from plaintext records, the way [`names`] gets one.
	fn text_table(records: &[(u32, &str)]) -> vag_data_labels::tttext::Table {
		let section: String = records
			.iter()
			.map(|(id, plain)| format!("{id:06},{}\r\n", vag_data_labels::glyphs::TableAlphabet::for_key(*id).encipher(plain)))
			.collect();
		vag_data_labels::tttext::read(section.as_bytes(), vag_data_labels::codes::CodePage::Windows1252)
	}

	#[test]
	fn a_read_that_comes_out_the_same_writes_nothing_and_another_is_written() {
		// What makes reading on every run cheap. That `names` reads every run is
		// not pinned here — it needs a `.rod` container to open.
		let here = TempDir::new("names-step");
		let (out, ids, source) = (here.0.join("names.json"), here.0.join("odx-ids.json"), here.0.join("TTTEXT.ROD"));
		let table = text_table(&[(17, "Invented flap position,2,90001"), (13, "Pump 2,")]);
		let first = written_names(&table, &source, &out, &ids).unwrap();
		assert_eq!(first.state, State::Wrote, "{first:?}");
		assert_eq!(first.detail, "2 measurement names, 1 with an IDE or MAS id");
		assert!(std::fs::read_to_string(&ids).unwrap().contains("\"IDE90001\""));
		let again = written_names(&table, &source, &out, &ids).unwrap();
		assert_eq!(again.state, State::Current, "the same read rewrote the files: {again:?}");
		assert_eq!(
			again.detail,
			format!("{}, all up to date", first.detail),
			"a current line still says what it holds"
		);
		// Another build's table is written whatever the files' times say — the
		// times are what used to let one pass for the other.
		let other = text_table(&[(17, "Invented flap position,2,90001"), (13, "Pump 3,")]);
		let step = written_names(&other, &source, &out, &ids).unwrap();
		assert_eq!(step.state, State::Wrote, "{step:?}");
		assert!(std::fs::read_to_string(&out).unwrap().contains("Pump 3"));
	}

	#[test]
	fn a_section_with_no_record_leaves_the_last_names_alone() {
		// `{}` over the last good read, reported as "0 names", is the failure.
		let here = TempDir::new("names-empty");
		let before = br#"{"000017": "Invented flap position"}"#;
		let out = here.write("names.json", before);
		let (ids, source) = (here.0.join("odx-ids.json"), here.0.join("TTTEXT.ROD"));
		let table = vag_data_labels::tttext::read(b"not a record\r\nnor this\r\n", vag_data_labels::codes::CodePage::Windows1252);
		let step = written_names(&table, &source, &out, &ids).unwrap();
		assert_eq!(step.state, State::Missing, "{step:?}");
		assert_eq!(std::fs::read(&out).unwrap(), before);
		assert!(!ids.exists());
		// Records of neither shape and nothing else: the same, and it shows one at -v.
		let step = written_names(&text_table(&[(5, "no separator")]), &source, &out, &ids).unwrap();
		assert_eq!(step.state, State::Missing, "{step:?}");
		assert!(step.why.as_deref().is_some_and(|why| why.contains("000005")), "{step:?}");
		assert_eq!(std::fs::read(&out).unwrap(), before);
	}

	#[test]
	fn a_line_of_neither_shape_makes_the_names_partial_and_keeps_the_rest() {
		let here = TempDir::new("names-partial");
		let (out, ids, source) = (here.0.join("names.json"), here.0.join("odx-ids.json"), here.0.join("TTTEXT.ROD"));
		let table = text_table(&[(17, "Invented flap position,2,90001"), (5, "no separator")]);
		let step = written_names(&table, &source, &out, &ids).unwrap();
		assert_eq!(step.state, State::Partial, "{step:?}");
		assert!(
			step.detail.starts_with("1 measurement name,"),
			"one of a noun is singular: {}",
			step.detail
		);
		assert!(
			step.detail.ends_with("1 record left out"),
			"the row counts what it left out: {}",
			step.detail
		);
		let why = step.why.as_deref().unwrap_or_default();
		assert!(why.contains("000005"), "the record left out is named at -v: {why}");
		assert!(std::fs::read_to_string(&out).unwrap().contains("000017"));
	}

	/// What the closing line says beside the steps, for a run under test: 3.1 s long.
	fn closing(scalings: bool, fault_labels: bool) -> Closing<'static> {
		Closing {
			name: "SK37X",
			landing: source::Landing::New,
			scalings,
			fault_labels,
			also: None,
			reads_instead: None,
			took: std::time::Duration::from_millis(3_140),
		}
	}

	/// Everything a run says after its heading at `level`, as stderr holds it.
	fn shown(steps: &[Step], closing: &Closing<'_>, level: u8) -> String {
		let mut said = Said::default();
		let mut lines: Vec<Line> = steps.iter().flat_map(|step| said.lines_of(step)).collect();
		lines.extend(closing_lines(steps, closing));
		let mut out: String = lines
			.iter()
			.filter(|line| line.level <= level)
			.map(|line| line.text.clone())
			.collect::<Vec<_>>()
			.join("\n");
		out.push('\n');
		out
	}

	#[test]
	fn a_run_is_a_line_per_step_under_its_heading_and_one_ready_line() {
		// The level-0 shape, pinned: a step a line, its verb in the past, then `Ready:`
		// and the hint under it.
		let steps = vec![
			Step::new(vag_data_db::VCDS, "copy", State::Current, "16577 files, all up to date").at("/x/rod"),
			Step::new(vag_data_db::VCDS, "labels", State::Wrote, "3035 label files").at("/x/cache.sqlite"),
			Step::new(
				vag_data_db::VCDS,
				"names",
				State::Current,
				"192469 measurement names, 42640 with an IDE or MAS id (unchanged)",
			)
			.at("/x/names.json"),
			Step::new(vag_data_db::VCDS, "keys", State::Wrote, "57 .rod keys"),
			Step::new(
				vag_data_db::ODIS,
				"channels",
				State::Wrote,
				"399283 channels, 669 of 717 variants, 0 refused, 4 unreadable",
			),
		];
		assert_eq!(
			shown(&steps, &closing(true, true), 0),
			"  copied 16577 files, all up to date\n\
			 \x20 parsed 3035 label files\n\
			 \x20 read 192469 measurement names, 42640 with an IDE or MAS id (unchanged)\n\
			 \x20 saved 57 .rod keys\n\
			 \x20 read 399283 channels, 669 of 717 variants, 0 refused, 4 unreadable\n\
			 Ready: fault text and scalings for SK37X.\n\
			 \x20 (use \"vagcan faults\" with the car connected)\n"
		);
	}

	#[test]
	fn the_heading_names_what_is_read_with_home_as_a_tilde() {
		let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set for tests"));
		let chosen = Chosen {
			source: source::Source::Vcds { dir: home.join("vcds-en") },
			names: None,
			project: None,
			vcds: crate::project::vcds_at(&home.join(".vagcan/vcds")),
			odis: None,
			landing: source::Landing::New,
		};
		assert_eq!(heading(&chosen), "Read the VCDS installation at ~/vcds-en");
		assert_eq!(home_short(Path::new("/elsewhere/SK37X")), "/elsewhere/SK37X");
	}

	#[test]
	fn at_v_a_step_line_carries_its_time_and_its_path_under_it() {
		// Somebody who has just waited several minutes is owed the paths: at `-v`, under
		// the step, which keeps its line as a quieter run printed it.
		let mut labels = Step::new(vag_data_db::VCDS, "labels", State::Wrote, "3035 label files").at("/x/vcds/cache.sqlite");
		labels.took = Some(std::time::Duration::from_millis(14_420));
		let steps = vec![labels];
		let v = shown(&steps, &closing(true, true), vag_cli_core::verbosity::NOTES);
		assert!(v.starts_with("  parsed 3035 label files\n    14.4 s, /x/vcds/cache.sqlite\n"), "{v}");
		assert!(v.ends_with("  (took 3.1 s)\n"), "{v}");
		let plain = shown(&steps, &closing(true, true), 0);
		assert!(!plain.contains("/x/vcds") && !plain.contains("took"), "paths and time are -v: {plain}");
	}

	#[test]
	fn a_step_that_could_not_run_says_so_without_condemning_the_rest() {
		let steps = vec![
			Step::new(vag_data_db::VCDS, "labels", State::Wrote, "3035 label files"),
			Step::new(vag_data_db::VCDS, "keys", State::Missing, "none of RD.rod, MUX.rod").because("none of them is in this installation"),
		];
		let plain = shown(&steps, &closing(false, true), 0);
		assert!(plain.contains("  skipped the .rod keys: none of RD.rod, MUX.rod\n"), "{plain}");
		assert!(plain.contains("  parsed 3035 label files\n"), "the rest stands: {plain}");
		assert!(plain.contains("Ready with 1 gap: "), "{plain}");
		let v = shown(&steps, &closing(false, true), vag_cli_core::verbosity::NOTES);
		assert!(
			v.contains("  skipped the .rod keys: none of RD.rod, MUX.rod\n    none of them is in this installation\n"),
			"the reason sits under the line at -v: {v}"
		);
		assert!(!plain.contains("none of them is"), "{plain}");
	}

	#[test]
	fn a_text_table_only_vcds_opens_is_reported_as_its_cost_and_its_remedy() {
		// The Russian build's table: the line says what did not open; `-v` what it costs
		// the reader; and what to type is one `help:` line at level 0, right under it.
		let steps = [
			Step::new(vag_data_db::VCDS, "names", State::Missing, "TTText-RUS.rod opens only inside VCDS")
				.because(NAMES_ONLY_IN_VCDS)
				.fix(READ_AN_ENGLISH_VCDS),
		];
		let plain = shown(&steps, &closing(false, true), 0);
		assert!(
			plain.starts_with(
				"  skipped the measurement names: TTText-RUS.rod opens only inside VCDS\n\
				 help: run `vagcan setup` with no path and pick \"Download VCDS\" (the English build; its names open)\n"
			),
			"{plain}"
		);
		let v = shown(&steps, &closing(false, true), vag_cli_core::verbosity::NOTES);
		assert!(v.contains("by reading id, not by name"), "{v}");
		assert_eq!(v.matches("Download VCDS").count(), 1, "said once: {v}");
		for jargon in ["shifted", "anchor", ".archive", "masks"] {
			assert!(!v.contains(jargon), "{jargon}: {v}");
		}
	}

	#[test]
	fn a_help_two_steps_share_is_said_once() {
		let steps = [
			Step::new(vag_data_db::VCDS, "names", State::Missing, "a").fix("help: one"),
			Step::new(vag_data_db::VCDS, "keys", State::Missing, "b").fix("help: one"),
		];
		assert_eq!(shown(&steps, &closing(false, true), 0).matches("help: one").count(), 1);
	}

	#[test]
	fn a_run_with_no_scalings_says_how_to_get_them() {
		// The close names only what the registry line cannot: an ODIS project. `Ready:`
		// says it at level 0, the remedy is a `-v` hint.
		let plain = shown(&[], &closing(false, true), 0);
		assert!(plain.starts_with("Ready: fault text and no scalings for SK37X.\n"), "{plain}");
		let v = shown(&[], &closing(false, true), vag_cli_core::verbosity::NOTES);
		assert!(v.contains(&format!("  ({SCALINGS_ARE_MEASURED})\n")), "{v}");
		assert!(v.contains("ODIS project"), "{v}");
		assert!(!v.contains("survey"), "{v}");
		assert!(!v.contains("no VCDS installation carries them"), "the old claim is false now: {v}");
	}

	#[test]
	fn a_failed_step_reads_failed_to() {
		let failed = Step::new(vag_data_db::VCDS, registry::NAME, State::Failed, "the cars' records did not read");
		assert_eq!(
			step_lines(&failed)[0].text,
			"  failed to read the channels: the cars' records did not read"
		);
		let fine = Step::new(vag_data_db::VCDS, "labels", State::Wrote, "1 label file");
		assert_eq!(step_lines(&fine)[0].text, "  parsed 1 label file");
	}

	#[test]
	fn a_folder_with_no_file_of_the_kind_offers_nothing_to_pick() {
		// The English install ships no fault text, and a picker opened on it had
		// no choices: an error, at step 1, whenever setup ran from a terminal.
		let here = tempfile::tempdir().unwrap();
		std::fs::write(here.path().join("TTTEXT.ROD"), b"").unwrap();
		std::fs::create_dir(here.path().join("folder.dat")).unwrap();
		assert!(!holds_any(here.path(), ".dat"), "a directory is not a file to pick");
		std::fs::write(here.path().join("Code-XYZ.DAT"), b"").unwrap();
		assert!(holds_any(here.path(), ".dat"), "and case aside, this one is");
	}

	#[test]
	fn a_car_not_connected_yet_is_the_next_step_and_not_a_gap_in_the_installation() {
		// With no car recorded there is no list of units to read. That is a step still to
		// take, not something the installation lacks, and it asks for no command: the car
		// commands read the channels as a side effect.
		let steps = vec![Step::new(vag_data_db::VCDS, registry::NAME, State::Pending, NEXT_WITH_THE_CAR).because(registry::PENDING_WHY)];
		let plain = shown(&steps, &closing(false, true), 0);
		assert!(!plain.contains("gap"), "{plain}");
		assert!(!plain.contains("newer VCDS"), "{plain}");
		assert!(
			plain.starts_with("  skipped the channels: read when a car command first meets the car\n"),
			"{plain}"
		);
		assert!(!plain.contains("help:"), "nothing to type at level 0: {plain}");
		let v = shown(&steps, &closing(false, true), vag_cli_core::verbosity::NOTES);
		assert!(v.contains("\n    no car recorded on this machine"), "{v}");
		assert!(!SCALINGS_WAIT_FOR_THE_CAR.contains("vagcan watch"), "{SCALINGS_WAIT_FOR_THE_CAR}");
		// And the scalings hint waits for the car rather than for the other source.
		assert!(v.contains(&format!("  ({SCALINGS_WAIT_FOR_THE_CAR})\n")), "{v}");
	}

	#[test]
	fn a_record_that_does_not_read_is_a_step_that_did_not_complete_and_the_report_still_comes() {
		// One unreadable `units.json` aborted `setup` at step 5 with no report, after steps
		// 1–4 had written their files (found in review, 2026-09-28). It is a gap now: the
		// line names the file, `-v` the cause, and under it at every level what to do.
		let bad = anyhow::Error::new(crate::units::NotARecord {
			path: PathBuf::from("/home/x/.vagcan/cars/BAD/units.json"),
			cause: "expected ident at line 1 column 2".into(),
		});
		let root = Path::new("/opt/VCDS 26.3");
		let steps = vec![
			Step::new(vag_data_db::VCDS, "labels", State::Wrote, "3035 label files"),
			registry::failed(&bad, root),
		];
		let plain = shown(&steps, &closing(false, true), 0);
		assert!(
			plain.contains(
				"  failed to read the channels: not a record this tool wrote: /home/x/.vagcan/cars/BAD/units.json\n\
				 help: move /home/x/.vagcan/cars/BAD/units.json aside, then run `vagcan setup '/opt/VCDS 26.3'`\n"
			),
			"{plain}"
		);
		assert!(plain.contains("Ready with 1 gap: "), "{plain}");
		assert!(plain.contains("3035 label files"), "the steps before it are still reported: {plain}");
		let v = shown(&steps, &closing(false, true), vag_cli_core::verbosity::NOTES);
		assert!(v.contains("\n    expected ident at line 1 column 2\n"), "the cause is at -v: {v}");
		// An error that is not a record's — the directory would not read — has no file of
		// its own to name: the line carries the error, which does.
		let other = registry::failed(&anyhow::anyhow!("permission denied").context("reading /x/cars"), root);
		assert_eq!(other.state, State::Failed);
		assert_eq!(other.detail, "the cars' records did not read: reading /x/cars: permission denied");
		let help = other.fix.as_deref().unwrap_or_default();
		assert!(help.starts_with("help: make ") && help.contains("readable"), "{help}");
		assert!(help.ends_with("then run `vagcan setup '/opt/VCDS 26.3'`"), "{help}");
	}

	#[test]
	fn every_line_keeps_to_eighty_columns_with_short_paths() {
		let steps = vec![
			Step::new(vag_data_db::VCDS, "labels", State::Wrote, "3035 label files").at("/x/cache.sqlite"),
			Step::new(vag_data_db::VCDS, registry::NAME, State::Pending, NEXT_WITH_THE_CAR).because(registry::PENDING_WHY),
			registry::failed(
				&anyhow::Error::new(crate::units::NotARecord {
					path: PathBuf::from("/x/units.json"),
					cause: "expected ident at line 1 column 2".into(),
				}),
				Path::new("/x/vcds"),
			),
		];
		for (scalings, fault_labels) in [(false, false), (false, true), (true, true)] {
			for line in shown(&steps, &closing(scalings, fault_labels), 0).lines() {
				assert!(line.chars().count() <= 80, "{} columns: {line:?}", line.chars().count());
			}
		}
	}

	#[test]
	fn a_run_that_read_scalings_does_not_close_by_saying_there_are_none() {
		let plain = shown(&[], &closing(true, true), 0);
		assert!(plain.starts_with("Ready: fault text and scalings for SK37X.\n"), "{plain}");
		let v = shown(&[], &closing(true, true), vag_cli_core::verbosity::NOTES);
		assert!(!v.contains("scalings: none yet"), "{v}");
	}

	#[test]
	fn a_run_that_copied_no_labels_does_not_claim_the_labels_are_copied_in() {
		// The fault line was unconditional, so an ODIS-only run — which copies nothing —
		// finished by announcing that the labels are in.
		let without = shown(&[], &closing(true, false), 0);
		assert!(without.starts_with("Ready: no fault text and scalings for SK37X.\n"), "{without}");
		assert!(without.contains("as numbers"), "{without}");
		assert!(!without.contains("copied in") && !without.contains("VCDS"), "{without}");
		let with = shown(&[], &closing(true, true), 0);
		assert!(!with.contains("as numbers"), "{with}");
	}

	#[test]
	fn the_ready_line_says_for_whom_and_a_hint_what_landing_meant() {
		// Spec §5 adds a second source to a project rather than replacing it, and a second
		// VCDS installation replaces the first: said at level 0, as a hint.
		for (name, landing, ready, hint) in [
			("SK37X", source::Landing::New, "for SK37X.", None),
			("SK37X", source::Landing::Adds, "for SK37X.", Some("  (added to the project SK37X)")),
			("SK37X", source::Landing::Rereads, "for SK37X.", None),
			(crate::project::VCDS_STORE, source::Landing::New, "for every car.", None),
			(
				crate::project::VCDS_STORE,
				source::Landing::Replaces,
				"for every car.",
				Some("  (this installation replaces the one read before)"),
			),
		] {
			let plain = shown(
				&[],
				&Closing {
					name,
					landing,
					..closing(true, true)
				},
				0,
			);
			let first = plain.lines().next().unwrap_or_default();
			assert!(first.ends_with(ready), "{plain}");
			assert_eq!(plain.lines().nth(1).filter(|l| !l.contains("vagcan faults")), hint, "{plain}");
		}
	}
	#[test]
	fn a_vcds_installation_is_read_into_the_machine_store_with_no_name_asked() {
		// One installation serves every car: no project is named or made for it. The
		// asker has no answers, so a question would fail the test.
		let install = synthetic_install("no-question");
		let dir = install.0.display().to_string();
		let mut io = crate::ui::menu::Scripted::new(vec![]);
		let opts = Options {
			dir: Some(&dir),
			refresh: false,
			archive_base: vendor::ARCHIVE_BASE,
			download: false,
		};
		let chosen = choose(&mut io, &mut source::no_dialog(), &opts)
			.unwrap()
			.expect("a VCDS folder is a source");
		assert!(chosen.project.is_none(), "{:?}", chosen.project);
		assert_eq!(chosen.vcds.dir, crate::datadir::vcds_dir().unwrap());
		assert!(io.typed.is_empty() && io.all_said().is_empty(), "something was asked: {}", io.all_said());
		assert!(
			!crate::datadir::projects_dir().unwrap().join(source::DEFAULT_ID).exists(),
			"a project was made"
		);
	}

	#[tokio::test]
	async fn a_run_against_something_that_is_not_an_installation_says_where_to_get_one() {
		let mut io = crate::ui::menu::Scripted::new(vec![]);
		let err = run_with(
			&mut io,
			&mut source::no_dialog(),
			Options {
				dir: Some("/definitely/not/here"),
				refresh: false,
				archive_base: vendor::ARCHIVE_BASE,
				download: false,
			},
		)
		.await
		.unwrap_err();
		let text = vag_cli_core::report::render(&err, 0, vag_cli_core::report::Colour::Never);
		assert!(text.contains("download VCDS"), "{text}");
		assert!(text.contains("Labels/"), "{text}");
		// And the other way in, since it is the whole point of the argument
		// being optional.
		assert!(text.contains("\n\nhelp: "), "{text}");
	}

	/// A cache an older vagcan wrote, in a temporary directory: `run_with` itself cannot be
	/// driven here, since a run writes its project under the real `~/.vagcan`, so the step it
	/// takes before reading anything is driven on its own.
	#[test]
	fn an_older_cache_stops_a_run_without_refresh_and_refresh_names_what_the_run_does_not_rebuild() {
		let here = tempfile::tempdir().unwrap();
		// Canonical, as the cache spells a directory (`/private/var`, not `/var`, on macOS).
		let base = here.path().canonicalize().unwrap();
		let cache = base.join("cache.sqlite");
		let other = base.join("other-odis");
		let vcds = base.join("vcds");
		let labels = vcds.join("Labels");
		std::fs::create_dir_all(&other).unwrap();
		std::fs::create_dir_all(vcds.join("UDS_EV")).unwrap();
		std::fs::create_dir_all(&labels).unwrap();
		for project in ["/x/read", &other.display().to_string(), "/gone/SK37X"] {
			vag_data_db::put_readings(&cache, project, "EV_A", &[]).unwrap();
		}
		vag_data_db::record_language(&cache, vag_data_db::VCDS, &labels.display().to_string(), "eng").unwrap();
		rusqlite::Connection::open(&cache).unwrap().execute_batch("DROP TABLE fault").unwrap();
		let reads = vec![(vag_data_db::ODIS, "/x/read".to_string())];

		// Without `--refresh`: stopped before anything is read, naming `--refresh`, and the
		// file is not touched.
		let before = std::fs::read(&cache).unwrap();
		let why = prepare_cache(&cache, false, &reads).unwrap_err().to_string();
		assert!(why.contains("older vagcan") && why.contains("vagcan setup --refresh"), "{why}");
		assert_eq!(std::fs::read(&cache).unwrap(), before, "a refused run changed the cache");

		// With it: removed, and every source the run does not read named with what reads it
		// again — the ODIS project on disk, the VCDS installation by its root, the project
		// that is gone as gone. What this run reads is not asked for again.
		let said = prepare_cache(&cache, true, &reads).unwrap().expect("an older cache is removed");
		assert!(!cache.exists());
		assert!(said.contains("was written by an older vagcan: removed"), "{said}");
		assert!(said.contains(READ_AGAIN), "{said}");
		assert!(said.contains(&format!("  vagcan setup --refresh {}\n", other.display())), "{said}");
		assert!(said.contains(&format!("  vagcan setup --refresh {}", vcds.display())), "{said}");
		assert!(said.contains("  /gone/SK37X — an ODIS project, not on this machine any more"), "{said}");
		assert!(!said.contains("/x/read"), "{said}");

		// The run's first write builds today's, and from then on nothing is removed.
		vag_data_db::put_readings(&cache, "/x/read", "EV_A", &[]).unwrap();
		assert!(prepare_cache(&cache, false, &reads).unwrap().is_none());
		assert!(prepare_cache(&cache, true, &reads).unwrap().is_none());
		assert!(cache.exists(), "a current cache was removed");
	}

	#[test]
	fn a_cache_file_that_is_not_a_database_is_named_and_refresh_replaces_it() {
		let here = tempfile::tempdir().unwrap();
		let cache = here.path().join("cache.sqlite");
		std::fs::write(&cache, b"not an SQLite file at all, but long enough to have a header").unwrap();
		let why = prepare_cache(&cache, false, &[]).unwrap_err().to_string();
		assert!(why.contains("is not a database") && why.contains("vagcan setup --refresh"), "{why}");
		let said = prepare_cache(&cache, true, &[]).unwrap().expect("removed");
		assert!(!cache.exists());
		assert!(said.contains("is not a database: removed"), "{said}");
		assert!(!said.contains(READ_AGAIN), "a file that held no sources names none: {said}");
	}

	#[test]
	fn a_run_reads_its_odis_project_and_the_vcds_labels_beside_it() {
		let here = tempfile::tempdir().unwrap();
		let vcds = here.path().join("vcds");
		std::fs::create_dir_all(vcds.join("Labels")).unwrap();
		std::fs::write(vcds.join("Labels/part.lbl"), b"001,1,Engine Speed,,").unwrap();
		let chosen = Chosen {
			source: source::Source::Odis {
				dir: PathBuf::from("/x/SK37X"),
			},
			names: Some(source::Source::Vcds { dir: vcds.clone() }),
			project: Some(crate::project::Project {
				id: "SK37X".to_string(),
				dir: here.path().join("project"),
			}),
			vcds: crate::project::vcds_at(&here.path().join("vcds-store")),
			odis: None,
			landing: source::Landing::New,
		};
		assert_eq!(
			this_run_reads(&chosen),
			[
				(vag_data_db::ODIS, "/x/SK37X".to_string()),
				(vag_data_db::VCDS, vcds.join("Labels").display().to_string())
			]
		);
	}

	/// A throwaway directory tree, removed on drop. Tests must not write into a
	/// real `~/.vagcan`, so the copy is exercised against a tiny synthetic
	/// install rather than the 122 MB one.
	struct TempDir(PathBuf);

	impl TempDir {
		fn new(tag: &str) -> TempDir {
			let path = std::env::temp_dir().join(format!("vagcan-setup-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
			let _ = std::fs::remove_dir_all(&path);
			std::fs::create_dir_all(&path).unwrap();
			TempDir(path)
		}
		fn write(&self, rel: &str, bytes: &[u8]) -> PathBuf {
			let path = self.0.join(rel);
			std::fs::create_dir_all(path.parent().unwrap()).unwrap();
			std::fs::write(&path, bytes).unwrap();
			path
		}
	}
	impl Drop for TempDir {
		fn drop(&mut self) {
			let _ = std::fs::remove_dir_all(&self.0);
		}
	}

	/// A stand-in for a VCDS install: one of each thing the copy must carry.
	fn synthetic_install(tag: &str) -> TempDir {
		let install = TempDir::new(tag);
		install.write("UDS_EV/RD.rod", b"registry");
		install.write("UDS_EV/EV_ECM.rod", b"a unit's own file");
		install.write("Labels/part.lbl", b"001,1,Engine Speed,,");
		install.write("Codes.dat", b"texts");
		install
	}

	fn detail(step: &Step) -> String {
		match step.state {
			State::Wrote | State::Current => step.detail.clone(),
			_ => panic!("expected a written step, got {step:?}"),
		}
	}

	#[test]
	fn the_copy_carries_every_rod_and_the_fault_text_flat() {
		// Fault naming and ODX lookup search by *name* and never by path, so
		// the directory a `.rod` sat in was never load-bearing — flattening
		// loses nothing and makes one pool shareable across every project.
		// The `.lbl`/`.clb` are not copied at all (D4): they are read once,
		// into `cache.sqlite`, and that cache is what survives of them.
		let install = synthetic_install("layout");
		let target = TempDir::new("layout-out");
		let step = copy_label_files(&install.0, &target.0, false).unwrap().0;
		assert!(detail(&step).starts_with("2 .rod files and the fault text (30 B)"), "{}", detail(&step));
		assert_eq!(step.path.as_deref(), Some(target.0.as_path()), "the pool is named at -v");
		for name in ["RD.rod", "EV_ECM.rod", "Codes.dat"] {
			assert!(target.0.join(name).is_file(), "{name} did not land in the pool");
		}
		assert!(!target.0.join("Labels").exists(), "the label files were copied after all");
		assert!(!target.0.join("UDS_EV").exists(), "the pool is flat");
	}

	#[test]
	fn copying_is_idempotent_and_freshness_gated_per_file() {
		// The rule the rest of setup follows: a second run has nothing to do,
		// one changed file recopies only itself, and --refresh copies the lot.
		let install = synthetic_install("fresh");
		let target = TempDir::new("fresh-out");

		let first = copy_label_files(&install.0, &target.0, false).unwrap().0;
		assert_eq!(first.state, State::Wrote);
		assert_eq!(detail(&first), "2 .rod files and the fault text (30 B)");

		let second = copy_label_files(&install.0, &target.0, false).unwrap().0;
		assert_eq!(second.state, State::Current, "a no-op rerun");
		assert_eq!(detail(&second), "2 .rod files and the fault text, all up to date");

		// One destination made to look stale: only it is copied again.
		let stale = target.0.join("Codes.dat");
		let past = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
		std::fs::File::options().write(true).open(&stale).unwrap().set_modified(past).unwrap();
		let third = copy_label_files(&install.0, &target.0, false).unwrap().0;
		assert_eq!(detail(&third), "1 of 3 files (5 B), the rest all up to date");

		// --refresh copies everything regardless of mtimes.
		let forced = copy_label_files(&install.0, &target.0, true).unwrap().0;
		assert_eq!(detail(&forced), "2 .rod files and the fault text (30 B)");
	}

	#[test]
	fn a_size_reads_in_the_unit_a_person_uses() {
		assert_eq!(size(512), "512 B");
		assert_eq!(size(14_200), "14.2 KB");
		assert_eq!(size(122_400_000), "122.4 MB");
		assert_eq!(size(1_200_000_000), "1.2 GB");
	}

	#[test]
	fn one_name_in_two_directories_is_copied_once_from_the_newer_file() {
		// The copy runs on several threads, and two of them writing one name would
		// share its staging file; APFS ignores case, so `ev_a.rod` is that name too.
		let install = TempDir::new("twice");
		install.write("UDS_EV/a/EV_A.rod", b"older");
		install.write("UDS_EV/b/ev_a.rod", b"newer");
		let past = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
		let older = install.0.join("UDS_EV/a/EV_A.rod");
		std::fs::File::options().write(true).open(&older).unwrap().set_modified(past).unwrap();
		let target = TempDir::new("twice-out");
		let step = copy_label_files(&install.0, &target.0, false).unwrap().0;
		assert_eq!(detail(&step), "1 .rod file (5 B)");
		let landed: Vec<_> = std::fs::read_dir(&target.0).unwrap().flatten().map(|e| e.path()).collect();
		assert_eq!(landed.len(), 1, "{landed:?}");
		assert_eq!(std::fs::read(&landed[0]).unwrap(), b"newer");
	}

	#[test]
	fn an_install_missing_every_input_is_reported_not_a_crash() {
		let empty = TempDir::new("empty");
		let target = TempDir::new("empty-out");
		let step = copy_label_files(&empty.0, &target.0, false).unwrap().0;
		assert_eq!(step.state, State::Missing, "{step:?}");
		assert!(step.detail.contains(ODX_DIR), "{step:?}");
		assert!(
			step.why.as_deref().is_some_and(|why| why.contains(&empty.0.display().to_string())),
			"{step:?}"
		);
	}

	#[test]
	fn the_fault_text_is_located_once_and_handed_to_the_next_step() {
		// `locate` opens a picker when the file is under a name nobody here
		// has seen, so looking for it again in step [2/5] asked the same
		// question twice in one run. Step [1/5] hands over what it found.
		let install = synthetic_install("once");
		let target = TempDir::new("once-out");
		let (_, codes) = copy_label_files(&install.0, &target.0, false).unwrap();
		assert_eq!(codes, Some(install.0.join("Codes.dat")));
	}

	#[test]
	fn a_variant_that_will_not_read_is_counted_and_the_two_halves_are_counted_apart() {
		// The fault table and the measurement chain are read separately and
		// fail separately; swallowing one half's errors made a project whose
		// fault tables were all refused look like a project with no faults.
		let mut skipped = Skipped::default();
		skipped.count(&vag_data_labels::odis::Error::Refused("MCD_ACCESS_KEY"));
		skipped.count(&vag_data_labels::odis::Error::Format("truncated".into()));
		skipped.count(&vag_data_labels::odis::Error::Missing("no such pool".into()));
		assert_eq!(skipped.note(), ", 1 refused, 2 unreadable");
		assert_eq!(Skipped::default().note(), "", "nothing skipped is nothing to say");
	}

	#[test]
	fn the_closing_fault_line_asks_the_project_this_run_wrote() {
		// The defect: it asked `OdisFaults::open()`, which resolves the *current*
		// project — `--project`, `VAGCAN_PROJECT`, config.toml — not the one this
		// run just wrote. Setting up a second car while config.toml still names
		// the first answered for the first. Two projects that differ only in
		// whether their caches hold fault rows must get different answers.
		let pool = TempDir::new("faultline-pool");
		let with = TempDir::new("faultline-with");
		let without = TempDir::new("faultline-without");
		let project = |dir: &TempDir| crate::project::Project {
			id: "SK37X".into(),
			dir: dir.0.clone(),
		};
		let stores = |dir: &TempDir| crate::project::Stores {
			project: Some(project(dir)),
			vcds: crate::project::vcds_at(&dir.0.join("no-vcds")),
		};
		let fault = vag_data_labels::odis::Fault {
			dop: "DTCDOP_VAGUDS".into(),
			code: 297,
			display_code: Some("B1168F2".into()),
			text: Some("Lenkwinkelsensor".into()),
			text_id: None,
			short_name: None,
			level: 2,
			temporary: false,
		};
		vag_data_db::put_all_faults(&project(&with).cache(), "/x/SK37X", [("EV_Brake", std::slice::from_ref(&fault))]).unwrap();
		vag_data_db::put_all_faults(&project(&without).cache(), "/x/SK37X", []).unwrap();

		assert!(
			fault_text_available(&pool.0, vag_cli_core::source::Source::Odis, &stores(&with)).unwrap(),
			"a project whose cache holds fault text was reported as having none"
		);
		assert!(
			!fault_text_available(&pool.0, vag_cli_core::source::Source::Odis, &stores(&without)).unwrap(),
			"a project with no fault text was reported as having some"
		);
		// No cache at all is no fault text, not an error.
		let empty = TempDir::new("faultline-empty");
		assert!(!fault_text_available(&pool.0, vag_cli_core::source::Source::Odis, &stores(&empty)).unwrap());
	}

	#[test]
	fn a_missing_output_is_never_mistaken_for_a_current_one() {
		let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
		assert!(!is_newer(Path::new("/definitely/not/here"), &here));
		assert!(!is_newer(&here, Path::new("/definitely/not/here")));
		assert!(is_newer(&here, &here), "a file is not older than itself");
	}

	#[test]
	fn a_relative_source_of_a_removed_cache_is_never_looked_for_from_here() {
		// Review of #37: `src` is a folder relative to where the tests run, as
		// `vendor/vcds-en` is in a checkout — and that is exactly why it must not be
		// offered as `setup --refresh src`.
		assert!(Path::new("src").is_dir(), "sanity: `src` resolves from the test's directory");
		for kind in [vag_data_db::ODIS, vag_data_db::VCDS] {
			let line = read_again(kind, "src");
			assert!(!line.contains("--refresh"), "{kind}: {line}");
			assert!(line.contains("relative path"), "{kind}: {line}");
		}
		// An absolute one is still looked for.
		let here = std::fs::canonicalize("src").unwrap().display().to_string();
		assert!(read_again(vag_data_db::ODIS, &here).contains("vagcan setup --refresh"));
	}
}
