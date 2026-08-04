//! Everything whose input is a recording this tool made.
//!
//! `vagcan watch --out` writes a CSV of whatever was on screen. These two
//! commands read it back afterwards, at a desk, and are the reason a drive is
//! worth recording at all — but neither has anything to say while the car is in
//! front of you, so neither belongs at the top level.
//!
//! The distinction from `vagcan vcds` is what the input *is*, not whether it is
//! offline: those read VCDS's files, these read ours.

use std::path::Path;

use anyhow::{Context, Result};
use clap::Subcommand;

use crate::ui::picker;
use crate::{analyse, calibrate, discover};

#[derive(Subcommand)]
pub enum Tool {
    /// Prove new scalings against ones already trusted — no VCDS needed.
    ///
    /// FOR: naming what raw bytes mean using measurements this project has
    /// already proven, instead of a parallel VCDS session. One clock, so no
    /// alignment error is possible. It cannot name anything, and cannot find a
    /// quantity unrelated to everything already known.
    ///
    /// IN: a `vagcan watch --out` recording holding BOTH converted reference
    /// columns and raw hex columns (the ones suffixed `_raw`).
    ///
    /// OUT: the fits that clear the bar, on stdout.
    Calibrate {
        /// Recording written by `vagcan watch --out`. Left out, the recordings
        /// in the current directory are offered as a list.
        #[arg(long, value_name = "FILE")]
        log: Option<String>,
        /// Minimum R² for a fit to count (the whole bar: R² ≥ 0.995, ≥ 20
        /// points over ≥ 4 distinct raw values).
        #[arg(long, default_value_t = 0.995, value_name = "R2")]
        min_r2: f64,
        /// Minimum matched samples for a fit to count (the whole bar: R² ≥
        /// 0.995, ≥ 20 points over ≥ 4 distinct raw values).
        #[arg(long, default_value_t = 20, value_name = "N")]
        min_points: usize,
    },

    /// Find which identifiers carry discrete state — a gear, a mode, a switch.
    ///
    /// FOR: the values that cannot be fitted. A two-level signal fits any line
    /// exactly, so a gear or a lamp is found by noticing what changed when, not
    /// by least squares.
    ///
    /// IN: a `vagcan watch --out` recording, ideally one where the thing you
    /// are looking for was deliberately operated.
    ///
    /// OUT: every identifier sorted into never-moved, stepped between a few
    /// values, or continuous — with the stepped ones ranked, on stdout.
    Discover {
        /// Recording written by `vagcan watch --out`. Left out, the recordings
        /// in the current directory are offered as a list.
        #[arg(long, value_name = "FILE")]
        log: Option<String>,
        /// Also list identifiers that changed at the same moments.
        #[arg(long)]
        pairs: bool,
    },
}

/// The path as typed, or the one picked off a list. `instead` is the command
/// line that needs no list, for whoever is running this down a pipe.
fn pick_when_absent(log: Option<String>, instead: &str) -> Result<Option<String>> {
    match log {
        Some(log) => Ok(Some(log)),
        None => pick_recording(instead),
    }
}

/// The recording to work on, when the command line did not name one.
///
/// **The current directory, because recordings have no home of their own.**
/// `watch --out` writes wherever it was told to; unlike a car's sessions, which
/// `datadir` files under a VIN, nothing has ever settled where a `.csv` from a
/// drive belongs. So the list is of the directory the person is standing in,
/// which is where their last `--out` put one — a guess, but the same guess they
/// made, and not a new convention invented by a file chooser.
///
/// Newest first: these are named by whoever ran `watch`, but the one wanted is
/// nearly always the last drive, and `entries` dates every row.
///
/// `None` is somebody who left the list — an answer, not a failure.
fn pick_recording(instead: &str) -> Result<Option<String>> {
    let mut chooser = picker::Console::new(instead);
    let picked = picker::pick_path(&mut chooser, Path::new("."), &[recordings()])?;
    Ok(picked.map(|path| path.to_string_lossy().into_owned()))
}

/// What a list of recordings is: the `.csv` files, last drive at the top.
fn recordings() -> picker::Level<'static> {
    picker::Level::files("recording")
        .ending(".csv")
        .newest_first()
        .filled_by("vagcan watch --out drive.csv   records one")
}

pub fn run(tool: Tool) -> Result<()> {
    match tool {
        Tool::Calibrate { log, min_r2, min_points } => {
            let Some(log) = pick_when_absent(log, "vagcan recording calibrate --log FILE.csv")?
            else {
                return Ok(());
            };
            calibrate::run(&log, analyse::Thresholds { min_r2, min_points, ..Default::default() })
        }
        Tool::Discover { log, pairs } => {
            let Some(log) = pick_when_absent(log, "vagcan recording discover --log FILE.csv")?
            else {
                return Ok(());
            };
            let text = std::fs::read_to_string(&log)
                .with_context(|| format!("reading the recording {log:?}"))?;
            let columns = discover::classify(&text).map_err(|e| anyhow::anyhow!("{log}: {e}"))?;
            print!("{}", discover::render(&columns));
            if pairs {
                let together = discover::co_changing(&columns, 0.5);
                if together.is_empty() {
                    println!("\nNo two candidates changed together.");
                } else {
                    println!("\nChanged at the same moments — probably one thing seen twice:");
                    for (a, b, overlap) in together {
                        println!("  {a} + {b}   {:.0}% of transitions coincide", overlap * 100.0);
                    }
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory holding what a few drives and one unrelated file leave behind.
    fn a_directory_with(names: &[&str]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "vagcan-recording-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in names {
            std::fs::write(dir.join(name), "t_s\n0.000\n").unwrap();
        }
        dir
    }

    #[test]
    fn the_list_offers_recordings_last_drive_first_and_nothing_else() {
        // `watch --out` names these, so they sort as text and the drive
        // somebody just finished is at the wrong end. And a directory a person
        // works in holds more than recordings: a survey's `.jsonl`, notes.
        let dir = a_directory_with(&[
            "2026-08-02-1030.csv",
            "2026-08-04-1241.csv",
            "survey-parked.jsonl",
            "notes.md",
        ]);
        let names: Vec<String> =
            picker::entries(&dir, &recordings()).into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["2026-08-04-1241.csv", "2026-08-02-1030.csv"]);
    }

    #[test]
    fn a_path_on_the_command_line_is_taken_as_typed_and_asks_nobody() {
        // The picker is what happens when the argument is absent; a path that
        // was given is never second-guessed, and no terminal is touched. This
        // is also what keeps every one of these commands working in a pipe.
        let given = pick_when_absent(Some("drive.csv".into()), "unused").unwrap();
        assert_eq!(given.as_deref(), Some("drive.csv"));
    }
}
