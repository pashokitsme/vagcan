//! One exchange's waits on the board: how long it waits for an answer, what each PDU on the
//! unit's answer id is to it, and how it ends without one.
//!
//! `crates/dash/vag-dash-fw/src/exchange.rs` compiled as it is — the firmware cannot be built
//! for the host. Times are milliseconds from the exchange's start, as `transact` reads them.

#[path = "../../../../crates/dash/vag-dash-fw/src/exchange.rs"]
mod exchange;

use exchange::{Ended, Heard, Timeouts, Waits};

/// The board's: `RESPONSE_TIMEOUT`, `SUPPRESSED_WAIT`, `PENDING_WAIT` (P2*), `PENDING_DEADLINE`.
const BOARD: Timeouts = Timeouts {
	answer_ms: 500,
	suppressed_ms: 150,
	pending_wait_ms: 5_000,
	pending_deadline_ms: 10_000,
};
/// The fault count's own deadline (`todo/dash/20`).
const COUNT: Option<u64> = Some(2_000);

const READ: &[u8] = &[0x22, 0xF1, 0x87];
const FAULTS: &[u8] = &[0x19, 0x02, 0x08];

#[test]
fn its_own_answer_ends_the_wait() {
	let mut waits = Waits::new(READ, BOARD, None);
	waits.sent(3);
	assert_eq!(waits.next_wait(3), Some(500));
	assert_eq!(waits.heard(&[0x62, 0xF1, 0x87, b'P'], 40), Heard::Answer);
	assert_eq!(waits.heard(&[0x7F, 0x22, 0x31], 40), Heard::Answer, "a refusal is its answer too");
	assert_eq!(waits.strays(), 0);
}

#[test]
fn a_late_answer_to_an_earlier_request_is_dropped_and_the_wait_goes_on_within_its_time() {
	// Review, 2026-09-27: the count's `19 02 08` cut at 2 s, and the unit's `59 02 …` or another
	// `7F 19 78` arriving inside the unit's next exchange — the panel's `F187` — was read as
	// that exchange's answer: a part number that did not parse, and the unit dashed for 2 s more.
	let mut waits = Waits::new(READ, BOARD, None);
	waits.sent(0);
	assert_eq!(waits.heard(&[0x59, 0x02, 0xFF, 0, 1, 2, 0x08], 100), Heard::Stray);
	assert_eq!(waits.next_wait(100), Some(400), "what is left of the same wait, not a new one");
	assert_eq!(waits.heard(&[0x7F, 0x19, 0x78], 150), Heard::Stray);
	assert_eq!(waits.heard(&[0x7F, 0x19, 0x21], 160), Heard::Stray);
	assert_eq!(waits.heard(&[], 170), Heard::Stray);
	assert_eq!(waits.strays(), 4);
	assert_eq!(waits.first_stray(), Some(&[0x59, 0x02, 0xFF][..]), "its first bytes, for the log");
	assert_eq!(waits.heard(&[0x62, 0xF1, 0x87, b'P'], 200), Heard::Answer);
	// Nothing of its own by the end of the wait: silence, as before.
	let mut waits = Waits::new(READ, BOARD, None);
	waits.sent(0);
	assert_eq!(waits.heard(&[0x59, 0x02, 0xFF], 499), Heard::Stray);
	assert_eq!(waits.next_wait(500), None);
	assert_eq!(waits.ended(), Ended::Silent);
}

#[test]
fn a_request_that_suppressed_its_answer_waits_for_its_own_refusal_only() {
	let mut waits = Waits::new(&[0x3E, 0x80], BOARD, None);
	waits.sent(0);
	assert_eq!(waits.next_wait(0), Some(150));
	assert_eq!(
		waits.heard(&[0x62, 0xF4, 0x0D, 0], 20),
		Heard::Stray,
		"someone else's answer is not a refusal"
	);
	assert_eq!(waits.heard(&[0x7F, 0x3E, 0x12], 30), Heard::Answer);
}

#[test]
fn a_78_is_waited_out_as_before_without_a_deadline_of_its_own() {
	let mut waits = Waits::new(FAULTS, BOARD, None);
	waits.sent(0);
	assert_eq!(waits.heard(&[0x7F, 0x19, 0x78], 100), Heard::Pending);
	assert_eq!(waits.next_wait(100), Some(5_000), "P2*");
	assert_eq!(waits.heard(&[0x7F, 0x19, 0x78], 5_000), Heard::Pending);
	assert_eq!(waits.next_wait(5_000), Some(5_000));
	assert_eq!(waits.heard(&[0x7F, 0x19, 0x78], 9_000), Heard::Pending);
	assert_eq!(waits.next_wait(9_000), Some(1_100), "the 78s together end 10 s after the first");
	assert_eq!(waits.next_wait(10_100), None);
	assert_eq!(waits.ended(), Ended::Silent, "no answer, as it always was");
	// A stray between two 78s does not start a new wait.
	let mut waits = Waits::new(FAULTS, BOARD, None);
	waits.sent(0);
	assert_eq!(waits.heard(&[0x7F, 0x19, 0x78], 100), Heard::Pending);
	assert_eq!(waits.heard(&[0x62, 0xF4, 0x0D, 0], 1_100), Heard::Stray);
	assert_eq!(waits.next_wait(1_100), Some(4_000));
}

#[test]
fn the_counts_exchange_ends_two_seconds_after_it_began_78s_included() {
	let mut waits = Waits::new(FAULTS, BOARD, COUNT);
	assert_eq!(waits.send_wait(1_000), Some(1_000), "the send's own deadline is inside it");
	waits.sent(2);
	assert_eq!(waits.next_wait(2), Some(500), "the first answer's wait is the board's");
	assert_eq!(waits.heard(&[0x7F, 0x19, 0x78], 40), Heard::Pending);
	assert_eq!(waits.next_wait(40), Some(1_960), "P2* cut to what is left of 2 s");
	assert_eq!(waits.next_wait(1_999), Some(1));
	assert_eq!(waits.next_wait(2_000), None);
	// The unit said it is there and busy: not silence.
	assert_eq!(waits.ended(), Ended::StillPending);
}

#[test]
fn a_unit_silent_from_the_start_of_the_counts_exchange_is_silent() {
	let mut waits = Waits::new(FAULTS, BOARD, COUNT);
	waits.sent(0);
	assert_eq!(waits.next_wait(0), Some(500));
	assert_eq!(waits.next_wait(500), None);
	assert_eq!(waits.ended(), Ended::Silent);
	// Nor is silence after a 78 that the limit did not cut: none, the limit being shorter than P2*,
	// but the rule is what cut the wait, not whether a 78 came.
	let mut waits = Waits::new(FAULTS, BOARD, Some(60_000));
	waits.sent(0);
	assert_eq!(waits.heard(&[0x7F, 0x19, 0x78], 100), Heard::Pending);
	assert_eq!(waits.next_wait(5_100), None);
	assert_eq!(waits.ended(), Ended::Silent);
}

#[test]
fn a_send_that_used_the_whole_deadline_leaves_no_wait() {
	let mut waits = Waits::new(FAULTS, BOARD, COUNT);
	assert_eq!(waits.send_wait(5_000), Some(2_000), "cut to the limit");
	waits.sent(2_000);
	assert_eq!(waits.next_wait(2_000), None);
	assert_eq!(waits.ended(), Ended::Silent, "no 78 was heard");
	let waits = Waits::new(FAULTS, BOARD, None);
	assert_eq!(waits.send_wait(1_000), Some(1_000));
}
