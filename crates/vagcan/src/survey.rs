//! `vagcan survey` — walk the whole car, not just the powertrain.
//!
//! Everything this project can read live has so far come from two control
//! units, because those are the two the ISO addressing block reaches. The
//! gateway's installation list names fifteen more
//! (`research/other-ecus.md` §3), each answering on VW's own block, and each
//! with an identifier space nobody here has swept.
//!
//! This command does the pass that document calls for: read the installation
//! list, then for every unit in it read the identification block and sweep the
//! identifier pages that are actually in use on this car. The result is a file
//! of *what answered*, per unit — the raw material for a measurement catalog,
//! obtained without the label corpus.
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
use vag_can::{IsoTpCan, SlcanBackend, SlcanBitrate, SlcanMode};
use vag_protocol::address::UnitAddress;
use vag_protocol::uds::UdsError;
use vag_protocol::{gateway, AsyncUdsClient, RawDtc};
use vag_transport::CanId;

use crate::scan::{self, DidHit};

/// The identifier pages observed in use on this car, across every unit seen in
/// the two captures (`research/other-ecus.md` §6.3): identification and coding
/// (`02xx`, `06xx`, `F1xx`), the BCM's group records (`19xx`), the powertrain
/// measurement bands (`20xx`–`22xx`, `38xx`), the gateway's lists (`2Axx`,
/// `2Bxx`) and the OBD-II mirror (`F4xx`).
///
/// Nine pages rather than the whole 65,536: the rest was refused everywhere it
/// was ever asked, and a full sweep costs minutes per unit for that answer.
pub const SURVEY_RANGES: &str =
    "0200-02FF,0600-06FF,1900-19FF,2000-22FF,2A00-2BFF,3800-38FF,F100-F1FF,F400-F4FF";

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
        let mut units: std::collections::BTreeMap<u16, std::collections::BTreeMap<u16, String>> =
            std::collections::BTreeMap::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            let Some(request) =
                value["request"].as_str().and_then(|s| u16::from_str_radix(s, 16).ok())
            else {
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
pub fn run_diff(before_path: &str, after_path: &str) -> Result<()> {
    let before = std::fs::read_to_string(before_path)
        .with_context(|| format!("reading {before_path:?}"))?;
    let after =
        std::fs::read_to_string(after_path).with_context(|| format!("reading {after_path:?}"))?;
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

    println!("{} identifiers changed between the two surveys:\n", changed.len());
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

/// Run the survey (see the module docs).
pub async fn run(
    device_path: &str,
    baud: u32,
    range: &str,
    out: Option<&str>,
    delay_ms: u64,
    only: Option<&str>,
) -> Result<()> {
    let ranges = scan::parse_ranges(range).map_err(|e| anyhow::anyhow!("--range: {e}"))?;
    let mut sink = match out {
        Some(path) => {
            let file =
                std::fs::File::create(path).with_context(|| format!("creating {path:?}"))?;
            Some(std::io::BufWriter::new(file))
        }
        None => None,
    };

    let mut backend =
        SlcanBackend::open_mode(device_path, baud, SlcanBitrate::Rate500k, SlcanMode::Normal)
            .await
            .with_context(|| crate::device::open_failure(device_path))?;

    let order = match only {
        // An explicit list skips the gateway read, so one unit can be re-run
        // without the rest.
        Some(spec) => {
            let mut ids = Vec::new();
            for token in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                ids.push(
                    vag_protocol::address::parse(token)
                        .map_err(|e| anyhow::anyhow!("--only: {e}"))?
                        .request,
                );
            }
            ids
        }
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
                    println!("the gateway did not give its installation list ({e}) — \
                              surveying the units this project already knows");
                    Vec::new()
                }
            };
            backend = uds.into_transport().into_backend();
            walk_order(&listed)
        }
    };

    println!(
        "surveying {} control units — {} identifiers each ({range})\n",
        order.len(),
        scan::total_dids(&ranges)
    );

    let started = Instant::now();
    let mut reports = Vec::new();
    for request in order {
        let Some(address) = UnitAddress::from_request(request) else {
            println!("  {request:03X} is in neither diagnostic block — skipped");
            continue;
        };
        let mut uds = AsyncUdsClient::new(IsoTpCan::new(
            backend,
            CanId::Standard(address.request),
            CanId::Standard(address.response),
        ));

        // VCDS opens an extended session before reading these units, and the
        // four unidentified ones answered nothing else. A refusal here is not
        // fatal — the engine answers `0x22` in the default session.
        let _ = uds.start_session(0x03).await;

        let mut report = UnitReport { request, ..Default::default() };
        for (did, _) in IDENT {
            match uds.read_data_by_identifier(*did).await {
                Ok(data) => {
                    report.answered = true;
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
            println!("{}", report.summary());
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
        report.stats = if batched {
            scan::scan_dids_fast(&mut uds, &ranges, delay, on_hit).await?
        } else {
            scan::scan_dids(&mut uds, &ranges, delay, 400, on_hit).await?
        };
        report.hits = hits;

        println!("{}", report.summary());
        if let Some(w) = sink.as_mut() {
            // JSON lines: a survey interrupted halfway keeps every unit it
            // finished.
            let line = serde_json::json!({
                "request": format!("{request:03X}"),
                "unit": address.label(),
                "batched": batched,
                "ident": report.ident.iter().map(|(did, data)| {
                    serde_json::json!({ "did": format!("{did:04X}"), "data": hex(data) })
                }).collect::<Vec<_>>(),
                "dids": report.hits.iter().map(|h| {
                    serde_json::json!({ "did": format!("{:04X}", h.did), "data": hex(&h.data) })
                }).collect::<Vec<_>>(),
                "confirmed_faults": report.confirmed(),
                "dtcs": report.dtcs.iter().map(|d| {
                    serde_json::json!({
                        "code": hex(&d.code),
                        "status": format!("{:02X}", d.status),
                    })
                }).collect::<Vec<_>>(),
            });
            writeln!(w, "{line}")?;
            w.flush()?;
        }
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
        println!(
            "written to {path}\n\n\
             Run this once parked and once driving, then compare:\n  \
             vagcan survey --diff parked.jsonl driving.jsonl\n\
             The identifiers whose bytes differ are the live measurements, and that list \n\
             needs no label file."
        );
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
            ident: vec![
                (0xF187, b"5E0920740D ".to_vec()),
                (0xF197, b"KOMBI        ".to_vec()),
            ],
            ..Default::default()
        };
        assert_eq!(report.part_number().as_deref(), Some("5E0920740D"));
        assert_eq!(report.component().as_deref(), Some("KOMBI"));
        assert!(report.summary().contains("KOMBI"));
    }

    #[test]
    fn a_silent_unit_is_reported_as_silent_rather_than_omitted() {
        // "did not answer" is a result; a blank line is not.
        let report = UnitReport { request: 0x773, ..Default::default() };
        assert!(report.summary().contains("did not answer"));
        assert!(report.component().is_none());
    }

    #[test]
    fn a_unit_that_only_refuses_still_counts_as_present() {
        // Four units on this car answer session control and refuse every
        // identifier. A refusal is the unit talking; treating it as absence
        // would drop them from the survey entirely.
        let report = UnitReport { request: 0x773, answered: true, ..Default::default() };
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
                RawDtc { code: [0x00, 0x01, 0x07], status: 0x10 },
                RawDtc { code: [0x00, 0x02, 0x07], status: 0x10 },
                RawDtc { code: [0x01, 0x04, 0x05], status: 0x08 },
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
            dtcs: vec![RawDtc { code: [0x00, 0x00, 0x4B], status: 0x10 }],
            ..Default::default()
        };
        assert!(!report.summary().contains("fault"), "{}", report.summary());
    }
}
