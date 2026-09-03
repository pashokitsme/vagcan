//! The plan this image is for — everything the device knows about its car.
//!
//! There is no plan in the checkout and there never will be one. What is
//! `include!`d here is written at **build time** by `build.rs`, which runs the
//! generator (`vag_cli_core::dash::build_for_car`) against
//! `~/.vagcan/dash/<VIN>/dash.toml`, the car's survey and the project's
//! catalog cache, and puts the result under `~/.vagcan/dash/<VIN>/plan.rs`.
//! The VIN comes from `VAGCAN_DASH_VIN`, and a build without one does not
//! build. So the plan is where the label files are — on the laptop — and the
//! image carries only what was resolved from them: addresses, identifiers, bit
//! layouts, scalings, labels. `todo/dash/01-plan-format.md` is the contract;
//! [`vag_dash_render::plan`] is the type.
//!
//! What this means for the rest of the firmware: [`PLAN`] is `&'static`
//! all the way down, there is nothing to load and no state in which it is
//! missing or corrupt, and every unit address, identifier and part number the
//! device ever uses is spelled in exactly one place, which is not here.

include!(env!("VAG_DASH_PLAN"));

/// How many channels the plan carries — the size of the value store. A
/// `const` rather than a call so an array can be sized by it; reading a
/// `static` in a constant is allowed because [`PLAN`] holds nothing mutable.
pub const CHANNEL_COUNT: usize = PLAN.channels.len();

/// How many control units the plan polls.
pub const UNIT_COUNT: usize = PLAN.units.len();
