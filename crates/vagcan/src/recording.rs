//! Everything whose input is a recording this tool made.
//!
//! `vagcan watch --out` writes a CSV of whatever was on screen. These two
//! commands read it back afterwards, at a desk, and are the reason a drive is
//! worth recording at all — but neither has anything to say while the car is in
//! front of you, so neither belongs at the top level.
//!
//! The distinction from `vagcan vcds` is what the input *is*, not whether it is
//! offline: those read VCDS's files, these read ours.

use anyhow::{Context, Result};
use clap::Subcommand;

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
        /// Recording written by `vagcan watch --out`.
        #[arg(long, value_name = "FILE")]
        log: String,
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
        /// Recording written by `vagcan watch --out`.
        #[arg(long, value_name = "FILE")]
        log: String,
        /// Also list identifiers that changed at the same moments.
        #[arg(long)]
        pairs: bool,
    },
}

pub fn run(tool: Tool) -> Result<()> {
    match tool {
        Tool::Calibrate { log, min_r2, min_points } => calibrate::run(
            &log,
            analyse::Thresholds { min_r2, min_points, ..Default::default() },
        ),
        Tool::Discover { log, pairs } => {
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
