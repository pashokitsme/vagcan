//! Asking ONE control unit what it will actually give us.
//!
//! The machinery a sweep is made of, and nothing that drives one: `vagcan dev
//! survey` is the only command built on this, and `--only` aims it at a single
//! unit. There used to be a second spelling — `vagcan scan`, one unit at a time
//! — whose flags matched `survey`'s field for field and which `survey` was
//! itself built on. A driver had two commands to learn and the tool had two
//! places for a guard to be forgotten in, which is exactly how `properties`
//! came to have none.
//!
//! A sweep finds values no label file mentions, and it is also *a fuzz test of a
//! diagnostic server*: a path with a defect in it crashes the server, and the
//! server here is a control unit the car is relying on. So the default is no
//! longer a sweep of anything. A unit is asked the identifiers some source
//! **declares** it answers — its ODIS variant, resolved through what the unit
//! itself reports, or a catalog proven on a car; see [`crate::declared`].
//! Sweeping identifier space nothing vouches for is `--blind`, aimed by hand at
//! units named one at a time, and it says what it costs.
//!
//! And every sweep, declared or blind, carries [`Guard`]: the moment a unit
//! that had been answering stops, or goes back on an identifier it already
//! answered, the run ends. See [`crate::anomaly`].
//!
//! Read-only by construction: the only service issued is `0x22`, which the UDS
//! client's allowlist already restricts us to.

use std::ops::RangeInclusive;
use std::time::Duration;

use vag_uds_client::AsyncUdsClient;
use vag_uds_client::uds::UdsError;
use vag_uds_transport::AsyncIsoTpTransport;

use crate::anomaly;

/// One identifier the ECU answered, with the bytes it returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DidHit {
	pub did: u16,
	pub data: Vec<u8>,
}

/// The outcome of a sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScanStats {
	/// Identifiers asked for.
	pub asked: usize,
	/// Identifiers that returned data.
	pub hits: usize,
	/// Identifiers the ECU refused (the expected answer for most of the space).
	pub refused: usize,
	/// Identifiers whose read failed on the transport (timeout, malformed).
	pub failed: usize,
}

/// Parse `--range`: a comma-separated list of inclusive hex spans,
/// e.g. `7400-7500,A000-A100`. A bare value is a one-identifier span.
pub fn parse_ranges(spec: &str) -> Result<Vec<RangeInclusive<u16>>, String> {
	let mut out = Vec::new();
	for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
		let (lo, hi) = match part.split_once('-') {
			Some((a, b)) => (a.trim(), b.trim()),
			None => (part, part),
		};
		let lo = u16::from_str_radix(lo, 16).map_err(|_| format!("bad hex DID {lo:?}"))?;
		let hi = u16::from_str_radix(hi, 16).map_err(|_| format!("bad hex DID {hi:?}"))?;
		if lo > hi {
			return Err(format!("range {part:?} runs backwards"));
		}
		out.push(lo..=hi);
	}
	if out.is_empty() {
		return Err("no ranges given".to_string());
	}
	Ok(out)
}

/// How many identifiers a range list covers.
pub fn total_dids(ranges: &[RangeInclusive<u16>]) -> usize {
	ranges.iter().map(|r| *r.end() as usize - *r.start() as usize + 1).sum()
}

/// The safety half of a sweep: the watchdog it carries with it.
///
/// A sweep is the most invasive thing this tool does, and it used to run
/// without one — a unit that stopped answering was counted in
/// [`ScanStats::failed`] and the sweep moved on to the next identifier, and
/// then to the next unit. Every sweep now carries one of these, so there is no
/// spelling of "sweep" that is unwatched.
///
/// `witness` is an identifier this unit is known to answer, re-read every
/// [`anomaly::WITNESS_EVERY`] requests. Most of an identifier space is refusals,
/// so a unit that has fallen over and one that simply implements nothing here
/// look identical from the outside; the witness is what tells them apart.
pub struct Guard<'a> {
	pub witness: Option<u16>,
	pub monitor: &'a mut anomaly::Monitor,
}

impl Guard<'_> {
	/// Re-read the witness. `true` means the run ends here.
	///
	/// Errors are not swallowed: a witness that times out is exactly the event
	/// this is looking for, and [`anomaly::Monitor`] is what decides whether it
	/// is one lost frame or a unit that has stopped talking.
	async fn check<T: AsyncIsoTpTransport>(&mut self, uds: &mut AsyncUdsClient<T>) -> bool {
		let Some(witness) = self.witness else { return false };
		let answer = anomaly::Answer::of(&uds.read_data_by_identifier(witness).await);
		self.monitor.saw(witness, answer).is_some()
	}
}

/// Read one identifier, count what came back, report it, and say whether the
/// sweep must stop.
///
/// **Both sweeps do exactly this, and each had its own copy.** What the copies
/// held is a *classification* — which UDS answer is a hit, which is the ordinary
/// refusal an unimplemented identifier gives, and which is a failure worth
/// counting as one — and two copies of a classification is two sets of numbers
/// that can disagree about the same car with nothing on screen saying which
/// sweep you were on.
///
/// `true` means [`anomaly::Monitor`] saw the unit change under the sweep and the
/// caller must return the statistics gathered so far. The monitor is asked
/// *after* the hit is reported, so an interrupted sweep keeps the identifier
/// that was being read when it stopped.
async fn read_one<T: AsyncIsoTpTransport, F: FnMut(&DidHit) -> std::io::Result<()>>(
	uds: &mut AsyncUdsClient<T>,
	did: u16,
	stats: &mut ScanStats,
	guard: &mut Guard<'_>,
	on_hit: &mut F,
) -> std::io::Result<bool> {
	stats.asked += 1;
	let result = uds.read_data_by_identifier(did).await;
	let answer = anomaly::Answer::of(&result);
	match result {
		Ok(data) => {
			stats.hits += 1;
			on_hit(&DidHit { did, data })?;
		}
		// A refusal is the normal answer for an identifier the ECU does not
		// implement — that is what the sweep is measuring.
		Err(UdsError::NegativeResponse { .. }) => stats.refused += 1,
		Err(_) => stats.failed += 1,
	}
	Ok(guard.monitor.saw(did, answer).is_some())
}

/// Sweep `ranges`, calling `on_hit` for every identifier that answers.
///
/// `on_hit` is invoked as results arrive rather than at the end, so an
/// interrupted sweep keeps everything it found. A `TesterPresent` goes out
/// every `keepalive_every` identifiers to hold the session open through the
/// long stretches of refusals; pass `0` to disable it.
///
/// **Returns early when `guard` fires**, with the statistics gathered so far.
/// The caller must ask [`anomaly::Monitor::halted`] afterwards rather than
/// treating the return as success: a sweep that stopped because a control unit
/// changed under it has not finished, it has been stopped.
pub async fn scan_dids<T, F>(
	uds: &mut AsyncUdsClient<T>,
	ranges: &[RangeInclusive<u16>],
	delay: Duration,
	keepalive_every: usize,
	guard: &mut Guard<'_>,
	mut on_hit: F,
) -> std::io::Result<ScanStats>
where
	T: AsyncIsoTpTransport,
	F: FnMut(&DidHit) -> std::io::Result<()>,
{
	let mut stats = ScanStats::default();
	for range in ranges {
		for did in range.clone() {
			if keepalive_every > 0 && stats.asked > 0 && stats.asked % keepalive_every == 0 {
				let _ = uds.tester_present().await;
			}
			if stats.asked > 0 && stats.asked % anomaly::WITNESS_EVERY == 0 && guard.check(uds).await {
				return Ok(stats);
			}
			if read_one(uds, did, &mut stats, guard, &mut on_hit).await? {
				return Ok(stats);
			}
			if !delay.is_zero() {
				tokio::time::sleep(delay).await;
			}
		}
	}
	Ok(stats)
}

/// Identifiers per presence probe.
///
/// Measured on the reference car: 8 identifiers in one request are answered,
/// 12 are refused outright with `0x31` — so the limit sits between, and asking
/// for more than the unit accepts makes every batch look empty. That failure
/// is silent and total, which is why [`probe_batching`] tests a full-size
/// batch rather than a token pair.
pub const BATCH: usize = 8;

/// Sweep by group testing — the fast path.
///
/// Most of the identifier space is unimplemented, and this control unit family
/// answers a multi-identifier request by returning only the identifiers it
/// supports, refusing (`0x31`) exactly when it supports none of them. That
/// makes one request a presence test for a whole batch: a refusal skips the
/// whole batch at once, and a positive answer is halved until responders are
/// isolated and read individually for their bytes.
///
/// Verified against the reference car before being relied on: a request mixing
/// a supported and an unsupported identifier returns just the supported one.
/// A control unit that refused the whole mixed request instead would make this
/// unsound — hence [`probe_batching`], which the command runs first.
///
/// **Returns early when `guard` fires**, exactly as [`scan_dids`] does, and with
/// the same obligation on the caller.
pub async fn scan_dids_fast<T, F>(
	uds: &mut AsyncUdsClient<T>,
	ranges: &[RangeInclusive<u16>],
	delay: Duration,
	guard: &mut Guard<'_>,
	mut on_hit: F,
) -> std::io::Result<ScanStats>
where
	T: AsyncIsoTpTransport,
	F: FnMut(&DidHit) -> std::io::Result<()>,
{
	let mut stats = ScanStats::default();

	// Work items are (first, last) inclusive spans, processed depth-first so a
	// hit is isolated and reported before moving on.
	let mut work: Vec<(u16, u16)> = Vec::new();
	for range in ranges.iter().rev() {
		let (start, end) = (*range.start(), *range.end());
		let mut at = start;
		loop {
			let last = at.saturating_add(BATCH as u16 - 1).min(end);
			work.push((at, last));
			if last >= end {
				break;
			}
			at = last + 1;
		}
	}
	work.reverse();

	while let Some((first, last)) = work.pop() {
		if !delay.is_zero() {
			tokio::time::sleep(delay).await;
		}
		if stats.asked > 0 && stats.asked % anomaly::WITNESS_EVERY == 0 && guard.check(uds).await {
			return Ok(stats);
		}
		if first == last {
			if read_one(uds, first, &mut stats, guard, &mut on_hit).await? {
				return Ok(stats);
			}
			continue;
		}

		let dids: Vec<u16> = (first..=last).collect();
		stats.asked += 1;
		let split_span = |work: &mut Vec<(u16, u16)>| {
			let mid = first + (last - first) / 2;
			work.push((mid + 1, last));
			work.push((first, mid));
		};
		// A group answer is about the span, not about any one identifier in it,
		// so nothing here is recorded *against* an identifier — a positive reply
		// does not say which member answered, and writing `first` down as
		// answered would make the single read of `first` two steps later look
		// like a unit going back on itself. `heard` says only that the unit is
		// still talking, which is all a batch reply proves.
		match uds.read_data_by_identifiers(&dids).await {
			// Something in this span answers — split and find out what.
			Ok(_) => {
				guard.monitor.heard();
				split_span(&mut work)
			}
			// ONLY requestOutOfRange means "none of these is implemented".
			// Any other refusal says something about the request, not about
			// the identifiers — responseTooLong or busyRepeatRequest on a
			// batch full of real values would otherwise write all of them off
			// as unimplemented, silently, since a refusal is the expected
			// answer. Fall back to probing the span in halves.
			Err(UdsError::NegativeResponse { nrc: 0x31, .. }) => {
				guard.monitor.heard();
				stats.refused += dids.len();
			}
			Err(UdsError::NegativeResponse { .. }) => {
				guard.monitor.heard();
				split_span(&mut work)
			}
			// A transport failure is not evidence either; the slow path loses
			// one identifier to a timeout, so this must not lose eight. It is
			// evidence about the *unit*, though: a span that times out and then
			// times out again in halves is a unit that has stopped talking.
			Err(_) => {
				if guard.monitor.silent_span(first).is_some() {
					return Ok(stats);
				}
				split_span(&mut work)
			}
		}
	}
	Ok(stats)
}

/// Check that group testing is sound on this control unit.
///
/// Asks for one identifier known to answer, padded out to a **full batch** with
/// identifiers that cannot, and reports whether the unit returned the supported
/// one anyway. Two failure modes are ruled out at once: a unit that refuses any
/// mixed request, and a unit whose per-request limit is below [`BATCH`]. Either
/// would make a refusal stop meaning "none supported", and the sweep would skip
/// real identifiers while reporting success.
pub async fn probe_batching<T: AsyncIsoTpTransport>(uds: &mut AsyncUdsClient<T>, known_good: u16) -> bool {
	let mut dids = vec![known_good];
	// 0x0000.. are not valid data identifiers on these units.
	dids.extend((0..BATCH as u16 - 1).map(|i| i + 1));
	uds.read_data_by_identifiers(&dids).await.is_ok()
}

#[cfg(test)]
mod tests {
	// `&[0x2000..=0x20FF]` is one range inside a slice of ranges, which is what
	// `scan_dids` and friends take. Clippy reads it as a possible typo for
	// `(0x2000..=0x20FF).collect()`; the parameter type makes that reading
	// impossible, and spelling one range as a vec! of one would be worse.
	#![allow(clippy::single_range_in_vec_init)]
	use super::*;
	use vag_uds_transport::MockAsyncTransport;

	fn req(did: u16) -> Vec<u8> {
		vec![0x22, (did >> 8) as u8, (did & 0xFF) as u8]
	}
	fn resp(did: u16, data: &[u8]) -> Vec<u8> {
		let mut v = vec![0x62, (did >> 8) as u8, (did & 0xFF) as u8];
		v.extend_from_slice(data);
		v
	}
	/// requestOutOfRange — what an ECU says about an identifier it lacks.
	fn refused() -> Vec<u8> {
		vec![0x7F, 0x22, 0x31]
	}
	/// Nothing came back. The mock answers an empty PDU, which the client
	/// cannot classify — the shape of a timeout as far as the sweep is
	/// concerned.
	fn silence() -> Vec<u8> {
		Vec::new()
	}

	/// A guard that watches nothing, for the tests about counting rather than
	/// about safety. `Guard` is not optional in the signature precisely so that
	/// there is no spelling of "sweep" without one.
	fn unwatched(monitor: &mut anomaly::Monitor) -> Guard<'_> {
		Guard { witness: None, monitor }
	}

	#[test]
	fn ranges_parse_from_hex_spans() {
		assert_eq!(parse_ranges("7400-7402").unwrap(), vec![0x7400..=0x7402]);
		assert_eq!(parse_ranges("A058, F190-F19A").unwrap(), vec![0xA058..=0xA058, 0xF190..=0xF19A]);
		assert_eq!(total_dids(&parse_ranges("0000-FFFF").unwrap()), 65_536);
		assert!(parse_ranges("F200-F100").is_err(), "backwards range");
		assert!(parse_ranges("zz").is_err(), "not hex");
		assert!(parse_ranges("").is_err(), "empty");
	}

	#[tokio::test]
	async fn a_sweep_records_answers_and_counts_refusals() {
		// Three identifiers: the middle one is not implemented.
		let script = vec![
			(req(0xA058), resp(0xA058, &[0x55, 0x55])),
			(req(0xA059), refused()),
			(req(0xA05A), resp(0xA05A, &[0x01])),
		];
		let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(script));

		let mut hits = Vec::new();
		let mut monitor = anomaly::Monitor::new(0x7E0);
		let stats = scan_dids(&mut uds, &[0xA058..=0xA05A], Duration::ZERO, 0, &mut unwatched(&mut monitor), |hit| {
			hits.push(hit.clone());
			Ok(())
		})
		.await
		.unwrap();

		assert_eq!(stats.asked, 3);
		assert_eq!(stats.hits, 2);
		assert_eq!(stats.refused, 1);
		assert_eq!(stats.failed, 0);
		assert_eq!(
			hits[0],
			DidHit {
				did: 0xA058,
				data: vec![0x55, 0x55]
			}
		);
		assert_eq!(
			hits[1],
			DidHit {
				did: 0xA05A,
				data: vec![0x01]
			}
		);
	}

	#[tokio::test]
	async fn hits_are_reported_as_they_arrive_not_at_the_end() {
		// The callback must see the first hit before the sweep reaches the
		// second, so an interrupted run keeps what it found.
		let script = vec![(req(0x0001), resp(0x0001, &[0xAA])), (req(0x0002), resp(0x0002, &[0xBB]))];
		let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(script));

		let mut seen_at = Vec::new();
		let mut n = 0usize;
		let mut monitor = anomaly::Monitor::new(0x7E0);
		scan_dids(&mut uds, &[0x0001..=0x0002], Duration::ZERO, 0, &mut unwatched(&mut monitor), |hit| {
			n += 1;
			seen_at.push((hit.did, n));
			Ok(())
		})
		.await
		.unwrap();

		assert_eq!(seen_at, vec![(0x0001, 1), (0x0002, 2)]);
	}

	#[tokio::test]
	async fn a_keepalive_is_interleaved_on_the_configured_cadence() {
		// With keepalive_every = 2, a TesterPresent precedes the third read.
		let script = vec![
			(req(0x0001), resp(0x0001, &[0xAA])),
			(req(0x0002), refused()),
			(vec![0x3E, 0x00], vec![0x7E, 0x00]),
			(req(0x0003), resp(0x0003, &[0xCC])),
		];
		let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(script));

		let mut monitor = anomaly::Monitor::new(0x7E0);
		let stats = scan_dids(&mut uds, &[0x0001..=0x0003], Duration::ZERO, 2, &mut unwatched(&mut monitor), |_| Ok(()))
			.await
			.unwrap();

		assert_eq!(stats.asked, 3);
		assert_eq!(stats.hits, 2);
		assert!(uds.into_transport().is_exhausted(), "the scripted exchange ran exactly");
	}

	#[tokio::test]
	async fn a_unit_that_goes_quiet_mid_sweep_ends_the_sweep() {
		// The defect: the old loop counted these three silences in
		// `stats.failed` and asked for 0x2004, and 0x2005, and then moved on to
		// the next unit. The script here has nothing after 0x2003 — the mock
		// panics if the sweep asks for anything more, so "it stopped" is
		// asserted by the exchange running out exactly.
		let script = vec![
			(req(0x2000), resp(0x2000, &[0x0B, 0x34])),
			(req(0x2001), silence()),
			(req(0x2002), silence()),
			(req(0x2003), silence()),
		];
		let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(script));
		let mut monitor = anomaly::Monitor::new(0x714);

		let stats = scan_dids(&mut uds, &[0x2000..=0x20FF], Duration::ZERO, 0, &mut unwatched(&mut monitor), |_| Ok(()))
			.await
			.unwrap();

		assert_eq!(stats.asked, 4, "it stopped after the third silence, not at the end of the range");
		let halt = monitor.halted().expect("a unit that went quiet must end the run");
		assert_eq!(halt.request, 0x714);
		assert_eq!(halt.did, 0x2003, "the notice names what was being asked");
		assert!(uds.into_transport().is_exhausted(), "nothing was asked after the halt");
	}

	#[tokio::test]
	async fn a_sweep_asks_only_what_it_was_given_and_nothing_in_between() {
		// `declared` hands the sweep spans built from the identifiers a source
		// vouched for. The mock panics on any PDU not in its script, so this is
		// the end-to-end statement: the gap between 0x2001 and 0x3800 is never
		// asked for, where the old default asked 2,300 identifiers around it.
		let declared: std::collections::BTreeSet<u16> = [0x2000, 0x2001, 0x3800].into_iter().collect();
		let ask = crate::declared::ask(&declared, None);
		let script = vec![
			(req(0x2000), resp(0x2000, &[0x01])),
			(req(0x2001), refused()),
			(req(0x3800), resp(0x3800, &[0x02])),
		];
		let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(script));
		let mut monitor = anomaly::Monitor::new(0x7E1);

		let mut asked = Vec::new();
		let stats = scan_dids(&mut uds, &ask.ranges, Duration::ZERO, 0, &mut unwatched(&mut monitor), |hit| {
			asked.push(hit.did);
			Ok(())
		})
		.await
		.unwrap();

		assert_eq!(stats.asked, 3, "three identifiers, not three pages");
		assert_eq!(asked, vec![0x2000, 0x3800]);
		assert!(uds.into_transport().is_exhausted(), "the scripted exchange ran exactly");
	}

	#[tokio::test]
	async fn a_witness_re_read_catches_a_unit_that_stops_answering_it() {
		// Most of an identifier space is refusals, so a unit that has fallen
		// over and one that simply implements nothing here look identical. The
		// witness is what tells them apart: 0xF187 answered during
		// identification, and when it stops the run ends — even though every
		// answer since has been an ordinary refusal.
		let mut script = vec![(req(0xF187), resp(0xF187, b"8V0906264H "))];
		for did in 0x2000..0x2000 + anomaly::WITNESS_EVERY as u16 {
			script.push((req(did), refused()));
		}
		// The witness re-read, at the cadence — and this time it is silent.
		script.push((req(0xF187), silence()));
		let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(script));

		let mut monitor = anomaly::Monitor::new(0x714);
		monitor.seed(0xF187);
		// Establish the witness the way the command does, by reading it.
		assert!(uds.read_data_by_identifier(0xF187).await.is_ok());

		let mut guard = Guard {
			witness: Some(0xF187),
			monitor: &mut monitor,
		};
		scan_dids(&mut uds, &[0x2000..=0x20FF], Duration::ZERO, 0, &mut guard, |_| Ok(()))
			.await
			.unwrap();

		let halt = monitor.halted().expect("the witness stopped answering");
		assert_eq!(halt.did, 0xF187);
		assert!(
			uds.into_transport().is_exhausted(),
			"it stopped at the witness, not at the end of the range"
		);
	}
}
