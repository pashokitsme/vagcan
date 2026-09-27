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
	answer_ms: exchange::RESPONSE_TIMEOUT_MS,
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
	assert_eq!(exchange::RESPONSE_TIMEOUT_MS, 500, "the board's answer timeout");
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
	// Nothing of its own by the end of the wait: not silence — the unit was heard on its own id,
	// finishing an earlier answer (review round 2, 2026-09-27: `Silent` marked it absent and
	// dropped its subscriptions, a run's speed included).
	let mut waits = Waits::new(READ, BOARD, None);
	waits.sent(0);
	assert_eq!(waits.heard(&[0x59, 0x02, 0xFF], 499), Heard::Stray);
	assert_eq!(waits.next_wait(500), None);
	assert_eq!(waits.ended(), Ended::Busy { asked_for_time: false });
}

#[test]
fn an_answer_for_another_identifier_or_sub_function_is_a_stray_too() {
	// One rule with the laptop (`schedule::answers`): a `22` answer echoes the identifier asked,
	// a `19` answer its sub-function.
	let mut waits = Waits::new(&[0x22, 0xF1, 0x87], BOARD, None);
	waits.sent(0);
	assert_eq!(waits.heard(&[0x62, 0x2A, 0x26, 0xFF, 0x13], 10), Heard::Stray, "the gateway's late list");
	assert_eq!(waits.heard(&[0x62, 0xF1, 0x87, b'P'], 20), Heard::Answer);
	// A batch whose first identifier the unit leaves out (ISO 14229-1): its answer is this
	// request's, whichever identifier it starts with (review round 3).
	let mut waits = Waits::new(&[0x22, 0x10, 0x00, 0x20, 0x00, 0x30, 0x00], BOARD, None);
	waits.sent(0);
	assert_eq!(waits.heard(&[0x62, 0x30, 0x00, 0x2A], 5), Heard::Answer, "the last one asked");
	let mut waits = Waits::new(&[0x19, 0x04, 0x01, 0x02, 0x03, 0xFF], BOARD, None);
	waits.sent(0);
	assert_eq!(
		waits.heard(&[0x59, 0x02, 0xFF, 0, 1, 2, 0x08], 10),
		Heard::Stray,
		"the count's late answer"
	);
	assert_eq!(waits.heard(&[0x59, 0x04, 0x01, 0x02, 0x03, 0x08], 20), Heard::Answer);
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
	assert_eq!(
		waits.ended(),
		Ended::Busy { asked_for_time: true },
		"a unit that said 78 is there, whoever's deadline ends the wait"
	);
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
	assert_eq!(waits.ended(), Ended::Busy { asked_for_time: true });
}

#[test]
fn a_unit_silent_from_the_start_of_the_counts_exchange_is_silent() {
	let mut waits = Waits::new(FAULTS, BOARD, COUNT);
	waits.sent(0);
	assert_eq!(waits.next_wait(0), Some(500));
	assert_eq!(waits.next_wait(500), None);
	assert_eq!(waits.ended(), Ended::Silent);
	// What was heard decides, not what ended the wait: a 78, then P2* running out.
	let mut waits = Waits::new(FAULTS, BOARD, Some(60_000));
	waits.sent(0);
	assert_eq!(waits.heard(&[0x7F, 0x19, 0x78], 100), Heard::Pending);
	assert_eq!(waits.next_wait(5_100), None);
	assert_eq!(waits.ended(), Ended::Busy { asked_for_time: true });
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
