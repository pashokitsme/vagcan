//! Reading a car, and reading the files that explain what it said.
//!
//! Everything `vagcan` does that is not a stopwatch: identification, fault
//! memory, the guarded sweeps, the live view, and the offline work over VCDS's
//! and ODIS's own files. The command surface itself is not here — that is
//! `vag-cli`, which owns the clap declarations and does nothing else.

// Core is re-exported at this crate's root so that every `crate::config::…`
// written when this was one crate still resolves — pointing at core instead of
// at a local module. The alternative was rewriting some hundreds of paths
// across thirty files, which would have made the diff about the paths rather
// than about the split.
pub use vag_cli_core::{analyse, config, datadir, device, extracted, glossary, plan, progress, project, ui, units, vcdslog};

pub mod anomaly;
pub mod calibrate;
pub mod declared;
pub mod discover;
pub mod faultnames;
pub mod faults;
pub mod labels;
pub mod migrate;
pub mod missing;
pub mod names;
pub mod props;
pub mod recording;
pub mod render;
pub mod safety;
pub mod scan;
pub mod setup;
pub mod sniff;
pub mod survey;
pub mod vcds;
pub mod watch;
