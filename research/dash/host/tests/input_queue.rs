//! The board's command queue, on the host: every input's commands wait there for the one task
//! that applies them, and a full queue drops the newest.
//!
//! `crates/dash/vag-dash-fw/src/input.rs` compiled as it is — the firmware cannot be built for
//! the host.

#[path = "../../../../crates/dash/vag-dash-fw/src/input.rs"]
mod input;

use futures::executor::block_on;
use input::{CAPACITY, CommandQueue, Source};
use vag_dash_render::control::Command;

#[test]
fn commands_come_out_in_the_order_they_went_in() {
	let queue = CommandQueue::new();
	assert!(queue.offer(Source::Pin(3), Command::Next));
	assert!(queue.offer(Source::Lever, Command::Stopwatch));
	assert!(queue.offer(Source::Sim, Command::Next));
	assert_eq!(block_on(queue.next()), (Source::Pin(3), Command::Next));
	assert_eq!(block_on(queue.next()), (Source::Lever, Command::Stopwatch));
	assert_eq!(block_on(queue.next()), (Source::Sim, Command::Next));
}

#[test]
fn a_full_queue_drops_the_newest_and_keeps_what_was_pressed_first() {
	assert_eq!(CAPACITY, 4, "four wait at most");
	let queue = CommandQueue::new();
	for pin in 0..CAPACITY as u8 {
		assert!(queue.offer(Source::Pin(pin), Command::Next), "{pin} fits");
	}
	assert!(!queue.offer(Source::Lever, Command::Previous), "the fifth is dropped, and said to be");
	assert_eq!(block_on(queue.next()), (Source::Pin(0), Command::Next), "the oldest is still first");
	// One taken, one fits again — and the dropped one is not in the queue.
	assert!(queue.offer(Source::Sim, Command::Stopwatch));
	let rest: Vec<_> = (0..CAPACITY).map(|_| block_on(queue.next())).collect();
	assert_eq!(
		rest,
		[
			(Source::Pin(1), Command::Next),
			(Source::Pin(2), Command::Next),
			(Source::Pin(3), Command::Next),
			(Source::Sim, Command::Stopwatch),
		]
	);
}

#[test]
fn the_log_names_each_source() {
	assert_eq!(Source::Sim.to_string(), "dashsim");
	assert_eq!(Source::Pin(4).to_string(), "GPIO4");
	assert_eq!(Source::Lever.to_string(), "lever");
}
