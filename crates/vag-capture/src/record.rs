use serde::{Deserialize, Serialize};
use vag_transport::CanId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Tx,
    Rx,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapturePayload {
    CanFrame { id: CanId, data: Vec<u8> },
    CableBytes { bytes: Vec<u8> },
    /// Out-of-band annotation carrying no bus traffic: the wall-clock anchor
    /// written at the head of a capture, and operator notes typed during it
    /// ("engine started", "pulling away").
    ///
    /// `ts_us` is monotonic from the start of the capture, which says nothing
    /// about *when* the capture happened. Correlating a capture with a VCDS
    /// CSV log needs an absolute reference, and guessing that offset after the
    /// fact has already cost two capture sessions (`research/rod-labels.md`
    /// §4.0a/§4.0b: the lag had to be fitted at ≈52 s, and several apparent
    /// correlations turned out to be window-fishing at wrong lags). The anchor
    /// makes the alignment arithmetic.
    Marker { note: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRecord {
    pub ts_us: u64,
    pub dir: Direction,
    pub payload: CapturePayload,
}

/// The note text of a capture's wall-clock anchor: microseconds since the Unix
/// epoch, so an offline analysis can convert `ts_us` into local time and line a
/// capture up with a VCDS CSV log.
///
/// Stored as an integer rather than a formatted date deliberately: no timezone
/// database is needed to write it, and every analysis language converts epoch
/// micros to local time in one call.
pub fn wall_clock_anchor(unix_us: u64) -> String {
    format!("capture start unix_us={unix_us}")
}

/// Recover the epoch microseconds from a [`wall_clock_anchor`] note.
pub fn parse_wall_clock_anchor(note: &str) -> Option<u64> {
    note.strip_prefix("capture start unix_us=")?.trim().parse().ok()
}

/// Append one record to a JSON-lines sink. Streaming, so an interrupted capture
/// keeps everything written up to the interruption.
pub fn write_record(mut w: impl std::io::Write, record: &CaptureRecord) -> std::io::Result<()> {
    let line = serde_json::to_string(record)?;
    w.write_all(line.as_bytes())?;
    w.write_all(b"\n")
}

pub fn write_records(mut w: impl std::io::Write, records: &[CaptureRecord]) -> std::io::Result<()> {
    for rec in records {
        let line = serde_json::to_string(rec)?;
        w.write_all(line.as_bytes())?;
        w.write_all(b"\n")?;
    }
    Ok(())
}

pub fn read_records(r: impl std::io::Read) -> std::io::Result<Vec<CaptureRecord>> {
    use std::io::BufRead;
    let reader = std::io::BufReader::new(r);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str(&line)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_through_jsonl() {
        let records = vec![
            CaptureRecord {
                ts_us: 1,
                dir: Direction::Tx,
                payload: CapturePayload::CanFrame {
                    id: CanId::Standard(0x7E0),
                    data: vec![0x02, 0x10, 0x03],
                },
            },
            CaptureRecord {
                ts_us: 2,
                dir: Direction::Rx,
                payload: CapturePayload::CableBytes { bytes: vec![0xAA, 0xBB] },
            },
        ];
        let mut buf = Vec::new();
        write_records(&mut buf, &records).unwrap();
        // JSON-lines: exactly two newline-terminated lines.
        assert_eq!(buf.iter().filter(|&&b| b == b'\n').count(), 2);
        let back = read_records(&buf[..]).unwrap();
        assert_eq!(back, records);
    }

    #[test]
    fn markers_round_trip_and_carry_the_wall_clock_anchor() {
        let unix_us = 1_753_900_000_000_000u64;
        let records = vec![
            CaptureRecord {
                ts_us: 0,
                dir: Direction::Rx,
                payload: CapturePayload::Marker { note: wall_clock_anchor(unix_us) },
            },
            CaptureRecord {
                ts_us: 5_000_000,
                dir: Direction::Rx,
                payload: CapturePayload::Marker { note: "engine started".to_string() },
            },
        ];
        let mut buf = Vec::new();
        write_records(&mut buf, &records).unwrap();
        let back = read_records(&buf[..]).unwrap();
        assert_eq!(back, records);

        let CapturePayload::Marker { note } = &back[0].payload else {
            panic!("expected a marker");
        };
        assert_eq!(parse_wall_clock_anchor(note), Some(unix_us));
        // A plain operator note is not an anchor.
        assert_eq!(parse_wall_clock_anchor("engine started"), None);
    }

    #[test]
    fn captures_written_before_markers_existed_still_parse() {
        // Adding a payload variant must not orphan the captures already on
        // disk, so pin the exact legacy line format.
        let legacy = r#"{"ts_us":7,"dir":"Rx","payload":{"CanFrame":{"id":{"Standard":2024},"data":[2,16,3]}}}"#;
        let back = read_records(legacy.as_bytes()).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(
            back[0].payload,
            CapturePayload::CanFrame { id: CanId::Standard(0x7E8), data: vec![0x02, 0x10, 0x03] }
        );
    }

    #[test]
    fn write_record_appends_one_line_at_a_time() {
        let mut buf = Vec::new();
        for (i, note) in ["a", "b", "c"].iter().enumerate() {
            write_record(
                &mut buf,
                &CaptureRecord {
                    ts_us: i as u64,
                    dir: Direction::Rx,
                    payload: CapturePayload::Marker { note: note.to_string() },
                },
            )
            .unwrap();
        }
        assert_eq!(read_records(&buf[..]).unwrap().len(), 3);
    }
}
