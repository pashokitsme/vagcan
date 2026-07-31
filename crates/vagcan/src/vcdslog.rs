//! Reading a VCDS "advanced measuring blocks" CSV export.
//!
//! This is the reference side of the crib: what VCDS *displayed* while our
//! adapter recorded what went over the wire. Format, from a real export:
//!
//! ```text
//! Воскресенье,12,Июль,2026,15:07:29:00009-VCID:…      <- wall-clock start
//! 8V0 906 264 H,ADVMB,1.8l R4 TFSI  H13 0005          <- the control unit
//! ,,G004,F0,G006,F0,…                                 <- source groups
//!
//! Маркер,ВРЕМЯ,Loc. IDE00025,ВРЕМЯ,Loc. IDE00075,…    <- ids
//! ,ШТАМП,Температура …,ШТАМП,Скорость автомобиля,…    <- names
//! ,, *C,, km/h,…                                      <- units
//! ,0.00,99,0.22,0,0.43,12.2,…                         <- samples
//! ```
//!
//! Two properties matter. Each measurement carries **its own time column** —
//! samples are staggered, not simultaneous — so every series keeps its own
//! timestamps rather than sharing a row index. And the file is **CP1251**, not
//! UTF-8, because this is the Russian build.

use std::collections::BTreeMap;

/// One measurement logged by VCDS.
#[derive(Debug, Clone, PartialEq)]
pub struct LoggedMeasurement {
    /// The `IDE#####` identifier VCDS uses for the measurement.
    pub ide: String,
    /// Display name, as VCDS shows it.
    pub name: String,
    /// Engineering unit (`°C`, `km/h`, `/min`, …).
    pub unit: String,
    /// `(seconds since the log started, displayed value)`.
    pub samples: Vec<(f64, f64)>,
}

impl LoggedMeasurement {
    /// True when the value never moves. A constant series cannot support a
    /// slope, so fitting one is how a scaling gets invented.
    pub fn is_constant(&self) -> bool {
        let mut values = self.samples.iter().map(|(_, v)| *v);
        match values.next() {
            None => true,
            Some(first) => values.all(|v| (v - first).abs() < f64::EPSILON),
        }
    }
}

/// A parsed VCDS log.
#[derive(Debug, Clone, PartialEq)]
pub struct VcdsLog {
    /// Local wall-clock start, as `(hour, minute, second)` — the file states
    /// the time of day but not the timezone, so it is kept as-is and matched
    /// against the capture's anchor converted to local time.
    pub started_hms: (u32, u32, u32),
    /// Control unit part number from the header, when present.
    pub part_number: Option<String>,
    pub measurements: Vec<LoggedMeasurement>,
}

/// Decode CP1251 (the Russian VCDS build's encoding).
///
/// The Cyrillic block is contiguous — `0xC0..=0xFF` maps straight onto
/// `U+0410..=U+044F` — so the whole table is three arms and no dependency.
fn cp1251(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match b {
            0x00..=0x7F => b as char,
            0xB0 => '°',
            0xC0..=0xFF => char::from_u32(0x0410 + (b as u32 - 0xC0)).unwrap_or('?'),
            _ => ' ',
        })
        .collect()
}

/// Split a CSV line on commas. VCDS does not quote its fields, and the values
/// we read (numbers, `IDE#####`, units) never contain a comma.
fn fields(line: &str) -> Vec<&str> {
    line.trim_end_matches('\r').split(',').collect()
}

/// The `HH:MM:SS` in the first header line, which also carries the sequence
/// number and VCID after a fourth colon.
fn parse_start_time(field: &str) -> Option<(u32, u32, u32)> {
    let mut parts = field.split(':');
    let h = parts.next()?.trim().parse().ok()?;
    let m = parts.next()?.trim().parse().ok()?;
    let s = parts.next()?.trim().parse().ok()?;
    Some((h, m, s))
}

/// Parse a VCDS ADVMB CSV export.
pub fn parse(bytes: &[u8]) -> Result<VcdsLog, String> {
    let text = cp1251(bytes);
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < 7 {
        return Err("too short to be a VCDS log (needs a header and samples)".to_string());
    }

    // Header line 1: …,<year>,<HH:MM:SS:seq-VCID:…>
    let head = fields(lines[0]);
    let started_hms = head
        .get(4)
        .and_then(|f| parse_start_time(f))
        .ok_or("no start time in the first header line")?;

    let part_number = fields(lines[1]).first().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

    // Find the id row: the one carrying `Loc. IDE…`. Names and units follow it.
    let id_row = lines
        .iter()
        .position(|l| l.contains("IDE"))
        .ok_or("no measurement id row (expected `Loc. IDExxxxx`)")?;
    let ids = fields(lines[id_row]);
    let names = lines.get(id_row + 1).map(|l| fields(l)).unwrap_or_default();
    let units = lines.get(id_row + 2).map(|l| fields(l)).unwrap_or_default();

    // Columns come in (time, value) pairs; the id sits on the value column.
    let mut by_column: BTreeMap<usize, LoggedMeasurement> = BTreeMap::new();
    for (col, field) in ids.iter().enumerate() {
        let Some(ide) = field.trim().strip_prefix("Loc. ") else {
            continue;
        };
        let ide = ide.trim().to_string();
        if ide.is_empty() {
            continue;
        }
        by_column.insert(
            col,
            LoggedMeasurement {
                ide,
                name: names.get(col).map(|s| s.trim().to_string()).unwrap_or_default(),
                unit: units.get(col).map(|s| s.trim().to_string()).unwrap_or_default(),
                samples: Vec::new(),
            },
        );
    }
    if by_column.is_empty() {
        return Err("the id row named no measurements".to_string());
    }

    for line in &lines[id_row + 3..] {
        if line.trim().is_empty() {
            continue;
        }
        let row = fields(line);
        for (col, measurement) in by_column.iter_mut() {
            // The value's time lives in the column immediately to its left.
            let (Some(t), Some(v)) = (row.get(col - 1), row.get(*col)) else {
                continue;
            };
            let (Ok(t), Ok(v)) = (t.trim().parse::<f64>(), v.trim().parse::<f64>()) else {
                continue;
            };
            measurement.samples.push((t, v));
        }
    }

    Ok(VcdsLog {
        started_hms,
        part_number,
        measurements: by_column.into_values().filter(|m| !m.samples.is_empty()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A miniature of the real export, byte-for-byte in the same shape
    /// (CP1251 Cyrillic included).
    fn sample() -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        // Header: day-of-week (Cyrillic), day, month, year, HH:MM:SS:seq-VCID
        out.extend(&[0xC2, 0xEE, 0xF1]); // "Вос"
        out.extend(b",12,");
        out.extend(&[0xC8, 0xFE, 0xEB]); // "Июл"
        out.extend(b",2026,15:07:29:00009-VCID:418318C6\r\n");
        out.extend(b"8V0 906 264 H,ADVMB,1.8l R4 TFSI  H13 0005,\r\n");
        out.extend(b",,G004,F0,G006,F0,\r\n");
        out.extend(b"\r\n");
        out.extend(b"Marker,TIME,Loc. IDE00025,TIME,Loc. IDE00405,\r\n");
        out.extend(b",STAMP,Coolant,STAMP,Engine speed,\r\n");
        out.extend(b",, *C,, /min,\r\n");
        out.extend(b",0.00,99,0.43,801,\r\n");
        out.extend(b",1.50,100,1.94,796,\r\n");
        out.extend(b",2.96,101,3.36,1520,\r\n");
        out
    }

    #[test]
    fn the_wall_clock_start_is_read_from_the_header() {
        // This is the whole point of the file for alignment purposes.
        let log = parse(&sample()).unwrap();
        assert_eq!(log.started_hms, (15, 7, 29));
        assert_eq!(log.part_number.as_deref(), Some("8V0 906 264 H"));
    }

    #[test]
    fn each_measurement_keeps_its_own_timestamps() {
        // VCDS staggers the samples: coolant at 0.00 s, engine speed at 0.43 s.
        // Treating a row as one instant would smear every series by ~0.5 s.
        let log = parse(&sample()).unwrap();
        assert_eq!(log.measurements.len(), 2);

        let coolant = &log.measurements[0];
        assert_eq!(coolant.ide, "IDE00025");
        assert_eq!(coolant.unit, "*C");
        assert_eq!(coolant.samples, vec![(0.00, 99.0), (1.50, 100.0), (2.96, 101.0)]);

        let rpm = &log.measurements[1];
        assert_eq!(rpm.ide, "IDE00405");
        assert_eq!(rpm.unit, "/min");
        assert_eq!(rpm.samples, vec![(0.43, 801.0), (1.94, 796.0), (3.36, 1520.0)]);
    }

    #[test]
    fn cyrillic_names_survive_the_encoding() {
        // The real files are CP1251; decoding them as UTF-8 or Latin-1 turns
        // every name into mojibake.
        let decoded = cp1251(&[0xD2, 0xE5, 0xEC, 0xEF, 0xE5, 0xF0, 0xE0, 0xF2, 0xF3, 0xF0, 0xE0]);
        assert_eq!(decoded, "Температура");
        // The degree sign VCDS uses in unit columns.
        assert_eq!(cp1251(&[0xB0]), "°");
    }

    #[test]
    fn a_constant_series_is_flagged_as_such() {
        // Fitting a slope to a constant is how a scaling gets invented.
        let flat = LoggedMeasurement {
            ide: "IDE00155".to_string(),
            name: "Ignition angle".to_string(),
            unit: "°".to_string(),
            samples: vec![(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)],
        };
        assert!(flat.is_constant());
        let moving = LoggedMeasurement { samples: vec![(0.0, 1.0), (1.0, 2.0)], ..flat };
        assert!(!moving.is_constant());
    }

    #[test]
    #[ignore = "needs research/dumps/coolant-rpm-speed.CSV (gitignored, real vehicle data)"]
    fn the_real_export_from_the_car_parses() {
        // Guards against the synthetic sample drifting from the real format.
        let bytes = std::fs::read("../../research/dumps/coolant-rpm-speed.CSV").unwrap();
        let log = parse(&bytes).unwrap();
        assert_eq!(log.started_hms, (15, 7, 29));
        assert_eq!(log.part_number.as_deref(), Some("8V0 906 264 H"));
        assert_eq!(log.measurements.len(), 7, "the header names seven measurements");

        let ides: Vec<&str> = log.measurements.iter().map(|m| m.ide.as_str()).collect();
        assert!(ides.contains(&"IDE00405"), "engine speed present: {ides:?}");

        let rpm = log.measurements.iter().find(|m| m.ide == "IDE00405").unwrap();
        assert_eq!(rpm.unit, "/min");
        assert!(rpm.samples.len() > 40, "{} samples", rpm.samples.len());
        let hi = rpm.samples.iter().map(|(_, v)| *v).fold(f64::MIN, f64::max);
        let lo = rpm.samples.iter().map(|(_, v)| *v).fold(f64::MAX, f64::min);
        assert!((780.0..800.0).contains(&lo) && hi > 3800.0, "the logged rev: {lo}..{hi}");
        assert!(!rpm.is_constant());
    }

    #[test]
    fn a_file_that_is_not_a_vcds_log_is_rejected_with_a_reason() {
        assert!(parse(b"hello\n").is_err());
        let no_ids = b"a,b,c,d,15:00:00:1\r\nx\r\ny\r\n\r\nz\r\nz\r\nz\r\nz\r\n";
        assert!(parse(no_ids).unwrap_err().contains("measurement id row"));
    }
}
