//! The board's count of its own exchanges: a window of [`bus_rate::WINDOW_MS`], what it says on
//! the console and in the `state` line.
//!
//! `crates/dash/vag-dash-fw/src/bus_rate.rs` compiled as it is — the firmware cannot be built
//! for the host.

#[path = "../../../../crates/dash/vag-dash-fw/src/bus_rate.rs"]
mod bus_rate;

use bus_rate::{BusRate, ConsoleLine, STATE_LONGEST, StateKeys, UNITS, WINDOW_MS};

/// A bus that is never idle: an exchange every 34 ms, `answer_ms` of it waiting for the answer.
fn saturated(rate: &mut BusRate, from_ms: u64, to_ms: u64, units: &[u16]) {
	let mut at = from_ms;
	let mut i = 0;
	while at + 34 <= to_ms {
		rate.ended(units[i % units.len()], at, at + 34, true);
		at += 34;
		i += 1;
	}
}

#[test]
fn a_window_closes_after_its_time_and_a_new_one_begins() {
	let mut rate = BusRate::new(1_000);
	assert_eq!(rate.closes_at_ms(), 1_000 + WINDOW_MS);
	saturated(&mut rate, 1_000, 11_000, &[0x7E0]);
	assert_eq!(rate.tick(10_999), None, "not yet");
	let window = rate.tick(11_000).expect("closed");
	assert_eq!(window.ms, 10_000);
	assert_eq!(window.exchanges, 294);
	assert_eq!(window.per_s_tenths(), 294, "29.4 a second");
	assert_eq!(window.answered_ms_tenths(), Some(340), "34.0 ms an answered exchange");
	assert_eq!(window.busy_percent(), 99);
	// The new window counts from the close.
	assert_eq!(rate.closes_at_ms(), 21_000);
	assert_eq!(rate.tick(21_000).map(|w| w.exchanges), Some(0));
}

/// A window closed late — the bus task was in a long exchange at its end — counts over the time
/// it really had.
#[test]
fn a_late_close_counts_over_the_time_the_window_had() {
	let mut rate = BusRate::new(0);
	for i in 0..300 {
		rate.ended(0x7E0, i * 40, i * 40 + 20, true);
	}
	let window = rate.tick(12_000).expect("closed");
	assert_eq!(window.ms, 12_000);
	assert_eq!(window.per_s_tenths(), 250, "300 in 12 s");
}

/// An exchange the unit did not answer holds the bus to its timeout: counted as busy, and kept
/// out of the time an answer takes.
#[test]
fn an_unanswered_exchange_is_busy_time_and_not_an_answers_time() {
	let mut rate = BusRate::new(0);
	rate.ended(0x7E0, 0, 30, true);
	rate.ended(0x7E1, 30, 530, false);
	let window = rate.tick(WINDOW_MS).unwrap();
	assert_eq!((window.exchanges, window.answered), (2, 1));
	assert_eq!(window.answered_ms_tenths(), Some(300));
	assert_eq!(window.busy_percent(), 5);
}

#[test]
fn units_past_the_counted_ones_are_summed_as_others() {
	let mut rate = BusRate::new(0);
	let units: Vec<u16> = (0..UNITS as u16 + 2).map(|i| 0x700 + 2 * i).collect();
	for (i, unit) in units.iter().enumerate() {
		rate.ended(*unit, i as u64 * 10, i as u64 * 10 + 10, true);
		rate.ended(units[0], i as u64 * 10, i as u64 * 10 + 10, true);
	}
	let window = rate.tick(WINDOW_MS).unwrap();
	assert_eq!(window.units[0].request, 0x700);
	assert_eq!(window.units[0].exchanges, units.len() as u16 + 1);
	assert_eq!(window.units[UNITS - 1].request, units[UNITS - 1]);
	assert_eq!(window.others, 2);
}

#[test]
fn the_console_line_says_the_rate_the_time_an_answer_takes_and_each_unit() {
	let mut rate = BusRate::new(0);
	saturated(&mut rate, 0, 10_000, &[0x7E0, 0x7E0, 0x75A]);
	rate.ended(0x710, 9_990, 10_000, false);
	let window = rate.tick(10_000).unwrap();
	assert_eq!(
		ConsoleLine(&window).to_string(),
		"bus: 29.5 exchanges/s, 29.4 answered in 34.0 ms each, 100 % busy — 7E0 19.6/s, 75A 9.8/s, 710 0.1/s"
	);
	let quiet = BusRate::new(0).tick(10_000).unwrap();
	assert_eq!(ConsoleLine(&quiet).to_string(), "bus: 0.0 exchanges/s, 0.0 answered, 0 % busy");
}

#[test]
fn the_state_keys_say_the_rate_and_the_time_an_answer_takes() {
	assert_eq!(StateKeys(None).to_string(), " exch=- exch_ms=-");
	let mut rate = BusRate::new(0);
	saturated(&mut rate, 0, 10_000, &[0x7E0]);
	let window = rate.tick(10_000);
	assert_eq!(StateKeys(window).to_string(), " exch=29.4/s exch_ms=34.0");
	let silent = {
		let mut rate = BusRate::new(0);
		rate.ended(0x7E0, 0, 500, false);
		rate.tick(10_000)
	};
	assert_eq!(StateKeys(silent).to_string(), " exch=0.1/s exch_ms=-");
}

/// The state line is sized by its longest: a rate past what the counters hold is cut to it.
#[test]
fn the_state_keys_never_pass_their_longest() {
	let mut rate = BusRate::new(0);
	for _ in 0..100_000 {
		rate.ended(0x7E0, 0, 0, true);
	}
	// One exchange of 2,000,000 s: twenty seconds an answered exchange on average.
	rate.ended(0x7E0, 0, 2_000_000_000, true);
	let keys = StateKeys(rate.tick(WINDOW_MS)).to_string();
	assert_eq!(keys, " exch=9999.9/s exch_ms=9999.9");
	assert_eq!(keys.len(), STATE_LONGEST);
}
