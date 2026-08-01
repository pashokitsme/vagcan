//! `vagcan watch` — poll a few identifiers as fast as the bus allows.
//!
//! Built for watching something that moves quickly — boost, throttle, rail
//! pressure — where a reading every two seconds is useless. Two things make it
//! quick:
//!
//! - **Batched reads.** This control unit family answers up to eight
//!   identifiers in one request (measured; twelve is refused), so a set of
//!   eight costs one round trip instead of eight.
//! - **No conversion work per sample.** Scalings come from the catalog, or the
//!   raw bytes are shown as they are.
//!
//! Anything unknown is printed as raw bytes rather than dressed up in a unit it
//! has not earned.

use std::collections::BTreeMap;
use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use vag_data::catalog::{MeasurementCatalog, MeasurementDef, ReadId};
use vag_protocol::AsyncUdsClient;
use vag_transport::AsyncIsoTpTransport;

/// Identifiers per request. Measured on the reference car: eight are answered,
/// twelve are refused outright.
const BATCH: usize = 8;

/// Curated sets, so the common cases need no hex typing.
///
/// Every identifier here is one this project proved on the reference car
/// (`research/rod-labels.md` §4.3/§4.3a) or one the OBD-II standard defines.
/// Nothing speculative: a preset that listed unproven identifiers would print
/// confident-looking numbers with no basis.
pub const PRESETS: &[(&str, u8, &str, &str)] = &[
    ("boost", 1, "2029,202A,206E,F423", "boost target + actual, engine speed, rail pressure"),
    ("engine", 1, "206E,202A,F404,F411,F40B,F40F", "speed, boost, load, throttle, manifold, intake air"),
    ("thermal", 1, "F405,F40F,F446,F43C,F442", "coolant, intake, ambient, catalyst, voltage"),
    ("gearbox", 2, "3816,3809,380A,380B,3804", "gear, selector, shaft speeds, pedal"),
    ("clutches", 2, "38F6,38F9,38AC,38AD", "clutch positions, nominal and actual (mm)"),
];

/// Look a preset up by name.
pub fn preset(name: &str) -> Option<(u8, &'static str, &'static str)> {
    PRESETS.iter().find(|(n, ..)| *n == name).map(|(_, ecu, dids, what)| (*ecu, *dids, *what))
}

/// Parse `--did 2029,202A` into identifiers.
pub fn parse_dids(spec: &str) -> Result<Vec<u16>> {
    let mut out = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let did = u16::from_str_radix(part, 16)
            .with_context(|| format!("{part:?} is not a hex data identifier"))?;
        out.push(did);
    }
    if out.is_empty() {
        anyhow::bail!("no identifiers given");
    }
    Ok(out)
}

/// Split a multi-identifier response into its records.
///
/// Same rule as the capture analysis: each requested identifier must appear in
/// the order asked, otherwise the response is not attributable and is dropped.
fn split(payload: &[u8], dids: &[u16]) -> Option<Vec<(u16, Vec<u8>)>> {
    crate::analyse::split_records(payload, dids)
}

/// How one identifier is displayed.
struct Column {
    did: u16,
    def: Option<MeasurementDef>,
}

impl Column {
    fn label(&self) -> String {
        match &self.def {
            Some(d) => d.name.to_string(),
            // An unnamed identifier is shown by number — there is nothing
            // honest to call it yet.
            None => format!("{:04X}", self.did),
        }
    }

    /// The label cut to the column, so long names cannot run into each other
    /// and leave the header unreadable.
    fn short_label(&self, width: usize) -> String {
        let full = self.label();
        if full.chars().count() <= width {
            return full;
        }
        full.chars().take(width - 1).collect::<String>() + "…"
    }

    /// A value we can convert is shown in its unit; anything else is shown as
    /// bytes tagged **(raw)**, so a number whose meaning is unproven can never
    /// be mistaken for one that is.
    fn render(&self, data: &[u8]) -> String {
        // A discrete state has a name, not a number.
        if let Some(def) = &self.def {
            if matches!(def.scaling, vag_data::catalog::Scaling::Enum { .. }) {
                return match def.describe(data) {
                    Some(state) => state,
                    // A code the definition does not list is unknown, and
                    // saying so beats inventing a state.
                    None => format!(
                        "{} (raw)",
                        data.iter().map(|b| format!("{b:02X}")).collect::<String>()
                    ),
                };
            }
        }
        match self.def.as_ref().and_then(|d| d.interpret(data)) {
            Some(v) => {
                let unit = self.def.as_ref().map(|d| d.unit.as_ref()).unwrap_or("");
                format!("{v:.2} {unit}")
            }
            None => {
                let hex: String = data.iter().map(|b| format!("{b:02X}")).collect();
                format!("{hex} (raw)")
            }
        }
    }
}

/// Poll `dids` on one control unit until Ctrl-C or the sample limit.
pub async fn run<T: AsyncIsoTpTransport>(
    uds: &mut AsyncUdsClient<T>,
    dids: &[u16],
    catalog: &MeasurementCatalog,
    hz: f64,
    seconds: Option<u64>,
    out: Option<&str>,
) -> Result<()> {
    let known: BTreeMap<u16, MeasurementDef> = catalog
        .defs
        .iter()
        .map(|d| match d.address {
            ReadId::Uds(did) => (did, d.clone()),
        })
        .collect();
    let columns: Vec<Column> =
        dids.iter().map(|did| Column { did: *did, def: known.get(did).cloned() }).collect();

    let mut sink = match out {
        Some(path) => {
            let file = std::fs::File::create(path)
                .with_context(|| format!("creating {path:?}"))?;
            let mut w = std::io::BufWriter::new(file);
            // Each value gets its OWN time column, because the values on one
            // row are not simultaneous: identifiers are polled in batches, so
            // the last column of a row can be most of a cycle newer than the
            // first. Writing one timestamp per row would assert a simultaneity
            // that does not exist, and any later column-against-column
            // analysis would inherit the error. (VCDS's own export does the
            // same thing for the same reason.)
            let header: Vec<String> = columns
                .iter()
                .map(|c| format!("{}_t_s,{}", c.label(), c.label()))
                .collect();
            writeln!(w, "t_s,{}", header.join(","))?;
            Some(w)
        }
        None => None,
    };

    println!(
        "  {:>7}  {}",
        "t",
        columns
            .iter()
            .map(|c| format!("{:>17} ", c.short_label(16)))
            .collect::<Vec<_>>()
            .join("")
    );

    let started = Instant::now();
    let period = Duration::from_secs_f64(1.0 / hz.max(0.1));
    let deadline = seconds.map(|s| started + Duration::from_secs(s));
    let mut samples = 0u64;

    loop {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
        let cycle = Instant::now();
        let t_s = started.elapsed().as_secs_f64();

        let mut values: BTreeMap<u16, (f64, Vec<u8>)> = BTreeMap::new();
        for chunk in dids.chunks(BATCH) {
            let answer = if chunk.len() == 1 {
                uds.read_data_by_identifier(chunk[0]).await.map(|d| vec![(chunk[0], d)])
            } else {
                uds.read_data_by_identifiers(chunk)
                    .await
                    .map(|payload| split(&payload, chunk).unwrap_or_default())
            };
            let at = started.elapsed().as_secs_f64();
            match answer {
                Ok(records) => {
                    values.extend(records.into_iter().map(|(did, data)| (did, (at, data))));
                }
                // A refusal mid-run is normal if the unit drops a parameter;
                // keep polling rather than aborting the session.
                Err(_) => continue,
            }
        }

        let row: Vec<String> = columns
            .iter()
            .map(|c| match values.get(&c.did) {
                Some((_, data)) => format!("{:>17} ", c.render(data)),
                None => format!("{:>17} ", "-"),
            })
            .collect();
        // A terminal gets one line that updates in place; a pipe or a file
        // gets one line per sample, because carriage returns would run the
        // whole session together into a single unreadable line.
        if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
            print!("\r  {t_s:>7.2}  {}", row.join(""));
            std::io::stdout().flush().ok();
        } else {
            println!("  {t_s:>7.2}  {}", row.join(""));
        }

        if let Some(w) = sink.as_mut() {
            let cells: Vec<String> = columns
                .iter()
                .map(|c| match values.get(&c.did) {
                    Some((at, data)) => {
                        let value = match c.def.as_ref().and_then(|d| d.interpret(data)) {
                            Some(v) => format!("{v}"),
                            None => data.iter().map(|b| format!("{b:02X}")).collect(),
                        };
                        format!("{at:.3},{value}")
                    }
                    // No answer this cycle: no time either, so nothing can be
                    // read as a sample that was never taken.
                    None => ",".to_string(),
                })
                .collect();
            writeln!(w, "{t_s:.3},{}", cells.join(","))?;
        }
        samples += 1;

        if let Some(rest) = period.checked_sub(cycle.elapsed()) {
            tokio::time::sleep(rest).await;
        }
    }

    let elapsed = started.elapsed().as_secs_f64();
    println!("\n\n{samples} samples in {elapsed:.1}s — {:.1} Hz", samples as f64 / elapsed);
    if let Some(w) = sink.as_mut() {
        w.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;
    use vag_data::catalog::{ReadId, Scaling};
    use vag_data::measure::{LinearScale, RawForm};

    fn boost(did: u16) -> MeasurementDef {
        MeasurementDef {
            name: Cow::Borrowed("Boost"),
            unit: Cow::Borrowed("bar"),
            address: ReadId::Uds(did),
            raw_form: RawForm::U16Be,
            scaling: Scaling::Linear(LinearScale { factor: 0.001, offset: 0.0 }),
        }
    }

    #[test]
    fn every_preset_names_real_identifiers_on_a_real_unit() {
        // A preset that does not parse would fail at the car, which is the
        // worst place to find out.
        for (name, ecu, dids, _) in PRESETS {
            let parsed = parse_dids(dids).unwrap_or_else(|e| panic!("preset {name}: {e}"));
            assert!(!parsed.is_empty(), "preset {name} is empty");
            assert!(*ecu == 1 || *ecu == 2, "preset {name} names control unit {ecu}");
        }
        assert!(preset("boost").is_some());
        assert!(preset("nonexistent").is_none());
    }

    #[test]
    fn identifiers_parse_as_hex_and_reject_nonsense() {
        assert_eq!(parse_dids("2029,202A").unwrap(), vec![0x2029, 0x202A]);
        assert_eq!(parse_dids(" F405 ").unwrap(), vec![0xF405]);
        assert!(parse_dids("zzz").is_err());
        assert!(parse_dids("").is_err());
    }

    #[test]
    fn a_known_identifier_shows_engineering_units_and_an_unknown_one_shows_bytes() {
        // Being explicit about what is known is the point: an unknown value
        // must not appear in a unit it has not earned.
        let known = Column { did: 0x202A, def: Some(boost(0x202A)) };
        assert_eq!(known.render(&[0x03, 0xDF]).trim(), "0.99 bar");
        assert_eq!(known.label(), "Boost");

        // Unproven: the bytes are shown and explicitly tagged, so nobody reads
        // them as an engineering value.
        let unknown = Column { did: 0x1234, def: None };
        assert_eq!(unknown.render(&[0x03, 0xDF]), "03DF (raw)");
        assert_eq!(unknown.label(), "1234");
    }

    #[test]
    fn a_gear_shows_its_name_and_an_unlisted_code_shows_as_raw() {
        let gear = vag_data::catalog::proven_gearbox()
            .into_iter()
            .find(|d| matches!(d.address, ReadId::Uds(0x3816)))
            .unwrap();
        let column = Column { did: 0x3816, def: Some(gear) };
        assert_eq!(column.render(&[0x05]), "4");
        assert_eq!(column.render(&[0x0C]), "R");
        assert_eq!(column.render(&[0x09]), "09 (raw)");
    }

    #[test]
    fn long_names_are_cut_to_the_column_instead_of_colliding() {
        let known = Column { did: 0x2029, def: Some(boost(0x2029)) };
        assert_eq!(known.short_label(16), "Boost");

        let long = Column {
            did: 0x38F6,
            def: Some(MeasurementDef {
                name: Cow::Borrowed("Clutch 2 position, specified"),
                ..boost(0x38F6)
            }),
        };
        let cut = long.short_label(16);
        assert_eq!(cut.chars().count(), 16, "fits the column: {cut:?}");
        assert!(cut.ends_with('…'), "and says it was cut: {cut:?}");
    }

    #[test]
    fn a_value_too_short_for_its_form_falls_back_to_raw_bytes() {
        // Never invent a number from bytes that cannot carry it.
        let known = Column { did: 0x202A, def: Some(boost(0x202A)) };
        assert_eq!(known.render(&[0x03]), "03 (raw)");
    }
}
