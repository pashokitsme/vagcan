//! What both command crates stand on.
//!
//! Which car this is, what channels it has, how to poll them, where its files
//! live, and the terminal widgets that draw the answer. Nothing here *does* a
//! command: [`vag_cli_diag`] and [`vag_cli_measure`] are its callers, not its
//! subject.
//!
//! One module names commands anyway, and deliberately. [`missing`] holds the
//! sentence a reader gets when the diagnostic data is not on the machine, and
//! that sentence has to say `vagcan setup`. It sits here because
//! [`project`] is one of its six callers and `core` cannot depend on `diag` —
//! the alternative was writing the same instruction a seventh time to cross the
//! boundary, which is the thing the module exists to stop.
//!
//! The split was not cosmetic. `measure` — an acceleration stopwatch, a third
//! of the old CLI's code — needed exactly twelve of these modules and nothing
//! else from diagnostics, which is what made it separable at all.
//!
//! [`missing`] is the thirteenth and arrived later, from `diag`. It says what a
//! machine that has never run `vagcan setup` is short of, and that sentence was
//! also being written out in [`project`], which is here — so the module had to
//! be somewhere both could reach or the wording would have been duplicated to
//! cross the crate boundary. `diag` re-exports it, so `crate::missing::…` there
//! still resolves.

pub mod analyse;
pub mod config;
pub mod dash;
pub mod datadir;
pub mod device;
pub mod extracted;
pub mod glossary;
pub mod missing;
pub mod plan;
pub mod progress;
pub mod project;
pub mod ui;
pub mod units;
pub mod vcdslog;
