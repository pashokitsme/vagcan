//! `vagcan scan` — ask ONE control unit what it will actually give us.
//!
//! This command was written to ask a unit directly, because a sweep of the
//! `ReadDataByIdentifier` space finds values no label file mentions. That is
//! still true, and it is still the only thing here that can discover a channel
//! nothing describes. It is also *a fuzz test of a diagnostic server* — the
//! operation that cost the reference car its power
//! steering, twice.
//!
//! So the default is no longer a sweep of anything. A unit is asked the
//! identifiers some source **declares** it answers — its ODIS variant, resolved
//! through what the unit itself reports, or a catalog proven on a car; see
//! [`crate::declared`]. Sweeping identifier space nothing vouches for is
//! `--blind`, aimed by hand at one unit, and it says what it costs.
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

/// The bands the existing capture crib already showed to be live on this car's
/// engine ECU (`research/labels/rod-labels.md` §4.0a/§4.0b), plus the standard
/// identification block. The default, because a full `0000-FFFF` sweep is
/// 65,536 requests — minutes at best, and most of it is refusals.
///
/// On the reference engine only the `F1xx` part of this answered: the two crib
/// bands returned nothing. They are kept anyway — one car's silence is not
/// evidence that another car's unit is silent there, and under group testing
/// ([`scan_dids_fast`]) 771 identifiers cost about a hundred requests, not 771.
/// What that run *did* show is that the default can finish having found only
/// what `properties` already prints, so [`summary`] now says so and names the
/// commands that go further.
pub const DEFAULT_RANGES: &str = "7400-7500,A000-A100,F100-F200";

/// How many identifiers a range list covers.
pub fn total_dids(ranges: &[RangeInclusive<u16>]) -> usize {
	ranges.iter().map(|r| *r.end() as usize - *r.start() as usize + 1).sum()
}

/// The safety half of a sweep: the watchdog it carries with it.
///
/// A sweep is the most invasive thing this tool does, and until 9 August 2026
/// it ran without one — a unit that stopped answering was counted in
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
			stats.asked += 1;
			let result = uds.read_data_by_identifier(did).await;
			let answer = anomaly::Answer::of(&result);
			match result {
				Ok(data) => {
					stats.hits += 1;
					on_hit(&DidHit { did, data })?;
				}
				// A refusal is the normal answer for an identifier the ECU
				// does not implement — that is what the sweep is measuring.
				Err(UdsError::NegativeResponse { .. }) => stats.refused += 1,
				Err(_) => stats.failed += 1,
			}
			// Judged after the hit is reported, so an interrupted sweep keeps
			// the identifier that was being read when it stopped.
			if guard.monitor.saw(did, answer).is_some() {
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
			stats.asked += 1;
			let result = uds.read_data_by_identifier(first).await;
			let answer = anomaly::Answer::of(&result);
			match result {
				Ok(data) => {
					stats.hits += 1;
					on_hit(&DidHit { did: first, data })?;
				}
				Err(UdsError::NegativeResponse { .. }) => stats.refused += 1,
				Err(_) => stats.failed += 1,
			}
			if guard.monitor.saw(first, answer).is_some() {
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

/// One report line for a hit: `A058  55 55`, plus the text when the bytes are
/// printable and the documented name when the identifier has one.
///
/// Both come from [`crate::props`], which is what `vagcan properties` renders
/// with — the two commands sweep the same identification block, so a name and a
/// value that read one way there must read the same way here. In particular the
/// text goes through [`crate::props::Property::text`], which cuts at a NUL and
/// trims VW's trailing-space padding: `properties` showed `8V0906264H` where
/// this printed `"8V0906264H "`.
pub fn format_hit(hit: &DidHit) -> String {
	let property = crate::props::Property {
		did: hit.did,
		data: hit.data.clone(),
	};
	let text = property.text().map(|t| format!("  \"{t}\"")).unwrap_or_default();
	let name = crate::props::name_of(hit.did).map(|n| format!("  — {n}")).unwrap_or_default();
	format!("{:04X}  {}{text}{name}", hit.did, property.hex())
}

/// The closing report of a sweep: what answered, and what to do next.
///
/// Kept pure so the advice is tested without a car. `found` is every identifier
/// that answered, in the order they were reported.
pub fn summary(unit_label: &str, total: usize, stats: ScanStats, found: &[u16], elapsed_s: f64) -> String {
	let mut out = format!(
		"\n{} of {total} identifiers answered ({} refused, {} unanswered) in {elapsed_s:.1}s \
         using {} requests\n",
		stats.hits, stats.refused, stats.failed, stats.asked
	);

	if stats.asked > 0 && stats.failed == stats.asked {
		out.push_str(
			"\nNothing answered at all. Check the ignition, the wiring (OBD-II pin 6 → CAN-H, \
             pin 14 → CAN-L), the termination jumper being OFF, and that --ecu names a control \
             unit this car has.\n",
		);
		return out;
	}

	// A sweep that only turned up identification data has told the user
	// nothing `properties` would not have told them faster, and on the
	// reference car that is exactly what the default range did. Say so, and
	// name the two commands that go further, rather than leaving the reader to
	// notice that every hit begins with F1.
	let ident = parse_ranges(crate::props::IDENT_RANGE).expect("the built-in range parses");
	let all_ident = !found.is_empty() && found.iter().all(|did| ident.iter().any(|r| r.contains(did)));
	if all_ident {
		let whole_space = format!("vagcan scan --ecu {unit_label} --blind --range 0000-FFFF");
		let width = whole_space.len();
		out.push_str(&format!(
			"\nEverything that answered is in the identification block, which\n\
             `vagcan properties --ecu {unit_label}` shows named and in order.\n\n\
             To go further:\n  \
             {:<width$}   every unit, the identifiers its own data declares\n  \
             {whole_space}   this unit's whole identifier space — a fuzz test of its\n\
             {:<width$}   diagnostic server, and slow.\n",
			"vagcan survey", ""
		));
	} else if found.is_empty() {
		out.push_str(
			"\nThe unit answered nothing that was asked of it. `vagcan survey` shows which \
             units this car has and what each one's own data declares. To go past that on \
             this unit, `--blind --range 0000-FFFF` sweeps its whole identifier space — \
             which is a fuzz test of its diagnostic server.\n",
		);
	}
	out
}

/// What a `vagcan scan` run was asked to do.
///
/// Bundled rather than passed positionally because two of these decide whether
/// this is a read or an experiment: `blind` turns the command back into the
/// sweep that cost this car its steering assist, and `while_driving` decides
/// whether that may happen at speed. Named fields cannot be swapped by
/// accident.
pub struct Options<'a> {
	pub unit: vag_uds_client::address::UnitAddress,
	/// Hex ranges to sweep **blind**. Meaningless without `blind`, and refused
	/// rather than ignored there — see [`crate::declared::blind_ranges`].
	pub range: Option<&'a str>,
	/// Where to write the answers, if anywhere.
	pub out: Option<&'a str>,
	pub delay_ms: u64,
	/// Sweep even though the car is moving.
	pub while_driving: bool,
	/// Ask identifiers nothing declares. Opt-in, and aimed at this one unit.
	pub blind: bool,
}

/// The identifiers read before anything else, to find out what unit this is.
///
/// `F187`/`F19E`/`F1A2` are what the variant lookup is keyed on, and `F187` is
/// also the witness the guard re-reads. All three are standardised
/// identification identifiers (ISO 14229 / VW's block) — not facts about any
/// particular car.
const IDENTITY: [u16; 3] = [0xF187, 0xF19E, 0xF1A2];

/// Read one control unit's identity, and seed the guard with what it answered.
///
/// The identification block is the sweep's *baseline*, not part of it: units on
/// the reference car answer `F187` and refuse half the rest of the block, and
/// policing that would stop a run on a unit behaving exactly as it always has.
/// So answers are recorded and nothing here is judged.
/// Returns what it read and the **witness** — the first of those identifiers
/// the unit actually answered, which is what the guard re-reads to ask "are you
/// still there". `None` for a unit that answered none of them: there is nothing
/// to re-read, and a unit that never spoke cannot have stopped.
async fn read_identity<T: AsyncIsoTpTransport>(uds: &mut AsyncUdsClient<T>, monitor: &mut anomaly::Monitor) -> ([Option<String>; 3], Option<u16>) {
	let mut out: [Option<String>; 3] = [None, None, None];
	let mut witness = None;
	for (slot, did) in IDENTITY.iter().enumerate() {
		if let Ok(bytes) = uds.read_data_by_identifier(*did).await {
			monitor.seed(*did);
			witness = witness.or(Some(*did));
			let text = String::from_utf8_lossy(&bytes).trim_end_matches(['\0', ' ']).to_string();
			out[slot] = (!text.is_empty()).then_some(text);
		}
	}
	(out, witness)
}

/// Sweep one control unit's identifiers against a real adapter (the `vagcan
/// scan` command).
pub async fn run(device_path: &str, baud: u32, options: Options<'_>) -> anyhow::Result<()> {
	use anyhow::Context as _;
	use std::io::Write;
	use std::time::Instant;
	use vag_uds_can::{IsoTpCan, SlcanBackend, SlcanBitrate};
	use vag_uds_transport::CanId;

	let Options {
		unit,
		range,
		out,
		delay_ms,
		while_driving,
		blind,
	} = options;

	// Checked before the adapter is opened: it is a single-user resource, and
	// holding it open to fail on a flag combination blocks the next attempt.
	let blind_ranges = crate::declared::blind_ranges(range, blind, DEFAULT_RANGES)?;

	let mut sink: Option<std::io::BufWriter<std::fs::File>> = match out {
		Some(path) => {
			let file = std::fs::File::create(path).with_context(|| format!("creating results file {path:?}"))?;
			Some(std::io::BufWriter::new(file))
		}
		None => None,
	};

	let mut backend = SlcanBackend::open(device_path, baud, SlcanBitrate::Rate500k)
		.await
		.with_context(|| crate::device::open_failure(device_path))?;

	// This is a sweep, and a sweep is a fuzz of the unit's diagnostic server:
	// requests it may never have been asked before, any one of which its
	// firmware may mishandle. That is what took the steering assist off the
	// reference car. `survey` is this command run over every unit and is
	// guarded the same way; guarding one and not the other would only mean the
	// danger moves to whichever spelling is unguarded.
	if !while_driving {
		backend = match crate::safety::require_stationary(backend).await {
			Ok(backend) => backend,
			Err((_, why)) => anyhow::bail!(
				"{why}\n\n\
                 A sweep asks a unit for identifiers it may never have been asked for \n\
                 before. On the reference car that made the steering assist stop assisting \n\
                 mid-drive. Sweep while parked, or pass --while-driving if you accept that \n\
                 risk with the car in motion."
			),
		};
	}

	let (store, extracted) = crate::declared::sources();
	let mut monitor = anomaly::Monitor::new(unit.request);
	let mut uds = AsyncUdsClient::new(IsoTpCan::new(backend, CanId::Standard(unit.request), CanId::Standard(unit.response)));

	// What this unit is, in its own words — the key everything below is looked
	// up by, and never a table about one car.
	let ([part_number, odx_name, version], witness) = read_identity(&mut uds, &mut monitor).await;
	let declared = crate::declared::declared(&store, &extracted, part_number.as_deref(), odx_name.as_deref(), version.as_deref());
	let ask = crate::declared::ask(&declared, blind_ranges.as_deref());
	let total = ask.total();

	let mut progress = crate::progress::Line::new();
	if ask.is_empty() {
		// The one case the default cannot sweep. Identified, not fuzzed.
		let label = unit.label();
		println!(
			"{}",
			crate::declared::no_source_notice(&label, &format!("vagcan scan --ecu {label} --blind"))
		);
		return Ok(());
	}

	println!(
		"scanning control unit {} ({:03X}) — {total} {}, {}",
		unit.label(),
		unit.request,
		crate::render::plural(total, "identifier"),
		match ask.source {
			crate::declared::Source::Blind => "swept blind".to_string(),
			_ => format!("declared for {}", odx_name.clone().or(part_number.clone()).unwrap_or_default()),
		}
	);

	// Group testing is only valid if the unit answers a mixed request with the
	// identifiers it does support. Establish that before relying on it, using an
	// identifier this unit has already answered rather than a hoped-for one:
	// probing with one it does not answer makes every batch look empty, and the
	// sweep then reports success having read nothing.
	let batched = probe_batching(&mut uds, witness.unwrap_or(0xF190)).await;
	println!(
		"{}\n",
		if batched {
			"probing in batches of 8"
		} else {
			"this unit refuses mixed requests — falling back to one at a time"
		}
	);

	let started = Instant::now();
	let mut found: Vec<u16> = Vec::new();
	let on_hit = |hit: &DidHit| {
		found.push(hit.did);
		println!("{}", format_hit(hit));
		if let Some(w) = sink.as_mut() {
			// JSON lines, so results join against a capture without a parser.
			let line = serde_json::json!({
					"did": format!("{:04X}", hit.did),
					"data": hit.data.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(""),
			});
			writeln!(w, "{line}")?;
		}
		Ok(())
	};
	// The witness: an identifier this unit answered a moment ago, re-read
	// through the sweep so a unit that falls over is caught while it is still
	// the most recent thing that happened to the car.
	let mut guard = Guard {
		witness,
		monitor: &mut monitor,
	};
	let stats = if batched {
		scan_dids_fast(&mut uds, &ask.ranges, Duration::from_millis(delay_ms), &mut guard, on_hit).await?
	} else {
		scan_dids(&mut uds, &ask.ranges, Duration::from_millis(delay_ms), 400, &mut guard, on_hit).await?
	};
	if let Some(w) = sink.as_mut() {
		w.flush()?;
	}
	// One last look before calling the unit healthy: a sweep short enough never
	// to reach a witness re-read would otherwise end without ever checking.
	guard.check(&mut uds).await;

	if let Some(anomaly) = monitor.halted() {
		// Not `println!`: this must not share a line with anything that
		// rewrites itself. See `crate::progress::Line::notice`.
		progress.notice(&anomaly.report());
		anyhow::bail!("the sweep was stopped: control unit {} changed while it was being read", anomaly.unit());
	}

	print!("{}", summary(&unit.label(), total, stats, &found, started.elapsed().as_secs_f64()));
	Ok(())
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
		// The shipped default must itself parse.
		assert!(parse_ranges(DEFAULT_RANGES).is_ok());
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
		let mut monitor = anomaly::Monitor::new(0x712);

		let stats = scan_dids(&mut uds, &[0x2000..=0x20FF], Duration::ZERO, 0, &mut unwatched(&mut monitor), |_| Ok(()))
			.await
			.unwrap();

		assert_eq!(stats.asked, 4, "it stopped after the third silence, not at the end of the range");
		let halt = monitor.halted().expect("a unit that went quiet must end the run");
		assert_eq!(halt.request, 0x712);
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

		let mut monitor = anomaly::Monitor::new(0x712);
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

	#[test]
	fn hits_print_as_hex_and_as_text_when_the_bytes_are_printable() {
		assert_eq!(
			format_hit(&DidHit {
				did: 0xA058,
				data: vec![0x55, 0x55]
			}),
			"A058  55 55",
		);
		// A part number reads as text — the common shape of an identity DID.
		assert_eq!(
			format_hit(&DidHit {
				did: 0xF187,
				data: b"8V0906264H".to_vec()
			}),
			"F187  38 56 30 39 30 36 32 36 34 48  \"8V0906264H\"  — VW spare part number",
		);
	}

	#[test]
	fn a_named_identifier_is_named_here_exactly_as_properties_names_it() {
		// What the reference engine returns for F187, padding included. The two
		// commands sweep the same block; disagreeing about whether it can be
		// named — or about the trailing space — is what this pins.
		let line = format_hit(&DidHit {
			did: 0xF187,
			data: b"8V0906264H ".to_vec(),
		});
		assert!(line.contains(crate::props::name_of(0xF187).unwrap()), "{line}");
		assert!(line.contains("\"8V0906264H\""), "the padding is trimmed: {line}");

		// An identifier with no documented name gets no invented one.
		let line = format_hit(&DidHit {
			did: 0x7401,
			data: vec![0x00, 0x01],
		});
		assert_eq!(line, "7401  00 01");
	}

	#[test]
	fn a_sweep_that_only_found_identification_data_says_where_to_go_next() {
		// The reference car's result with the default range: every hit an F1xx
		// identifier, i.e. a subset of what `properties` prints.
		let stats = ScanStats {
			asked: 100,
			hits: 3,
			refused: 97,
			failed: 0,
		};
		let text = summary("01", 771, stats, &[0xF187, 0xF190, 0xF19E], 12.5);
		assert!(text.contains("3 of 771 identifiers answered"), "{text}");
		assert!(text.contains("vagcan properties --ecu 01"), "{text}");
		assert!(text.contains("0000-FFFF"), "{text}");
		assert!(text.contains("vagcan survey"), "{text}");

		// One hit outside the block means the sweep earned its time; no advice.
		let text = summary("01", 771, stats, &[0xF187, 0xA058], 12.5);
		assert!(!text.contains("vagcan properties"), "{text}");
	}

	#[test]
	fn silence_and_emptiness_are_told_apart() {
		// Nothing on the wire at all: a wiring or ignition problem.
		let dead = ScanStats {
			asked: 50,
			hits: 0,
			refused: 0,
			failed: 50,
		};
		let text = summary("01", 771, dead, &[], 4.0);
		assert!(text.contains("Nothing answered at all"), "{text}");

		// The unit answered — with refusals. That is a range worth widening,
		// not a cable worth checking.
		let refusing = ScanStats {
			asked: 50,
			hits: 0,
			refused: 400,
			failed: 0,
		};
		let text = summary("01", 771, refusing, &[], 4.0);
		assert!(!text.contains("Nothing answered at all"), "{text}");
		assert!(text.contains("--range 0000-FFFF"), "{text}");
	}
}
