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

mod registry;
pub mod source;
pub mod vendor;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

// The installation's layout — where the ODX files are, what the text table is
// called per language build, which code page it is in — is `core`'s now, because
// the registry read that needs it runs from `watch` and `measure` as well as
// from here. Same names, so every use below reads as it did.
use crate::registry::{ODX_DIR, TEXT_TABLES, text_page};

/// The fault text store, one file in the install root beside `Labels/`.
///
/// Same story as [`TEXT_TABLES`]: `Codes.dat` in the English build,
/// `Code-RUS.dat` in the Russian one.
pub(crate) const CODES_FILES: &[&str] = &["Codes.dat", "Code-RUS.dat"];

/// The language a VCDS build's fault-text file is in, as ISO 639-2, from its
/// name — the only place the build says. Recorded on the source the same way
/// an ODIS project's `<LANGUAGE>` is, so the two can be compared.
fn codes_language(file_name: &str) -> Option<&'static str> {
	match file_name {
		"Codes.dat" => Some("eng"),
		"Code-RUS.dat" => Some("rus"),
		_ => None,
	}
}

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
	/// The projects that were already on disk when this run started — read
	/// before anything was created, so [`Chosen::project`] is not in it.
	///
	/// Kept rather than re-read because two later decisions turn on it and both
	/// would get a different answer afterwards: whether the name this run
	/// landed on is a merge, and whether the old pre-project layout can be moved
	/// without asking whose car it is.
	existing: Vec<String>,
	/// The opened ODIS project, when that is what this is. Opened before the
	/// directory was created, and kept rather than reopened: it holds 88 MB of
	/// inflated string pools.
	odis: Option<vag_data_labels::odis::Project>,
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
	let asked = source::project_id(io, &source, &existing, &rereads)?;
	// Opened before a directory exists, so its own name can win.
	let odis = match &source {
		source::Source::Odis { dir } => Some(open_odis(io, dir)?),
		_ => None,
	};
	let id = match &odis {
		Some(project) => prefer_its_own_name(io, project.id(), &asked, &existing, &rereads)?,
		None => asked,
	};
	Ok(Some(Chosen {
		source,
		names,
		project: crate::project::open_or_create(&id)?,
		existing,
		odis,
	}))
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

/// An `s`, when there is more than one of something.
fn plural(n: usize) -> &'static str {
	match n {
		1 => "",
		_ => "s",
	}
}

/// Which project the layout from before projects existed should move into.
///
/// **Spec §6 asks this, and the reason is the one irreversible step in the
/// command.** The move is a move: `data/measured/` holds rows proven by driving
/// a car, spec §4.5's only data proven on the actual vehicle, and nothing but
/// another drive can recreate one. Every other file here is extracted from
/// somebody else's and can be extracted again.
///
/// The failure this prevents is not hypothetical, and it is the *first* run
/// after upgrading: `vagcan setup ~/Downloads/SK37X` for a second car, while
/// `data/measured/` still holds the first car's rows. Filing them into whichever
/// project this run happened to choose puts one car's proven data inside
/// another car's store — and while [`crate::migrate`] copies before it removes,
/// so nothing is destroyed, by the time the closing report says what moved it
/// has already moved.
///
/// **Asked only when the answer is genuinely open.** A machine with no projects
/// yet, being set up from a VCDS installation, has exactly one car in play and
/// one place the data can belong; a question there is a question with one
/// answer. It is asked when a project was already on disk, or when the source is
/// an ODIS project — which names a specific vehicle, and so may well not be the
/// vehicle the old data came from.
///
/// A run with no terminal takes the default without blocking
/// ([`crate::ui::menu::Asker::line`]), which is the project this run chose —
/// the behaviour there has always been, now stated rather than assumed.
fn migration_target(io: &mut impl crate::ui::menu::Asker, old: &crate::migrate::Old, chosen: &Chosen) -> Result<crate::project::Project> {
	let id = migration_target_id(io, old, chosen)?;
	match id == chosen.project.id {
		true => Ok(chosen.project.clone()),
		false => crate::project::open_or_create(&id),
	}
}

/// The name half of [`migration_target`], split out so the whole question — what
/// it says, what it defaults to, what it does with an answer that is not a
/// directory name — is testable without creating a directory in the owner's own
/// `~/.vagcan`.
fn migration_target_id(io: &mut impl crate::ui::menu::Asker, old: &crate::migrate::Old, chosen: &Chosen) -> Result<String> {
	let open = !chosen.existing.is_empty() || matches!(chosen.source, source::Source::Odis { .. });
	if !open {
		return Ok(chosen.project.id.clone());
	}

	let files = old.files();
	io.say(&format!(
		"There is data here from before vagcan kept one project per car: {files} file{} under {}.",
		plural(files),
		old.extracted.parent().unwrap_or(&old.extracted).display()
	))?;
	match old.proven() {
		// The sentence that has to be in front of them before they answer.
		// Everything else here is extracted and can be extracted again.
		0 => io.say(
			"None of it is measured-on-a-car data, so nothing about this is\n\
             unrepeatable — it is only a question of where it lands.",
		)?,
		1 => io.say(
			"One of them is a row proven by driving a car. Nothing but another\n\
             drive can recreate it, and this moves it rather than copying it.\n\
             If that data is this car's, press Enter. If it is a different\n\
             car's, give that car a name and it will be moved there instead.",
		)?,
		n => io.say(&format!(
			"{n} of them are rows proven by driving a car. Nothing but another\n\
             drive can recreate one, and this moves them rather than copying\n\
             them.\n\
             If that data is this car's, press Enter. If it is a different\n\
             car's, give that car a name and it will be moved there instead."
		))?,
	}
	if !chosen.existing.is_empty() {
		io.say(&format!("Projects already here: {}", chosen.existing.join(", ")))?;
	}
	loop {
		let typed = io.line("Which car is that data?", &chosen.project.id)?;
		match crate::project::folder_name(typed.trim()) {
			Ok(id) => return Ok(id),
			// Asked again rather than refused: a name is one keystroke, and
			// losing the run over a slash would be a poor trade — the same rule
			// `source::project_id` follows for the same question.
			Err(why) => io.say(&format!("{why}"))?,
		}
	}
}

/// Run one stage of setup, and say how long it took when somebody is measuring.
///
/// Silent unless `VAGCAN_TIMING` is set in the environment, and then one line
/// per stage on **stderr**, so it never lands in output somebody is piping.
/// This is the instrument the 2026-09-10 performance work was done with, kept
/// because "which stage got slow" is the first question the next regression
/// asks, and an `eprintln!` somebody has to re-add is not an instrument.
fn timed<T>(stage: &str, work: impl FnOnce() -> T) -> T {
	let started = std::time::Instant::now();
	let out = work();
	timing(stage, started.elapsed());
	out
}

/// The line [`timed`] prints, for a stage whose time was added up by hand.
fn timing(stage: &str, took: std::time::Duration) {
	if std::env::var_os("VAGCAN_TIMING").is_some() {
		eprintln!("[timing] {stage}: {:.2}s", took.as_secs_f64());
	}
}

/// Open an ODIS project and say which one it turned out to be.
fn open_odis(io: &mut impl crate::ui::menu::Asker, dir: &Path) -> Result<vag_data_labels::odis::Project> {
	let project = timed("open (string pools)", || vag_data_labels::odis::Project::open(dir))
		.with_context(|| format!("reading the ODIS project at {}", dir.display()))?;
	io.say(&format!(
		"{} pools, project version {}.",
		project.pools().len(),
		project.version().unwrap_or("unknown")
	))?;
	Ok(project)
}

/// The name the project gives itself, where it can be a directory name.
///
/// Falls back to what the folder was called when `<SHORT-NAME>` holds something
/// a directory cannot be named: nothing is sanitised into shape here, because a
/// mangled name files a car where no later run would look for it.
///
/// **`existing` is why this takes four arguments.** `source::project_id` has
/// already said "New — nothing has been read into it yet" or "added to the one
/// already there", and it said it about `asked`. Swapping the name here can
/// turn one into the other, and the case where it does is the ordinary re-run:
/// a second download unzips to `SK37X (1)`, which cleans to a name no project
/// has, so the person is told "New" — and then `<SHORT-NAME>` files it into the
/// `SK37X` that has been there all along. Design §5 makes that a merge, and a
/// merge nobody was told about is the one this has to say out loud.
fn prefer_its_own_name(io: &mut impl crate::ui::menu::Asker, named: &str, asked: &str, existing: &[String], rereads: &[String]) -> Result<String> {
	if named == asked {
		return Ok(asked.to_string());
	}
	match crate::project::folder_name(named) {
		Ok(own) => {
			let merge = match source::landing(&own, existing, rereads) {
				source::Landing::Rereads => {
					"\n    That project is already here and has read this source before:\n    reading it again replaces what it wrote."
				}
				source::Landing::Adds => "\n    That project is already here: this source is added to it, and\n    what other sources put there stays.",
				source::Landing::New => "",
			};
			// Both names last, one to a line. Neither has a knowable width when
			// this sentence is written — a project id may be sixty-four
			// characters — and an earlier draft ran to 278 columns on a real
			// terminal by putting them in the middle of it.
			io.say(&format!(
				"One car, one store: the project names itself in its own index.xml,\n\
                 so that is what it is filed under, not what the folder happens\n\
                 to be called.\n    \
                 the folder:  {asked}\n    \
                 filed under: {own}{merge}"
			))?;
			Ok(own)
		}
		Err(_) => Ok(asked.to_string()),
	}
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
	println!(
		"\n      None of {known:?} is in {}.\n      \
         Ross-Tech names this file differently in each language build, so pick the\n      \
         {what} out of what is actually there:",
		dir.display()
	);
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

/// Which language build a directory holds, named by the file that says so.
///
/// No marker is written for this: a directory that contains `Code-RUS.dat` and
/// `TTText-RUS.rod` *is* the Russian build, and a note beside it saying so
/// would be one more thing to keep in step with what is actually there.
fn build_of(dir: &Path) -> Option<&'static str> {
	CODES_FILES
		.iter()
		.find(|name| dir.join(name).is_file())
		.or_else(|| TEXT_TABLES.iter().find(|name| dir.join(ODX_DIR).join(name).is_file()))
		.copied()
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
fn replace_if_another_build(root: &Path, target: &Path, refresh: bool) -> Result<()> {
	if !target.exists() {
		return Ok(());
	}
	let (here, incoming) = (build_of(target), build_of(root));
	let mismatch = matches!((here, incoming), (Some(a), Some(b)) if a != b);
	if !mismatch && !refresh {
		return Ok(());
	}
	if mismatch {
		println!(
			"{} already holds the build that ships {}, and this one ships {}.\n\
             Clearing it first — layering the two would leave names from one and fault\n\
             text from the other, with nothing to say which was which.\n",
			target.display(),
			here.unwrap_or("?"),
			incoming.unwrap_or("?")
		);
	}
	std::fs::remove_dir_all(target).with_context(|| format!("clearing {}", target.display()))?;
	Ok(())
}

/// What one step of the run did, for the closing report.
///
/// A step that was skipped is worth as much as one that ran: somebody who
/// expected minutes and got seconds needs to be told why, or they will assume
/// it failed.
#[derive(Debug)]
enum Step {
	Wrote {
		what: &'static str,
		path: PathBuf,
		detail: String,
	},
	Skipped {
		what: &'static str,
		path: PathBuf,
		why: &'static str,
	},
	/// Wrote something, but not all of it. Distinct from `Wrote` because a
	/// number on its own reads as a total: "124,294 names" looks complete until
	/// you know the table held 195,910. Distinct from `Missing` because what it
	/// did write is real and usable.
	Partial {
		what: &'static str,
		path: PathBuf,
		detail: String,
		why: String,
	},
	Missing {
		what: &'static str,
		why: String,
	},
	/// Not done yet, for a reason that is a step still to take rather than
	/// something the source lacks: the registry's channels wait for a car to
	/// have been connected. Not a gap — "Done, with gaps" and "a newer VCDS may have it" would
	/// send the reader to the wrong place — and `why` says what to type.
	Pending {
		what: &'static str,
		why: String,
	},
	/// Did not complete, for a reason on this machine rather than in the
	/// source: a record of a car's units that does not read. A gap in the
	/// report, never an abort of the run — the steps before it wrote real
	/// files, and a run that stopped without its report would leave them
	/// unmentioned (found in review, 2026-09-28). `why` names the file and
	/// says to move it aside.
	Failed {
		what: &'static str,
		why: String,
	},
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
	run_with(&mut io, &mut source::native_folder, opts).await
}

/// The rule behind [`run`], with the asking behind [`crate::ui::menu::Asker`] and
/// [`source::Dialog`] so the flow is testable without a terminal — and without a
/// window, which CI has even less of.
async fn run_with(io: &mut impl crate::ui::menu::Asker, dialog: &mut impl source::Dialog, opts: Options<'_>) -> Result<()> {
	let Some(mut chosen) = choose(io, dialog, &opts)? else { return Ok(()) };
	let project = &chosen.project;
	io.say(&format!("Writing into {}\n", project.dir.display()))?;

	// Before the parse, not after: whatever an older build left in
	// `~/.vagcan/data/` belongs to a car too, and moving it afterwards would
	// have this run's rows sitting beside an unmigrated copy of the last one's.
	if let Some(old) = crate::migrate::pending()? {
		let into = migration_target(io, &old, &chosen)?;
		let report = crate::migrate::run(&old, &into)?;
		if report.moved() > 0 || report.left_behind.is_some() {
			io.say(&crate::migrate::describe(&report, &into))?;
		}
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
	println!("\n{}", report(&steps, scalings, fault_labels));
	Ok(())
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
	replace_if_another_build(root, &pool, refresh)?;
	std::fs::create_dir_all(&pool).with_context(|| format!("creating {}", pool.display()))?;
	println!("Reading the VCDS installation at {}", root.display());

	// The copy runs first and the derivations then read from it, so afterwards
	// `~/.vagcan` is the one set of raw files the car commands read. The
	// installation is still kept: the channels of a unit met later are read
	// from it (step 5, and `registry::ensure` at the car).
	// The fault text is looked for once, in step [1/5], and handed on: finding
	// it can mean asking the person which file it is, and one run must ask
	// that once.
	let (copied, codes) = copy_label_files(root, &pool, refresh)?;
	Ok(vec![
		copied,
		label_cache(root, codes.as_deref(), project, refresh)?,
		names(&pool, project)?,
		rod_keys(&pool, project)?,
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
	let step = match registry::recorded_units() {
		Ok(units) => registry::registry_channels(root, project, &units).await?,
		Err(e) => {
			println!(
				"[5/5] Channels — a car's record on this machine does not read, so nothing\n      \
                 was read from the registry."
			);
			crate::registry::note_installation(project, root)?;
			registry::failed(&e)
		}
	};
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
	println!("Reading the ODIS project at {}", dir.display());
	let source = dir.display().to_string();

	println!("[1/2] Control units — walking each variant's measurement chain and fault table.");
	let variants = timed("variants", || odis.variants()).with_context(|| format!("listing the variants of {}", dir.display()))?;
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
		timed("readings (walk)", || {
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
	let codes = timed("faults (sqlite)", || vag_data_db::put_all_faults(&project.cache(), &source, fault_rows))
		.map_err(|e| anyhow::anyhow!("writing the fault codes to {}: {e}", project.cache().display()))?;
	let channels = timed("readings (sqlite)", || {
		vag_data_db::put_all_readings(&project.cache(), &source, channel_rows)
	})
	.map_err(|e| anyhow::anyhow!("writing the channels to {}: {e}", project.cache().display()))?;
	let units = Step::Wrote {
		what: "the control units this project describes",
		path: project.cache(),
		detail: format!(
			"{with_channels} of {} variants, {channels} channels{}; {with_faults} with fault text, {codes} codes{}",
			variants.len(),
			chain_skipped.note(),
			fault_skipped.note()
		),
	};

	println!("[2/2] Names — every object in every pool, for the (text id, name)\n      pairs they carry.");
	let names = {
		let _spinner = crate::progress::Spinner::new("reading every object in the project".to_string());
		timed("names (walk)", || odis.names()).with_context(|| format!("reading the names of {}", dir.display()))?
	};
	let merged = timed("names (merge + write)", || merge_names(&project.odis_names(), names))?;

	crate::project::record_source(
		project,
		crate::project::SourceEntry {
			kind: vag_data_db::ODIS,
			path: source,
			version: odis.version().map(str::to_string),
			detail: Some(format!("{} variants, {} pools", variants.len(), odis.pools().len())),
		},
	)?;
	Ok(vec![
		units,
		Step::Wrote {
			what: "the measurement names",
			path: project.odis_names(),
			detail: format!("{merged} names"),
		},
	])
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
	println!(
		"[1/5] Raw files — copying the .rod files and the fault text into the\n      \
         shared pool, which every car command reads instead of the installation."
	);
	// What a copy cut short left behind, before anything is copied: one staging
	// file per kill, each under its pid, and nothing reads them.
	let stale = crate::datadir::remove_stale_copies(target);
	if stale > 0 {
		println!(
			"      {stale} staging file{} of a copy cut short removed",
			if stale == 1 { "" } else { "s" }
		);
	}
	let mut plan: Vec<(PathBuf, PathBuf)> = Vec::new();
	let odx = root.join(ODX_DIR);
	match odx.is_dir() {
		true => collect_rod_files(&odx, target, &mut plan)?,
		// A stripped or partial installation still yields whatever it has; a
		// missing input is reported, not fatal.
		false => println!("      {ODX_DIR}: not in this installation, skipped"),
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
		None => println!(
			"      the fault text file: not in this installation, skipped\n      \
             — faults will read as numbers"
		),
	}
	if plan.is_empty() {
		return Ok((
			Step::Missing {
				what: "the raw files",
				why: format!("no {ODX_DIR}/ and no fault text under {}", root.display()),
			},
			found,
		));
	}

	let total = plan.len();
	let (mut copied, mut skipped) = (0usize, 0usize);
	let mut progress = crate::progress::Line::new();
	for (at, (src, dst)) in plan.iter().enumerate() {
		if !refresh && is_newer(dst, src) {
			skipped += 1;
			continue;
		}
		progress.update(&format!("copying — {} of {total}", at + 1));
		// Staged and renamed into place: a copy cut short by Ctrl-C must not
		// leave a truncated file under the final name, newer than its source and
		// so skipped as current by the next run (see `datadir::copy_file`).
		crate::datadir::copy_file(src, dst)?;
		copied += 1;
	}
	progress.finish();
	Ok((
		Step::Wrote {
			what: "the raw files",
			path: target.to_path_buf(),
			detail: format!("{copied} files copied, {skipped} already current"),
		},
		found,
	))
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
	println!("[2/5] Label files — parsing every .lbl and decrypting every .clb.");
	let db = crate::labels::load_cached(root, &project.cache(), refresh)?;
	// The language of this build's fault text, on the same source row the
	// label files were written under, so that `faults` can tell a VCDS source
	// in one language from an ODIS project in another.
	if let Some(language) = codes.and_then(|codes| codes.file_name().and_then(|n| n.to_str()).and_then(codes_language)) {
		let labels = crate::labels::label_dir_under(root)?;
		vag_data_db::record_language(&project.cache(), vag_data_db::VCDS, &labels.to_string_lossy(), language)
			.map_err(|e| anyhow::anyhow!("recording the language of {} in {}: {e}", root.display(), project.cache().display()))?;
	}
	Ok(Step::Wrote {
		what: "the label files",
		path: project.cache(),
		detail: format!("{} label files", db.len()),
	})
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
		return Ok(Step::Missing {
			what: "the measurement names",
			why: format!("none of {TEXT_TABLES:?} is under {}", odx.display()),
		});
	};
	// How long it takes is the spinner's job to say, and it says it in elapsed
	// seconds rather than in a sentence nobody can act on.
	// Ask what opening it would cost before starting, because the two cases look
	// identical while running. A shifted text table has no anchor, so the only
	// route is sixty full-space searches — hours to days — and a spinner that
	// will not stop today is indistinguishable from one that stops in ninety
	// seconds. The Russian build ships exactly such a table.
	let name = source.file_name().unwrap_or_default().to_string_lossy().into_owned();
	if !keyed_already(&source, project)?
		&& let Ok(bytes) = std::fs::read(&source)
		&& vag_data_labels::rod::key_cost(&bytes, "TXT") == Some(vag_data_labels::rod::KeyCost::AnchorSweep)
	{
		{
			println!("[3/5] Measurement names — skipped: {name} masks its key.");
			return Ok(Step::Missing {
				what: "the measurement names",
				why: format!(
					"{name} is a *shifted* container, so its text section has no\n    \
                     anchor to search from — the only route is every legal anchor\n    \
                     against the full space, which is hours to days rather than\n    \
                     the minute or two an ordinary table costs. Step 5 names its\n    \
                     channels from this table too, so there they go by their\n    \
                     identifiers. See .archive/research/labels/tttext2.md §3.3"
				),
			});
		}
	}
	println!("[3/5] Measurement names — opening {name}, then reading every record under its own key.");
	let scratch = out.with_file_name("tttext-scratch");
	let _ = std::fs::remove_dir_all(&scratch);
	let sections = crate::vcds::rod::open(
		&source.to_string_lossy(),
		true,
		Some(&project.rod_keys().to_string_lossy()),
		Some(&scratch.to_string_lossy()),
	)?;
	let text = scratch.join("TXT.bin");
	if !text.is_file() {
		let _ = std::fs::remove_dir_all(&scratch);
		let shut = sections.iter().find(|s| s.tag == "TXT").map_or("there is no such section", |s| {
			crate::vcds::rod::why_shut(s).unwrap_or("it opened, and nothing was written")
		});
		let why = format!("the [TXT] section of {} did not open: {shut}", source.display());
		return Ok(Step::Missing {
			what: "the measurement names",
			why,
		});
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
		return Ok(Step::Missing {
			what: "the measurement names",
			why: format!(
				"the [TXT] section of {} read into no name: {} records of neither shape{example},\n    \
                 {} lines that were not records. The names already here are left as they were",
				source.display(),
				table.malformed.len(),
				table.not_records
			),
		});
	}
	let (names_text, count) = crate::vcds::tttext::names_json(table)?;
	let (ids_text, with_ids) = crate::vcds::tttext::odx_ids_json(table)?;
	let names_changed = write_if_changed(out, &names_text)?;
	let ids_changed = write_if_changed(ids, &ids_text)?;
	let detail = format!("{count} names, {with_ids} of them naming an IDE or MAS id (odx-ids.json)");
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
		return Ok(Step::Partial {
			what: "the measurement names",
			path: out.to_path_buf(),
			detail,
			why: format!(
				"{} records read as neither `<name>,` nor `<name>,<kind>,<value>`, and {} lines\n    \
                 were not records; both are left out rather than guessed at{example}",
				table.malformed.len(),
				table.not_records
			),
		});
	}
	if !names_changed && !ids_changed {
		return Ok(Step::Skipped {
			what: "the measurement names",
			path: out.to_path_buf(),
			why: "the text table reads as it did last time",
		});
	}
	Ok(Step::Wrote {
		what: "the measurement names",
		path: out.to_path_buf(),
		detail,
	})
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
		return Ok(Step::Missing {
			what: "the .rod section keys",
			why: format!("none of {SHARED_ROD_FILES:?} is in {}", pool.display()),
		});
	}
	println!("[4/5] .rod section keys — searching for the ones not already cached.");
	let mut shut = Vec::new();
	for file in &present {
		for section in crate::vcds::rod::open(&file.to_string_lossy(), true, Some(&cache.to_string_lossy()), None)? {
			if let Some(why) = crate::vcds::rod::why_shut(&section) {
				shut.push(format!("[{}] in {} did not open: {why}", section.tag, file.display()));
			}
		}
	}
	let keys = std::fs::read_to_string(&cache)
		.ok()
		.and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
		.and_then(|v| v.as_object().map(|m| m.len()))
		.unwrap_or(0);
	Ok(match shut.is_empty() {
		true => Step::Wrote {
			what: "the .rod section keys",
			path: cache,
			detail: format!("{keys} keys"),
		},
		false => Step::Partial {
			what: "the .rod section keys",
			path: cache,
			detail: format!("{keys} keys"),
			why: shut.join("\n    "),
		},
	})
}

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

/// What to say about scalings when the project has none.
///
/// A VCDS installation's label files carry names and no numbers
/// (`.archive/research/labels/rod-labels.md` §4.0c); its registry carries them
/// for the units of the cars this machine has met, and the registry step says
/// itself why it brought none — no car yet, or units the installation cannot
/// open. So this names only what that step cannot, and says nothing that would
/// send somebody whose units are already on record back to the car.
const SCALINGS_ARE_MEASURED: &str = "No scalings yet. An ODIS project brings them for every unit it describes.";

/// The same, when the registry step is still to come: the last word of a VCDS
/// setup with no car yet is the command that finishes it, not the other source.
/// Wrapped by hand, as everything `setup` prints is: eighty columns.
const SCALINGS_WAIT_FOR_THE_CAR: &str = "No scalings yet. An ODIS project brings them for every unit it describes; this\n\
                                        installation brings them for the units it has files for, once `vagcan watch`\n\
                                        has run with the car.";

/// The command that turns a pending registry step into channels, for the
/// closing "Next:" — with what it does, since a reader who has just been told
/// "not yet" needs to see which line is the one that changes that.
const NEXT_WITH_THE_CAR: &str = "vagcan watch        with the car: records its units, reads their channels";

/// The last word when step 5 did not complete: what to do, not the other source.
const AFTER_A_FAILED_STEP: &str = "Step 5 did not complete: move the file it names aside, then re-run\n\
                                  vagcan setup <VCDS installation>.";

/// The closing report: what is on disk now, and what to do with it.
///
/// Every line names a file. Somebody who has just waited several minutes is
/// owed the paths, not a count of successes — and somebody whose run was short
/// of one artefact needs to see which one without re-reading the scroll.
///
/// `scalings` is whether the project now holds per-variant scalings — the one
/// fact the closing sentence turns on, and asked of the store rather than of
/// which branch ran, so a VCDS run into a project that already has them does not
/// tell its reader they have none.
fn report(steps: &[Step], scalings: bool, fault_labels: bool) -> String {
	use std::fmt::Write as _;

	// "Done." on its own reads as full success; when an artefact is missing the
	// header has to say so, or a reader takes the fast finish for a complete one.
	// A partial artefact counts as a gap too: a run that recovered 63 % of the
	// names finished successfully and is still not what "Done." promises.
	let any_gap = steps
		.iter()
		.any(|s| matches!(s, Step::Missing { .. } | Step::Partial { .. } | Step::Failed { .. }));
	let mut out = String::from(if any_gap { "Done, with gaps.\n\n" } else { "Done.\n\n" });
	for step in steps {
		match step {
			Step::Wrote { what, path, detail } => {
				let _ = writeln!(out, "  {what}: {detail}\n    {}", path.display());
			}
			Step::Skipped { what, path, why } => {
				let _ = writeln!(out, "  {what}: unchanged, {why}\n    {}", path.display());
			}
			Step::Partial { what, path, detail, why } => {
				let _ = writeln!(out, "  {what}: {detail} — PARTIAL\n    {why}\n    {}", path.display());
			}
			Step::Missing { what, why } => {
				let _ = writeln!(out, "  {what}: NOT recovered — {why}");
			}
			Step::Pending { what, why } => {
				let _ = writeln!(out, "  {what}: not yet — {why}");
			}
			Step::Failed { what, why } => {
				let _ = writeln!(out, "  {what}: NOT done\n    {why}");
			}
		}
	}
	if steps.iter().any(|s| matches!(s, Step::Missing { .. })) {
		let _ = writeln!(
			out,
			"\nThe rest is usable. What is missing above is missing from the installation \n\
             that was read, so a different or newer VCDS may have it."
		);
	}
	// The fault line is conditional because it was a lie on the ODIS-only path:
	// it announced that the labels are copied in after a run that copied
	// nothing. `fault_labels` is whether either source can name a code now —
	// VCDS's label files, or the fault text an ODIS project carries — so the
	// line promises names only when `vagcan faults` will print them.
	let faults = match fault_labels {
		true => "vagcan faults       stored faults, named",
		false => "vagcan faults       stored faults, as numbers — no fault text read",
	};
	let _ = write!(
		out,
		"\nNext:  vagcan devices      is the adapter connected?\n       \
         vagcan info         which car is this?\n       \
         {faults}"
	);
	// A registry step still to come is finished by a command with the car, and
	// that command is the next step — named here, or a VCDS-only owner is left
	// with three commands that read the car and none that says which one
	// brings the channels.
	let pending = steps.iter().any(|s| matches!(s, Step::Pending { .. }));
	if pending {
		let _ = write!(out, "\n       {NEXT_WITH_THE_CAR}");
	}
	// Said only where it is true. An ODIS project declares a scaling per variant,
	// so "they must be measured" would be false there; the paragraph that once
	// replaced it was more noise than use (owner, 2026-09-13), so an ODIS run
	// closes on the commands. A VCDS run with no car yet closes on the car.
	if !scalings {
		let _ = write!(out, "\n\n{}", if pending { SCALINGS_WAIT_FOR_THE_CAR } else { SCALINGS_ARE_MEASURED });
	}
	// A step that did not complete closes on what to do about it.
	if steps.iter().any(|s| matches!(s, Step::Failed { .. })) {
		let _ = write!(out, "\n\n{AFTER_A_FAILED_STEP}");
	}
	out
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
	fn the_report_names_every_file_it_wrote() {
		// Somebody who has just waited several minutes is owed the paths. A
		// count of successes tells them nothing they can open.
		let steps = vec![
			Step::Wrote {
				what: "the label files",
				path: PathBuf::from("/home/x/.vagcan/data/SK37X/cache.sqlite"),
				detail: "3035 label files".to_string(),
			},
			Step::Skipped {
				what: "the measurement names",
				path: PathBuf::from("/home/x/.vagcan/data/SK37X/names.json"),
				why: "newer than the text table it came from",
			},
		];
		let r = report(&steps, false, true);
		assert!(r.contains("/home/x/.vagcan/data/SK37X/cache.sqlite"), "{r}");
		assert!(r.contains("3035 label files"), "{r}");
		// A skipped step is reported, not silently absent: a run that took a
		// second when minutes were expected reads as a failure otherwise.
		assert!(r.contains("unchanged"), "{r}");
		assert!(r.contains("names.json"), "{r}");
	}

	#[test]
	fn a_project_that_names_itself_beats_the_folder_it_was_unzipped_into() {
		// D7, and the failure it prevents: an unzip produces `SK37X (1)`, the
		// picker cleans that to `SK-37X-copy`, and the project inside still
		// calls itself `SK37X`. Filing it under the folder's name would put one
		// car in two stores — the two-directory bug `datadir::existing_folder`
		// was written to undo, arriving by a different door.
		let mut io = crate::ui::menu::Scripted::new(vec![]);
		assert_eq!(prefer_its_own_name(&mut io, "SK37X", "SK-37X-copy", &[], &[]).unwrap(), "SK37X");
		let said = io.all_said();
		assert!(said.contains("SK37X"), "{said}");
		assert!(said.contains("One car, one store"), "it says why: {said}");

		// Agreement is silent — there is nothing to explain.
		let mut io = crate::ui::menu::Scripted::new(vec![]);
		assert_eq!(prefer_its_own_name(&mut io, "SK37X", "SK37X", &[], &[]).unwrap(), "SK37X");
		assert!(io.said.is_empty(), "{:?}", io.said);

		// A `<SHORT-NAME>` that could not be a directory falls back rather than
		// being sanitised: a mangled name files a car where nothing looks for it.
		let mut io = crate::ui::menu::Scripted::new(vec![]);
		assert_eq!(prefer_its_own_name(&mut io, "../escape", "SK37X", &[], &[]).unwrap(), "SK37X");
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
	fn a_name_swap_onto_a_project_that_exists_says_it_is_a_merge_after_all() {
		// The ordinary re-run, and the one case where the swap changes the
		// answer somebody was already given: a second download unzips to
		// `SK37X (1)`, which cleans to a name no project has, so
		// `source::project_id` says "New — nothing has been read into it yet".
		// Then `<SHORT-NAME>` files it into the `SK37X` that has been there all
		// along. Design §5 makes that a merge, and nobody has been told.
		let mut io = crate::ui::menu::Scripted::new(vec![]);
		let existing = ["SK37X".to_string()];
		assert_eq!(prefer_its_own_name(&mut io, "SK37X", "SK-37X-1", &existing, &[]).unwrap(), "SK37X");
		let said = io.all_said();
		assert!(said.contains("already here"), "{said}");
		assert!(said.contains("what other sources put there stays"), "{said}");

		// A swap onto a name nothing holds is still new, and says nothing extra.
		let mut io = crate::ui::menu::Scripted::new(vec![]);
		assert_eq!(prefer_its_own_name(&mut io, "SK37X", "SK-37X-1", &[], &[]).unwrap(), "SK37X");
		assert!(!io.all_said().contains("already here"), "{:?}", io.said);

		// The same folder set up a second time lands on a project that has read
		// it: that is a reread, not a merge.
		let mut io = crate::ui::menu::Scripted::new(vec![]);
		assert_eq!(prefer_its_own_name(&mut io, "SK37X", "SK-37X-1", &existing, &existing).unwrap(), "SK37X");
		let said = io.all_said();
		assert!(said.contains("reading it again replaces"), "{said}");
		assert!(!said.contains("is added to it"), "{said}");
	}

	/// A `Chosen` as `choose` would have built one. No directory is created:
	/// only [`migration_target_id`] is under test, and it creates nothing.
	fn chosen(id: &str, existing: &[&str], odis: bool) -> Chosen {
		let dir = std::path::PathBuf::from("/nowhere/projects").join(id);
		Chosen {
			source: match odis {
				true => source::Source::Odis {
					dir: PathBuf::from("/nowhere/SK37X"),
				},
				false => source::Source::Vcds {
					dir: PathBuf::from("/nowhere/VCDS"),
				},
			},
			// Not what `migration_target_id` turns on.
			names: None,
			project: crate::project::Project { id: id.to_string(), dir },
			existing: existing.iter().map(|s| s.to_string()).collect(),
			odis: None,
		}
	}

	/// A `~/.vagcan/data/` with one proven row in it, in a temporary directory.
	fn old_layout(root: &Path) -> crate::migrate::Old {
		let old = crate::migrate::Old {
			extracted: root.join("data").join("extracted"),
			measured: root.join("data").join("measured"),
		};
		std::fs::create_dir_all(&old.extracted).unwrap();
		std::fs::create_dir_all(&old.measured).unwrap();
		std::fs::write(old.extracted.join("names.json"), b"{}").unwrap();
		std::fs::write(old.measured.join("04E-906-027-AH.json"), b"{}").unwrap();
		old
	}

	#[test]
	fn with_one_car_and_a_vcds_install_there_is_nothing_to_ask() {
		// A machine with no projects yet, being set up from an installation,
		// has one car in play and one place the data can belong. A question
		// there is a question with one answer.
		let here = TempDir::new("mig-quiet");
		let old = old_layout(&here.0);
		let mut io = crate::ui::menu::Scripted::new(vec![]);
		assert_eq!(migration_target_id(&mut io, &old, &chosen("default", &[], false)).unwrap(), "default");
		assert!(io.typed.is_empty(), "it asked anyway: {:?}", io.typed);
		assert!(io.said.is_empty(), "{:?}", io.said);
	}

	#[test]
	fn setting_up_a_second_car_asks_whose_the_old_data_is_before_moving_it() {
		// The failure this exists for, and it is the *first* run after
		// upgrading: `vagcan setup ~/Downloads/SK37X` for a second car while
		// `data/measured/` still holds the first car's proven rows. Filing them
		// into whichever project this run chose puts one car's proven data
		// inside another car's store.
		let here = TempDir::new("mig-ask");
		let old = old_layout(&here.0);
		let mut io = crate::ui::menu::Scripted::new(vec![crate::ui::menu::Answer::Type("carA".to_string())]);
		assert_eq!(migration_target_id(&mut io, &old, &chosen("SK37X", &[], true)).unwrap(), "carA");

		let said = io.all_said();
		assert!(said.contains("proven by driving a car"), "what is at stake is named: {said}");
		assert!(said.contains("rather than copying"), "and that it is irreversible: {said}");
		// One row is "a row … recreate it", not "1 of them are rows".
		assert!(said.contains("One of them is a row"), "{said}");
		assert!(said.contains("2 files under"), "the total is counted too: {said}");
		// The default is this run's project, so pressing Enter does what the
		// unprompted version used to do.
		assert_eq!(io.defaults(), ["SK37X"]);
	}

	#[test]
	fn a_project_already_on_disk_is_enough_to_make_the_question_worth_asking() {
		let here = TempDir::new("mig-existing");
		let old = old_layout(&here.0);
		let mut io = crate::ui::menu::Scripted::new(vec![crate::ui::menu::Answer::Type(String::new())]);
		// An empty line takes the default, which is this run's project.
		assert_eq!(
			migration_target_id(&mut io, &old, &chosen("default", &["carA"], false)).unwrap(),
			"default"
		);
		assert!(io.all_said().contains("carA"), "the ones already here are named: {:?}", io.said);
	}

	#[test]
	fn with_nothing_unrepeatable_at_stake_the_question_says_so() {
		// Most machines: `setup` has run, no row was ever proved on a car. The
		// question is still asked — the data still lands somewhere — but it must
		// not imply a risk that is not there.
		let here = TempDir::new("mig-norows");
		let old = old_layout(&here.0);
		std::fs::remove_file(old.measured.join("04E-906-027-AH.json")).unwrap();
		let mut io = crate::ui::menu::Scripted::new(vec![crate::ui::menu::Answer::Type(String::new())]);
		migration_target_id(&mut io, &old, &chosen("SK37X", &[], true)).unwrap();
		let said = io.all_said();
		// A phrase that survives the hard wrap: the sentence is broken across
		// lines at 80 columns, so an assertion spanning the break tests the
		// wrap rather than the copy.
		assert!(said.contains("only a question of where it lands"), "{said}");
		assert!(!said.contains("proven by driving"), "it warned about rows that are not there: {said}");
	}

	#[test]
	fn an_answer_that_could_not_be_a_folder_is_asked_again_rather_than_losing_the_run() {
		let here = TempDir::new("mig-badname");
		let old = old_layout(&here.0);
		let mut io = crate::ui::menu::Scripted::new(vec![
			crate::ui::menu::Answer::Type("car A/1".to_string()),
			crate::ui::menu::Answer::Type("car-A-1".to_string()),
		]);
		assert_eq!(migration_target_id(&mut io, &old, &chosen("SK37X", &[], true)).unwrap(), "car-A-1");
		assert_eq!(io.typed.len(), 2, "it gave up instead of asking again");
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
	fn nothing_setup_prints_about_a_project_runs_past_eighty_columns() {
		// Caught three times on real terminals now. A path's width is unknown
		// when the sentence is written, so the rule is that the sentence ends
		// with it — and the fixed text around it still has to fit.
		use crate::ui::menu::Asker as _;
		let mut io = crate::ui::menu::Scripted::new(vec![]);
		let long = Path::new("/Users/somebody/Downloads/an-odis-project-with-a-long-name/SK37X");
		let here = ["SK37X".to_string()];
		prefer_its_own_name(&mut io, "SK37X", "SK-37X-copy", &here, &[]).unwrap();
		prefer_its_own_name(&mut io, "SK37X", "SK-37X-copy", &here, &here).unwrap();
		open_odis(&mut io, long).ok();
		io.say(&format!("Writing into {}", long.display())).unwrap();
		for line in io.all_said().lines() {
			assert!(line.chars().count() <= 80, "{} columns: {line:?}", line.chars().count());
		}
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
		assert!(matches!(first, Step::Wrote { .. }), "{first:?}");
		assert!(std::fs::read_to_string(&ids).unwrap().contains("\"IDE90001\""));
		let again = written_names(&table, &source, &out, &ids).unwrap();
		assert!(matches!(again, Step::Skipped { .. }), "the same read rewrote the files: {again:?}");
		// Another build's table is written whatever the files' times say — the
		// times are what used to let one pass for the other.
		let other = text_table(&[(17, "Invented flap position,2,90001"), (13, "Pump 3,")]);
		let step = written_names(&other, &source, &out, &ids).unwrap();
		assert!(matches!(step, Step::Wrote { .. }), "{step:?}");
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
		assert!(matches!(step, Step::Missing { .. }), "{step:?}");
		assert_eq!(std::fs::read(&out).unwrap(), before);
		assert!(!ids.exists());
		// Records of neither shape and nothing else: the same, and it shows one.
		match written_names(&text_table(&[(5, "no separator")]), &source, &out, &ids).unwrap() {
			Step::Missing { why, .. } => assert!(why.contains("000005"), "{why}"),
			other => panic!("not missing: {other:?}"),
		}
		assert_eq!(std::fs::read(&out).unwrap(), before);
	}

	#[test]
	fn a_line_of_neither_shape_makes_the_names_partial_and_keeps_the_rest() {
		let here = TempDir::new("names-partial");
		let (out, ids, source) = (here.0.join("names.json"), here.0.join("odx-ids.json"), here.0.join("TTTEXT.ROD"));
		let table = text_table(&[(17, "Invented flap position,2,90001"), (5, "no separator")]);
		match written_names(&table, &source, &out, &ids).unwrap() {
			Step::Partial { detail, why, .. } => {
				assert!(detail.starts_with("1 names"), "{detail}");
				assert!(why.contains("000005"), "the record left out is named: {why}");
			}
			other => panic!("not partial: {other:?}"),
		}
		assert!(std::fs::read_to_string(&out).unwrap().contains("000017"));
	}

	#[test]
	fn a_step_that_could_not_run_says_so_without_condemning_the_rest() {
		let steps = vec![Step::Missing {
			what: "the .rod section keys",
			why: "none of them is in this installation".to_string(),
		}];
		let r = report(&steps, false, true);
		assert!(r.contains("NOT recovered"), "{r}");
		assert!(r.contains("The rest is usable"), "{r}");
	}

	#[test]
	fn a_run_with_no_scalings_says_how_to_get_them() {
		// The closing lines are the last chance to say what a run did not bring.
		// It used to be "no VCDS installation carries them"; since 2026-09-28 one
		// does, through its registry, for the units of the cars this machine has
		// met — and the registry step says so itself, with the reason it has
		// none. The close names only what that step cannot: an ODIS project.
		// Telling somebody whose units are on record but whose files the install
		// cannot open to connect the car again would send them back for nothing.
		let r = report(&[], false, true);
		assert!(r.contains("No scalings yet"), "{r}");
		assert!(r.contains("ODIS project"), "{r}");
		assert!(!r.contains("survey"), "{r}");
		assert!(!r.contains("no VCDS installation carries them"), "the old claim is false now: {r}");
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
		// still to take, not something the installation lacks: "Done, with gaps"
		// and "a newer VCDS may have it" sent people to the wrong place.
		let steps = vec![Step::Pending {
			what: "channels from the VCDS registry",
			why: registry::PENDING_WHY.to_string(),
		}];
		let r = report(&steps, false, true);
		assert!(r.starts_with("Done.\n"), "{r}");
		assert!(r.contains("channels from the VCDS registry: not yet — no car"), "{r}");
		assert!(!r.contains("newer VCDS"), "{r}");
		// And the closing names the command that finishes the step, as the next
		// step: "Next:" listed devices, info and faults and left a VCDS-only owner
		// with no car to work out which one reads the channels, and the last
		// word sent them to ODIS (found in review, 2026-09-28).
		let next = r.split("Next:").nth(1).expect("a Next: block");
		assert!(next.contains("vagcan watch"), "{r}");
		assert!(next.contains("records its units, reads their channels"), "{r}");
		assert!(r.trim_end().ends_with("has run with the car."), "{r}");
		// Without a pending step, the closing is what it was.
		let r = report(&[], false, true);
		assert!(!r.contains("vagcan watch"), "{r}");
		assert!(r.trim_end().ends_with(SCALINGS_ARE_MEASURED), "{r}");
	}

	#[test]
	fn a_record_that_does_not_read_is_a_step_that_did_not_complete_and_the_report_still_comes() {
		// One unreadable `units.json` aborted `setup` at step 5 with no report,
		// after steps 1–4 had written their files (found in review, 2026-09-28).
		// It is a gap now, with the path and what to do about it — and the
		// closing word is that, not the other source.
		let bad = anyhow::Error::new(crate::units::NotARecord {
			path: PathBuf::from("/home/x/.vagcan/cars/BAD/units.json"),
			cause: "expected ident at line 1 column 2".into(),
		});
		let steps = vec![
			Step::Wrote {
				what: "the label files",
				path: PathBuf::from("/home/x/.vagcan/data/SK37X/cache.sqlite"),
				detail: "3035 label files".to_string(),
			},
			registry::failed(&bad),
		];
		let r = report(&steps, false, true);
		assert!(r.starts_with("Done, with gaps.\n"), "{r}");
		assert!(
			r.contains(
				"channels from the VCDS registry: NOT done\n    a car's record on this machine is not one this tool wrote:\n    /home/x/.vagcan/cars/BAD/units.json\n    (expected ident at line 1 column 2)\n"
			),
			"{r}"
		);
		assert!(r.contains("3035 label files"), "the steps before it are still reported: {r}");
		assert!(!r.contains("newer VCDS"), "not a gap in the installation: {r}");
		assert!(r.trim_end().ends_with(AFTER_A_FAILED_STEP), "the last word is what to do: {r}");
		assert_eq!(r.matches("aside").count(), 1, "said once, at the close: {r}");
		// An error that is not a record's — the directory would not read — is laid out too.
		let other = registry::failed(&anyhow::anyhow!("reading /x/cars: permission denied"));
		let Step::Failed { why, .. } = &other else { panic!("{other:?}") };
		assert!(
			why.starts_with("the records of the cars on this machine could not be read:\n    reading /x/cars"),
			"{why}"
		);
	}

	#[test]
	fn the_closing_report_keeps_to_eighty_columns_whatever_its_steps() {
		// The module's rule, applied to the report with every kind of step and
		// both closings (found in review, 2026-09-28: two closing lines ran to 88
		// and 177 columns). Paths are short here; a real one may run a line
		// over, and the rule for those is that the line ends with it.
		let steps = vec![
			Step::Wrote {
				what: "the label files",
				path: PathBuf::from("/x/cache.sqlite"),
				detail: "3035 label files".to_string(),
			},
			Step::Pending {
				what: registry::WHAT,
				why: registry::PENDING_WHY.to_string(),
			},
			registry::failed(&anyhow::Error::new(crate::units::NotARecord {
				path: PathBuf::from("/x/units.json"),
				cause: "expected ident at line 1 column 2".into(),
			})),
		];
		for (scalings, fault_labels) in [(false, false), (false, true), (true, true)] {
			for line in report(&steps, scalings, fault_labels).lines() {
				assert!(line.chars().count() <= 80, "{} columns: {line:?}", line.chars().count());
			}
		}
	}

	#[test]
	fn a_run_that_read_scalings_does_not_close_by_saying_there_are_none() {
		// The defect this footer split exists for. An ODIS run reported
		// "633 of 717 variants, 310734 channels" and then, two lines later,
		// that no installation carries scalings and they must be measured. A
		// reader who believes the footer goes and drives the car to establish
		// rows the tool already has. It now closes on the commands.
		let r = report(&[], true, true);
		assert!(!r.contains("No scalings yet"), "{r}");
		assert!(!r.contains("carries scalings"), "the paragraph the owner dropped is gone: {r}");
		assert!(r.trim_end().ends_with("stored faults, named"), "{r}");
	}

	#[test]
	fn a_run_that_copied_no_labels_does_not_claim_the_labels_are_copied_in() {
		// The defect: the fault line was unconditional, so an ODIS-only run —
		// which copies nothing — finished by announcing that the labels are in.
		// Naming a code still reads those files off disk, so the reader was told
		// the opposite of what they had.
		let without = report(&[], true, false);
		assert!(without.contains("as numbers"), "{without}");
		assert!(!without.contains("copied in"), "{without}");
		// And it must not teach the wrong lesson on the way past: neither
		// source is named as the only place fault text can come from.
		assert!(!without.contains("VCDS"), "{without}");

		let with = report(&[], true, true);
		assert!(with.contains("stored faults, named"), "{with}");
		assert!(!with.contains("as numbers"), "{with}");
	}

	#[test]
	fn no_line_of_either_closing_footer_runs_past_eighty_columns() {
		// Caught twice on real terminals: a sentence written against a short
		// test path wraps on a real one. Neither of these interpolates
		// anything, so their width is knowable here and worth pinning.
		for r in [report(&[], false, true), report(&[], true, true), report(&[], true, false)] {
			for line in r.lines() {
				assert!(line.chars().count() <= 80, "{} columns: {line:?}", line.chars().count());
			}
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
		match step {
			Step::Wrote { detail, .. } => detail.clone(),
			other => panic!("expected a written step, got {other:?}"),
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
		assert!(detail(&step).starts_with("3 files copied"), "{}", detail(&step));
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
		assert_eq!(detail(&first), "3 files copied, 0 already current");

		let second = copy_label_files(&install.0, &target.0, false).unwrap().0;
		assert_eq!(detail(&second), "0 files copied, 3 already current", "a no-op rerun");

		// One destination made to look stale: only it is copied again.
		let stale = target.0.join("Codes.dat");
		let past = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
		std::fs::File::options().write(true).open(&stale).unwrap().set_modified(past).unwrap();
		let third = copy_label_files(&install.0, &target.0, false).unwrap().0;
		assert_eq!(detail(&third), "1 files copied, 2 already current");

		// --refresh copies everything regardless of mtimes.
		let forced = copy_label_files(&install.0, &target.0, true).unwrap().0;
		assert_eq!(detail(&forced), "3 files copied, 0 already current");
	}

	#[test]
	fn an_install_missing_every_input_is_reported_not_a_crash() {
		let empty = TempDir::new("empty");
		let target = TempDir::new("empty-out");
		let step = copy_label_files(&empty.0, &target.0, false).unwrap().0;
		match step {
			Step::Missing { why, .. } => assert!(why.contains(ODX_DIR), "{why}"),
			other => panic!("expected Missing, got {other:?}"),
		}
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
}
