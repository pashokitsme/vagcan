//! Bus time, estimated: the board's own scheduler run over a plan offline, every exchange
//! costed at one constant (`.archive/tasks/done/dash/24-bus-time-check.md`).
//!
//! The board asks one unit one request at a time, and a request carries up to
//! [`Budget::max_dids_per_request`] identifiers of that unit — so what a page costs is
//! exchanges, not identifiers: ten channels of one unit are two exchanges a round, eight
//! channels of eight units are eight. This module runs the [`Planner`] the board runs, under
//! the board's [`Budget::board`], over every rate the board sets (`Plan::rates_in`, as the
//! firmware's panel subscribes them), answers every request after [`EXCHANGE_MS`], and counts
//! what comes back. The planner has no clock and no bus, so the run is exact and takes
//! milliseconds.
//!
//! **An estimate, and it refuses nothing** (2026-10-20). The model holds a car that answers
//! everything, every time, after one fixed time per exchange, and the one time the repository
//! has is an upper bound with a radio and a host in it: at it, every page of the owner's plan
//! came out short, and that plan runs on the car. A unit that does not answer is backed off on
//! the car and costs less; a host on the board's link costs more, and is not in it. The board
//! counts its own exchanges (`exch=` in its `state`, a `bus:` line on its console); a strict
//! check waits for that number from a drive.

use std::collections::BTreeMap;

use vag_dash_render::plan::{Mode, Page, Plan, Timing};
use vag_uds_client::schedule::{Answer, Budget, Class, Delivery, Next, Planner, SubId, Unit};

/// How long one exchange is taken to hold the board's one conversation: request out, answer
/// back, the board on to the next. **34 ms — an upper bound, from the reference car,
/// 2026-09-26.**
///
/// Evidence: `research/captures/ble-10hz.csv`, `vagcan watch --device ble --hz 10` for 140 s
/// on the car (`todo/dash/17` §4), the board running the owner's plan of the day beside it. Every
/// reading carries the board's time of the answer it came in, so the readings of one exchange
/// share a stamp: 21,918 readings under 4,139 stamps in 140.3 s — 29.5 exchanges a second, 5.3
/// identifiers an exchange, 1,793 of them full batches of 8. No channel got the 10 a second it
/// asked for (8.3 the most), so the planner always had a request waiting: 1 s / 29.5 = 34 ms.
///
/// An upper bound, not the car's CAN time: the host was across the radio, which costs the board
/// time of its own (`research/dash/can-bring-up.md` §9.15), and an exchange of the panel's own,
/// with none of the host's identifiers in it, is not in the file. The other figure, 45–48 a
/// second (≈ 21 ms) in §9.15, is `measure`'s speed channel alone against `benchecu` on the
/// bench, a unit that answers at once. One exchange of one identifier was 20 ms (median) in the
/// same file, a full batch 32 ms; one cost for both errs on the slow side for a single read.
///
/// The number that replaces it is the board's own: `exch_ms=` in its `state` line, from a drive.
pub(super) const EXCHANGE_MS: u64 = 34;

/// Run before counting: the subscriptions start due together, a burst the board makes once, and
/// a background read starves only after [`Budget::starve_after_ms`].
const WARM_UP_MS: u64 = 10_000;
/// What is counted: a minute of the bus after the warm-up.
const WINDOW_MS: u64 = 60_000;

/// What is on the glass for one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Glass {
	/// A plan page, by index, the stopwatch off.
	Page(usize),
	/// The stopwatch during a run: the speed as the board's clock, every page's cells behind it.
	Run,
}

/// A read the board wants at its own rate — the page on the glass, an alarm, the lever, a run's
/// speed — that the bus would serve slower.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Thinned {
	pub channel: u16,
	pub asked_per_s: f64,
	pub got_per_s: f64,
}

/// One run of the bus with one thing on the glass.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Run {
	pub glass: Glass,
	/// The share of the minute the bus was in an exchange, 0 to 1.
	pub busy: f64,
	/// What the board wants at its rate and would not get, in plan order.
	pub thinned: Vec<Thinned>,
	/// Whether a page not on the glass was read slower than its rate — by design, the first
	/// thing thinned.
	pub behind_thinned: bool,
}

/// What one thing on the glass costs: with the lever's gate open — cruise off, the lever read
/// at its fastest, or no lever — and, where the plan has a lever, with it closed — cruise on.
/// A stopwatch run reads the lever at one rate whatever the gate, so it has one run.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Estimate {
	pub cruise_off: Run,
	pub cruise_on: Option<Run>,
}

/// One estimate per plan page, then a stopwatch run's where the plan has a stopwatch.
pub(super) fn estimate(plan: &Plan, exchange_ms: u64) -> Vec<Estimate> {
	let pages = (0..plan.pages.len()).map(Glass::Page).map(|glass| Estimate {
		cruise_off: simulate(plan, glass, true, exchange_ms),
		cruise_on: plan.stalk.map(|_| simulate(plan, glass, false, exchange_ms)),
	});
	let run = plan.stopwatch.map(|_| Estimate {
		cruise_off: simulate(plan, Glass::Run, true, exchange_ms),
		cruise_on: None,
	});
	pages.chain(run).collect()
}

/// The panel's subscriptions with `glass` up: what the firmware's `PanelReads::follow_pages`
/// asks of its planner — the page on the glass's reads shown ([`Page::reads`]), every page's
/// listed, the rocker's unit taken as matched so the lever and the cruise status are read.
fn rates(plan: &Plan, glass: Glass, gate_open: bool) -> Vec<(u16, Class, u32)> {
	let (shown, stopwatch) = match glass {
		Glass::Page(page) => (plan.pages.get(page).into_iter().flat_map(Page::reads).collect(), Timing::Off),
		// The stopwatch's page is no plan page: no cells, no voltage.
		Glass::Run => (Vec::new(), Timing::Timing),
	};
	let listed: Vec<u16> = plan.pages.iter().flat_map(Page::reads).collect();
	let mode = Mode {
		gate_open,
		stopwatch,
		lever: true,
	};
	plan
		.rates_in(&shown, &listed, mode)
		.map(|rate| {
			let class = match (rate.timing, rate.foreground) {
				(true, _) => Class::Timing,
				(_, true) => Class::Foreground,
				(_, false) => Class::Background,
			};
			(rate.channel, class, rate.period_ms)
		})
		.collect()
}

/// Run the bus for [`WARM_UP_MS`] and then [`WINDOW_MS`] with `glass` up and the lever's gate
/// as `gate_open`, every exchange answered after `exchange_ms`.
pub(super) fn simulate(plan: &Plan, glass: Glass, gate_open: bool, exchange_ms: u64) -> Run {
	let mut planner = Planner::new(Budget::board());
	// Every record two bytes long, and said so: an answer cut by the lengths, as a car's
	// answer is cut when it reads one way. The length changes how the answer is cut, not which
	// identifiers go in a request.
	const RECORD: u16 = 2;
	let mut subs: Vec<(SubId, u16, Class, u32)> = Vec::new();
	for (channel, class, period_ms) in rates(plan, glass, gate_open) {
		let Some(read) = plan.channel(channel) else {
			continue;
		};
		let at = usize::from(read.unit);
		let Some(owner) = plan.units.get(at) else { continue };
		let unit = Unit {
			request: owner.request,
			response: owner.response,
		};
		// The board subscribes a unit's channels when its part number answers, and the part
		// checks go out one exchange after another: unit by unit, one exchange apart.
		let since = at as u64 * exchange_ms;
		let sub = planner.subscribe(since, class, unit, read.did, period_ms, Some(RECORD));
		subs.push((sub, channel, class, period_ms));
	}

	let end = WARM_UP_MS + WINDOW_MS;
	let counted = |at: u64| at > WARM_UP_MS && at <= end;
	let mut readings: BTreeMap<SubId, u32> = BTreeMap::new();
	let mut exchanges = 0u32;
	let mut now = 0u64;
	while now < end {
		match planner.due(now) {
			Next::Send(out) => {
				if now >= WARM_UP_MS {
					exchanges += 1;
				}
				let at = now + exchange_ms.max(1);
				for delivery in planner.answered(at, out.token, Answer::Pdu(answer(&out.pdu, RECORD))) {
					if let Delivery::Reading { sub, at_ms, .. } = delivery
						&& counted(at_ms)
					{
						*readings.entry(sub).or_default() += 1;
					}
				}
				now = at;
			}
			Next::Idle { until_ms: Some(at) } => now = at.max(now + 1),
			// Nothing will come due: nothing is subscribed.
			Next::Idle { until_ms: None } => break,
		}
	}

	let seconds = WINDOW_MS as f64 / 1000.0;
	let mut thinned = Vec::new();
	let mut behind_thinned = false;
	for (sub, channel, class, period_ms) in subs {
		let asked = WINDOW_MS as f64 / f64::from(period_ms);
		let got = f64::from(readings.get(&sub).copied().unwrap_or(0));
		if served(asked, got) {
			continue;
		}
		match class {
			Class::Background => behind_thinned = true,
			_ => thinned.push(Thinned {
				channel,
				asked_per_s: asked / seconds,
				got_per_s: got / seconds,
			}),
		}
	}
	Run {
		glass,
		busy: (f64::from(exchanges) * exchange_ms as f64 / WINDOW_MS as f64).min(1.0),
		thinned,
		behind_thinned,
	}
}

/// Whether a read that wanted `asked` readings in the window got them. The planner keeps a
/// read's phase, so a read it serves gets every one; the window's two ends can cut one off
/// each, and a reading more than that short is a reading the bus did not make.
fn served(asked: f64, got: f64) -> bool {
	got + 2.0 >= asked.floor()
}

/// A positive answer to a `22` request: every identifier asked, in order, each with `record`
/// bytes of zeros.
fn answer(request: &[u8], record: u16) -> Vec<u8> {
	let mut pdu = vec![0x62];
	for did in request.get(1..).unwrap_or_default().chunks_exact(2) {
		pdu.extend_from_slice(did);
		pdu.extend(std::iter::repeat_n(0, usize::from(record)));
	}
	pdu
}

#[cfg(test)]
mod tests {
	use super::*;
	use vag_dash_render::alarm::{Alarm, Bound, ChannelId, PageId};
	use vag_dash_render::plan::{Channel, StopwatchPlan, Unit as PlanUnit};

	/// A channel of the plan's unit `unit`, by index ([`plan`] gives unit `i` the request `700 + 2i`).
	fn channel(unit: u8, did: u16, hz: f32) -> Channel {
		Channel {
			unit,
			did,
			bit_offset: 0,
			bit_length: 8,
			signed: false,
			big_endian: true,
			factor: 1.0,
			offset: 0.0,
			decimals: 0,
			unit_text: "",
			label: "x",
			proven: true,
			hz,
			setpoint: None,
			zero_at_rest: false,
		}
	}

	fn leak<T>(items: Vec<T>) -> &'static [T] {
		Box::leak(items.into_boxed_slice())
	}

	/// A plan of `channels`, one values page of `cells`, an alarm on each of `alarmed`.
	fn plan(channels: Vec<Channel>, cells: Vec<u16>, alarmed: &[u16]) -> Plan {
		let count = channels.iter().map(|c| u16::from(c.unit) + 1).max().unwrap_or(0);
		let units = (0..count)
			.map(|i| PlanUnit {
				request: 0x700 + 2 * i,
				response: 0x700 + 2 * i + 0x6A,
				part_number: "",
			})
			.collect();
		let alarms = alarmed
			.iter()
			.map(|c| Alarm::bounds(ChannelId(*c), PageId(0), Some(Bound { at: 5.0, off: 4.0 }), None))
			.collect();
		Plan {
			vin: "",
			language: "en",
			units: leak(units),
			channels: leak(channels),
			pages: leak(vec![Page::Values {
				title: "P",
				cells: leak(cells),
				volts: None,
			}]),
			alarms: leak(alarms),
			stalk: None,
			stopwatch: None,
			buttons: &[],
			standstill: None,
		}
	}

	/// Ten alarm channels of one unit at 10/s: a hundred identifiers a second, which the old
	/// count of reads would have refused, are two requests a round of 8 and 2 — twenty exchanges a
	/// second, which a bus of 34 ms an exchange serves.
	#[test]
	fn ten_alarm_channels_on_one_unit_are_two_exchanges_a_round() {
		let channels = (0..10).map(|i| channel(0, 0x2000 + i, 10.0)).collect();
		let alarmed: Vec<u16> = (0..10).collect();
		let run = simulate(&plan(channels, vec![0], &alarmed), Glass::Page(0), true, EXCHANGE_MS);
		assert!(run.thinned.is_empty(), "{run:?}");
		// Twenty exchanges a second of 34 ms: 68 % of the bus.
		assert!((0.6..0.75).contains(&run.busy), "{}", run.busy);
	}

	/// Eight alarm channels on eight units at 9/s: 72 identifiers a second, under the old count's
	/// 75, are 72 exchanges a second — more than twice what a bus of 34 ms an exchange makes.
	#[test]
	fn eight_channels_on_eight_units_at_9_hz_are_more_than_the_bus_makes() {
		let channels = (0..8).map(|i| channel(i, 0x2000, 9.0)).collect();
		let alarmed: Vec<u16> = (0..8).collect();
		let run = simulate(&plan(channels, vec![0], &alarmed), Glass::Page(0), true, EXCHANGE_MS);
		assert!(run.busy > 0.99, "{}", run.busy);
		assert!(!run.thinned.is_empty(), "{run:?}");
		for thinned in &run.thinned {
			assert!(thinned.got_per_s < 0.7 * thinned.asked_per_s, "{thinned:?}");
		}
	}

	/// At the edge: one exchange a round per unit, each unit at the same rate. 29 a second fits
	/// a bus that makes 1000 / 34 = 29.4; 30 do not.
	#[test]
	fn the_edge_is_where_the_exchanges_fill_the_second() {
		// Ten units at 2.9/s and at 3/s: 29 and 30 exchanges a second.
		let at = |hz: f32| {
			let channels = (0..10).map(|i| channel(i, 0x2000, hz)).collect();
			let alarmed: Vec<u16> = (0..10).collect();
			simulate(&plan(channels, vec![0], &alarmed), Glass::Page(0), true, EXCHANGE_MS)
		};
		let fits = at(2.9);
		assert!(fits.thinned.is_empty(), "{fits:?}");
		assert!(fits.busy > 0.95, "{}", fits.busy);
		let over = at(3.0);
		assert!(!over.thinned.is_empty(), "{over:?}");
	}

	/// The page on the glass is what is held to its rate; the pages not on it are read once a
	/// second at most and thinned first.
	#[test]
	fn the_pages_behind_the_glass_are_thinned_first() {
		// Page 0: one channel of unit 0 at 20/s. Pages 1 to 10: forty units' channels, read behind it.
		let mut channels: Vec<Channel> = vec![channel(0, 0x2000, 20.0)];
		channels.extend((1..=40).map(|i| channel(i, 0x2000, 2.0)));
		let mut plan = plan(channels, vec![0], &[]);
		let behind: Vec<Page> = (0..10u16)
			.map(|p| Page::Values {
				title: "",
				cells: leak((1 + 4 * p..5 + 4 * p).collect()),
				volts: None,
			})
			.collect();
		plan.pages = leak([plan.pages.to_vec(), behind].concat());
		let run = simulate(&plan, Glass::Page(0), true, EXCHANGE_MS);
		assert!(run.thinned.is_empty(), "{run:?}");
		assert!(run.behind_thinned, "{run:?}");
		assert!(run.busy > 0.99, "{}", run.busy);
	}

	/// During a run the speed is the board's clock and the pages' cells drop behind it: a page
	/// too heavy for the bus does not thin the run's speed.
	#[test]
	fn a_run_reads_its_speed_ahead_of_the_pages() {
		let mut channels: Vec<Channel> = (0..4).map(|i| channel(i, 0x2000, 20.0)).collect();
		channels.push(channel(4, 0xF40D, 20.0));
		let mut plan = plan(channels, vec![0, 1, 2, 3], &[]);
		plan.stopwatch = Some(StopwatchPlan {
			speed: 4,
			km_h_per_unit: 1.0,
			marks: &[100],
		});
		let estimates = estimate(&plan, EXCHANGE_MS);
		let runs: Vec<&Run> = estimates.iter().map(|e| &e.cruise_off).collect();
		assert_eq!(runs.iter().map(|r| r.glass).collect::<Vec<_>>(), [Glass::Page(0), Glass::Run]);
		assert!(estimates.iter().all(|e| e.cruise_on.is_none()), "no lever, no second run");
		assert!(!runs[0].thinned.is_empty(), "four units at 20/s on the glass: {:?}", runs[0]);
		// The speed's 20 exchanges a second and the four units' cells once a second behind it.
		assert!(runs[1].thinned.is_empty(), "{:?}", runs[1]);
		assert!((0.75..0.9).contains(&runs[1].busy), "{:?}", runs[1]);
	}

	#[test]
	fn an_answer_carries_every_identifier_asked() {
		assert_eq!(answer(&[0x22, 0xF4, 0x0D, 0x20, 0x2A], 2), [0x62, 0xF4, 0x0D, 0, 0, 0x20, 0x2A, 0, 0]);
	}
}
