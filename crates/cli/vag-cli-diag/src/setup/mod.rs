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
//! Both land in one project under `~/.vagcan/data/<id>/`
//! ([`crate::project`]), and a second source is **added** to a project rather
//! than replacing what is in it (design §5). What each branch writes:
//!
//! | source | what it gives | where it lands |
//! |---|---|---|
//! | VCDS | the `.rod` files and the fault text, raw | `~/.vagcan/rod/`, shared |
//! | VCDS | the label files, parsed | `data/<id>/cache.sqlite` |
//! | VCDS | every name in `TTTEXT.ROD`, by text id | `data/<id>/names.json` |
//! | VCDS | the `IDE`/`MAS` id each of those texts names | `data/<id>/odx-ids.json` |
//! | VCDS | `.rod` section keys | `data/<id>/rod-keys.json` |
//! | ODIS | every variant's channels, by identifier, **with scalings** | `data/<id>/cache.sqlite` |
//! | ODIS | every `(text id, name)` pair in the project | `data/<id>/names-odis.json` |
//!
//! The copy is what lets every car command run without the installation: fault
//! naming reads `.rod` files straight off disk at run time, so those are taken
//! from the install. The `.lbl`/`.clb` files are **not** copied (D4) — they are
//! read once, here, into `cache.sqlite`, and that cache is what survives of
//! them. The consequence is D5, honoured in [`crate::labels::load_project`]: a
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
/// of them, a blocked section costs about a minute of every core, and which
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
	project: crate::project::Project,
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

	let existing = crate::project::list()?;
	let rereads = rereads(&source, &existing)?;
	let named = source::project_id(io, crate::ui::can_ask(), &source, &existing)?;
	// Opened before a directory exists, so its own name can win.
	let odis = match &source {
		source::Source::Odis { dir } => Some(open_odis(dir)?),
		_ => None,
	};
	let id = match &odis {
		Some(project) => prefer_its_own_name(project.id(), &named.id),
		None => named.id.clone(),
	};
	let project = crate::project::open_or_create(&id)?;
	let renamed = (id != named.id).then_some(named.id.as_str());
	let (level, line) = project_line(&project, named.unasked, renamed);
	vag_cli_core::verbosity::say(level, &line);
	Ok(Some(Chosen {
		landing: source::landing(&id, &existing, &rereads),
		source,
		names,
		project,
		odis,
	}))
}

/// The run's one `project` line: the name, how it came to be that name when that
/// is not the plain answer, and the directory last.
///
/// Level 0 when the name was taken with nobody asked — a directory created under a
/// name that appears nowhere is found later with no idea how it got there — and
/// `-v` otherwise, where the closing summary has named the project already.
fn project_line(project: &crate::project::Project, unasked: bool, renamed: Option<&str>) -> (u8, String) {
	let how = match (unasked, renamed) {
		(true, _) => " (no terminal: default taken)".to_string(),
		(false, Some(folder)) => format!(" (its index.xml; the folder is `{folder}`)"),
		(false, None) => String::new(),
	};
	let level = if unasked { 0 } else { vag_cli_core::verbosity::NOTES };
	(level, format!("project  {}{how}  {}", project.id, project.dir.display()))
}

/// The projects in `existing` that have read `source` before.
fn rereads(source: &source::Source, existing: &[String]) -> Result<Vec<String>> {
	let (kind, dir) = match source {
		source::Source::Odis { dir } => (vag_data_db::ODIS, dir),
		source::Source::Vcds { dir } => (vag_data_db::VCDS, dir),
		source::Source::DownloadVcds => return Ok(Vec::new()),
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
	vag_cli_core::wire!("{}", timing(stage, started.elapsed()));
	out
}

/// Run one step, and keep how long it took on the step itself.
fn timed_step(work: impl FnOnce() -> Result<Step>) -> Result<Step> {
	let started = std::time::Instant::now();
	let mut step = work()?;
	step.took = Some(started.elapsed());
	Ok(step)
}

/// Seconds as a step's `-v` line gives them: `14.4 s`.
fn seconds(took: std::time::Duration) -> String {
	format!("{:.1} s", took.as_secs_f64())
}

/// A `timing` line: the stage, then seconds.
fn timing(stage: &str, took: std::time::Duration) -> String {
	format!("timing  {stage}  {:.2}s", took.as_secs_f64())
}

/// Open an ODIS project and say at `-v` which one it turned out to be.
fn open_odis(dir: &Path) -> Result<vag_data_labels::odis::Project> {
	let started = std::time::Instant::now();
	let project = vag_data_labels::odis::Project::open(dir).with_context(|| format!("reading the ODIS project at {}", dir.display()))?;
	vag_cli_core::note!("{}", opened_odis(dir, started.elapsed(), project.pools().len(), project.version()));
	Ok(project)
}

/// The `-v` line for an opened ODIS project: how long its string pools took, what
/// it is, and where.
fn opened_odis(dir: &Path, took: std::time::Duration, pools: usize, version: Option<&str>) -> String {
	format!(
		"odis  {}  {pools} {}, version {}  {}",
		seconds(took),
		plural(pools, "pool"),
		version.unwrap_or("unknown"),
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
		vag_cli_core::note!(
			"vcds copy: {} held the build that ships {}, this one ships {}: cleared first",
			target.display(),
			here.unwrap_or("?"),
			incoming.unwrap_or("?")
		);
	}
	std::fs::remove_dir_all(target).with_context(|| format!("clearing {}", target.display()))?;
	Ok(if mismatch { here } else { None })
}

/// How a step ended: the `state` column of the closing table.
///
/// A step that had nothing to do is worth a row as much as one that ran: somebody
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
	fn word(self) -> &'static str {
		match self {
			State::Wrote => "wrote",
			State::Current => "current",
			State::Partial => "partial",
			State::Missing => "missing",
			State::Pending => "pending",
			State::Failed => "failed",
		}
	}

	/// Whether the run is short of something because of it.
	fn gap(self) -> bool {
		matches!(self, State::Partial | State::Missing | State::Failed)
	}
}

/// What one step of the run did: a row of the closing table, and what `-v` adds.
#[derive(Debug)]
struct Step {
	/// Which source the step read: [`vag_data_db::VCDS`] or [`vag_data_db::ODIS`].
	source: &'static str,
	/// The step, in a word: `copy`, `labels`, `names`, `keys`, `registry`, `channels`, `faults`.
	name: &'static str,
	state: State,
	/// The row's last column, one line: the counts, or a gap's reason in a few words.
	detail: String,
	/// The whole reason, at `-v`, for a step short of something.
	why: Option<String>,
	/// What it wrote into, at `-v`.
	path: Option<PathBuf>,
	/// How long it took, at `-v`, on the same line as its path.
	took: Option<std::time::Duration>,
	/// What to type about it, at level 0 after the table: a `Fix: …` line, for a
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
pub const WITHOUT_A_TERMINAL: &str = "vagcan setup /path/to/ODIS-project      (or the path to a VCDS installation)";

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
	run_with(&mut io, &mut source::native_folder, opts, Data::Stdout).await
}

/// [`run`] inside another command — [`crate::rescue`]'s, which fetches label data
/// for a command that stopped short of it and then runs that command again. The
/// closing table goes to stderr there: stdout is the data of the command asked
/// for, and `vagcan faults > out.txt` must hold the faults and nothing of `setup`.
pub(crate) async fn run_inside(opts: Options<'_>) -> Result<()> {
	let mut io = crate::ui::Console::new(WITHOUT_A_TERMINAL);
	// On the main thread, for the reason [`run`] gives.
	run_with(&mut io, &mut source::native_folder, opts, Data::Stderr).await
}

/// Where the closing table goes: stdout when `setup` is the command, stderr when
/// it runs inside another one ([`run_inside`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Data {
	Stdout,
	Stderr,
}

/// The rule behind [`run`], with the asking behind [`crate::ui::menu::Asker`] and
/// [`source::Dialog`] so the flow is testable without a terminal — and without a
/// window, which CI has even less of.
async fn run_with(io: &mut impl crate::ui::menu::Asker, dialog: &mut impl source::Dialog, opts: Options<'_>, data: Data) -> Result<()> {
	let Some(mut chosen) = choose(io, dialog, &opts)? else { return Ok(()) };
	let project = &chosen.project;
	let reads = this_run_reads(&chosen);
	let removed = prepare_cache(&project.cache(), opts.refresh, &reads)?;
	if let Some(said) = &removed {
		// Its first line, as it happens; in full at the close, where it is read.
		vag_cli_core::note!("{}", said.lines().next().unwrap_or_default());
	}

	// **The pair's order: the VCDS installation's files, the ODIS project, then
	// the VCDS registry.** Step 2 trusts its label cache while the cache file is
	// newer than the label files, and the ODIS read writes that file — read
	// after it, an installation updated in place would never be read again. The
	// registry step asks whether this run's ODIS rows describe a unit the
	// installation has nothing for, so it comes after them. The names do not
	// care: the text table's read replaces `names.json`, `read_odis` merges into
	// `names-odis.json`.
	let beside = match &chosen.names {
		Some(source::Source::Vcds { dir }) => Some(dir),
		_ => None,
	};
	let mut steps = Vec::new();
	if let Some(dir) = beside {
		let (dir, project, refresh) = (dir.clone(), project.clone(), opts.refresh);
		steps.extend(off_the_main_task(move || read_vcds_files(&dir, &project, refresh)).await?);
	}
	steps.extend(match &chosen.source {
		source::Source::Odis { dir } => {
			let odis = chosen.odis.take().expect("an ODIS source opens its project in `choose`");
			let (dir, project) = (dir.clone(), project.clone());
			off_the_main_task(move || read_odis(&odis, &dir, &project)).await?
		}
		source::Source::Vcds { dir } => read_vcds(dir, project, opts.refresh).await?,
		// `choose` turns a download into the installation it fetched.
		source::Source::DownloadVcds => unreachable!("the download is resolved to an installation before this point"),
	});
	if let Some(dir) = beside {
		steps.push(read_vcds_registry(dir, project).await?);
	}

	// Written down so a later command needs no flag. Not a preference — the
	// answer to "which car did I just set up", which is the one a bare
	// `vagcan faults` has to be able to reach.
	crate::project::remember(&project.id)?;
	// Asked of the store, not of which branch just ran: a VCDS run into a
	// project that already holds ODIS rows must not close by telling its reader
	// there are no scalings anywhere.
	let scalings = !crate::extracted::open(project).is_empty();
	let fault_labels = fault_text_available(&crate::project::rod_pool()?, project)?;
	let closing = Closing {
		project: &project.id,
		landing: chosen.landing,
		scalings,
		fault_labels,
	};
	let report = report(&steps, &closing);
	for note in &report.notes {
		vag_cli_core::verbosity::say(vag_cli_core::verbosity::NOTES, note);
	}
	match data {
		Data::Stdout => vag_cli_core::out!("{}", report.data),
		Data::Stderr => {
			for line in report.data.lines() {
				vag_cli_core::say!("{line}");
			}
		}
	}
	for (level, line) in &report.after {
		vag_cli_core::verbosity::say(*level, line);
	}
	// Last, where it is read: a removed cache leaves sources to read again.
	if let Some(said) = removed {
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

/// Whether `vagcan faults` will be able to name a code once this run is done:
/// VCDS's fault labels in the shared pool, or fault rows in **this** project's
/// cache.
///
/// Asked of the project this run wrote, not of `OdisFaults::open`, which
/// resolves the *current* project (`--project`, `VAGCAN_PROJECT`, config.toml)
/// and answered for the first car while a second one was being set up. And an
/// error is an error: a cache this run has just written that will not answer
/// is not "no fault text". Only a cache that is not there at all is.
fn fault_text_available(pool: &Path, project: &crate::project::Project) -> Result<bool> {
	if crate::faultnames::has_fault_labels(pool) {
		return Ok(true);
	}
	let cache = project.cache();
	if !cache.is_file() {
		return Ok(false);
	}
	let (_, codes) = vag_data_db::fault_counts(&cache).map_err(|e| anyhow::anyhow!("counting the fault texts in {}: {e}", cache.display()))?;
	Ok(codes > 0)
}

/// The VCDS branch: its five steps, into a project.
async fn read_vcds(root: &Path, project: &crate::project::Project, refresh: bool) -> Result<Vec<Step>> {
	let (dir, project_, refresh_) = (root.to_path_buf(), project.clone(), refresh);
	let mut steps = off_the_main_task(move || read_vcds_files(&dir, &project_, refresh_)).await?;
	steps.push(read_vcds_registry(root, project).await?);
	Ok(steps)
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
/// names, the keys. Apart from step 5 so the pair can read the ODIS project
/// between them (see [`run_with`]).
fn read_vcds_files(root: &Path, project: &crate::project::Project, refresh: bool) -> Result<Vec<Step>> {
	let pool = crate::project::rod_pool()?;
	let replaced = replace_if_another_build(root, &pool, refresh)?;
	std::fs::create_dir_all(&pool).with_context(|| format!("creating {}", pool.display()))?;
	vag_cli_core::note!("vcds  {}", root.display());

	// The copy runs first and the derivations then read from it, so afterwards
	// `~/.vagcan` is the one set of raw files the car commands read. The
	// installation is still kept: the channels of a unit met later are read
	// from it (step 5, and `registry::ensure` at the car).
	// The fault text is looked for once, in step [1/5], and handed on: finding
	// it can mean asking the person which file it is, and one run must ask
	// that once.
	let started = std::time::Instant::now();
	let (mut copied, codes) = copy_label_files(root, &pool, refresh)?;
	copied.took = Some(started.elapsed());
	if let Some(build) = replaced {
		copied.detail.push_str(&format!("; replaced the {build} build"));
	}
	Ok(vec![
		copied,
		timed_step(|| label_cache(root, codes.as_deref(), project, refresh))?,
		timed_step(|| names(&pool, project))?,
		timed_step(|| rod_keys(&pool, project))?,
	])
}

/// Step 5, and the installation recorded as one of the project's sources.
///
/// Async for one reason: the read is minutes of CPU, and made on the main task it
/// would sit between Ctrl-C and the `select!` that acts on it (see
/// [`registry::registry_channels`]). Nothing else in `setup` moves off the main
/// thread — the folder panel needs it (see [`run`]).
async fn read_vcds_registry(root: &Path, project: &crate::project::Project) -> Result<Step> {
	// The records of the cars on this machine, before anything is announced:
	// one that does not read is a step that did not complete, never an abort
	// with the steps before it unreported. The installation is still written
	// down when it is another than the one logged — this run's, so that the
	// next `setup`, or the first command with a car, reads from the right one
	// — and what was tried with the same one is kept: nothing was read here,
	// and a log wiped clean made every unit count as unread (found in review).
	let started = std::time::Instant::now();
	let mut step = match registry::recorded_units() {
		Ok(units) => registry::registry_channels(root, project, &units).await?,
		Err(e) => {
			crate::registry::note_installation(project, root)?;
			registry::failed(&e, root)
		}
	};
	step.took = Some(started.elapsed());
	// From the installation itself, not the pool: see `registry`.
	crate::project::record_source(
		project,
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
					let finished = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
					reporter.set(&format!("{finished} of {} — {}", variants.len(), variant.name));
					(faults, readings)
				})
				.collect()
		})
	};
	let (mut with_channels, mut with_faults) = (0usize, 0usize);
	// One tally per half, for the reason above.
	let (mut chain_skipped, mut fault_skipped) = (Skipped::default(), Skipped::default());
	let mut channel_rows: Vec<(&str, &[vag_data_labels::odis::Reading])> = Vec::new();
	let mut fault_rows: Vec<(&str, &[vag_data_labels::odis::Fault])> = Vec::new();
	for (variant, (faults, readings)) in variants.iter().zip(&walked) {
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
	let faults_took = faults_started.elapsed();
	let channels = timed("odis readings (sqlite)", || {
		vag_data_db::put_all_readings(&project.cache(), &source, channel_rows)
	})
	.map_err(|e| anyhow::anyhow!("writing the channels to {}: {e}", project.cache().display()))?;
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
	units.took = Some(started.elapsed().saturating_sub(faults_took));
	let mut faults = Step::new(
		vag_data_db::ODIS,
		"faults",
		State::Wrote,
		format!(
			"{codes} {}, {with_faults} {}{}",
			plural(codes, "code"),
			plural(with_faults, "variant"),
			fault_skipped.note()
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
		Ok(Step::new(vag_data_db::ODIS, "names", State::Wrote, format!("{merged} {}", plural(merged, "name"))).at(project.odis_names()))
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
	Ok(vec![units, faults, names])
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
/// — is D5, and `labels::load_project` is where it is honoured.
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
		vag_cli_core::note!("vcds copy: {stale} staging {} of a copy cut short removed", plural(stale, "file"));
	}
	let mut plan: Vec<(PathBuf, PathBuf)> = Vec::new();
	let odx = root.join(ODX_DIR);
	match odx.is_dir() {
		true => collect_rod_files(&odx, target, &mut plan)?,
		// A stripped or partial installation still yields whatever it has; a
		// missing input is reported, not fatal.
		false => vag_cli_core::note!("vcds copy: no {ODX_DIR}/ in this installation, skipped"),
	}
	// The fault text, under whichever name this language build gives it. Copied
	// under that same name: `faultnames` looks for the whole list too, so the
	// build stays recognisable rather than being flattened to the English one.
	let found = locate(root, CODES_FILES, "fault text file", ".dat")?;
	match &found {
		Some(codes) => {
			vag_cli_core::wire!("vcds copy: the fault text is {}", codes.display());
			let name = codes.file_name().unwrap_or_default();
			plan.push((codes.clone(), target.join(name)));
		}
		None => vag_cli_core::note!("vcds copy: no fault text file in this installation, skipped: faults read as numbers"),
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
			vag_cli_core::wire!("vcds copy: {} → {}", src.display(), dst.display());
			crate::datadir::copy_file(src, dst)?;
			bytes.fetch_add(std::fs::metadata(dst).map_or(0, |m| m.len()), Ordering::Relaxed);
			copied.fetch_add(1, Ordering::Relaxed);
			Ok(())
		})
	})?;
	progress.finish();
	let (copied, skipped, bytes) = (copied.into_inner(), skipped.into_inner(), bytes.into_inner());
	let (state, detail) = match copied {
		0 => (State::Current, format!("{total} {}, all current", plural(total, "file"))),
		_ => (
			State::Wrote,
			format!("{total} {}, {copied} copied ({}), {skipped} current", plural(total, "file"), size(bytes)),
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
	let db = crate::labels::load_cached(root, &project.cache(), refresh)?;
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
	Ok(
		Step::new(
			vag_data_db::VCDS,
			"labels",
			State::Wrote,
			format!("{} {}", db.len(), plural(db.len(), "label file")),
		)
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
	vag_cli_core::wire!("vcds names: the text table is {}", source.display());
	// How long it takes is the spinner's job to say, and it says it in elapsed
	// seconds rather than in a sentence nobody can act on.
	// Ask what opening it would cost before starting, because the two cases look
	// identical while running. A shifted text table has no anchor, so the only
	// route is sixty full-space searches — hours to days — and a spinner that
	// will not stop today is indistinguishable from one that stops in ninety
	// seconds. The Russian build ships exactly such a table.
	let name = source.file_name().unwrap_or_default().to_string_lossy().into_owned();
	let keyed = keyed_already(&source, project)?;
	vag_cli_core::wire!("vcds names: its key is {}", if keyed { "cached" } else { "not cached" });
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
	let detail = format!("{count} {}, {with_ids} naming an IDE or MAS id", plural(count, "name"));
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
	let state = match names_changed || ids_changed {
		true => State::Wrote,
		false => State::Current,
	};
	Ok(Step::new(vag_data_db::VCDS, "names", state, detail).at(out))
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
	let mut shut = Vec::new();
	for file in &present {
		for section in crate::vcds::rod::open(
			&file.to_string_lossy(),
			vag_data_labels::rod::Search::Classic,
			Some(&cache.to_string_lossy()),
			None,
		)? {
			let why = crate::vcds::rod::why_shut(&section);
			vag_cli_core::wire!("vcds keys: [{}] in {}: {}", section.tag, file.display(), why.unwrap_or("open"));
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
		(0, true) => "every section opened by its file name, nothing to cache".to_string(),
		(n, _) => format!("{n} {}", plural(n, "key")),
	};
	Ok(match shut.is_empty() {
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
const READ_AN_ENGLISH_VCDS: &str = "Fix: `vagcan setup` with no path, then \"Download VCDS\" (the English build; its names open)";

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
const NEXT_WITH_THE_CAR: &str = "read the first time a car command meets the car";

/// What the closing line says beside the steps.
struct Closing<'a> {
	/// The project the run wrote into.
	project: &'a str,
	/// What landing on it meant for the source read.
	landing: source::Landing,
	/// Whether the project now holds per-variant scalings — asked of the store
	/// rather than of which branch ran, so a VCDS run into a project that already
	/// has them does not tell its reader they have none.
	scalings: bool,
	/// Whether `vagcan faults` will name a code: VCDS's fault labels, or the fault
	/// text an ODIS project carries.
	fault_labels: bool,
}

/// The closing report, as [`run_with`] writes it.
#[derive(Debug, Default)]
struct Report {
	/// `-v`, before the table: what each step wrote into, and every gap's reason.
	notes: Vec<String>,
	/// The table and its summary line: the run's data.
	data: String,
	/// After the table, at their level: what to do next.
	after: Vec<(u8, String)>,
}

/// The closing report: one row per step, one summary line, and at `-v` the paths
/// and the reasons.
///
/// Every row is one line with its counts. Somebody who has just waited several
/// minutes is owed what each step came to, and somebody whose run was short of
/// one artefact needs to see which one without re-reading the scroll — the
/// `state` column, and `gaps` in the summary.
fn report(steps: &[Step], closing: &Closing<'_>) -> Report {
	let mut table = vag_cli_core::table::Table::new(&["source", "step", "state", "detail"]);
	let mut notes = Vec::new();
	for step in steps {
		table.row(vec![
			step.source.to_string(),
			step.name.to_string(),
			step.state.word().to_string(),
			step.detail.clone(),
		]);
		if let Some(note) = step_note(step) {
			notes.push(note);
		}
	}
	let gaps = steps.iter().filter(|s| s.state.gap()).count();
	let landing = match closing.landing {
		source::Landing::New => "new project",
		source::Landing::Adds => "source added",
		source::Landing::Rereads => "source read again",
	};
	let summary = format!(
		"{}: {landing} · {} {} · {gaps} {} · {} · {}",
		closing.project,
		steps.len(),
		plural(steps.len(), "step"),
		plural(gaps, "gap"),
		if closing.fault_labels { "fault text" } else { "no fault text" },
		if closing.scalings { "scalings" } else { "no scalings" },
	);
	let mut data = String::new();
	if !table.is_empty() {
		data.push_str(&table.render(vag_cli_core::table::terminal_width()));
		data.push('\n');
	}
	data.push_str(&summary);
	data.push('\n');

	let mut after = Vec::new();
	let pending = steps.iter().any(|s| s.state == State::Pending);
	// Where to go from here, at `-v`. The fault line promises names only when
	// `vagcan faults` will print them: it once announced labels copied in after an
	// ODIS-only run that copied nothing.
	let faults = match closing.fault_labels {
		true => "vagcan faults (stored faults, named)",
		false => "vagcan faults (stored faults, as numbers)",
	};
	after.push((vag_cli_core::verbosity::NOTES, format!("next: vagcan devices, vagcan info, {faults}")));
	// Said only where it is true. An ODIS project declares a scaling per variant,
	// so "none yet" would be false there.
	if !closing.scalings {
		let scalings = if pending { SCALINGS_WAIT_FOR_THE_CAR } else { SCALINGS_ARE_MEASURED };
		after.push((vag_cli_core::verbosity::NOTES, scalings.to_string()));
	}
	// A step whose remedy is a command closes on it, at every level: one `Fix:` line each.
	for fix in steps.iter().filter_map(|s| s.fix.as_ref()) {
		if !after.iter().any(|(_, line)| line == fix) {
			after.push((0, fix.clone()));
		}
	}
	Report { notes, data, after }
}

/// A step's one `-v` line: `vcds labels  14.4 s  <path>` — its time, its reason
/// when it fell short, and what it wrote into last. `None` when there is none of
/// the three to say.
fn step_note(step: &Step) -> Option<String> {
	let parts: Vec<String> = [
		step.took.map(seconds),
		step.why.clone(),
		step.path.as_ref().map(|path| path.display().to_string()),
	]
	.into_iter()
	.flatten()
	.collect();
	match parts.is_empty() {
		true => None,
		false => Some(format!("{} {}  {}", step.source, step.name, parts.join("  "))),
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
	fn the_project_is_one_line_in_one_format() {
		// `-v` once said the project three times in three formats, one of them a
		// paragraph on what a project is. It is one `project  <name>  <dir>` line
		// now, with how the name came about between the two when that is not the
		// plain answer.
		let project = project_at("SK37X");
		assert_eq!(
			project_line(&project, false, None),
			(vag_cli_core::verbosity::NOTES, "project  SK37X  /home/x/.vagcan/data/SK37X".to_string())
		);
		// Filed under its index.xml's name: the folder it was not filed under is named.
		assert_eq!(
			project_line(&project, false, Some("SK-37X-copy")),
			(
				vag_cli_core::verbosity::NOTES,
				"project  SK37X (its index.xml; the folder is `SK-37X-copy`)  /home/x/.vagcan/data/SK37X".to_string()
			)
		);
		// With no terminal the default was taken unasked, and that is said at level 0:
		// a directory under a name that appears nowhere is a mystery found later.
		assert_eq!(
			project_line(&project, true, None),
			(0, "project  SK37X (no terminal: default taken)  /home/x/.vagcan/data/SK37X".to_string())
		);
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
			Data::Stdout,
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
		let files = at("steps.extend(off_the_main_task(move || read_vcds_files(&dir, &project, refresh)).await?);");
		let odis = at("off_the_main_task(move || read_odis(&odis, &dir, &project)).await?");
		let registry = at("steps.push(read_vcds_registry(dir, project).await?);");
		assert!(files < odis, "the ODIS read moved ahead of the label files' freshness check");
		assert!(odis < registry, "the registry step moved ahead of the ODIS rows it asks about");
	}

	#[test]
	fn what_setup_says_about_a_project_at_v_is_one_line_each() {
		// Caught three times on real terminals when this was wrapped prose. A
		// note is one line now, its path or name last, so its width is the
		// terminal's business and nothing is wrapped by hand.
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
		// The time beside what was opened, the path last.
		assert_eq!(
			opened_odis(long, took, 230, Some("2610.2.688")),
			format!("odis  14.4 s  230 pools, version 2610.2.688  {}", long.display())
		);
		assert_eq!(
			opened_odis(long, took, 1, None),
			format!("odis  14.4 s  1 pool, version unknown  {}", long.display())
		);
		// A stage inside a step, at -vv: the instrument the performance work used.
		assert_eq!(
			timing("odis names (walk)", std::time::Duration::from_millis(32_590)),
			"timing  odis names (walk)  32.59s"
		);
	}

	#[test]
	fn a_step_at_v_is_one_line_its_time_and_its_path_together() {
		// `-v` once gave a step's time on one line (`timing  vcds labels  14.40s`)
		// and its path on another (`vcds labels  <path>`). One line per step now.
		let mut step = Step::new(vag_data_db::VCDS, "labels", State::Wrote, "3035 label files").at("/x/cache.sqlite");
		step.took = Some(std::time::Duration::from_millis(14_420));
		assert_eq!(step_note(&step).as_deref(), Some("vcds labels  14.4 s  /x/cache.sqlite"));
		// A step short of something: its reason between the time and the path.
		let mut step = Step::new(vag_data_db::VCDS, "keys", State::Partial, "2 keys; 1 section shut")
			.because("[TXT] in RD.rod did not open")
			.at("/x/rod-keys.json");
		step.took = Some(std::time::Duration::from_millis(60));
		assert_eq!(
			step_note(&step).as_deref(),
			Some("vcds keys  0.1 s  [TXT] in RD.rod did not open  /x/rod-keys.json")
		);
		// Nothing to say is no line.
		assert_eq!(step_note(&Step::new(vag_data_db::VCDS, "keys", State::Wrote, "0 keys")), None);
		let r = report(&[step], &closing(true, true));
		assert_eq!(r.notes.len(), 1, "one line per step: {:?}", r.notes);
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
		assert_eq!(first.detail, "2 names, 1 naming an IDE or MAS id");
		assert!(std::fs::read_to_string(&ids).unwrap().contains("\"IDE90001\""));
		let again = written_names(&table, &source, &out, &ids).unwrap();
		assert_eq!(again.state, State::Current, "the same read rewrote the files: {again:?}");
		assert_eq!(again.detail, first.detail, "a current row still says what it holds");
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
		assert!(step.detail.starts_with("1 name,"), "one of a noun is singular: {}", step.detail);
		assert!(
			step.detail.ends_with("1 record left out"),
			"the row counts what it left out: {}",
			step.detail
		);
		let why = step.why.as_deref().unwrap_or_default();
		assert!(why.contains("000005"), "the record left out is named at -v: {why}");
		assert!(std::fs::read_to_string(&out).unwrap().contains("000017"));
	}

	/// What the closing line says beside the steps, for a report under test.
	fn closing(scalings: bool, fault_labels: bool) -> Closing<'static> {
		Closing {
			project: "SK37X",
			landing: source::Landing::New,
			scalings,
			fault_labels,
		}
	}

	/// What `-v` adds after the table, as one blob.
	fn after(r: &Report, level: u8) -> String {
		r.after
			.iter()
			.filter(|(at, _)| *at == level)
			.map(|(_, line)| line.as_str())
			.collect::<Vec<_>>()
			.join("\n")
	}

	#[test]
	fn the_report_is_one_row_per_step_and_one_summary_line() {
		// The level-0 shape, pinned: a table with one line per step — its source,
		// its name, how it ended and its counts — then the summary line.
		let steps = vec![
			Step::new(vag_data_db::VCDS, "copy", State::Wrote, "16577 files, 16577 copied (122.4 MB), 0 current").at("/x/rod"),
			Step::new(vag_data_db::VCDS, "labels", State::Wrote, "3035 label files").at("/x/cache.sqlite"),
			Step::new(vag_data_db::VCDS, "names", State::Current, "195910 names, 30521 naming an IDE or MAS id").at("/x/names.json"),
			Step::new(
				vag_data_db::ODIS,
				"channels",
				State::Wrote,
				"399283 channels, 669 of 717 variants, 0 refused, 4 unreadable",
			),
		];
		let r = report(&steps, &closing(true, true));
		assert_eq!(
			r.data,
			"source  step      state    detail\n\
			 vcds    copy      wrote    16577 files, 16577 copied (122.4 MB), 0 current\n\
			 vcds    labels    wrote    3035 label files\n\
			 vcds    names     current  195910 names, 30521 naming an IDE or MAS id\n\
			 odis    channels  wrote    399283 channels, 669 of 717 variants, 0 refused, 4 unreadable\n\
			 \n\
			 SK37X: new project · 4 steps · 0 gaps · fault text · scalings\n"
		);
	}

	#[test]
	fn the_report_names_every_file_it_wrote_at_v() {
		// Somebody who has just waited several minutes is owed the paths. They
		// are at `-v`; the row says what each step came to.
		let steps = vec![
			Step::new(vag_data_db::VCDS, "labels", State::Wrote, "3035 label files").at("/home/x/.vagcan/data/SK37X/cache.sqlite"),
			Step::new(vag_data_db::VCDS, "names", State::Current, "2 names, 1 naming an IDE or MAS id").at("/home/x/.vagcan/data/SK37X/names.json"),
		];
		let r = report(&steps, &closing(false, true));
		assert!(r.data.contains("3035 label files"), "{}", r.data);
		// A step with nothing to do is a row, not silently absent: a run that took
		// a second when minutes were expected reads as a failure otherwise.
		assert!(r.data.contains("vcds    names   current"), "{}", r.data);
		assert!(!r.data.contains("/home/x"), "paths are -v: {}", r.data);
		let notes = r.notes.join("\n");
		assert!(notes.contains("vcds labels  /home/x/.vagcan/data/SK37X/cache.sqlite"), "{notes}");
		assert!(notes.contains("vcds names  /home/x/.vagcan/data/SK37X/names.json"), "{notes}");
	}

	#[test]
	fn a_step_that_could_not_run_says_so_without_condemning_the_rest() {
		let steps = vec![
			Step::new(vag_data_db::VCDS, "labels", State::Wrote, "3035 label files"),
			Step::new(vag_data_db::VCDS, "keys", State::Missing, "none of RD.rod, MUX.rod").because("none of them is in this installation"),
		];
		let r = report(&steps, &closing(false, true));
		assert!(r.data.contains("vcds    keys    missing  none of RD.rod, MUX.rod\n"), "{}", r.data);
		assert!(r.data.contains("vcds    labels  wrote"), "the rest stands: {}", r.data);
		assert!(r.data.contains("· 1 gap ·"), "{}", r.data);
		assert!(
			r.notes.contains(&"vcds keys  none of them is in this installation".to_string()),
			"{:?}",
			r.notes
		);
	}

	#[test]
	fn a_text_table_only_vcds_opens_is_reported_as_its_cost_and_its_remedy() {
		// The Russian build's table: the report once spent seven lines on why
		// (containers, anchors, a research file). The row says what did not
		// open; `-v` what it costs the reader; and what to type is one `Fix:` line
		// at level 0 — it was at `-v` only, where a reader at level 0 never saw it.
		let steps = [
			Step::new(vag_data_db::VCDS, "names", State::Missing, "TTText-RUS.rod opens only inside VCDS")
				.because(NAMES_ONLY_IN_VCDS)
				.fix(READ_AN_ENGLISH_VCDS),
		];
		let r = report(&steps, &closing(false, true));
		assert!(
			r.data.contains("vcds    names  missing  TTText-RUS.rod opens only inside VCDS\n"),
			"{}",
			r.data
		);
		let notes = r.notes.join("\n");
		assert!(notes.contains("by reading id, not by name"), "{notes}");
		assert_eq!(
			after(&r, 0),
			"Fix: `vagcan setup` with no path, then \"Download VCDS\" (the English build; its names open)"
		);
		assert!(!notes.contains("Download VCDS"), "said once, at level 0: {notes}");
		for jargon in ["shifted", "anchor", ".archive", "masks"] {
			assert!(!r.data.contains(jargon) && !notes.contains(jargon), "{jargon}: {}", r.data);
		}
	}

	#[test]
	fn a_run_with_no_scalings_says_how_to_get_them() {
		// The closing lines are the last chance to say what a run did not bring.
		// It used to be "no VCDS installation carries them"; since 2026-09-28 one
		// does, through its registry, for the units of the cars this machine has
		// met — and the registry row says so itself, with the reason it has none.
		// The close names only what that row cannot: an ODIS project. The summary
		// says it at level 0, the remedy is `-v`.
		let r = report(&[], &closing(false, true));
		assert!(r.data.ends_with("· no scalings\n"), "{}", r.data);
		let v = after(&r, vag_cli_core::verbosity::NOTES);
		assert!(v.contains("scalings: none yet"), "{v}");
		assert!(v.contains("ODIS project"), "{v}");
		assert!(!v.contains("survey"), "{v}");
		assert!(!v.contains("no VCDS installation carries them"), "the old claim is false now: {v}");
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
		// With no car recorded there is no list of units to read. That is a step
		// still to take, not something the installation lacks: "with gaps" and "a
		// newer VCDS may have it" sent people to the wrong place.
		let steps = vec![Step::new(vag_data_db::VCDS, registry::NAME, State::Pending, NEXT_WITH_THE_CAR).because(registry::PENDING_WHY)];
		let r = report(&steps, &closing(false, true));
		assert!(r.data.contains("· 0 gaps ·"), "{}", r.data);
		assert!(!r.data.contains("newer VCDS"), "{}", r.data);
		// The row says when the step finishes: a VCDS-only reader with no car was
		// left to wonder, and the last word sent them to ODIS (found in review,
		// 2026-09-28). It asks for no command: the car commands read the channels
		// as a side effect, and a run made only to feed the tool is not asked for.
		assert!(
			r.data
				.contains("vcds    registry  pending  read the first time a car command meets the car\n"),
			"{}",
			r.data
		);
		assert!(after(&r, 0).is_empty(), "nothing to type at level 0: {:?}", r.after);
		assert!(
			r.notes.iter().any(|n| n.starts_with("vcds registry  no car recorded on this machine")),
			"{:?}",
			r.notes
		);
		assert!(!SCALINGS_WAIT_FOR_THE_CAR.contains("vagcan watch"), "{SCALINGS_WAIT_FOR_THE_CAR}");
		// And the scalings note waits for the car rather than for the other source.
		assert!(
			after(&r, vag_cli_core::verbosity::NOTES).ends_with(SCALINGS_WAIT_FOR_THE_CAR),
			"{:?}",
			r.after
		);
		// Without a pending step, the scalings note is the plain one.
		let r = report(&[], &closing(false, true));
		assert!(
			after(&r, vag_cli_core::verbosity::NOTES).ends_with(SCALINGS_ARE_MEASURED),
			"{:?}",
			r.after
		);
	}

	#[test]
	fn a_record_that_does_not_read_is_a_step_that_did_not_complete_and_the_report_still_comes() {
		// One unreadable `units.json` aborted `setup` at step 5 with no report,
		// after steps 1–4 had written their files (found in review, 2026-09-28).
		// It is a gap now: the row names the file, `-v` the cause, and the last
		// word at every level is what to do about it — naming that file and the
		// installation to read again, not "the file the failed row names".
		let bad = anyhow::Error::new(crate::units::NotARecord {
			path: PathBuf::from("/home/x/.vagcan/cars/BAD/units.json"),
			cause: "expected ident at line 1 column 2".into(),
		});
		let root = Path::new("/opt/VCDS 26.3");
		let steps = vec![
			Step::new(vag_data_db::VCDS, "labels", State::Wrote, "3035 label files"),
			registry::failed(&bad, root),
		];
		let r = report(&steps, &closing(false, true));
		assert!(
			r.data
				.contains("vcds    registry  failed  not a record this tool wrote: /home/x/.vagcan/cars/BAD/units.json\n"),
			"{}",
			r.data
		);
		assert!(r.data.contains("· 1 gap ·"), "{}", r.data);
		assert!(r.data.contains("3035 label files"), "the steps before it are still reported: {}", r.data);
		assert!(!r.data.contains("newer VCDS"), "not a gap in the installation: {}", r.data);
		assert!(
			r.notes.iter().any(|n| n == "vcds registry  expected ident at line 1 column 2"),
			"the cause is at -v: {:?}",
			r.notes
		);
		assert_eq!(
			after(&r, 0),
			"Fix: move /home/x/.vagcan/cars/BAD/units.json aside, then `vagcan setup '/opt/VCDS 26.3'`",
			"the last word is what to do, at level 0"
		);
		// An error that is not a record's — the directory would not read — has no file
		// of its own to name: the row carries the error, which does, and the fix says
		// what to make readable rather than pointing at a file the row does not name.
		let other = registry::failed(&anyhow::anyhow!("permission denied").context("reading /x/cars"), root);
		assert_eq!(other.state, State::Failed);
		assert_eq!(other.detail, "the cars' records did not read: reading /x/cars: permission denied");
		let fix = other.fix.as_deref().unwrap_or_default();
		assert!(fix.starts_with("Fix: make ") && fix.contains("readable"), "{fix}");
		assert!(fix.ends_with("then `vagcan setup '/opt/VCDS 26.3'`"), "{fix}");
		assert!(!fix.contains("row names"), "{fix}");
	}

	#[test]
	fn every_row_and_note_is_one_line_and_the_rows_keep_to_eighty_columns_with_short_paths() {
		// The old rule was eighty columns of hand-wrapped prose. A row now is one
		// line, cut to the terminal by the table; with short paths it still fits
		// the terminal to write for.
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
			let r = report(&steps, &closing(scalings, fault_labels));
			for line in r.data.lines() {
				assert!(line.chars().count() <= 80, "{} columns: {line:?}", line.chars().count());
			}
			for note in r.notes.iter().chain(r.after.iter().map(|(_, line)| line)) {
				assert!(!note.contains('\n'), "a note on two lines: {note:?}");
			}
		}
	}

	#[test]
	fn a_run_that_read_scalings_does_not_close_by_saying_there_are_none() {
		// The defect this footer split exists for. An ODIS run reported
		// "633 of 717 variants, 310734 channels" and then, two lines later,
		// that no installation carries scalings and they must be measured. A
		// reader who believes the footer goes and drives the car to establish
		// rows the tool already has.
		let r = report(&[], &closing(true, true));
		assert!(r.data.ends_with("· fault text · scalings\n"), "{}", r.data);
		let v = after(&r, vag_cli_core::verbosity::NOTES);
		assert!(!v.contains("scalings: none yet"), "{v}");
		assert!(!v.contains("carries scalings"), "the paragraph the owner dropped is gone: {v}");
	}

	#[test]
	fn a_run_that_copied_no_labels_does_not_claim_the_labels_are_copied_in() {
		// The defect: the fault line was unconditional, so an ODIS-only run —
		// which copies nothing — finished by announcing that the labels are in.
		// Naming a code still reads those files off disk, so the reader was told
		// the opposite of what they had.
		let without = report(&[], &closing(true, false));
		assert!(without.data.contains("· no fault text ·"), "{}", without.data);
		let v = after(&without, vag_cli_core::verbosity::NOTES);
		assert!(v.contains("as numbers"), "{v}");
		assert!(!v.contains("copied in"), "{v}");
		// And it must not teach the wrong lesson on the way past: neither
		// source is named as the only place fault text can come from.
		assert!(!v.contains("VCDS") && !without.data.contains("VCDS"), "{v}");

		let with = report(&[], &closing(true, true));
		assert!(with.data.contains("· fault text ·"), "{}", with.data);
		let v = after(&with, vag_cli_core::verbosity::NOTES);
		assert!(v.contains("stored faults, named"), "{v}");
		assert!(!v.contains("as numbers"), "{v}");
	}

	#[test]
	fn the_summary_says_what_landing_on_the_project_meant() {
		// Spec §5 adds a second source to a project rather than replacing it, and
		// somebody who believes they are starting fresh would otherwise find out
		// from the data: the merge is said at level 0, in the summary.
		for (landing, said) in [
			(source::Landing::New, "SK37X: new project ·"),
			(source::Landing::Adds, "SK37X: source added ·"),
			(source::Landing::Rereads, "SK37X: source read again ·"),
		] {
			let r = report(
				&[],
				&Closing {
					landing,
					..closing(true, true)
				},
			);
			assert!(r.data.starts_with(said), "{}", r.data);
		}
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
			Data::Stdout,
		)
		.await
		.unwrap_err();
		let text = err.to_string();
		assert!(text.contains(crate::missing::VCDS_DOWNLOAD), "{text}");
		assert!(text.contains("Labels/"), "{text}");
		// And the other way in, since it is the whole point of the argument
		// being optional.
		assert!(text.contains("offers to download"), "{text}");
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
			project: crate::project::Project {
				id: "SK37X".to_string(),
				dir: here.path().join("project"),
			},
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
		assert!(detail(&step).starts_with("3 files, 3 copied"), "{}", detail(&step));
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
		assert_eq!(detail(&first), "3 files, 3 copied (30 B), 0 current");

		let second = copy_label_files(&install.0, &target.0, false).unwrap().0;
		assert_eq!(second.state, State::Current, "a no-op rerun");
		assert_eq!(detail(&second), "3 files, all current");

		// One destination made to look stale: only it is copied again.
		let stale = target.0.join("Codes.dat");
		let past = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
		std::fs::File::options().write(true).open(&stale).unwrap().set_modified(past).unwrap();
		let third = copy_label_files(&install.0, &target.0, false).unwrap().0;
		assert_eq!(detail(&third), "3 files, 1 copied (5 B), 2 current");

		// --refresh copies everything regardless of mtimes.
		let forced = copy_label_files(&install.0, &target.0, true).unwrap().0;
		assert_eq!(detail(&forced), "3 files, 3 copied (30 B), 0 current");
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
		assert_eq!(detail(&step), "1 files, 1 copied (5 B), 0 current");
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
			fault_text_available(&pool.0, &project(&with)).unwrap(),
			"a project whose cache holds fault text was reported as having none"
		);
		assert!(
			!fault_text_available(&pool.0, &project(&without)).unwrap(),
			"a project with no fault text was reported as having some"
		);
		// No cache at all is no fault text, not an error.
		let empty = TempDir::new("faultline-empty");
		assert!(!fault_text_available(&pool.0, &project(&empty)).unwrap());
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
