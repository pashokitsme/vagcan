//! `vagcan setup <VCDS-DIR>` — the one command that makes this tool usable.
//!
//! Everything the label files contribute is derived from a VCDS installation,
//! and none of it may be redistributed: it is Ross-Tech's data. So it is not in
//! this repository and never will be, and the price of that is a step somebody
//! has to run once. This is that step, and it is deliberately one command with
//! one argument.
//!
//! Four things happen, and after them the installation can be deleted:
//!
//! | what | how | where it lands |
//! |---|---|---|
//! | the raw label files, copied | [`copy_label_files`] | `~/.vagcan/data/extracted/{UDS_EV,Labels,Codes.dat}` |
//! | the label files, parsed | [`crate::labels::load_cached`] | `~/.vagcan/data/extracted/cache.sqlite` |
//! | measurement names | [`crate::vcds::rod`] then [`crate::vcds::tttext`] | `~/.vagcan/data/extracted/names.json` |
//! | `.rod` section keys | [`crate::vcds::rod`] | `~/.vagcan/data/extracted/rod-keys.json` |
//!
//! The copy is what makes the installation disposable. Fault naming reads label
//! files straight off disk at run time — `UDS_EV/` (every `.rod`, incl. the
//! `RD.rod` registry and `MUX.rod`), `Labels/` (the pre-UDS `.lbl`/`.clb`) and
//! `Codes.dat` (the fault text store) — so those ~122 MB have to outlive the
//! ~145 MB install, and the owner accepted that cost so the rest can go. The
//! copy runs **first**, and the three derivations then read from the copy, so
//! `~/.vagcan` is the one set of label files everything afterwards points at.
//!
//! The three derivations are not new work — each already had a tool, reachable
//! only by knowing it existed, what to feed it, and what it left behind, which
//! is a poor thing to ask of somebody who has just cloned a repository.
//!
//! **Offline.** No adapter is opened and no car is addressed.
//!
//! ## Running it twice
//!
//! Each step is skipped when what it would write is already newer than what it
//! would read, and `--refresh` forces the lot. That is [`crate::labels`]'s rule
//! — a cache is trusted only while it is newer than the label files it came from —
//! applied to the other two artefacts rather than a second rule invented for
//! them. It matters because the names step is minutes of CPU: a second
//! `vagcan setup` on an unchanged installation has nothing to do and should
//! take a second to establish that.

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

/// The directories copied out of the installation so it can then be deleted.
///
/// The exact set fault naming and label lookup read off disk at run time:
/// `UDS_EV/` holds every `.rod` (the `RD.rod` registry, the shared `MUX.rod`,
/// and each unit's own file, named by its `F19E`), and `Labels/` the pre-UDS
/// `.lbl`/`.clb`. Copied whole, so the per-unit `.rod` a given car will name —
/// unknowable here — are all there. The fault text file goes with them, under
/// whichever of [`CODES_FILES`] this build uses.
const LABEL_FILE_INPUTS: &[&str] = &[ODX_DIR, "Labels"];

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
	/// Which language build to download, when one is being downloaded.
	pub lang: Option<&'a str>,
	/// Redo every step, whatever is already on disk.
	pub refresh: bool,
	/// Where the archives are served from. A parameter so the download path is
	/// testable against a local file rather than the network.
	pub archive_base: &'a str,
}

/// The installation this run will read, fetching one if that is the answer.
///
/// Returning `None` is a complete, successful outcome: somebody who declines
/// the download has been told where to get VCDS themselves, and a non-zero exit
/// would be this tool disagreeing with a decision it offered them.
fn installation(opts: &Options<'_>) -> Result<Option<PathBuf>> {
	if let Some(dir) = opts.dir {
		let root = Path::new(dir);
		anyhow::ensure!(
			root.is_dir(),
			"{dir:?} is not a directory.\n\n\
             Point this at a VCDS installation root — the directory holding \
             `Labels/` and `{ODX_DIR}/`.\n\
             With no path at all, `vagcan setup` offers to download one.\n\
             Ross-Tech's own: {}",
			crate::missing::VCDS_DOWNLOAD
		);
		return Ok(Some(root.to_path_buf()));
	}
	// A language on the command line is somebody who has already decided;
	// asking them again is a prompt with one answer.
	if opts.lang.is_none() && !vendor::confirm_download()? {
		println!(
			"Nothing downloaded.\n\n\
             Point at an installation you have:\n    \
             vagcan setup /path/to/VCDS\n\n\
             Ross-Tech's own download page: {}",
			crate::missing::VCDS_DOWNLOAD
		);
		return Ok(None);
	}
	let lang = vendor::choose_language(opts.lang)?;
	Ok(Some(vendor::fetch(&lang, opts.archive_base)?))
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
fn keyed_already(source: &Path) -> Result<bool> {
	let cache = crate::datadir::rod_keys()?;
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
	let Some(root) = installation(&opts)? else { return Ok(()) };
	let root = root.as_path();
	let target = crate::datadir::extracted_dir()?;
	replace_if_another_build(root, &target, opts.refresh)?;
	std::fs::create_dir_all(&target).with_context(|| format!("creating {}", target.display()))?;

	println!("Reading the VCDS installation at {}", root.display());
	println!("Writing everything to {}\n", target.display());

	// The copy runs first, and the derivations then read from it: after this,
	// `~/.vagcan` is the one set of label files everything points at, and the install can go.
	let steps = [
		copy_label_files(root, &target, opts.refresh)?,
		label_cache(&target, opts.refresh)?,
		names(&target, opts.refresh)?,
		rod_keys(&target)?,
	];

	println!("\n{}", report(&steps));
	Ok(())
}

/// Step 1: copy the raw label files in, so the installation is disposable.
///
/// The directories [`LABEL_FILE_INPUTS`] names plus this build's fault text
/// file, copied preserving their layout, so afterwards
/// `~/.vagcan/data/extracted/` holds `UDS_EV/`, `Labels/` and the fault text.
/// Idempotent and freshness-gated per file, the same rule the rest of setup
/// follows: a file is copied only when it is missing from the destination or
/// newer than what is there, and `--refresh` copies the lot.
fn copy_label_files(root: &Path, target: &Path, refresh: bool) -> Result<Step> {
	println!("[1/4] Raw label files — copying UDS_EV/, Labels/ and the fault text (~122 MB) so the installation can be deleted afterwards.");
	let mut plan: Vec<(PathBuf, PathBuf)> = Vec::new();
	for name in LABEL_FILE_INPUTS {
		let src = root.join(name);
		if !src.exists() {
			// A stripped or partial installation still yields whatever it has;
			// a missing input is reported, not fatal.
			println!("      {name}: not in this installation, skipped");
			continue;
		}
		collect_copies(&src, &target.join(name), &mut plan)?;
	}
	// The fault text, under whichever name this language build gives it. Copied
	// under that same name: `faultnames` looks for the whole list too, so the
	// build stays recognisable rather than being flattened to the English one.
	match locate(root, CODES_FILES, "fault text file", ".dat")? {
		Some(codes) => {
			let name = codes.file_name().unwrap_or_default();
			collect_copies(&codes, &target.join(name), &mut plan)?;
		}
		None => println!("      the fault text file: not in this installation, skipped — faults will read as numbers"),
	}
	if plan.is_empty() {
		return Ok(Step::Missing {
			what: "the raw label files",
			why: format!("none of {LABEL_FILE_INPUTS:?} is under {}", root.display()),
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
		what: "the raw label files",
		path: target.to_path_buf(),
		detail: format!("{copied} files copied, {skipped} already current"),
	})
}

/// Every file under `src`, paired with where it lands under `dst`.
///
/// A file (`Codes.dat`) is one pair; a directory is walked. The plan is built
/// before anything is written, so the copy can report progress against a known
/// total and an unreadable directory fails before it has half-copied a tree.
fn collect_copies(src: &Path, dst: &Path, plan: &mut Vec<(PathBuf, PathBuf)>) -> Result<()> {
	if src.is_file() {
		plan.push((src.to_path_buf(), dst.to_path_buf()));
		return Ok(());
	}
	if src.is_dir() {
		for entry in std::fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
			let entry = entry?;
			collect_copies(&entry.path(), &dst.join(entry.file_name()), plan)?;
		}
	}
	Ok(())
}

/// Step 2: parse the copied label files into the SQLite cache.
///
/// Reads the copy under `~/.vagcan`, not the installation, so the source it
/// records is the one the runtime path later passes —
/// otherwise the first `units --identify` after setup would rebuild the cache
/// from a directory whose name no longer matched.
fn label_cache(root: &Path, refresh: bool) -> Result<Step> {
	println!("[2/4] Label files — parsing every .lbl and decrypting every .clb.");
	let db = crate::labels::load_cached(root, refresh)?;
	Ok(Step::Wrote {
		what: "the label files",
		path: crate::datadir::label_cache()?,
		detail: format!("{} label files", db.len()),
	})
}

/// Step 3: recover the measurement names from the global text table.
///
/// Two of the existing tools, chained the way `vagcan vcds`'s own help
/// documents: `rod --dump` writes the decrypted, inflated `[TXT]` section, and
/// `tttext` reads it. The intermediate file is this function's business and
/// nobody else's, so it goes in a scratch directory and is removed again.
fn names(root: &Path, refresh: bool) -> Result<Step> {
	let odx = root.join(ODX_DIR);
	let out = crate::datadir::names_catalog()?;
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
	if !keyed_already(&source)?
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
		Some(&crate::datadir::rod_keys()?.to_string_lossy()),
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

	let mut words = vec![format!("{}:{LABEL_WORD_WEIGHT}", root.join("Labels").display())];
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
	if coverage.read < coverage.total {
		let pct = 100.0 * coverage.read as f32 / coverage.total.max(1) as f32;
		return Ok(Step::Partial {
			what: "the measurement names",
			path: out,
			detail: format!("{count} names"),
			why: format!(
				"{} of {} records in the text table were read ({pct:.1} %). The rest held a letter \
				 the attack could not settle, and a half-read name is worse than none, so they are \
				 withheld rather than guessed",
				coverage.read, coverage.total
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
fn rod_keys(root: &Path) -> Result<Step> {
	let cache = crate::datadir::rod_keys()?;
	let present: Vec<PathBuf> = SHARED_ROD_FILES
		.iter()
		.map(|name| root.join(ODX_DIR).join(name))
		.filter(|p| p.is_file())
		.collect();
	if present.is_empty() {
		return Ok(Step::Missing {
			what: "the .rod section keys",
			why: format!("none of {SHARED_ROD_FILES:?} is under {}", root.join(ODX_DIR).display()),
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
				path: PathBuf::from("/home/x/.vagcan/data/extracted/cache.sqlite"),
				detail: "3035 label files".to_string(),
			},
			Step::Skipped {
				what: "the measurement names",
				path: PathBuf::from("/home/x/.vagcan/data/extracted/names.json"),
				why: "newer than the text table it came from",
			},
		];
		let r = report(&steps);
		assert!(r.contains("/home/x/.vagcan/data/extracted/cache.sqlite"), "{r}");
		assert!(r.contains("3035 label files"), "{r}");
		// A skipped step is reported, not silently absent: a run that took a
		// second when minutes were expected reads as a failure otherwise.
		assert!(r.contains("unchanged"), "{r}");
		assert!(r.contains("names.json"), "{r}");
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
		let err = run(Options {
			dir: Some("/definitely/not/here"),
			lang: None,
			refresh: false,
			archive_base: vendor::ARCHIVE_BASE,
		})
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
	fn the_copy_carries_the_three_inputs_preserving_their_layout() {
		// After it, ~/.vagcan holds the exact tree fault naming reads off disk —
		// UDS_EV/ (registry and each unit's own .rod), Labels/, Codes.dat.
		let install = synthetic_install("layout");
		let target = TempDir::new("layout-out");
		let step = copy_label_files(&install.0, &target.0, false).unwrap();
		assert!(detail(&step).starts_with("4 files copied"), "{}", detail(&step));
		for rel in ["UDS_EV/RD.rod", "UDS_EV/EV_ECM.rod", "Labels/part.lbl", "Codes.dat"] {
			assert!(target.0.join(rel).is_file(), "{rel} did not land under the target");
		}
	}

	#[test]
	fn copying_is_idempotent_and_freshness_gated_per_file() {
		// The rule the rest of setup follows: a second run has nothing to do,
		// one changed file recopies only itself, and --refresh copies the lot.
		let install = synthetic_install("fresh");
		let target = TempDir::new("fresh-out");

		let first = copy_label_files(&install.0, &target.0, false).unwrap();
		assert_eq!(detail(&first), "4 files copied, 0 already current");

		let second = copy_label_files(&install.0, &target.0, false).unwrap();
		assert_eq!(detail(&second), "0 files copied, 4 already current", "a no-op rerun");

		// One destination made to look stale: only it is copied again.
		let stale = target.0.join("Codes.dat");
		let past = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
		std::fs::File::options().write(true).open(&stale).unwrap().set_modified(past).unwrap();
		let third = copy_label_files(&install.0, &target.0, false).unwrap();
		assert_eq!(detail(&third), "1 files copied, 3 already current");

		// --refresh copies everything regardless of mtimes.
		let forced = copy_label_files(&install.0, &target.0, true).unwrap();
		assert_eq!(detail(&forced), "4 files copied, 0 already current");
	}

	#[test]
	fn an_install_missing_every_input_is_reported_not_a_crash() {
		let empty = TempDir::new("empty");
		let target = TempDir::new("empty-out");
		let step = copy_label_files(&empty.0, &target.0, false).unwrap();
		match step {
			Step::Missing { why, .. } => assert!(why.contains("UDS_EV"), "{why}"),
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
