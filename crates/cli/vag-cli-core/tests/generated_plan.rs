//! The generated source of a plan with a lever and a stopwatch compiles, warning-free, and
//! says what the plan it came from says. The firmware `include!`s such a file under
//! `-D warnings`, but CI builds it on an empty plan, so this is where that half of
//! `dash::to_rust` is compiled. The fixture is written by `dash.rs`'s
//! `the_generated_source_of_a_lever_plan_is_the_one_checked_in`.

#![deny(warnings)]

mod generated {
	include!("fixtures/lever_plan.rs");
}

use generated::PLAN;
use vag_dash_render::plan::state_of;
use vag_dash_render::stalk::StateIndex;

#[test]
fn the_generated_lever_plan_compiles_and_carries_the_lever_and_the_stopwatch() {
	let stalk = PLAN.stalk.expect("a lever");
	assert_eq!((stalk.rocker, stalk.switch, stalk.cruise), (3, 4, 5));
	assert_eq!(stalk.states.measure, StateIndex(3));
	// The fixture's ladder: 75..=110 is its second state.
	assert_eq!(state_of(stalk.rocker_states, 80), Some(StateIndex(1)));
	assert_eq!(state_of(stalk.switch_states, 150), Some(StateIndex(3)));
	let stopwatch = PLAN.stopwatch.expect("a stopwatch");
	assert_eq!((stopwatch.speed, stopwatch.marks), (1, &[60u16, 100][..]));
	assert!((stopwatch.km_h_per_unit - 0.0271).abs() < 1e-7);
}
