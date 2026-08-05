//! `vagcan setup <VCDS-DIR>` — the one command that makes this tool usable.
//!
//! Everything the label corpus contributes is derived from a VCDS installation,
//! and none of it may be redistributed: it is Ross-Tech's data. So it is not in
//! this repository and never will be, and the price of that is a step somebody
//! has to run once. This is that step, and it is deliberately one command with
//! one argument.
//!
//! Three things come out of an installation, and each already had a tool:
//!
//! | what | how | where it lands |
//! |---|---|---|
//! | the label corpus, parsed | [`crate::labels::load_cached`] | `~/.vagcan/data/extracted/cache.sqlite` |
//! | measurement names | [`crate::vcds::rod`] then [`crate::vcds::tttext`] | `~/.vagcan/data/extracted/names.json` |
//! | `.rod` section keys | [`crate::vcds::rod`] | `~/.vagcan/data/extracted/rod-keys.json` |
//!
//! Nothing here is new work — this module runs the three in order, with the
//! arguments they want, and says what happened. That matters more than it
//! sounds: each was reachable only by knowing it existed, what to feed it, and
//! what it left behind, which is a poor thing to ask of somebody who has just
//! cloned a repository.
//!
//! **Offline.** No adapter is opened and no car is addressed.
//!
//! ## Running it twice
//!
//! Each step is skipped when what it would write is already newer than what it
//! would read, and `--refresh` forces the lot. That is [`crate::labels`]'s rule
//! — a cache is trusted only while it is newer than the corpus it came from —
//! applied to the other two artefacts rather than a second rule invented for
//! them. It matters because the names step is minutes of CPU: a second
//! `vagcan setup` on an unchanged installation has nothing to do and should
//! take a second to establish that.

pub mod vendor;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Where a VCDS installation keeps the ODX files, relative to its root.
///
/// A property of Ross-Tech's layout, not of any car.
const ODX_DIR: &str = "UDS_EV";

/// The global text table every measurement name comes out of.
const TEXT_TABLE: &str = "TTTEXT.ROD";

/// Corpus-wide `.rod` files whose keys every car needs.
///
/// `RD.rod` is the fault registry — the hop from a unit's own fault number to
/// the code that names it (`research/labels/fault-naming-hop.md`) — and `MUX.rod`
/// carries the shared multiplexer tables. Both are one file for the whole
/// corpus, so recovering their keys once serves every vehicle.
///
/// Per-unit files are deliberately not swept. There are over sixteen thousand
/// of them, a blocked section costs about a minute of every core, and which
/// handful a given car needs is a question only that car can answer — it names
/// its own file in identifier `F19E`.
const SHARED_ROD_FILES: &[&str] = &["RD.rod", "MUX.rod"];

/// A general English word list, where the system has one.
///
/// The attack on the text table is dictionary-driven, and the corpus's own
/// label files are the strong prior; this is the weak one, for the words VW
/// uses that no label file happens to contain. Absent on many systems, which is
/// why it is looked for rather than required.
const SYSTEM_WORDS: &str = "/usr/share/dict/words";

/// Weight of the corpus's own vocabulary against the general list.
///
/// The label files are in-domain: when both offer a reading, the corpus's word
/// has to win, or the search prefers an English rarity to the term VW actually
/// uses.
const CORPUS_WORD_WEIGHT: &str = "8";
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

/// What one step of the run did, for the closing report.
///
/// A step that was skipped is worth as much as one that ran: somebody who
/// expected minutes and got seconds needs to be told why, or they will assume
/// it failed.
enum Step {
    Wrote { what: &'static str, path: PathBuf, detail: String },
    Skipped { what: &'static str, path: PathBuf, why: &'static str },
    Missing { what: &'static str, why: String },
}

pub fn run(opts: Options<'_>) -> Result<()> {
    let Some(root) = installation(&opts)? else { return Ok(()) };
    let root = root.as_path();
    let target = crate::datadir::extracted_dir()?;
    std::fs::create_dir_all(&target)
        .with_context(|| format!("creating {}", target.display()))?;

    println!("Reading the VCDS installation at {}", root.display());
    println!("Writing everything to {}\n", target.display());

    let steps =
        [label_cache(root, opts.refresh)?, names(root, opts.refresh)?, rod_keys(root)?];

    println!("\n{}", report(&steps));
    Ok(())
}

/// Step 1: parse the corpus into the SQLite cache.
fn label_cache(root: &Path, refresh: bool) -> Result<Step> {
    println!("[1/3] Label corpus — parsing every .lbl and decrypting every .clb.");
    let db = crate::labels::load_cached(root, refresh)?;
    Ok(Step::Wrote {
        what: "the label corpus",
        path: crate::datadir::label_cache()?,
        detail: format!("{} label files", db.len()),
    })
}

/// Step 2: recover the measurement names from the global text table.
///
/// Two of the existing tools, chained the way `vagcan vcds`'s own help
/// documents: `rod --dump` writes the decrypted, inflated `[TXT]` section, and
/// `tttext` reads it. The intermediate file is this function's business and
/// nobody else's, so it goes in a scratch directory and is removed again.
fn names(root: &Path, refresh: bool) -> Result<Step> {
    let source = root.join(ODX_DIR).join(TEXT_TABLE);
    let out = crate::datadir::names_catalog()?;
    if !source.is_file() {
        return Ok(Step::Missing {
            what: "the measurement names",
            why: format!("{} is not in this installation", source.display()),
        });
    }
    if !refresh && is_newer(&out, &source) {
        println!("[2/3] Measurement names — already recovered from this installation.");
        return Ok(Step::Skipped {
            what: "the measurement names",
            path: out,
            why: "newer than the text table it came from",
        });
    }

    println!(
        "[2/3] Measurement names — opening {TEXT_TABLE}, then reading its cipher.\n      \
         This is the slow part: every record is under its own substitution, and the\n      \
         attack bootstraps over several passes. Minutes, not seconds."
    );
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
        return Ok(Step::Missing {
            what: "the measurement names",
            why: format!(
                "the [TXT] section of {} did not decode — see the section listing above",
                source.display()
            ),
        });
    }

    let mut words = vec![format!("{}:{CORPUS_WORD_WEIGHT}", root.join("Labels").display())];
    if Path::new(SYSTEM_WORDS).exists() {
        words.push(format!("{SYSTEM_WORDS}:{GENERAL_WORD_WEIGHT}"));
    }
    crate::vcds::tttext::run(crate::vcds::tttext::Options {
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
    Ok(Step::Wrote {
        what: "the measurement names",
        path: out,
        detail: format!("{count} names"),
    })
}

/// Step 3: recover the keys of the `.rod` sections every car needs.
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
    if !cfg!(feature = "rod-crack") {
        // Silence here would look like the sections are genuinely unreadable,
        // which is the one conclusion that must never be reached by accident.
        println!(
            "[3/3] .rod section keys — this build has no key search, so only keys already\n      \
             cached are used. To recover the missing ones:\n          \
             cargo install --path crates/vagcan --features rod-crack\n      \
             then run this again."
        );
    } else {
        println!(
            "[3/3] .rod section keys — searching for the ones not already cached.\n      \
             About a minute of every core per blocked section."
        );
    }
    for file in &present {
        crate::vcds::rod::run(
            &file.to_string_lossy(),
            true,
            Some(&cache.to_string_lossy()),
            None,
        )?;
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

    let mut out = String::from("Done.\n\n");
    for step in steps {
        match step {
            Step::Wrote { what, path, detail } => {
                let _ = writeln!(out, "  {what}: {detail}\n    {}", path.display());
            }
            Step::Skipped { what, path, why } => {
                let _ = writeln!(out, "  {what}: unchanged, {why}\n    {}", path.display());
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
         vagcan faults --labels <VCDS-DIR>    stored faults, named\n\n\
         Scalings are a separate thing and no installation carries them — the corpus \n\
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
                what: "the label corpus",
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
    fn the_report_does_not_promise_scalings_the_corpus_cannot_supply() {
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

    #[test]
    fn a_missing_output_is_never_mistaken_for_a_current_one() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(!is_newer(Path::new("/definitely/not/here"), &here));
        assert!(!is_newer(&here, Path::new("/definitely/not/here")));
        assert!(is_newer(&here, &here), "a file is not older than itself");
    }
}
