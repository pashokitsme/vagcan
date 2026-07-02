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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRecord {
    pub ts_us: u64,
    pub dir: Direction,
    pub payload: CapturePayload,
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
}
