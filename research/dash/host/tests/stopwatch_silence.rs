//! A stopwatch run's speed through the board's planner (`Budget::board()`), against the
//! stopwatch's silence (`stopwatch::SILENCE_MS`): the longest gap between two speed answers a
//! run survives.
//!
//! Review round 4 (2026-09-27): a single `Busy` on a speed read — a late answer to an earlier
//! request on its id, then its own answer too late — backed the unit off 250 ms, and with a
//! silent page unit beside it the gap between two speed answers passed 1.3 s: 1600–1790 ms with
//! the page unit read every 1050–1240 ms, and the run aborted. One `Busy` between answers costs
//! no backoff; from the second in a row it does, as silence does.

use vag_dash_render::stopwatch::SILENCE_MS;
use vag_uds_client::schedule::{Answer, Budget, Class, Delivery, Next, Planner, Unit};

/// The unit the run's speed is read from, and the identifier (synthetic).
const SPEED: Unit = Unit {
	request: 0x7E1,
	response: 0x7E9,
};
const SPEED_DID: u16 = 0x3800;
/// A page unit that does not answer: each of its reads holds the bus for the answer timeout.
const SILENT: Unit = Unit {
	request: 0x714,
	response: 0x77E,
};
/// The board's answer timeout (`exchange::RESPONSE_TIMEOUT_MS` in the firmware).
const ANSWER_TIMEOUT_MS: u64 = 500;

/// How long a speed read takes to answer: a unit behind the gateway answers in about 4–20 ms.
const SPEED_ANSWER_MS: u64 = 10;

/// The longest gap between two speed answers in 3 s: the speed read every 50 ms as the run's
/// timing channel, a silent unit's cell every `page_period_ms` from 100 ms on (its reads go ahead
/// of the speed under the panel's floor), and the first speed read after it `Busy` after the whole
/// answer timeout — the round-4 firmware review's probe.
fn longest_speed_gap(page_period_ms: u32) -> u64 {
	let mut p = Planner::new(Budget::board());
	let speed = p.subscribe(0, Class::Timing, SPEED, SPEED_DID, 50, None);
	let (mut now, mut page, mut busy_done, mut last, mut longest) = (0u64, false, false, None::<u64>, 0u64);
	while now < 3_000 {
		if !page && now >= 100 {
			p.subscribe(now, Class::Foreground, SILENT, 0x1000, page_period_ms, None);
			page = true;
		}
		match p.due(now) {
			Next::Send(out) => {
				let (answer, cost) = if out.unit == SILENT {
					(Answer::NoAnswer, ANSWER_TIMEOUT_MS)
				} else if !busy_done && page {
					busy_done = true;
					(Answer::Busy { asked_for_time: false }, ANSWER_TIMEOUT_MS)
				} else {
					let mut pdu = vec![0x62];
					pdu.extend_from_slice(&SPEED_DID.to_be_bytes());
					pdu.push(0);
					(Answer::Pdu(pdu), SPEED_ANSWER_MS)
				};
				now += cost;
				for delivery in p.answered(now, out.token, answer) {
					if let Delivery::Reading { sub, at_ms, .. } = delivery {
						if sub == speed {
							if let Some(before) = last {
								longest = longest.max(at_ms - before);
							}
							last = Some(at_ms);
						}
					}
				}
			}
			Next::Idle { until_ms } => now = until_ms.unwrap_or(now + 10).max(now + 1),
		}
	}
	longest
}

#[test]
fn one_busy_speed_read_beside_a_silent_page_unit_leaves_the_run_its_speed() {
	for page_period_ms in [1_050, 1_100, 1_150, 1_200, 1_240] {
		let gap = longest_speed_gap(page_period_ms);
		assert!(
			gap < SILENCE_MS,
			"the page unit read every {page_period_ms} ms: the longest gap between two speed answers is {gap} ms, past the stopwatch's silence of {SILENCE_MS} ms"
		);
	}
}
