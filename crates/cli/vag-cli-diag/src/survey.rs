//! `vagcan survey` — walk the whole car, not just the powertrain.
//!
//! Everything this project can read live has so far come from two control
//! units, because those are the two the ISO addressing block reaches. The
//! gateway's installation list names fifteen more
//! (`research/car/other-ecus.md` §3), each answering on VW's own block, and each
//! with an identifier space nobody here has swept.
//!
//! This command does the pass that document calls for: read the installation
//! list, then for every unit in it read the identification block, the fault
//! memory, and **the identifiers that unit's own data declares it answers** —
//! resolved from what the unit reports about itself, through
//! [`crate::declared`]. The result is a file of *what answered*, per unit.
//!
//! It used to sweep nine fixed pages of identifiers at every unit, on no
//! evidence any of them existed. That is a fuzz test of a diagnostic server,
//! and a path with a defect in it crashes the server. Sweeping identifier
//! space nothing vouches for is now `--blind`, aimed at units named one at a
//! time.
//!
//! And the run **stops** the moment a unit that had been answering goes quiet
//! or goes back on an identifier it already answered ([`crate::anomaly`]): the
//! whole run, not that unit, because a unit that has started misbehaving is
//! not made safer by the rest of the sweep, and "it recovered" is a guess
//! until somebody looks.
//!
//! Two runs of this, one parked and one driving, differ exactly in the live
//! measurements. That difference is the point: an identifier whose bytes never
//! move proves nothing about what it measures, and this project has repeatedly
//! had to throw away conclusions drawn from one that did not.
//!
//! Read-only: the services issued are `0x22` and session control.

use std::io::Write as _;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use vag_uds_can::{IsoTpCan, SlcanBackend, SlcanBitrate, SlcanMode};
use vag_uds_client::address::UnitAddress;
use vag_uds_client::uds::UdsError;
use vag_uds_client::{AsyncUdsClient, RawDtc, gateway};
use vag_uds_transport::CanId;

use crate::render::hex_packed;
use crate::scan::{self, DidHit};

/// The identifier pages observed in use on this car, across every unit seen in
/// the two captures (`research/car/other-ecus.md` §6.3): identification and coding
/// (`02xx`, `06xx`, `F1xx`), the BCM's group records (`19xx`), the powertrain
/// measurement bands (`20xx`–`22xx`, `38xx`), the gateway's lists (`2Axx`,
/// `2Bxx`) and the OBD-II mirror (`F4xx`).
///
/// **This is no longer what a survey asks.** It was: every unit got these 2816
/// identifiers whether or not anything said they existed, which is the fuzz test
/// of a diagnostic server — the most invasive thing this tool can do to a car.
/// It is now only the default *blind* range: what `--blind <unit>` sweeps when
/// somebody aims one by hand and names no range of their own.
///
/// It is also, unavoidably, one car's answer — the pages *this* Škoda was seen
/// using. That was defensible as a way to keep a blind sweep under an hour; it
/// was never a statement about any other car, and nothing now depends on it
/// being one.
pub const SURVEY_RANGES: &str = "0200-02FF,0600-06FF,1900-19FF,2000-22FF,2A00-2BFF,3800-38FF,F100-F1FF,F400-F4FF";

/// Identification identifiers, read before the sweep so the report can name the
/// unit even if the sweep is cut short.
const IDENT: &[(u16, &str)] = &[
	(0xF187, "part number"),
	(0xF189, "software version"),
	(0xF191, "hardware number"),
	(0xF197, "component"),
	(0xF19E, "ODX label file"),
	(0xF1A2, "coding index"),
	(0xF1A3, "hardware version"),
];

/// What one control unit answered.
#[derive(Debug, Clone, Default)]
pub struct UnitReport {
	pub request: u16,
	/// Identification fields that answered, in the order above.
	pub ident: Vec<(u16, Vec<u8>)>,
	pub hits: Vec<DidHit>,
	pub stats: scan::ScanStats,
	/// Fault codes, as the unit reports them with the status mask `0xFF` —
	/// which includes codes that have merely never been *tested* since the
	/// last clear. See [`UnitReport::confirmed`].
	pub dtcs: Vec<RawDtc>,
	/// Whether the unit said anything, including a refusal. A refusal is an
	/// answer; silence is the unit not being there.
	pub answered: bool,
}

impl UnitReport {
	/// The unit's component string, when it gave one — the only name that comes
	/// from the car rather than from a table.
	pub fn component(&self) -> Option<String> {
		self.text(0xF197)
	}

	pub fn part_number(&self) -> Option<String> {
		self.text(0xF187)
	}

	/// The ODX label file the unit names — with `odx_version`, the pair that
	/// picks which variant of its family this unit is, and therefore which
	/// identifiers anything knows it answers.
	pub fn odx_name(&self) -> Option<String> {
		self.text(0xF19E)
	}

	pub fn odx_version(&self) -> Option<String> {
		self.text(0xF1A2)
	}

	fn text(&self, did: u16) -> Option<String> {
		let (_, bytes) = self.ident.iter().find(|(d, _)| *d == did)?;
		let s = String::from_utf8_lossy(bytes).trim_end_matches(['\0', ' ']).to_string();
		(!s.is_empty()).then_some(s)
	}

	/// Codes the unit has actually confirmed, as opposed to listed.
	///
	/// Asking with mask `0xFF` returns every code the unit knows about: on the
	/// reference car the body control module answers 508, of which 505 carry
	/// status `0x10` — testNotCompletedSinceClear, i.e. "this test has not run
	/// since the memory was cleared". Reporting that as 508 faults would be
	/// alarming and wrong. Bit 3 (`0x08`, confirmedDTC) is the one that means
	/// the unit stored a failure.
	pub fn confirmed(&self) -> usize {
		self.dtcs.iter().filter(|d| d.status & 0x08 != 0).count()
	}

	/// One line per unit for the console.
	pub fn summary(&self) -> String {
		let address = UnitAddress::from_request(self.request)
			.map(|a| a.label())
			.unwrap_or_else(|| format!("{:03X}", self.request));
		let component = self.component().unwrap_or_default();
		let part = self.part_number().unwrap_or_default();
		if !self.answered {
			return format!("  {address:<4} {:03X}  did not answer", self.request);
		}
		let faults = match self.confirmed() {
			0 => String::new(),
			n => format!(", {n} stored faults"),
		};
		format!(
			"  {address:<4} {:03X}  {:<14} {:<16} {} identifiers{faults}",
			self.request,
			part,
			component,
			self.hits.len()
		)
	}
}

/// Which units to walk: the gateway's list, plus the three that are never in it.
///
/// The list covers VW's block only — the engine and the gearbox live on the ISO
/// block and the gateway does not list itself (§3). Leaving those out would
/// survey the car minus its three most-read units.
fn walk_order(listed: &[u16]) -> Vec<u16> {
	const ALWAYS: [u16; 3] = [0x7E0, 0x7E1, 0x710];
	let mut out: Vec<u16> = ALWAYS.to_vec();
	for id in listed {
		// `0x776`/`0x777` are in the bitmap but are also response ids of units
		// already in it, and `0x776 + 0x6A` collides with the engine's request
		// id. §3 says to try rather than trust them; a timeout is cheap and a
		// wrong assumption is not.
		if !out.contains(id) {
			out.push(*id);
		}
	}
	out
}

/// One unit's identifiers as a survey recorded them.
fn dids_of(line: &serde_json::Value) -> std::collections::BTreeMap<u16, String> {
	let mut out = std::collections::BTreeMap::new();
	let Some(entries) = line["dids"].as_array() else { return out };
	for entry in entries {
		let (Some(did), Some(data)) = (entry["did"].as_str(), entry["data"].as_str()) else {
			continue;
		};
		if let Ok(did) = u16::from_str_radix(did, 16) {
			out.insert(did, data.to_string());
		}
	}
	out
}

/// Compare two survey files and report the identifiers whose bytes changed.
///
/// This is the step the survey exists for. One pass parked and one driving
/// differ exactly in what is live, and that list is obtained without a label
/// file, without VCDS and without guessing — an identifier that moved between
/// two known conditions is a measurement, and one that did not is not evidence
/// of anything.
pub fn diff(before: &str, after: &str) -> Vec<(u16, u16, String, String)> {
	let read = |text: &str| {
		let mut units: std::collections::BTreeMap<u16, std::collections::BTreeMap<u16, String>> = std::collections::BTreeMap::new();
		for line in text.lines().filter(|l| !l.trim().is_empty()) {
			let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
				continue;
			};
			let Some(request) = value["request"].as_str().and_then(|s| u16::from_str_radix(s, 16).ok()) else {
				continue;
			};
			units.insert(request, dids_of(&value));
		}
		units
	};
	let (a, b) = (read(before), read(after));
	let mut out = Vec::new();
	for (request, before_dids) in &a {
		let Some(after_dids) = b.get(request) else { continue };
		for (did, before_data) in before_dids {
			let Some(after_data) = after_dids.get(did) else { continue };
			if before_data != after_data {
				out.push((*request, *did, before_data.clone(), after_data.clone()));
			}
		}
	}
	out
}

/// Print a survey diff (`vagcan survey --diff a.jsonl b.jsonl`).
/// One unit's line in a survey file.
///
/// Extracted from the sweep so the shape can be asserted without a car — and
/// specifically so the round trip can be: `watch` decides which declared
/// channels this vehicle does not have by reading `asked` back out of here
/// (`plan::answered_from_survey`), and a writer and a reader that agree only by
/// inspection will stop agreeing.
fn unit_line(report: &UnitReport, address: &vag_uds_client::address::UnitAddress, batched: bool, ask: &crate::declared::Ask) -> String {
	serde_json::json!({
			"request": format!("{:03X}", report.request),
			"unit": address.label(),
			"batched": batched,
			// What was asked, beside what answered. Without it a reader cannot
			// tell an identifier this unit refused from one nobody ever put to
			// it, and that difference is what decides whether a channel is
			// missing from this car or missing from this run.
			"asked": ask.spans_text(),
			"ident": report.ident.iter().map(|(did, data)| {
					serde_json::json!({ "did": format!("{did:04X}"), "data": hex_packed(data) })
			}).collect::<Vec<_>>(),
			"dids": report.hits.iter().map(|h| {
					serde_json::json!({ "did": format!("{:04X}", h.did), "data": hex_packed(&h.data) })
			}).collect::<Vec<_>>(),
			"confirmed_faults": report.confirmed(),
			"dtcs": report.dtcs.iter().map(|d| {
					serde_json::json!({
							"code": hex_packed(&d.code),
							"status": format!("{:02X}", d.status),
					})
			}).collect::<Vec<_>>(),
	})
	.to_string()
}

pub fn run_diff(before_path: &str, after_path: &str) -> Result<()> {
	let before = std::fs::read_to_string(before_path).with_context(|| format!("reading {before_path:?}"))?;
	let after = std::fs::read_to_string(after_path).with_context(|| format!("reading {after_path:?}"))?;
	let changed = diff(&before, &after);

	if changed.is_empty() {
		println!(
			"Nothing changed between the two surveys.\n\n\
             Either the car was in the same state both times, or the two files are the same \n\
             run. The point of the comparison is to catch what moves between conditions — \n\
             parked and driving, cold and warm, lights off and on."
		);
		return Ok(());
	}

	println!(
		"{} {} changed between the two surveys:\n",
		changed.len(),
		crate::render::plural(changed.len(), "identifier")
	);
	let mut unit = None;
	for (request, did, before_data, after_data) in &changed {
		if unit != Some(*request) {
			let label = UnitAddress::from_request(*request)
				.map(|a| a.label())
				.unwrap_or_else(|| format!("{request:03X}"));
			println!("  {label}  {request:03X}");
			unit = Some(*request);
		}
		println!("    {did:04X}  {before_data}  ->  {after_data}");
	}
	println!(
		"\nThese are the live values. To watch them: \n  \
         vagcan watch --survey {after_path} --did \"{}\"",
		changed
			.iter()
			.take(4)
			.map(|(request, did, _, _)| format!("{request:03X}:{did:04X}"))
			.collect::<Vec<_>>()
			.join(" ")
	);
	Ok(())
}

/// Fold the units this run read into the survey the car already had.
///
/// The cache is a whole-car file and a run need not be a whole-car run:
/// the safe habit is to sweep one unit at a time where possible, so `--only 713`
/// is the *recommended* habit and must not cost the other fourteen units. A
/// unit this run visited is replaced by what it just said; every other unit is
/// left exactly as it was.
///
/// Keyed by request id, which is also how [`crate::plan::with_survey`]
/// reads the file back — so a line that names no unit is dropped rather than
/// carried forward: nothing can replace it and nothing can watch it.
pub fn merge_survey(cached: &str, fresh: &[String]) -> String {
	let request_of = |line: &str| -> Option<u16> {
		let value = serde_json::from_str::<serde_json::Value>(line).ok()?;
		u16::from_str_radix(value["request"].as_str()?, 16).ok()
	};
	let mut units: std::collections::BTreeMap<u16, String> = std::collections::BTreeMap::new();
	for line in cached.lines().filter(|l| !l.trim().is_empty()) {
		if let Some(request) = request_of(line) {
			units.insert(request, line.to_string());
		}
	}
	for line in fresh {
		if let Some(request) = request_of(line) {
			units.insert(request, line.clone());
		}
	}
	units.values().map(|line| format!("{line}\n")).collect()
}

/// Write the merged survey where `watch` looks for it, and say where that was.
///
/// Best effort, and deliberately at the *end* of the run: a sweep is eight
/// minutes long and interrupting one is a normal thing to do (the rule is to
/// stop the moment anything changes). Streaming into the cache would let a
/// Ctrl-C two units in replace a complete cache with two units of it, which is
/// worse than not writing at all. `--out` is still written line by line, so an
/// interrupted run keeps its evidence there.
fn cache_survey(vin: &str, fresh: &[String]) -> Result<std::path::PathBuf> {
	let path = crate::datadir::survey_cache(vin)?;
	if let Some(dir) = path.parent() {
		std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
	}
	let cached = std::fs::read_to_string(&path).unwrap_or_default();
	std::fs::write(&path, merge_survey(&cached, fresh)).with_context(|| format!("writing {}", path.display()))?;
	Ok(path)
}

/// What a survey run was asked to do.
///
/// Bundled rather than passed positionally: four of these are booleans, and
/// `while_driving` is the one that decides whether the most invasive operation
/// this tool has is allowed to happen at speed. Named fields cannot be swapped
/// by accident.
pub struct Options<'a> {
	/// Hex ranges to sweep **blind**, on the units `blind` names and no others.
	/// Meaningless without `blind`, and refused there rather than ignored.
	pub range: Option<&'a str>,
	/// Where to write the answers, if anywhere.
	pub out: Option<&'a str>,
	/// Pause between reads, in milliseconds.
	pub delay_ms: u64,
	/// Survey only these units, skipping the gateway read.
	pub only: Option<&'a str>,
	/// Units to sweep blind, named one by one.
	///
	/// **There is no value of this that means "the whole car".** A blind sweep
	/// is a fuzz test of a diagnostic server, and what would turn one unit's
	/// crash into a whole-car event is doing it to every unit by default. It is
	/// now something somebody types a unit number into.
	pub blind: Option<&'a str>,
	/// Ask each unit for an extended diagnostic session first.
	pub extended: bool,
	/// Sweep even though the car is moving.
	pub while_driving: bool,
}

/// Run the survey (see the module docs).
pub async fn run(device_path: &str, baud: u32, options: Options<'_>) -> Result<()> {
	let Options {
		range,
		out,
		delay_ms,
		only,
		blind,
		extended,
		while_driving,
	} = options;
	// Every argument is checked before the adapter is opened: it is a
	// single-user resource, and holding it open to fail on a typo blocks the
	// next attempt.
	let blind_ranges = crate::declared::blind_ranges(range, blind.is_some(), SURVEY_RANGES)?;
	// Which units may be swept blind — named one at a time, never "all".
	let blind_units: std::collections::BTreeSet<u16> = match blind {
		Some(spec) => vag_uds_client::address::parse_list(spec)
			.map_err(|e| anyhow::anyhow!("--blind: {e}"))?
			.iter()
			.map(|u| u.request)
			.collect(),
		None => Default::default(),
	};
	// An explicit list skips the gateway read, so one unit can be re-run
	// without the rest.
	let requested = match only {
		Some(spec) => Some(
			vag_uds_client::address::parse_list(spec)
				.map_err(|e| anyhow::anyhow!("--only: {e}"))?
				.iter()
				.map(|u| u.request)
				.collect::<Vec<_>>(),
		),
		None => None,
	};
	let mut sink = match out {
		Some(path) => {
			let file = std::fs::File::create(path).with_context(|| format!("creating {path:?}"))?;
			Some(std::io::BufWriter::new(file))
		}
		None => None,
	};

	let mut backend = SlcanBackend::open_mode(device_path, baud, SlcanBitrate::Rate500k, SlcanMode::Normal)
		.await
		.with_context(|| crate::device::open_failure(device_path))?;

	if extended {
		// An extended session is workshop mode; see `crate::safety`.
		backend = match crate::safety::require_stationary(backend).await {
			Ok(backend) => backend,
			Err((_, why)) => anyhow::bail!("--extended refused: {why}"),
		};
	}

	// A full identifier sweep is a fuzz of the unit's diagnostic server, and a
	// unit whose firmware mishandles one request can crash on it — including a
	// unit the driver is relying on. Reading a moving car is therefore opt-in,
	// with the reason stated rather than buried in a flag.
	if !while_driving {
		backend = match crate::safety::require_stationary(backend).await {
			Ok(backend) => backend,
			Err((_, why)) => anyhow::bail!(
				"{why}\n\n\
                 A sweep asks a unit thousands of requests it may never have been asked \n\
                 before, and a unit that mishandles one can stop doing its job while the \n\
                 car is in motion. Sweep while parked, or pass --while-driving if you \n\
                 accept that risk with the car moving."
			),
		};
	}

	// Which car this is, so the result can be filed under it. Read after the
	// guards and before the sweep: it is one ordinary identifier read, and a
	// car that will not answer it simply gets no cache.
	let (back, vin) = crate::units::read_vin(backend).await;
	backend = back;

	let order = match requested {
		Some(ids) => ids,
		None => {
			let address = UnitAddress::from_request(0x710).expect("the gateway is in VW's block");
			let mut uds = AsyncUdsClient::new(IsoTpCan::new(
				backend,
				CanId::Standard(address.request),
				CanId::Standard(address.response),
			));
			let listed = match uds.read_data_by_identifier(gateway::INSTALLATION_LIST).await {
				Ok(bitmap) => gateway::decode_installation_list(&bitmap),
				Err(e) => {
					// Without the list there is still a car to read; say what
					// was lost rather than stopping.
					println!(
						"the gateway did not give its installation list ({e}) — \
                              surveying the units this project already knows"
					);
					Vec::new()
				}
			};
			backend = uds.into_transport().into_backend();
			walk_order(&listed)
		}
	};
	let (store, extracted) = crate::declared::sources();
	println!(
		"surveying {} control units — each asked only the identifiers its own data \n\
         declares it answers{}\n",
		order.len(),
		match blind_units.is_empty() {
			true => String::new(),
			false => format!(
				", except {} swept blind ({} identifiers)",
				blind_units.iter().map(|r| format!("{r:03X}")).collect::<Vec<_>>().join(", "),
				scan::total_dids(blind_ranges.as_deref().unwrap_or_default())
			),
		}
	);

	let started = Instant::now();
	let mut reports = Vec::new();
	// One JSON line per unit this run read, for the car's own cache.
	let mut fresh: Vec<String> = Vec::new();
	let total = order.len();
	let mut progress = crate::progress::Line::new();
	for (at, request) in order.into_iter().enumerate() {
		progress.update(&format!(
			"sweeping {request:03X} — unit {} of {total}, {:.0}s so far",
			at + 1,
			started.elapsed().as_secs_f64()
		));
		let Some(address) = UnitAddress::from_request(request) else {
			// `finish` first: this line would otherwise be written onto the
			// progress line, which rewrites itself.
			progress.finish();
			println!("  {request:03X} is in neither diagnostic block — skipped");
			continue;
		};
		let mut uds = AsyncUdsClient::new(IsoTpCan::new(
			backend,
			CanId::Standard(address.request),
			CanId::Standard(address.response),
		));

		// No session change by default. `0x10 0x03` is workshop mode, and a
		// unit that assists the driver is entitled to stop assisting while it
		// is in one.
		if extended {
			let _ = uds.start_session(0x03).await;
		}

		let mut report = UnitReport {
			request,
			..Default::default()
		};
		// The identification block is the sweep's baseline, not part of it: what
		// answers here is what the guard may later re-read to ask "are you still
		// there". Nothing here is judged — units on this car answer `F187` and
		// refuse half the rest, and halting on that would stop a run over a unit
		// behaving exactly as it always has. See `crate::anomaly`.
		let mut monitor = crate::anomaly::Monitor::new(request);
		for (did, _) in IDENT {
			match uds.read_data_by_identifier(*did).await {
				Ok(data) => {
					report.answered = true;
					monitor.seed(*did);
					report.ident.push((*did, data));
				}
				// A refusal proves the unit is there and listening, which is
				// exactly what the sweep needs to know.
				Err(UdsError::NegativeResponse { .. }) => report.answered = true,
				Err(_) => {}
			}
		}

		// Stored codes, before the sweep: they are the cheapest description of
		// a unit nobody has identified, and a sweep can be interrupted.
		if report.answered {
			if let Ok(dtcs) = uds.read_dtcs_by_status_mask(0xFF).await {
				report.dtcs = dtcs;
			}
		}

		// A unit that answered nothing at all is not on the bus. Sweeping it
		// anyway costs one timeout per identifier — minutes of waiting to
		// rediscover the silence already established.
		if !report.answered {
			progress.finish();
			println!("{}", report.summary());
			backend = uds.into_transport().into_backend();
			reports.push(report);
			continue;
		}

		// What this unit's own data says it answers. Resolved through what the
		// unit just reported about itself — never a table keyed on this car.
		let declared = crate::declared::declared(
			&store,
			&extracted,
			report.part_number().as_deref(),
			report.odx_name().as_deref(),
			report.odx_version().as_deref(),
		);
		let aimed = blind_units.contains(&request).then(|| blind_ranges.as_deref().unwrap_or_default());
		let ask = crate::declared::ask(&declared, aimed);

		// Nothing declares anything for this unit and nobody aimed a blind sweep
		// at it. It has been identified and its faults read; it is not fuzzed.
		if ask.is_empty() {
			progress.finish();
			println!("{}", report.summary());
			println!(
				"{}",
				crate::declared::no_source_notice(&address.label(), &format!("vagcan survey --only {0} --blind {0}", address.label()))
			);
			backend = uds.into_transport().into_backend();
			reports.push(report);
			continue;
		}

		// Group testing needs one identifier known to answer on *this* unit;
		// the ident block just supplied one, or the unit gets the slow path.
		let known_good = report.ident.first().map(|(d, _)| *d);
		let batched = match known_good {
			Some(did) => scan::probe_batching(&mut uds, did).await,
			None => false,
		};

		let mut hits = Vec::new();
		let on_hit = |hit: &DidHit| {
			hits.push(hit.clone());
			Ok(())
		};
		let delay = Duration::from_millis(delay_ms);
		// The witness the guard re-reads through the sweep, so a unit that falls
		// over is caught while it is still the most recent thing that happened.
		let mut guard = scan::Guard {
			witness: known_good,
			monitor: &mut monitor,
		};
		report.stats = if batched {
			scan::scan_dids_fast(&mut uds, &ask.ranges, delay, &mut guard, on_hit).await?
		} else {
			scan::scan_dids(&mut uds, &ask.ranges, delay, 400, &mut guard, on_hit).await?
		};
		report.hits = hits;

		progress.finish();
		println!("{}", report.summary());

		// Built whether or not anyone asked for a file: this is also what goes
		// into the car's own cache, and a run that has to be repeated with
		// `--out` to be kept is a run nobody keeps.
		let line = unit_line(&report, &address, batched, &ask);
		if let Some(w) = sink.as_mut() {
			// JSON lines: a survey interrupted halfway keeps every unit it
			// finished — and, below, the unit it stopped on, which is the one
			// worth having.
			writeln!(w, "{line}")?;
			w.flush()?;
		}

		// The rule: "Stop when something changes… finish nothing and start
		// nothing." The whole run, not this unit: a unit that has started
		// misbehaving is not made safer by sweeping the next one, and "it
		// recovered" is a guess until somebody looks. The notice goes on a
		// surface nothing rewrites.
		//
		// Written to `--out` above and deliberately **not** to `fresh`: this
		// unit's hit list stops where the sweep did, and folding a partial line
		// into the car's whole-car cache would destroy the "before" half of the
		// comparison step 3 of "If a unit stops behaving" asks for.
		if let Some(anomaly) = monitor.halted() {
			progress.notice(&anomaly.report());
			match out {
				Some(path) => println!("what this run read, up to and including the unit it stopped on, is in {path}."),
				None => println!(
					"Nothing was written: re-run with --out to keep the evidence.\n\
                     This car's cached survey has been left exactly as it was, so it is \n\
                     still the \"before\" for `vagcan survey --diff`."
				),
			}
			anyhow::bail!("the survey was stopped: control unit {} changed while it was being read", anomaly.unit());
		}
		fresh.push(line);
		reports.push(report);
		backend = uds.into_transport().into_backend();
	}

	let answered = reports.iter().filter(|r| r.answered).count();
	println!(
		"\n{answered} of {} control units answered, {} identifiers in total, in {:.0}s",
		reports.len(),
		reports.iter().map(|r| r.hits.len()).sum::<usize>(),
		started.elapsed().as_secs_f64()
	);
	if let Some(path) = out {
		println!("written to {path}");
	}
	// The car keeps its own copy whether or not `--out` was given, because the
	// thing that made these units invisible was never the sweep — it was having
	// to remember the file name of one afterwards. With this on disk, `watch`
	// offers every identifier this car answers, on every unit, with no flag.
	match (&vin, fresh.is_empty()) {
		// Nothing answered, so there is nothing to file and nothing to say
		// about a cache that would be unchanged either way.
		(_, true) => {}
		(Some(vin), false) => match cache_survey(vin, &fresh) {
			Ok(path) => println!(
				"`vagcan watch` now offers every identifier above, on every unit, as \n\
                 raw bytes — no flag needed. Filed under this car in\n  {}",
				path.display()
			),
			// Not fatal: the sweep is the expensive part and it is done. Say
			// what was lost rather than throwing the run away over a file.
			Err(e) => println!("could not cache the survey for this car: {e:#}"),
		},
		(None, false) => println!(
			"the engine did not report a VIN, so this survey could not be filed under \n\
             the car. Pass --out FILE to keep it, and `watch --survey FILE` to use it."
		),
	}
	println!(
		"\nRun this once parked and once driving, then compare:\n  \
         vagcan survey --out parked.jsonl\n  \
         vagcan survey --out driving.jsonl\n  \
         vagcan survey --diff parked.jsonl driving.jsonl\n\
         The identifiers whose bytes differ are the live measurements, and that list \n\
         needs no label file."
	);
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	/// One survey line, as the writer below emits it.
	fn line(request: &str, did: &str) -> String {
		format!("{{\"request\":\"{request}\",\"dids\":[{{\"did\":\"{did}\",\"data\":\"0B34\"}}]}}")
	}

	#[test]
	fn a_written_line_reads_back_as_the_sweep_it_was() {
		// The round trip that `watch`'s filter rests on. It calls a declared
		// channel missing from *this car* when the survey asked for it and got
		// nothing, and missing from *this run* when the sweep never covered it —
		// so a writer and a reader that drift apart turn one into the other.
		let report = UnitReport {
			request: 0x713,
			ident: vec![(0xF187, b"5Q0614517AQ".to_vec())],
			hits: vec![DidHit {
				did: 0x1001,
				data: vec![0x0B, 0x34],
			}],
			stats: scan::ScanStats::default(),
			dtcs: Vec::new(),
			answered: true,
		};
		let declared: std::collections::BTreeSet<u16> = [0x1001, 0x1002, 0x1003].into_iter().collect();
		let ask = crate::declared::ask(&declared, None);
		let address = vag_uds_client::address::UnitAddress::from_request(0x713).expect("713 is a VW unit");

		let text = unit_line(&report, &address, true, &ask);
		let seen = crate::plan::answered_from_survey(&text);

		assert_eq!(seen.saw(0x713, 0x1001), Some(true), "it answered");
		assert_eq!(seen.saw(0x713, 0x1002), Some(false), "asked, and it did not");
		assert_eq!(seen.saw(0x713, 0x2029), None, "outside what this sweep asked, so not this car's answer");
		assert_eq!(seen.saw(0x7E0, 0x1001), None, "another unit entirely");
	}

	#[test]
	fn re_surveying_one_unit_keeps_the_rest_of_the_car_in_the_cache() {
		// `--only 713` costs seconds and is the recommended way to re-read a
		// unit. If that overwrote the cache, the safe habit would
		// silently cost `watch` the other fourteen units.
		let cached = format!("{}\n{}\n", line("7E0", "2029"), line("713", "1001"));
		let fresh = vec![line("713", "1002")];
		let merged = merge_survey(&cached, &fresh);
		assert!(merged.contains("7E0"), "{merged}");
		assert!(merged.contains("1002"), "the re-read unit is the new one: {merged}");
		assert!(!merged.contains("1001"), "and not the old one: {merged}");
		assert_eq!(merged.lines().count(), 2);
	}

	#[test]
	fn a_car_with_no_cache_yet_gets_exactly_what_this_run_read() {
		let fresh = vec![line("7E0", "2029")];
		assert_eq!(merge_survey("", &fresh), format!("{}\n", fresh[0]));
		// And a run that read nothing leaves what was there — an interrupted
		// sweep must not be able to empty a good cache.
		let cached = format!("{}\n", line("7E0", "2029"));
		assert_eq!(merge_survey(&cached, &[]), cached);
	}

	#[test]
	fn a_line_that_names_no_unit_is_dropped_rather_than_kept_unattributable() {
		// `watch` reads the cache by request id; a line without one can neither
		// be replaced by a later run nor watched, so keeping it would only make
		// the file grow.
		let merged = merge_survey("not json\n{\"dids\":[]}\n", &[line("7E0", "2029")]);
		assert_eq!(merged.lines().count(), 1, "{merged}");
	}

	#[test]
	fn a_diff_reports_only_what_actually_moved() {
		let parked = "{\"request\":\"7E0\",\"dids\":[{\"did\":\"2029\",\"data\":\"0B34\"},                      {\"did\":\"206E\",\"data\":\"02BD\"}]}";
		let driving = "{\"request\":\"7E0\",\"dids\":[{\"did\":\"2029\",\"data\":\"0B34\"},                       {\"did\":\"206E\",\"data\":\"0CC8\"}]}";
		let changed = diff(parked, driving);
		assert_eq!(changed.len(), 1, "{changed:?}");
		assert_eq!(changed[0].0, 0x7E0);
		assert_eq!(changed[0].1, 0x206E);
		assert_eq!((changed[0].2.as_str(), changed[0].3.as_str()), ("02BD", "0CC8"));
	}

	#[test]
	fn a_survey_file_writes_its_bytes_with_no_separator_between_them() {
		// The file format, not a preference. `--diff` compares these strings
		// as text, and every dump already on disk was written packed — so a
		// separator arriving here would report every identifier in an old pair
		// of files as having moved. The writer and the reader are asserted
		// together for that reason.
		assert_eq!(hex_packed(&[0x0B, 0x34]), "0B34");
		let line = |data: &[u8]| {
			format!(
				"{{\"request\":\"7E0\",\"dids\":[{{\"did\":\"2029\",\"data\":\"{}\"}}]}}",
				hex_packed(data)
			)
		};
		let changed = diff(&line(&[0x0B, 0x34]), &line(&[0x0C, 0x40]));
		assert_eq!(changed.len(), 1, "{changed:?}");
		assert_eq!((changed[0].2.as_str(), changed[0].3.as_str()), ("0B34", "0C40"));
	}

	#[test]
	fn an_identifier_missing_from_one_run_is_not_called_a_change() {
		// A unit that was asleep during one pass has not "changed"; reporting
		// it would drown the real movement in noise.
		let a = "{\"request\":\"7E0\",\"dids\":[{\"did\":\"2029\",\"data\":\"0B34\"}]}";
		let b = "{\"request\":\"7E0\",\"dids\":[{\"did\":\"202A\",\"data\":\"0B34\"}]}";
		assert!(diff(a, b).is_empty());
		// And a unit absent from the second file entirely is skipped, not
		// reported as every identifier changing.
		assert!(diff(a, "").is_empty());
	}

	#[test]
	fn the_walk_covers_the_units_the_gateway_cannot_list() {
		// The installation list is VW's block only: it has no bit for the
		// engine or the gearbox, and the gateway omits itself. A survey driven
		// by the list alone would miss the three most-read units on the car.
		let listed = vec![0x70C, 0x70E, 0x714];
		let order = walk_order(&listed);
		for must in [0x7E0, 0x7E1, 0x710] {
			assert!(order.contains(&must), "{must:03X} missing from {order:03X?}");
		}
		for id in listed {
			assert!(order.contains(&id));
		}
	}

	#[test]
	fn a_unit_listed_twice_is_walked_once() {
		// 0x710 is in ALWAYS; a gateway that also listed itself must not make
		// the survey read it twice.
		let order = walk_order(&[0x710, 0x714, 0x714]);
		assert_eq!(order.iter().filter(|id| **id == 0x710).count(), 1);
		assert_eq!(order.iter().filter(|id| **id == 0x714).count(), 1);
	}

	#[test]
	fn the_default_pages_parse_and_stay_cheap() {
		let ranges = scan::parse_ranges(SURVEY_RANGES).unwrap();
		// A full sweep is 65,536 identifiers per unit; the point of naming
		// pages is that a whole-car pass stays in minutes, not hours.
		assert!(scan::total_dids(&ranges) < 3_000, "{}", scan::total_dids(&ranges));
	}

	#[test]
	fn a_report_names_the_unit_from_what_it_said_about_itself() {
		let report = UnitReport {
			request: 0x714,
			answered: true,
			ident: vec![(0xF187, b"5E0920740D ".to_vec()), (0xF197, b"KOMBI        ".to_vec())],
			..Default::default()
		};
		assert_eq!(report.part_number().as_deref(), Some("5E0920740D"));
		assert_eq!(report.component().as_deref(), Some("KOMBI"));
		assert!(report.summary().contains("KOMBI"));
	}

	#[test]
	fn a_silent_unit_is_reported_as_silent_rather_than_omitted() {
		// "did not answer" is a result; a blank line is not.
		let report = UnitReport {
			request: 0x773,
			..Default::default()
		};
		assert!(report.summary().contains("did not answer"));
		assert!(report.component().is_none());
	}

	#[test]
	fn a_unit_that_only_refuses_still_counts_as_present() {
		// Four units on this car answer session control and refuse every
		// identifier. A refusal is the unit talking; treating it as absence
		// would drop them from the survey entirely.
		let report = UnitReport {
			request: 0x773,
			answered: true,
			..Default::default()
		};
		assert!(!report.summary().contains("did not answer"));
	}

	#[test]
	fn only_confirmed_codes_are_counted_as_faults() {
		// Measured on the reference car: the body control module lists 508
		// codes, 505 of them status 0x10 — "never tested since the memory was
		// cleared", which is not a fault. Counting those would report a car
		// with hundreds of faults that has two.
		let report = UnitReport {
			request: 0x70E,
			answered: true,
			dtcs: vec![
				RawDtc {
					code: [0x00, 0x01, 0x07],
					status: 0x10,
				},
				RawDtc {
					code: [0x00, 0x02, 0x07],
					status: 0x10,
				},
				RawDtc {
					code: [0x01, 0x04, 0x05],
					status: 0x08,
				},
			],
			..Default::default()
		};
		assert_eq!(report.confirmed(), 1);
		assert!(report.summary().contains("1 stored faults"), "{}", report.summary());
	}

	#[test]
	fn a_unit_with_nothing_confirmed_says_nothing_about_faults() {
		let report = UnitReport {
			request: 0x713,
			answered: true,
			dtcs: vec![RawDtc {
				code: [0x00, 0x00, 0x4B],
				status: 0x10,
			}],
			..Default::default()
		};
		assert!(!report.summary().contains("fault"), "{}", report.summary());
	}
}
