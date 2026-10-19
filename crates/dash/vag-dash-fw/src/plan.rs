//! The plan this image is for — everything the device knows about its car.
//!
//! There is no plan in the checkout and there never will be one. What is
//! `include!`d here is written at **build time** by `build.rs`, which runs the
//! generator (`vag_cli_core::dash::build_for_car`) against
//! `~/.vagcan/dash/<VIN>/dash.toml`, the car's record of its units
//! (`~/.vagcan/cars/<VIN>/units.json`) and the project's catalog cache, and
//! puts the result under `~/.vagcan/dash/<VIN>/plan.rs`.
//! The car is `VAGCAN_DASH_VIN`; unset, it is the one car under
//! `~/.vagcan/dash/`, and with none or several there the build stops rather
//! than pick. (`VAGCAN_DASH_NO_CAR` builds an empty plan instead — for CI, which
//! has no car; that image is never flashed.) So the plan is where the label
//! files are — on the laptop — and the
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

/// How many chart pages the plan has — how many histories the panel keeps.
pub const CHART_COUNT: usize = PLAN.chart_count();

/// How many alarm rules the plan carries — one per channel and kind of alarm its
/// `[[channel]]`s have — and so the size of the panel's alarm state. No count caps it; the
/// generator's `bus:` line says what the alarms read on every page.
pub const ALARM_COUNT: usize = PLAN.alarms.len();

/// How many `[[button]]`s the plan carries — the size of the input task's button table.
pub const BUTTON_COUNT: usize = PLAN.buttons.len();

/// How many pages the plan has: the pages the panel turns through. The generator caps them
/// at `vag_dash_render::pages::MAX_PAGES`, so the count is a `u8`, as the active page is.
pub const PAGE_COUNT: u8 = PLAN.pages.len() as u8;

const _: () = assert!(
	PLAN.pages.len() <= vag_dash_render::pages::MAX_PAGES,
	"the plan has more [[page]]s than the board holds"
);

// `PLAN_ID`, the plan's id (`vag_cli_core::dash::plan_id`), comes with the generated source:
// what the settings are saved against, and what the Hello reply and `state` name.

// What the generator refuses, refused again where it would bite: a pin the board has no
// input for, one given twice, or more buttons than free pins is an image that does not
// build rather than a button that silently does nothing.
// Every join the generator wrote — a channel's unit, a page's cells, its strip's voltage, a
// chart's history slot, the standstill's channel — names something this plan has: the board
// indexes by them and never searches (`todo/dash/27`), so one that did not would be a panic on
// the car rather than an image that does not build.
const _: () = assert!(
	PLAN.joins_hold(),
	"the plan's indices do not join: build it again with `vagcan dev dash build <VIN>`"
);

const _: () = assert!(
	PLAN.buttons_fit(),
	"the plan's [[button]]s do not fit this board: at most 3, on GPIO 3, 4 or 5, each once"
);
