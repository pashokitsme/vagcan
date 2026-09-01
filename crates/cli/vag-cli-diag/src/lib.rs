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
//
// `missing` is the one that went the other way: it was this crate's, and moved
// into core because `core::project` reports the same shortage and the sentence
// may only be written once. Re-exported for the same reason as the rest —
// `crate::missing::…` reads the same here as it always did.
pub use vag_cli_core::{analyse, config, datadir, device, extracted, glossary, missing, plan, progress, project, ui, units, vcdslog};

pub mod anomaly;
pub mod calibrate;
pub mod declared;
pub mod discover;
pub mod faultnames;
pub mod faults;
pub mod labels;
pub mod migrate;
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
