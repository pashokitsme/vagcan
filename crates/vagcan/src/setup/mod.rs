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
//! Both land in one project under `~/.vagcan/projects/<id>/`
//! ([`crate::project`]), and a second source is **added** to a project rather
//! than replacing what is in it (design §5). What each branch writes:
//!
//! | source | what it gives | where it lands |
//! |---|---|---|
//! | VCDS | the `.rod` files and the fault text, raw | `~/.vagcan/rod/`, shared |
//! | VCDS | the label files, parsed | `projects/<id>/cache.sqlite` |
//! | VCDS | measurement names, out of `TTTEXT.ROD` | `projects/<id>/names.json` |
//! | VCDS | `.rod` section keys | `projects/<id>/rod-keys.json` |
//! | ODIS | every variant's channels, by identifier, **with scalings** | `projects/<id>/cache.sqlite` |
//! | ODIS | every `(text id, name)` pair in the project | `projects/<id>/names.json` |
//!
//! The copy is what makes a VCDS installation disposable: fault naming reads
//! `.rod` files straight off disk at run time, so those have to outlive the
//! install. The `.lbl`/`.clb` files are **not** copied (D4) — they are read once,
//! here, into `cache.sqlite`, and that cache is what survives of them. The
//! consequence is D5, honoured in [`crate::labels::load_project`]: a cache whose
//! label files are gone is trusted rather than declared stale, or every run
//! after somebody deletes their installation would try to rebuild from nothing.
//!
//! **Offline.** No adapter is opened and no car is addressed.
//!
//! ## Running it twice
//!
//! Each VCDS step is skipped when what it would write is already newer than what
//! it would read, and `--refresh` forces the lot. That is [`crate::labels`]'s
//! rule — a cache is trusted only while it is newer than the label files it came
//! from — applied to the other artefacts rather than a second rule invented for
//! them. It matters because the names step is minutes of CPU.

pub mod source;
pub mod vendor;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Where a VCDS installation keeps the ODX files, relative to its root.
///
/// A property of Ross-Tech's layout, not of any car.
const ODX_DIR: &str = "UDS_EV";

/// The global text table every measurement name comes out of.
///
/// **Ross-Tech names it per language build**, and that is not a detail: the
/// English one ships `TTTEXT.ROD`, the Russian one `TTText-RUS.rod`. Matching
/// only the English spelling left anyone who chose Russian with an install that
/// recovered no names at all and never said why. Nothing suggests the list is
/// closed, so a name nobody here has seen is a question to ask, not a verdict.
const TEXT_TABLES: &[&str] = &["TTTEXT.ROD", "TTText-RUS.rod"];

/// The fault text store, one file in the install root beside `Labels/`.
///
/// Same story as [`TEXT_TABLES`]: `Codes.dat` in the English build,
/// `Code-RUS.dat` in the Russian one.
pub(crate) const CODES_FILES: &[&str] = &["Codes.dat", "Code-RUS.dat"];

/// Label files-wide `.rod` files whose keys every car needs.
///
/// `RD.rod` is the fault registry — the hop from a unit's own fault number to
/// the code that names it (`research/labels/fault-naming-hop.md`) — and `MUX.rod`
/// carries the shared multiplexer tables. Both are one file for the whole
/// label files, so recovering their keys once serves every vehicle.
///
/// Per-unit files are deliberately not swept. There are over sixteen thousand
/// of them, a blocked section costs about a minute of every core, and which
/// handful a given car needs is a question only that car can answer — it names
/// its own file in identifier `F19E`.
const SHARED_ROD_FILES: &[&str] = &["RD.rod", "MUX.rod"];

/// A general English word list, where the system has one.
///
/// The attack on the text table is dictionary-driven, and the label files' own
/// label files are the strong prior; this is the weak one, for the words VW
/// uses that no label file happens to contain. Absent on many systems, which is
/// why it is looked for rather than required.
const SYSTEM_WORDS: &str = "/usr/share/dict/words";

/// Weight of the label files' own vocabulary against the general list.
///
/// The label files are in-domain: when both offer a reading, the label files' word
/// has to win, or the search prefers an English rarity to the term VW actually
/// uses.
const LABEL_WORD_WEIGHT: &str = "8";
const GENERAL_WORD_WEIGHT: &str = "1";

pub struct Options<'a> {
	/// The VCDS installation root. Without one, an installation is offered for
	/// download and the run continues into the same parse.
	pub dir: Option<&'a str>,
	/// Redo every step, whatever is already on disk.
	pub refresh: bool,
	/// Where the archives are served from. A parameter so the download path is
	/// testable against a local file rather than the network.
	pub archive_base: &'a str,
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
	odis: Option<vag_data::odis::Project>,
}

/// Ask what to read, work out what to call it, and open the store.
fn choose(io: &mut impl crate::ui::menu::Asker, opts: &Options<'_>) -> Result<Option<Chosen>> {
	let Some(source) = source::choose(io, opts.dir)? else { return Ok(None) };
	// The download is not a source, it is how one is obtained. Picking it from
	// the menu *is* the consent — `vendor::confirm_download` asked a second
	// `[y/N]` for the same decision, and one decision is one question.
	let source = match source {
		source::Source::DownloadVcds => source::Source::Vcds {
			dir: vendor::fetch(opts.archive_base)?,
		},
		named => named,
	};

	let existing = crate::project::list()?;
	let asked = source::project_id(io, &source, &existing)?;
	// Opened before a directory exists, so its own name can win.
	let odis = match &source {
		source::Source::Odis { dir } => Some(open_odis(io, dir)?),
		_ => None,
	};
	let id = match &odis {
		Some(project) => prefer_its_own_name(io, project.id(), &asked, &existing)?,
		None => asked,
	};
	Ok(Some(Chosen {
		source,
		project: crate::project::open_or_create(&id)?,
		existing,
		odis,
	}))
}

/// Open an ODIS project, saying how long it will be.
fn open_odis(io: &mut impl crate::ui::menu::Asker, dir: &Path) -> Result<vag_data::odis::Project> {
	io.say(&format!(
		"Opening the ODIS project at {} — its two string pools are read whole, which takes a moment.",
		dir.display()
	))?;
	let project = vag_data::odis::Project::open(dir).with_context(|| format!("reading the ODIS project at {}", dir.display()))?;
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
fn prefer_its_own_name(io: &mut impl crate::ui::menu::Asker, named: &str, asked: &str, existing: &[String]) -> Result<String> {
	if named == asked {
		return Ok(asked.to_string());
	}
	match crate::project::folder_name(named) {
		Ok(own) => {
			let merge = match existing.iter().any(|id| *id == own) {
				true => " That project is already here: this source is added to it, and nothing already in it is replaced.",
				false => "",
			};
			io.say(&format!(
				"The project calls itself `{own}` in its own index.xml, so that is what it is filed under — \
                 not `{asked}`, which is only what the folder happens to be called. One car, one store.{merge}"
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
	if !std::io::stdin().is_terminal() {
		return Ok(None);
	}
	println!(
		"\n      None of {known:?} is in {}.\n      \
         Ross-Tech names this file differently in each language build, so pick the\n      \
         {what} out of what is actually there:",
		dir.display()
	);
	let mut chooser = crate::ui::picker::Console::new(format!(
		"re-run `vagcan setup` from a terminal, or point it at a build whose {what} is one of {known:?}"
	));
	crate::ui::picker::pick_path(&mut chooser, dir, &[crate::ui::picker::Level::files(what).ending(suffix)])
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
}

pub fn run(opts: Options<'_>) -> Result<()> {
	let mut io = crate::ui::menu::Console::new("vagcan setup /path/to/VCDS      (or the path to an extracted ODIS project)");
	run_with(&mut io, opts)
}

/// The rule behind [`run`], with the asking behind [`crate::ui::menu::Asker`] so
/// the flow is testable without a terminal.
fn run_with(io: &mut impl crate::ui::menu::Asker, opts: Options<'_>) -> Result<()> {
	let Some(chosen) = choose(io, &opts)? else { return Ok(()) };
	let project = &chosen.project;
	io.say(&format!("Writing into {}\n", project.dir.display()))?;

	// Before the parse, not after: whatever an older build left in
	// `~/.vagcan/data/` belongs to this car too, and moving it afterwards would
	// have this run's rows sitting beside an unmigrated copy of the last one's.
	if let Some(old) = crate::migrate::pending()? {
		let report = crate::migrate::run(&old, project)?;
		if report.moved() > 0 || report.left_behind.is_some() {
			io.say(&crate::migrate::describe(&report, project))?;
		}
	}

	let steps = match &chosen.source {
		source::Source::Odis { dir } => {
			let odis = chosen.odis.as_ref().expect("an ODIS source opens its project in `choose`");
			read_odis(odis, dir, project)?
		}
		source::Source::Vcds { dir } => read_vcds(dir, project, opts.refresh)?,
		// `choose` turns a download into the installation it fetched.
		source::Source::DownloadVcds => unreachable!("the download is resolved to an installation before this point"),
	};

	// Written down so a later command needs no flag. Not a preference — the
	// answer to "which car did I just set up", which is the one a bare
	// `vagcan faults` has to be able to reach.
	crate::project::remember(&project.id)?;
	println!("\n{}", report(&steps));
	Ok(())
}

/// The VCDS branch: the four steps this command has always run, into a project.
fn read_vcds(root: &Path, project: &crate::project::Project, refresh: bool) -> Result<Vec<Step>> {
	let pool = crate::project::rod_pool()?;
	replace_if_another_build(root, &pool, refresh)?;
	std::fs::create_dir_all(&pool).with_context(|| format!("creating {}", pool.display()))?;
	println!("Reading the VCDS installation at {}", root.display());

	// The copy runs first and the derivations then read from it, so afterwards
	// `~/.vagcan` is the one set of raw files everything points at and the
	// installation can go.
	let steps = vec![
		copy_label_files(root, &pool, refresh)?,
		label_cache(root, project, refresh)?,
		names(&pool, root, project, refresh)?,
		rod_keys(&pool, project)?,
	];
	crate::project::record_source(
		project,
		crate::project::SourceEntry {
			kind: vag_db::VCDS,
			path: root.display().to_string(),
			version: build_of(root).map(str::to_string),
			detail: None,
		},
	)?;
	Ok(steps)
}

/// The ODIS branch: every variant's channels into `cache.sqlite`, every name it
/// knows into `names.json`.
///
/// **A variant that will not read costs itself and nothing else.** A project
/// describes hundreds of control units, and one whose measurement chain reaches
/// a refused type ([`vag_data::odis::Error::Refused`] — a flash job, an access
/// key) or a shape this reader has no loader for must not cost the other
/// hundreds. The count of what was skipped is reported rather than hidden.
fn read_odis(odis: &vag_data::odis::Project, dir: &Path, project: &crate::project::Project) -> Result<Vec<Step>> {
	println!("Reading the ODIS project at {}", dir.display());
	let source = dir.display().to_string();

	println!("[1/2] Control units — walking each variant's measurement chain.");
	let variants = odis.variants().with_context(|| format!("listing the variants of {}", dir.display()))?;
	let (mut with_channels, mut channels, mut refused, mut unreadable) = (0usize, 0usize, 0usize, 0usize);
	let mut progress = crate::progress::Line::new();
	for (at, variant) in variants.iter().enumerate() {
		progress.update(&format!("{} of {} — {}", at + 1, variants.len(), variant.name));
		let readings = match odis.readings(variant) {
			Ok(readings) => readings,
			// The refusal list is enforced by the parser and honoured here: a
			// refused type is a file this tool declines to read, not a broken
			// one, so the variant is skipped and the rest of the project stands.
			Err(vag_data::odis::Error::Refused(_)) => {
				refused += 1;
				continue;
			}
			Err(_) => {
				unreadable += 1;
				continue;
			}
		};
		if readings.is_empty() {
			continue;
		}
		channels += vag_db::put_readings(&project.cache(), &source, &variant.name, &readings)
			.map_err(|e| anyhow::anyhow!("writing {}'s channels to {}: {e}", variant.name, project.cache().display()))?;
		with_channels += 1;
	}
	progress.finish();
	let mut skipped = String::new();
	if refused + unreadable > 0 {
		skipped = format!(", {refused} refused, {unreadable} unreadable");
	}
	let units = Step::Wrote {
		what: "the control units this project describes",
		path: project.cache(),
		detail: format!("{with_channels} of {} variants, {channels} channels{skipped}", variants.len()),
	};

	println!("[2/2] Names — every object in every pool, for the (text id, name) pairs they carry.");
	let names = {
		let _spinner = crate::progress::Spinner::new("reading every object in the project".to_string());
		odis.names().with_context(|| format!("reading the names of {}", dir.display()))?
	};
	let merged = merge_names(&project.names(), names)?;

	crate::project::record_source(
		project,
		crate::project::SourceEntry {
			kind: vag_db::ODIS,
			path: source,
			version: odis.version().map(str::to_string),
			detail: Some(format!("{} variants, {} pools", variants.len(), odis.pools().len())),
		},
	)?;
	Ok(vec![
		units,
		Step::Wrote {
			what: "the measurement names",
			path: project.names(),
			detail: format!("{merged} names"),
		},
	])
}

/// Fold an ODIS project's names into whatever `names.json` already holds.
///
/// **What is already there wins.** `names.json` is keyed by the label files' own
/// text id, and the two sources agree about what that id means — that is the
/// whole finding `research/labels/odis-crib.md` rests on. Where they disagree,
/// the incumbent is what every earlier run of this tool has been reporting, and
/// silently changing a name under somebody is worse than not adding one.
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

/// Step 1: copy the raw files into the shared pool, so the installation is
/// disposable.
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
fn copy_label_files(root: &Path, target: &Path, refresh: bool) -> Result<Step> {
	println!("[1/4] Raw files — copying the .rod files and the fault text into the shared pool, so the installation can be deleted afterwards.");
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
	match locate(root, CODES_FILES, "fault text file", ".dat")? {
		Some(codes) => {
			let name = codes.file_name().unwrap_or_default();
			plan.push((codes.clone(), target.join(name)));
		}
		None => println!("      the fault text file: not in this installation, skipped — faults will read as numbers"),
	}
	if plan.is_empty() {
		return Ok(Step::Missing {
			what: "the raw files",
			why: format!("no {ODX_DIR}/ and no fault text under {}", root.display()),
		});
	}

	let total = plan.len();
	let (mut copied, mut skipped) = (0usize, 0usize);
	let mut progress = crate::progress::Line::new();
	for (at, (src, dst)) in plan.iter().enumerate() {
		if !refresh && is_newer(dst, src) {
			skipped += 1;
			continue;
		}
		if let Some(parent) = dst.parent() {
			std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
		}
		progress.update(&format!("copying — {} of {total}", at + 1));
		std::fs::copy(src, dst).with_context(|| format!("copying {} to {}", src.display(), dst.display()))?;
		copied += 1;
	}
	progress.finish();
	Ok(Step::Wrote {
		what: "the raw files",
		path: target.to_path_buf(),
		detail: format!("{copied} files copied, {skipped} already current"),
	})
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
/// them. This is the one moment they are ever read, and after it the
/// installation can go.
fn label_cache(root: &Path, project: &crate::project::Project, refresh: bool) -> Result<Step> {
	println!("[2/4] Label files — parsing every .lbl and decrypting every .clb.");
	let db = crate::labels::load_cached(root, &project.cache(), refresh)?;
	Ok(Step::Wrote {
		what: "the label files",
		path: project.cache(),
		detail: format!("{} label files", db.len()),
	})
}

/// Step 3: recover the measurement names from the global text table.
///
/// Two of the existing tools, chained the way `vagcan vcds`'s own help
/// documents: `rod --dump` writes the decrypted, inflated `[TXT]` section, and
/// `tttext` reads it. The intermediate file is this function's business and
/// nobody else's, so it goes in a scratch directory and is removed again.
fn names(pool: &Path, install: &Path, project: &crate::project::Project, refresh: bool) -> Result<Step> {
	// The text table is read out of the pool it was just copied into, so the
	// keys recovered from it match the bytes every later run will open.
	let odx = pool.to_path_buf();
	let out = project.names();
	let Some(source) = locate(&odx, TEXT_TABLES, "measurement text table", ".rod")? else {
		return Ok(Step::Missing {
			what: "the measurement names",
			why: format!("none of {TEXT_TABLES:?} is under {}", odx.display()),
		});
	};
	if !refresh && is_newer(&out, &source) {
		println!("[3/4] Measurement names — already recovered from this installation.");
		return Ok(Step::Skipped {
			what: "the measurement names",
			path: out,
			why: "newer than the text table it came from",
		});
	}

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
		&& vag_data::rod::key_cost(&bytes, "TXT") == Some(vag_data::rod::KeyCost::AnchorSweep)
	{
		{
			println!("[3/4] Measurement names — skipped: {name} masks its key.");
			return Ok(Step::Missing {
				what: "the measurement names",
				why: format!(
					"{name} is a *shifted* container, so its text section has no anchor to search from — \
                     the only route is every legal anchor against the full space, which is hours to days \
                     rather than the minute or two an ordinary table costs. Everything else in this \
                     installation is recovered; only the names are out of reach. See \
                     research/labels/tttext2.md §3.3"
				),
			});
		}
	}
	println!("[3/4] Measurement names — opening {name}, then reading its cipher.");
	let scratch = out.with_file_name("tttext-scratch");
	let _ = std::fs::remove_dir_all(&scratch);
	crate::vcds::rod::run(
		&source.to_string_lossy(),
		true,
		Some(&project.rod_keys().to_string_lossy()),
		Some(&scratch.to_string_lossy()),
	)?;
	let text = scratch.join("TXT.bin");
	if !text.is_file() {
		let _ = std::fs::remove_dir_all(&scratch);
		let why = format!("the [TXT] section of {} did not decode — see the section listing above", source.display());
		return Ok(Step::Missing {
			what: "the measurement names",
			why,
		});
	}

	// The installation's own label files are the strong prior for the attack.
	// They are read here and copied nowhere (D4) — this is the last moment they
	// are in reach, which is why the installation is still a parameter.
	let mut words = vec![format!("{}:{LABEL_WORD_WEIGHT}", install.join("Labels").display())];
	if Path::new(SYSTEM_WORDS).exists() {
		words.push(format!("{SYSTEM_WORDS}:{GENERAL_WORD_WEIGHT}"));
	}
	let coverage = crate::vcds::tttext::run(crate::vcds::tttext::Options {
		file: &text.to_string_lossy(),
		words: &words,
		names: None,
		// The readings themselves are not wanted here — only the ones that
		// clear the gate, in the form `vagcan vcds names` searches.
		out: None,
		catalog: Some(&out.to_string_lossy()),
		partial: None,
		passes: 4,
		steps: None,
		check: 0,
		gated: false,
	})?;
	let _ = std::fs::remove_dir_all(&scratch);

	let count = std::fs::read_to_string(&out)
		.ok()
		.and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
		.and_then(|v| v.as_object().map(|m| m.len()))
		.unwrap_or(0);
	// The attack withholds any record it could not read outright, so falling
	// short is the expected outcome, not a failure — but it has to be said. A
	// bare "124294 names" reads as the whole table to somebody who has no idea
	// how big the table is.
	if coverage.read < coverage.candidates {
		let pct = 100.0 * coverage.read as f32 / coverage.candidates.max(1) as f32;
		let short = coverage.total.saturating_sub(coverage.candidates);
		return Ok(Step::Partial {
			what: "the measurement names",
			path: out,
			detail: format!("{count} names"),
			why: format!(
				"{} of {} records long enough to carry a name were read ({pct:.1} %); the rest held a \
				 letter the attack could not settle, and a half-read name is worse than none, so they \
				 are withheld rather than guessed. A further {short} records in the table are under a \
				 dozen letters — acronyms and status codes, which are not names and are not counted \
				 against this",
				coverage.read, coverage.candidates
			),
		});
	}
	Ok(Step::Wrote {
		what: "the measurement names",
		path: out,
		detail: format!("{count} names"),
	})
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
	println!("[4/4] .rod section keys — searching for the ones not already cached.");
	for file in &present {
		crate::vcds::rod::run(&file.to_string_lossy(), true, Some(&cache.to_string_lossy()), None)?;
	}
	let keys = std::fs::read_to_string(&cache)
		.ok()
		.and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
		.and_then(|v| v.as_object().map(|m| m.len()))
		.unwrap_or(0);
	Ok(Step::Wrote {
		what: "the .rod section keys",
		path: cache,
		detail: format!("{keys} keys"),
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

/// The closing report: what is on disk now, and what to do with it.
///
/// Every line names a file. Somebody who has just waited several minutes is
/// owed the paths, not a count of successes — and somebody whose run was short
/// of one artefact needs to see which one without re-reading the scroll.
fn report(steps: &[Step]) -> String {
	use std::fmt::Write as _;

	// "Done." on its own reads as full success; when an artefact is missing the
	// header has to say so, or a reader takes the fast finish for a complete one.
	// A partial artefact counts as a gap too: a run that recovered 63 % of the
	// names finished successfully and is still not what "Done." promises.
	let any_gap = steps.iter().any(|s| matches!(s, Step::Missing { .. } | Step::Partial { .. }));
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
		}
	}
	if steps.iter().any(|s| matches!(s, Step::Missing { .. })) {
		let _ = writeln!(
			out,
			"\nThe rest is usable. What is missing above is missing from the installation \n\
             that was read, so a different or newer VCDS may have it."
		);
	}
	let _ = write!(
		out,
		"\nNext:  vagcan devices      is the adapter connected?\n       \
         vagcan info         which car is this?\n       \
         vagcan faults       stored faults, named — the labels are copied in now\n\n\
         Scalings are a separate thing and no installation carries them — the label files \n\
         has names, not numbers. Those are measured: `vagcan survey`, then \n\
         `vagcan watch --out drive.csv`, then `vagcan recording calibrate`."
	);
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_report_names_every_file_it_wrote() {
		// Somebody who has just waited several minutes is owed the paths. A
		// count of successes tells them nothing they can open.
		let steps = vec![
			Step::Wrote {
				what: "the label files",
				path: PathBuf::from("/home/x/.vagcan/projects/SK37X/cache.sqlite"),
				detail: "3035 label files".to_string(),
			},
			Step::Skipped {
				what: "the measurement names",
				path: PathBuf::from("/home/x/.vagcan/projects/SK37X/names.json"),
				why: "newer than the text table it came from",
			},
		];
		let r = report(&steps);
		assert!(r.contains("/home/x/.vagcan/projects/SK37X/cache.sqlite"), "{r}");
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
		assert_eq!(prefer_its_own_name(&mut io, "SK37X", "SK-37X-copy", &[]).unwrap(), "SK37X");
		let said = io.all_said();
		assert!(said.contains("SK37X"), "{said}");
		assert!(said.contains("One car, one store"), "it says why: {said}");

		// Agreement is silent — there is nothing to explain.
		let mut io = crate::ui::menu::Scripted::new(vec![]);
		assert_eq!(prefer_its_own_name(&mut io, "SK37X", "SK37X", &[]).unwrap(), "SK37X");
		assert!(io.said.is_empty(), "{:?}", io.said);

		// A `<SHORT-NAME>` that could not be a directory falls back rather than
		// being sanitised: a mangled name files a car where nothing looks for it.
		let mut io = crate::ui::menu::Scripted::new(vec![]);
		assert_eq!(prefer_its_own_name(&mut io, "../escape", "SK37X", &[]).unwrap(), "SK37X");
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
		assert_eq!(prefer_its_own_name(&mut io, "SK37X", "SK-37X-1", &existing).unwrap(), "SK37X");
		let said = io.all_said();
		assert!(said.contains("already here"), "{said}");
		assert!(said.contains("nothing already in it is replaced"), "{said}");

		// A swap onto a name nothing holds is still new, and says nothing extra.
		let mut io = crate::ui::menu::Scripted::new(vec![]);
		assert_eq!(prefer_its_own_name(&mut io, "SK37X", "SK-37X-1", &[]).unwrap(), "SK37X");
		assert!(!io.all_said().contains("already here"), "{:?}", io.said);
	}

	#[test]
	fn a_name_already_in_names_json_is_not_changed_under_somebody() {
		// The two sources agree about what a text id means — that is the whole
		// finding `research/labels/odis-crib.md` rests on — so where they do
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

	#[test]
	fn a_step_that_could_not_run_says_so_without_condemning_the_rest() {
		let steps = vec![Step::Missing {
			what: "the .rod section keys",
			why: "none of them is in this installation".to_string(),
		}];
		let r = report(&steps);
		assert!(r.contains("NOT recovered"), "{r}");
		assert!(r.contains("The rest is usable"), "{r}");
	}

	#[test]
	fn the_report_does_not_promise_scalings_the_label_files_cannot_supply() {
		// The single most expensive misunderstanding available here: a reader
		// who has just parsed 300 MB of label files reasonably assumes the
		// numbers came with the names. They did not, and the closing lines are
		// the last chance to say so.
		let r = report(&[]);
		assert!(r.contains("no installation carries them"), "{r}");
		assert!(r.contains("recording calibrate"), "{r}");
	}

	#[test]
	fn a_run_against_something_that_is_not_an_installation_says_where_to_get_one() {
		let mut io = crate::ui::menu::Scripted::new(vec![]);
		let err = run_with(
			&mut io,
			Options {
				dir: Some("/definitely/not/here"),
				refresh: false,
				archive_base: vendor::ARCHIVE_BASE,
			},
		)
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
		let step = copy_label_files(&install.0, &target.0, false).unwrap();
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

		let first = copy_label_files(&install.0, &target.0, false).unwrap();
		assert_eq!(detail(&first), "3 files copied, 0 already current");

		let second = copy_label_files(&install.0, &target.0, false).unwrap();
		assert_eq!(detail(&second), "0 files copied, 3 already current", "a no-op rerun");

		// One destination made to look stale: only it is copied again.
		let stale = target.0.join("Codes.dat");
		let past = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
		std::fs::File::options().write(true).open(&stale).unwrap().set_modified(past).unwrap();
		let third = copy_label_files(&install.0, &target.0, false).unwrap();
		assert_eq!(detail(&third), "1 files copied, 2 already current");

		// --refresh copies everything regardless of mtimes.
		let forced = copy_label_files(&install.0, &target.0, true).unwrap();
		assert_eq!(detail(&forced), "3 files copied, 0 already current");
	}

	#[test]
	fn an_install_missing_every_input_is_reported_not_a_crash() {
		let empty = TempDir::new("empty");
		let target = TempDir::new("empty-out");
		let step = copy_label_files(&empty.0, &target.0, false).unwrap();
		match step {
			Step::Missing { why, .. } => assert!(why.contains(ODX_DIR), "{why}"),
			other => panic!("expected Missing, got {other:?}"),
		}
	}

	#[test]
	fn a_missing_output_is_never_mistaken_for_a_current_one() {
		let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
		assert!(!is_newer(Path::new("/definitely/not/here"), &here));
		assert!(!is_newer(&here, Path::new("/definitely/not/here")));
		assert!(is_newer(&here, &here), "a file is not older than itself");
	}
}
