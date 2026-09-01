//! What both command crates stand on.
//!
//! Which car this is, what channels it has, how to poll them, where its files
//! live, and the terminal widgets that draw the answer. Nothing here knows
//! about any particular command: [`vag_cli_diag`] and [`vag_cli_measure`] are
//! its callers, not its subject.
//!
//! The split was not cosmetic. `measure` — an acceleration stopwatch, a third
//! of the old CLI's code — needed exactly these twelve modules and nothing
//! else from diagnostics, which is what made it separable at all.

pub mod analyse;
pub mod config;
pub mod datadir;
pub mod device;
pub mod extracted;
pub mod glossary;
pub mod plan;
pub mod progress;
pub mod project;
pub mod ui;
pub mod units;
pub mod vcdslog;
