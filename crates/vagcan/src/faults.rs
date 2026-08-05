//! `vagcan faults` — what the car has stored against itself.
//!
//! `survey` reports fault *counts* as a by-product of walking the car; this
//! command is the fault reader proper: every unit, every confirmed code, and
//! on request the extended data the unit keeps beside it — which is where the
//! occurrence counter and the mileage stamp live on these control units.
//!
//! Two honesty rules run through it:
//!
//! * **Only confirmed codes are called faults.** Asking with status mask
//!   `0xFF` returns everything the unit knows about, including tests that have
//!   simply never run since the memory was last cleared. On the reference car
//!   the body control module answers 508 codes that way, of which three are
//!   actual stored faults.
//! * **A code is printed as a code until something names it.** The texts live
//!   in the label corpus; where this project cannot resolve one, it shows the
//!   raw bytes rather than a plausible-sounding invention.
//!
//! Read-only: the service issued is `0x19`, which reads. Clearing faults is
//! `0x14`, which the client's allowlist rejects.

use anyhow::{Context, Result};
use vag_can::{IsoTpCan, SlcanBackend, SlcanBitrate, SlcanMode};
use vag_protocol::address::UnitAddress;
use vag_protocol::dtc::{CarTime, FaultContext, UnitStamp};
use vag_protocol::{gateway, AsyncUdsClient, RawDtc};
use vag_transport::CanId;

/// Status bit 3 — the unit confirmed this failure, as opposed to merely
/// listing the code.
pub const CONFIRMED: u8 = 0x08;

/// Status bit 0 — the test is failing at this moment, not historically.
pub const FAILED_NOW: u8 = 0x01;

/// What one unit reported.
#[derive(Debug, Clone, Default)]
pub struct UnitFaults {
    /// The unit's own component string, when it gave one.
    pub component: Option<String>,
    /// Every code the unit listed, confirmed or not.
    pub all: Vec<RawDtc>,
}

impl UnitFaults {
    pub fn confirmed(&self) -> Vec<&RawDtc> {
        self.all.iter().filter(|d| d.status & CONFIRMED != 0).collect()
    }
}

/// How a code is written: the three bytes, and the decimal fault number VW's
/// own tools print.
///
/// **All three bytes are one number**, big-endian. An earlier version of this
/// function split them into a two-byte number and a symptom byte, which was
/// wrong: `00 01 29` is fault 297, not fault 1 symptom 0x29. The refutation is
/// this car's own VCDS scan, which prints `0297` beside the brake unit's code
/// and `291104` beside the steering column's `04 71 20`
/// (`research/VCDS-RUS/Scans/`), matching the 24-bit reading in all four cases
/// checked and the 16-bit one in none.
pub fn format_code(code: [u8; 3]) -> String {
    let number = u32::from_be_bytes([0, code[0], code[1], code[2]]);
    format!("{:02X}{:02X}{:02X}  ({number})", code[0], code[1], code[2])
}

/// How long ago a fault happened, told against the unit's own counters.
///
/// Both halves are differences taken on the same control unit, so neither
/// depends on the car's clock being set right: the stamp is a packed date
/// (`vag_protocol::dtc::CarTime`) and the difference between two of them is
/// real elapsed time even when the date itself is days out, as it is here.
/// The difference is taken through `seconds_between`, which unpacks first —
/// subtracting the raw stamps overshoots by 4 counts per minute boundary.
pub fn describe_age(context: &FaultContext, now: Option<&UnitStamp>) -> String {
    let mut parts = vec![format!("{} km", context.mileage_km)];
    match context.occurrences {
        0xFF => parts.push("255+ times".to_string()),
        n => parts.push(format!("{n}×")),
    }
    if let Some(now) = now {
        if let Some(seconds) = vag_protocol::dtc::seconds_between(context.clock, now.clock) {
            let seconds = seconds as f64;
            parts.push(match seconds {
                s if s < 90.0 => format!("{s:.0} s ago"),
                s if s < 5_400.0 => format!("{:.0} min ago", s / 60.0),
                s if s < 172_800.0 => format!("{:.1} h ago", s / 3_600.0),
                s => format!("{:.1} days ago", s / 86_400.0),
            });
        }
        if now.mileage_km >= context.mileage_km {
            parts.push(format!("{} km ago", now.mileage_km - context.mileage_km));
        }
    }
    parts.join(", ")
}

/// When a fault was stored, by the car's own clock.
///
/// Exact, to the second: the stamp is a packed date and time, and two VCDS
/// printouts of this car reproduce field for field (`vag_protocol::dtc::CarTime`).
/// It is the *car's* clock, which on this car runs some days behind real time,
/// so the wording says whose clock it is.
pub fn describe_when(context: &FaultContext) -> String {
    match CarTime::parse(context.clock) {
        Some(t) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} by the car's own clock",
            t.year, t.month, t.day, t.hour, t.minute, t.second
        ),
        // An unset or corrupt stamp is not a moment.
        None => format!("clock {:08X}, not a date", context.clock),
    }
}

/// Decode the status byte into the states it actually asserts.
///
/// Straight out of ISO 14229-1 table D.1 — every bit is defined there, so
/// nothing here is inferred from this car.
pub fn describe_status(status: u8) -> String {
    const BITS: [(u8, &str); 8] = [
        (0x01, "failed now"),
        (0x02, "failed this cycle"),
        (0x04, "pending"),
        (0x08, "confirmed"),
        (0x10, "not tested since clear"),
        (0x20, "failed since clear"),
        (0x40, "not tested this cycle"),
        (0x80, "warning lamp"),
    ];
    let set: Vec<&str> =
        BITS.iter().filter(|(bit, _)| status & bit != 0).map(|(_, name)| *name).collect();
    if set.is_empty() { format!("{status:02X}") } else { set.join(", ") }
}

/// The identifiers by which a unit names its own description file: the ODX
/// name, and the coding index whose first three digits pick the variant
/// (`research/labels/fault-naming-hop.md` §10.4). Both come off the car.
const ODX_NAME: u16 = 0xF19E;
const ODX_VERSION: u16 = 0xF1A2;

/// Read one identifier value as the padded ASCII these units store.
fn ident_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim_end_matches(['\0', ' ']).to_string()
}

/// Name the faults in a survey this tool recorded (`vagcan faults --from`).
///
/// The naming chain needs nothing from the car that a survey does not already
/// hold — the fault codes, and the two identifiers that pick each unit's
/// description file — so it runs offline against a recorded file. That is what
/// makes the whole chain testable without the adapter, and it is how the
/// figures in `research/labels/fault-naming-hop.md` §11.3 are reproduced.
pub fn run_named(
    survey_path: &str,
    labels_dir: &str,
    iv_cache: &str,
    all_codes: bool,
) -> Result<()> {
    let text = std::fs::read_to_string(survey_path)
        .with_context(|| format!("reading {survey_path:?}"))?;
    let root = crate::datadir::resolve(labels_dir);
    let mut namer = crate::faultnames::Namer::open(&root, &crate::datadir::resolve(iv_cache))?;
    println!(
        "{} rows of fault registry, {} texts, from {}\n",
        namer.registry_rows(),
        namer.codes_texts(),
        root.display()
    );

    let (mut named, mut unnamed) = (0usize, 0usize);
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let Some(codes) = value["dtcs"].as_array() else { continue };
        let codes: Vec<(RawDtc, u8)> = codes
            .iter()
            .filter_map(|d| {
                let code = parse_code(d["code"].as_str()?)?;
                let status = u8::from_str_radix(d["status"].as_str()?, 16).ok()?;
                Some((RawDtc { code, status }, status))
            })
            .filter(|(dtc, _)| all_codes || dtc.status & CONFIRMED != 0)
            .collect();
        if codes.is_empty() {
            continue;
        }
        let unit = value["unit"].as_str().unwrap_or("--").to_string();
        let ident = |did: u16| -> String {
            value["ident"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|f| f["did"].as_str() == Some(&format!("{did:04X}")))
                .and_then(|f| f["data"].as_str())
                .and_then(hex_bytes)
                .map(|b| ident_text(&b))
                .unwrap_or_default()
        };
        let (odx, version) = (ident(ODX_NAME), ident(ODX_VERSION));
        println!("{unit}  {odx}");
        let lookup = namer.unit(&odx, &version);
        if let Some(note) = crate::faultnames::unit_note(&lookup) {
            println!("  ({note})");
        }
        let catalogue = match &lookup {
            vag_data::UnitLookup::Found { catalogue, .. } => Some(catalogue),
            _ => None,
        };
        for (dtc, _) in &codes {
            let naming = catalogue.map(|c| namer.name(c, dtc.code));
            let named_here =
                matches!(naming, Some(crate::faultnames::Naming::Named { .. }));
            if named_here {
                named += 1;
            } else {
                unnamed += 1;
            }
            println!("  {}   {}", format_code(dtc.code), describe_status(dtc.status));
            if let Some(line) = naming.as_ref().and_then(|n| n.line()) {
                println!("      {line}");
            }
        }
        println!();
    }
    println!(
        "{named} of {} {} named.",
        named + unnamed,
        crate::render::plural(named + unnamed, "code")
    );
    Ok(())
}

fn parse_code(text: &str) -> Option<[u8; 3]> {
    let bytes = hex_bytes(text)?;
    (bytes.len() == 3).then(|| [bytes[0], bytes[1], bytes[2]])
}

fn hex_bytes(text: &str) -> Option<Vec<u8>> {
    (text.len() % 2 == 0)
        .then(|| {
            (0..text.len() / 2)
                .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok())
                .collect::<Option<Vec<u8>>>()
        })
        .flatten()
}

/// Read faults from the car (see the module docs).
#[allow(clippy::too_many_arguments)]
pub async fn run(
    device_path: &str,
    baud: u32,
    only: Option<&str>,
    details: bool,
    all_codes: bool,
    supported: bool,
    extended: bool,
    labels_dir: Option<&str>,
    iv_cache: &str,
) -> Result<()> {
    // Arguments first: the adapter is a single-user resource, so a typo in
    // --ecu must not cost the port before it is reported.
    // The corpus is opened before the port for the same reason: a missing
    // `Codes.dat` is a mistake to report, not one to make after taking the
    // adapter and reading the whole car.
    let mut namer = match labels_dir {
        Some(dir) => Some(crate::faultnames::Namer::open(
            &crate::datadir::resolve(dir),
            &crate::datadir::resolve(iv_cache),
        )?),
        None => None,
    };
    let requested = match only {
        Some(spec) => Some(
            vag_protocol::address::parse_list(spec)
                .map_err(|e| anyhow::anyhow!("--ecu: {e}"))?
                .iter()
                .map(|u| u.request)
                .collect::<Vec<_>>(),
        ),
        None => None,
    };

    let mut backend =
        SlcanBackend::open_mode(device_path, baud, SlcanBitrate::Rate500k, SlcanMode::Normal)
            .await
            .with_context(|| crate::device::open_failure(device_path))?;

    if extended {
        // An extended session is workshop mode; see `crate::safety`.
        backend = match crate::safety::require_stationary(backend).await {
            Ok(backend) => backend,
            Err((_, why)) => anyhow::bail!("--extended refused: {why}"),
        };
    }

    let order = match requested {
        Some(ids) => ids,
        None => {
            let gw = UnitAddress::from_request(0x710).expect("the gateway is in VW's block");
            let mut uds = AsyncUdsClient::new(IsoTpCan::new(
                backend,
                CanId::Standard(gw.request),
                CanId::Standard(gw.response),
            ));
            let listed = match uds.read_data_by_identifier(gateway::INSTALLATION_LIST).await {
                Ok(bitmap) => gateway::decode_installation_list(&bitmap),
                Err(e) => {
                    println!("the gateway did not list the car's units ({e})");
                    Vec::new()
                }
            };
            backend = uds.into_transport().into_backend();
            // The engine, the gearbox and the gateway itself are never in the
            // list — the first two live on the other id block, and the
            // gateway does not list itself.
            let mut ids = vec![0x7E0, 0x7E1, 0x710];
            for id in listed {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            ids
        }
    };

    // The caveat belongs before the codes, not after them. A list of hex on a
    // screen headed "faults" is read as a verdict on the car; by the time a
    // disclaimer arrives at the bottom the reader has already had the fright.
    if !supported {
        println!(
            "Stored codes are a record that something happened once — not a diagnosis, and \n\
             not necessarily a fault present now. Only codes marked \"failed now\" are \n\
             currently failing.{}\n",
            match &namer {
                // Where a code cannot be named the reason is printed beside
                // it, so this line does not have to promise anything.
                Some(_) => "",
                None => "\nNothing below is named: pass --labels <VCDS install> to name them.",
            }
        );
    }

    let mut total = 0usize;
    let mut failing_now = 0usize;
    let count = order.len();
    let mut progress = crate::progress::Line::new();
    for (at, request) in order.into_iter().enumerate() {
        progress.update(&format!("reading faults — {request:03X}, unit {} of {count}", at + 1));
        let Some(address) = UnitAddress::from_request(request) else { continue };
        let mut uds = AsyncUdsClient::new(IsoTpCan::new(
            backend,
            CanId::Standard(address.request),
            CanId::Standard(address.response),
        ));
        // See `crate::safety`: an extended session is workshop mode, and this
        // command reads faults perfectly well without one.
        if extended {
            let _ = uds.start_session(0x03).await;
        }

        // The unit names itself; nothing here maps an address to a name.
        let component = uds
            .read_data_by_identifier(0xF197)
            .await
            .ok()
            .map(|b| String::from_utf8_lossy(&b).trim_end_matches(['\0', ' ']).to_string())
            .filter(|s| !s.is_empty());
        let unit = UnitFaults {
            component,
            all: uds.read_dtcs_by_status_mask(0xFF).await.unwrap_or_default(),
        };
        // The unit's own "now": the same odometer and counter its faults are
        // stamped with, so ages are differences rather than dates.
        let now = uds
            .read_data_by_identifier(UnitStamp::DID)
            .await
            .ok()
            .and_then(|data| UnitStamp::parse(&data));

        if supported {
            // The unit's whole catalogue of codes, in its own order — which is
            // how the label corpus stores fault names, so the two lists are
            // worth comparing.
            match uds.read_supported_dtcs().await {
                Ok(list) => {
                    println!(
                        "\n{}  {:03X}  {}  — {} {} supported",
                        address.label(),
                        request,
                        unit.component.clone().unwrap_or_default(),
                        list.len(),
                        crate::render::plural(list.len(), "code")
                    );
                    for dtc in &list {
                        println!("  {}", format_code(dtc.code));
                    }
                    total += list.len();
                }
                Err(e) => println!("\n{}  {:03X}  no supported list ({e})", address.label(), request),
            }
            backend = uds.into_transport().into_backend();
            continue;
        }

        let mut show: Vec<RawDtc> = if all_codes {
            unit.all.clone()
        } else {
            unit.confirmed().into_iter().cloned().collect()
        };
        // Something failing right now outranks a code stored months ago.
        show.sort_by_key(|d| (d.status & FAILED_NOW == 0, d.code));
        failing_now += show.iter().filter(|d| d.status & FAILED_NOW != 0).count();
        if !show.is_empty() {
            progress.finish();
            // A unit with no short number shows a dash rather than repeating
            // its id, which reads as a rendering fault.
            let number = match vag_protocol::address::short_number(request) {
                Some(n) => format!("{n:02X}"),
                None => "--".to_string(),
            };
            println!(
                "\n{number}  {request:03X}  {}",
                unit.component.clone().unwrap_or_default()
            );
            // The unit names its own description file, so the catalogue is
            // fetched only once something has to be named out of it.
            let lookup = match &mut namer {
                Some(namer) => {
                    let read = |data: Option<Vec<u8>>| data.map(|b| ident_text(&b)).unwrap_or_default();
                    let odx = read(uds.read_data_by_identifier(ODX_NAME).await.ok());
                    let version = read(uds.read_data_by_identifier(ODX_VERSION).await.ok());
                    Some(namer.unit(&odx, &version))
                }
                None => None,
            };
            if let Some(note) = lookup.as_ref().and_then(crate::faultnames::unit_note) {
                println!("  ({note})");
            }
            let catalogue = match &lookup {
                Some(vag_data::UnitLookup::Found { catalogue, .. }) => Some(catalogue),
                _ => None,
            };
            for dtc in &show {
                println!("  {}   {}", format_code(dtc.code), describe_status(dtc.status));
                // The name goes under the code, never instead of it: the
                // number is what the car said and the name is this project's
                // reading of it.
                if let (Some(namer), Some(catalogue)) = (&namer, catalogue) {
                    if let Some(line) = namer.name(catalogue, dtc.code).line() {
                        println!("      {line}");
                    }
                }
                // Extended data carries when it happened: the odometer at the
                // time and how often. Read for every fault, since that is the
                // question a stored code raises.
                let records = uds.read_dtc_extended(dtc.code).await.unwrap_or_default();
                for record in &records {
                    match FaultContext::parse(&record.data) {
                        Some(context) => {
                            println!("      {}", describe_age(&context, now.as_ref()));
                            println!("      {}", describe_when(&context));
                        }
                        // A record this project cannot read is shown, not
                        // dropped — and not guessed at either.
                        None if details => println!(
                            "      record {:02X}: {}",
                            record.record,
                            record.data.iter().map(|b| format!("{b:02X}")).collect::<String>()
                        ),
                        None => {}
                    }
                    if details {
                        println!(
                            "      raw {:02X}: {}",
                            record.record,
                            record.data.iter().map(|b| format!("{b:02X}")).collect::<String>()
                        );
                    }
                }
            }
            total += show.len();
        }
        backend = uds.into_transport().into_backend();
    }

    if supported {
        return Ok(());
    }
    if total == 0 {
        println!("No stored codes.");
        return Ok(());
    }
    println!(
        "\n{total} stored {}. {}",
        if total == 1 { "code" } else { "codes" },
        match failing_now {
            0 => "None is failing now — all of them are history.".to_string(),
            1 => "1 is failing now; the rest are history.".to_string(),
            n => format!("{n} are failing now; the rest are history."),
        }
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_confirmed_codes_count_as_faults() {
        let unit = UnitFaults {
            all: vec![
                RawDtc { code: [0x00, 0x01, 0x07], status: 0x10 },
                RawDtc { code: [0x06, 0x09, 0x01], status: 0x08 },
            ],
            ..Default::default()
        };
        let confirmed = unit.confirmed();
        assert_eq!(confirmed.len(), 1);
        assert_eq!(confirmed[0].code, [0x06, 0x09, 0x01]);
    }

    #[test]
    fn the_status_byte_is_read_bit_by_bit_from_the_standard() {
        assert_eq!(describe_status(0x08), "confirmed");
        assert_eq!(describe_status(0x10), "not tested since clear");
        assert_eq!(describe_status(0x09), "failed now, confirmed");
        // Nothing asserted: report the byte rather than an empty line.
        assert_eq!(describe_status(0x00), "00");
    }

    #[test]
    fn a_fault_is_dated_by_the_cars_own_counters_not_by_a_calendar() {
        // Real record from the body control module, against the stamp read
        // from the same unit: 9 occurrences, 42 km and 17.9 hours ago.
        let context = FaultContext {
            priority: 6,
            occurrences: 9,
            cycle_counter: 0x02B8,
            mileage_km: 212_763,
            clock: 0x69F9_044B,
        };
        let now = UnitStamp { mileage_km: 212_805, clock: 0x69FA_005C };
        let text = describe_age(&context, Some(&now));
        assert!(text.contains("212763 km"), "{text}");
        assert!(text.contains("9×"), "{text}");
        // The two stamps are 2026-07-28 00:18:19 and 2026-07-28 08:01:32 by
        // the car's clock — seven and three quarter hours apart. Reading the
        // stamp as a counter, as this once did, gave 17.9 h.
        assert!(text.contains("7.7 h ago"), "{text}");
        assert!(text.contains("42 km ago"), "{text}");

        // With nothing to compare against, only what the record itself says.
        let alone = describe_age(&context, None);
        assert!(alone.contains("212763 km"));
        assert!(!alone.contains("ago"), "{alone}");
    }

    #[test]
    fn a_fault_is_stamped_with_the_cars_own_date_and_time() {
        // Both anchors come from VCDS printouts of this car: the brake unit's
        // fault at 2026.07.27 00:00:03 and the steering assist's at
        // 2026.07.28 23:50:02. Six fields, two exact matches.
        let at = |clock| {
            describe_when(&FaultContext {
                priority: 1,
                occurrences: 1,
                cycle_counter: 0,
                mileage_km: 1,
                clock,
            })
        };
        assert_eq!(at(0x69F6_0003), "2026-07-27 00:00:03 by the car's own clock");
        assert_eq!(at(0x69F9_7C82), "2026-07-28 23:50:02 by the car's own clock");
        // An unset stamp is reported as one, not as the year 2000.
        assert!(at(0x0000_0000).contains("not a date"));
    }

    #[test]
    fn a_saturated_occurrence_count_is_not_reported_as_exactly_255() {
        let context = FaultContext {
            priority: 6,
            occurrences: 0xFF,
            cycle_counter: 0x02B9,
            mileage_km: 212_795,
            clock: 0x69F9_68D9,
        };
        assert!(describe_age(&context, None).contains("255+ times"));
    }

    #[test]
    fn all_three_bytes_are_one_fault_number() {
        // Checked against this car's own VCDS scan, which prints these decimal
        // numbers for these units: reading the first two bytes as the number
        // would give 1, 1137 and 260 instead.
        assert_eq!(format_code([0x00, 0x01, 0x29]), "000129  (297)");
        assert_eq!(format_code([0x04, 0x71, 0x20]), "047120  (291104)");
        assert_eq!(format_code([0x01, 0x04, 0x05]), "010405  (66565)");
    }
}
