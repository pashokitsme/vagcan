//! `vagcan sniff` — watch the car's CAN bus without touching it.
//!
//! The point of this command is to sit on the OBD-II bus **alongside VCDS**
//! while VCDS runs a normal session, and record every request and response in
//! the clear. That is the crib the measurement work has been missing: prior
//! captures were taken on the HEX clone's USB link, where the payload is
//! ciphered and multi-frame group reads never decoded
//! (`research/rod-labels.md` §4.0a–§4.0c).
//!
//! This module holds the session logic — frame filtering, capture writing,
//! display formatting — with the I/O passed in, so it is testable without an
//! adapter. The CLI wiring lives in `main.rs`.

use std::io::Write;

use vag_can::backend::{CAN_EFF_FLAG, CAN_EFF_MASK};
use vag_can::sniff::{IsoTpSniffer, SnifferPdu};
use vag_capture::{
    wall_clock_anchor, write_record, CapturePayload, CaptureRecord, Direction,
};
use vag_transport::CanId;

/// True for ids that carry UDS diagnostics, i.e. the traffic a parallel VCDS
/// session produces.
///
/// Standard: the whole `0x700..=0x7FF` block — `0x7DF` is the functional
/// broadcast, `0x7E0..=0x7EF` the eight tester/ECU pairs, and the rest of the
/// block is used by VAG gateways for further ECUs.
/// Extended: ISO 15765-4 normal-fixed addressing (`0x18DA_xxxx` physical,
/// `0x18DB_xxxx` functional) plus the `0x17FC_00xx` / `0x17FE_00xx` pairs MQB
/// gateways use.
pub fn is_diag_id(raw: u32) -> bool {
    if raw & CAN_EFF_FLAG == 0 {
        return (0x700..=0x7FF).contains(&raw);
    }
    let id = raw & CAN_EFF_MASK;
    let high = id >> 16;
    matches!(high, 0x18DA | 0x18DB | 0x17FC | 0x17FE)
}

/// Which way a diagnostic PDU was travelling, inferred from its id.
///
/// Requests and responses are separate ids in UDS-on-CAN, so direction is a
/// property of the id — the capture's own `Direction` field cannot say, since
/// a listener only ever receives.
fn arrow(raw: u32) -> &'static str {
    if raw & CAN_EFF_FLAG != 0 {
        let id = raw & CAN_EFF_MASK;
        // Normal fixed addressing: 0x18DA <target> <source>. The tester is F1.
        return match (id >> 16, id & 0xFF) {
            (0x18DA | 0x18DB, 0xF1) => "<-",
            (0x18DA | 0x18DB, _) => "->",
            _ => "  ",
        };
    }
    match raw {
        0x7DF => "->",              // functional request to every ECU
        0x7E0..=0x7E7 => "->",      // tester → ECU
        0x7E8..=0x7EF => "<-",      // ECU → tester
        _ => "  ",
    }
}

/// Render one raw id the way it is usually written.
fn id_text(raw: u32) -> String {
    if raw & CAN_EFF_FLAG != 0 {
        format!("{:08X}", raw & CAN_EFF_MASK)
    } else {
        format!("{:03X}", raw)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

/// One display line for a completed PDU.
///
/// Long payloads are cut for the terminal; the capture file always holds every
/// byte, so nothing is lost by shortening what scrolls past on the car.
pub fn format_pdu(ts_us: u64, pdu: &SnifferPdu) -> String {
    let secs = ts_us as f64 / 1e6;
    let mut body = hex(&pdu.data);
    let full_len = pdu.data.len();
    if full_len > 16 {
        body = format!("{} …", hex(&pdu.data[..16]));
    }
    let tail = if pdu.frames > 1 {
        format!("   ({full_len}B, {} frames)", pdu.frames)
    } else {
        String::new()
    };
    format!("{secs:9.3}  {:>8} {}  {body}{tail}", id_text(pdu.id), arrow(pdu.id))
}

/// What a finished session saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SniffStats {
    /// CAN frames observed (before filtering).
    pub frames_seen: usize,
    /// CAN frames written to the capture (after `--diag-only`).
    pub frames_kept: usize,
    /// Complete ISO-TP PDUs reassembled.
    pub pdus: usize,
    /// Multi-frame messages abandoned (gap, restart, or timeout).
    pub dropped: usize,
    /// Operator markers written.
    pub markers: usize,
}

/// A sniffing session: filters frames, streams them to a capture file, and
/// reassembles diagnostic conversations for display.
pub struct SniffSession<W: Write> {
    sniffer: IsoTpSniffer,
    capture: Option<W>,
    diag_only: bool,
    stats: SniffStats,
}

impl<W: Write> SniffSession<W> {
    /// Start a session. When a capture sink is given, the wall-clock anchor is
    /// written as its first record — `ts_us` is monotonic from now, and the
    /// anchor is what makes it absolute enough to line up with a VCDS CSV.
    pub fn new(capture: Option<W>, unix_us: u64, diag_only: bool) -> std::io::Result<Self> {
        let mut session =
            SniffSession { sniffer: IsoTpSniffer::new(), capture, diag_only, stats: SniffStats::default() };
        if session.capture.is_some() {
            session.write_marker(0, &wall_clock_anchor(unix_us))?;
            // The anchor is bookkeeping, not an operator note.
            session.stats.markers = 0;
        }
        Ok(session)
    }

    /// Feed one observed frame. Returns a display line when the frame
    /// completed a diagnostic PDU.
    pub fn on_frame(
        &mut self,
        raw_id: u32,
        data: &[u8],
        ts_us: u64,
    ) -> std::io::Result<Option<String>> {
        self.stats.frames_seen += 1;
        let diag = is_diag_id(raw_id);
        if self.diag_only && !diag {
            return Ok(None);
        }
        self.stats.frames_kept += 1;

        if let Some(w) = self.capture.as_mut() {
            let id = if raw_id & CAN_EFF_FLAG != 0 {
                CanId::Extended(raw_id & CAN_EFF_MASK)
            } else {
                CanId::Standard(raw_id as u16)
            };
            // Direction is Rx because we only ever receive — including when the
            // frame is a request some other tester transmitted. The bus-level
            // direction is recovered from the id, not from this field.
            write_record(
                w,
                &CaptureRecord {
                    ts_us,
                    dir: Direction::Rx,
                    payload: CapturePayload::CanFrame { id, data: data.to_vec() },
                },
            )?;
        }

        // Only diagnostic ids are worth reassembling; broadcast powertrain
        // frames are not ISO-TP and would be noise.
        if !diag {
            return Ok(None);
        }
        let pdu = self.sniffer.observe(raw_id, data);
        self.stats.dropped = self.sniffer.dropped();
        match pdu {
            Some(pdu) => {
                self.stats.pdus += 1;
                Ok(Some(format_pdu(ts_us, &pdu)))
            }
            None => Ok(None),
        }
    }

    /// Record an operator note ("engine started", "pulling away").
    pub fn on_marker(&mut self, note: &str, ts_us: u64) -> std::io::Result<()> {
        self.write_marker(ts_us, note)?;
        self.stats.markers += 1;
        Ok(())
    }

    fn write_marker(&mut self, ts_us: u64, note: &str) -> std::io::Result<()> {
        if let Some(w) = self.capture.as_mut() {
            write_record(
                w,
                &CaptureRecord {
                    ts_us,
                    dir: Direction::Rx,
                    payload: CapturePayload::Marker { note: note.to_string() },
                },
            )?;
        }
        Ok(())
    }

    pub fn stats(&self) -> SniffStats {
        self.stats
    }

    /// Flush the capture sink. Called before reporting, so an interrupted
    /// session still leaves a complete file on disk.
    pub fn finish(mut self) -> std::io::Result<()> {
        if let Some(w) = self.capture.as_mut() {
            w.flush()?;
        }
        Ok(())
    }
}

/// Run a sniffing session against a real adapter (the `vagcan sniff` command).
///
/// The loop polls the adapter with a short receive window rather than awaiting
/// frames inside a `select!`: Ctrl-C and marker input then reach us between
/// whole frames, and no half-consumed serial read can be cancelled.
pub async fn run(
    device_path: &str,
    baud: u32,
    out: Option<&str>,
    diag_only: bool,
    seconds: Option<u64>,
    active: bool,
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant, SystemTime};
    use vag_can::{CanBackend, CanError, SlcanBackend, SlcanBitrate, SlcanMode};

    let mode = if active { SlcanMode::Normal } else { SlcanMode::Silent };
    let mut backend = SlcanBackend::open_mode(device_path, baud, SlcanBitrate::Rate500k, mode)
        .await
        .with_context(|| crate::device::open_failure(device_path))?;

    let capture: Option<Box<dyn Write>> = match out {
        Some(path) => {
            let file = std::fs::File::create(path)
                .with_context(|| format!("creating capture file {path:?}"))?;
            Some(Box::new(std::io::BufWriter::new(file)))
        }
        None => None,
    };
    let unix_us = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);
    let mut session = SniffSession::new(capture, unix_us, diag_only)?;

    println!(
        "listening at 500 kbit/s, {}{}{}",
        if active { "NORMAL mode — the adapter will acknowledge frames" } else { "listen-only" },
        match out {
            Some(path) => format!(", writing {path}"),
            None => String::new(),
        },
        if diag_only { ", diagnostic traffic only" } else { "" },
    );
    println!("type a note + Enter to mark the capture; Ctrl-C to stop\n");

    // Ctrl-C and stdin are watched by their own tasks, so the receive loop is
    // never interrupted mid-frame.
    let stop = Arc::new(AtomicBool::new(false));
    let signalled = stop.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signalled.store(true, Ordering::Relaxed);
        }
    });
    // Markers are read on a plain OS thread, NOT `tokio::io::stdin()`. That
    // reads on a runtime blocking thread, and dropping the runtime waits for
    // blocking work to finish — a read parked on an idle terminal never
    // finishes, so the process would print its summary after Ctrl-C and then
    // hang forever. A detached OS thread does not hold the process open.
    let (notes_tx, notes_rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        use std::io::BufRead as _;
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if notes_tx.send(line).is_err() {
                break;
            }
        }
    });

    let started = Instant::now();
    let deadline = seconds.map(|s| started + Duration::from_secs(s));
    while !stop.load(Ordering::Relaxed) {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
        let ts_us = started.elapsed().as_micros() as u64;
        while let Ok(note) = notes_rx.try_recv() {
            let note = if note.trim().is_empty() { "mark".to_string() } else { note };
            session.on_marker(&note, ts_us)?;
            println!("{:9.3}  -- {note}", ts_us as f64 / 1e6);
        }

        match backend.recv_frame(Duration::from_millis(200)).await {
            Ok((id, data)) => {
                let ts_us = started.elapsed().as_micros() as u64;
                if let Some(line) = session.on_frame(id, &data, ts_us)? {
                    println!("{line}");
                }
            }
            // A quiet window is normal — the bus may simply be idle.
            Err(CanError::Timeout) => {}
            Err(CanError::MalformedFrame(what)) => eprintln!("skipped: {what}"),
            Err(e) => {
                eprintln!("receive failed: {e}");
                break;
            }
        }
    }

    let _ = backend.close_channel().await;
    let stats = session.stats();
    session.finish()?;
    // "written" is only true when there is a file; without one it reads as a
    // promise of a capture that does not exist.
    let written = match out {
        Some(path) => format!(", {} written to {path}", stats.frames_kept),
        None => String::new(),
    };
    println!(
        "\nstopped after {:.1}s: {} frames seen{written}, {} messages reassembled, \
         {} incomplete, {} markers",
        started.elapsed().as_secs_f64(),
        stats.frames_seen,
        stats.pdus,
        stats.dropped,
        stats.markers
    );
    if stats.frames_seen > 0 && stats.pdus == 0 {
        // The usual case, and it looks like a failure until someone says it is
        // not: this port carries diagnostics on demand and nothing else.
        println!(
            "\nNo diagnostic conversation to show — the frames seen are the car's own \n\
             background traffic. This port only carries diagnostics while something is \n\
             asking: run VCDS, or another vagcan command from a second adapter, while \n\
             this is sniffing."
        );
    }
    if stats.frames_seen == 0 {
        println!(
            "\nNo traffic at all. On this platform the diagnostic line is nearly silent when \
             nothing is querying it, so that alone is not proof of a fault — but check the \
             ignition, OBD-II pin 6 → CAN-H / pin 14 → CAN-L, and the termination jumper being \
             OFF."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vag_capture::{parse_wall_clock_anchor, read_records};

    #[test]
    fn diag_ids_are_recognised_across_both_addressing_forms() {
        assert!(is_diag_id(0x7E0), "tester → engine");
        assert!(is_diag_id(0x7E8), "engine → tester");
        assert!(is_diag_id(0x7DF), "functional broadcast");
        assert!(is_diag_id(0x18DA_10F1 | CAN_EFF_FLAG), "normal fixed addressing");
        assert!(is_diag_id(0x17FC_0076 | CAN_EFF_FLAG), "MQB gateway pair");
        // Powertrain broadcast traffic is not diagnostics.
        assert!(!is_diag_id(0x0FD));
        assert!(!is_diag_id(0x3C0));
        assert!(!is_diag_id(0x18FE_F100 | CAN_EFF_FLAG));
    }

    #[test]
    fn direction_comes_from_the_id_not_from_who_received_it() {
        assert_eq!(arrow(0x7E0), "->");
        assert_eq!(arrow(0x7E8), "<-");
        assert_eq!(arrow(0x18DA_10F1 | CAN_EFF_FLAG), "<-", "…F1 = to the tester");
        assert_eq!(arrow(0x18DA_F110 | CAN_EFF_FLAG), "->", "…10 = to an ECU");
        assert_eq!(arrow(0x0FD), "  ", "not diagnostics, no direction claimed");
    }

    #[test]
    fn a_single_frame_request_prints_one_line() {
        let mut s: SniffSession<Vec<u8>> = SniffSession::new(None, 0, false).unwrap();
        let line = s
            .on_frame(0x7E0, &[0x03, 0x22, 0xF1, 0x90, 0, 0, 0, 0], 1_234_567)
            .unwrap()
            .expect("completed PDU");
        assert!(line.contains("7E0"), "{line}");
        assert!(line.contains("->"), "{line}");
        assert!(line.contains("22 F1 90"), "{line}");
        assert!(line.contains("1.235"), "timestamp in seconds: {line}");
    }

    #[test]
    fn a_multi_frame_response_prints_once_when_complete() {
        let mut s: SniffSession<Vec<u8>> = SniffSession::new(None, 0, false).unwrap();
        assert_eq!(s.on_frame(0x7E8, &[0x10, 0x11, 0x62, 0xF1, 0x90, 1, 2, 3], 0).unwrap(), None);
        assert_eq!(s.on_frame(0x7E8, &[0x21, 4, 5, 6, 7, 8, 9, 10], 1000).unwrap(), None);
        let line = s
            .on_frame(0x7E8, &[0x22, 11, 12, 13, 14, 0xAA, 0xAA, 0xAA], 2000)
            .unwrap()
            .expect("third frame completes it");

        assert!(line.contains("<-"), "{line}");
        assert!(line.contains("(17B, 3 frames)"), "{line}");
        assert_eq!(s.stats().pdus, 1);
        assert_eq!(s.stats().frames_seen, 3);
    }

    #[test]
    fn the_capture_opens_with_a_wall_clock_anchor_and_holds_every_frame() {
        let unix_us = 1_753_900_000_000_000u64;
        let mut s = SniffSession::new(Some(Vec::new()), unix_us, false).unwrap();
        s.on_frame(0x7E0, &[0x03, 0x22, 0xF1, 0x90, 0, 0, 0, 0], 10).unwrap();
        s.on_marker("engine started", 20).unwrap();
        s.on_frame(0x0FD, &[1, 2, 3], 30).unwrap();

        let buf = std::mem::take(&mut s.capture).unwrap();
        let records = read_records(&buf[..]).unwrap();
        assert_eq!(records.len(), 4, "anchor + 2 frames + 1 marker");

        let CapturePayload::Marker { note } = &records[0].payload else {
            panic!("the first record must be the anchor");
        };
        assert_eq!(parse_wall_clock_anchor(note), Some(unix_us));

        assert_eq!(
            records[1].payload,
            CapturePayload::CanFrame {
                id: CanId::Standard(0x7E0),
                data: vec![0x03, 0x22, 0xF1, 0x90, 0, 0, 0, 0],
            }
        );
        assert!(matches!(records[2].payload, CapturePayload::Marker { .. }));
        // Broadcast traffic is kept too: it is an independent crib.
        assert_eq!(
            records[3].payload,
            CapturePayload::CanFrame { id: CanId::Standard(0x0FD), data: vec![1, 2, 3] }
        );
        assert_eq!(s.stats().markers, 1, "the anchor is not an operator marker");
    }

    #[test]
    fn diag_only_drops_broadcast_traffic_from_the_file_as_well() {
        let mut s = SniffSession::new(Some(Vec::new()), 0, true).unwrap();
        s.on_frame(0x0FD, &[1, 2, 3], 10).unwrap();
        s.on_frame(0x3C0, &[1, 2, 3], 20).unwrap();
        s.on_frame(0x7E0, &[0x03, 0x22, 0xF1, 0x90, 0, 0, 0, 0], 30).unwrap();

        assert_eq!(s.stats().frames_seen, 3);
        assert_eq!(s.stats().frames_kept, 1);

        let buf = std::mem::take(&mut s.capture).unwrap();
        let records = read_records(&buf[..]).unwrap();
        assert_eq!(records.len(), 2, "anchor + the one diagnostic frame");
    }

    #[test]
    fn extended_ids_survive_the_round_trip_to_the_capture() {
        let raw = 0x18DA_10F1 | CAN_EFF_FLAG;
        let mut s = SniffSession::new(Some(Vec::new()), 0, true).unwrap();
        s.on_frame(raw, &[0x03, 0x62, 0xF1, 0x90, 0, 0, 0, 0], 10).unwrap();

        let buf = std::mem::take(&mut s.capture).unwrap();
        let records = read_records(&buf[..]).unwrap();
        assert_eq!(
            records[1].payload,
            CapturePayload::CanFrame {
                id: CanId::Extended(0x18DA_10F1),
                data: vec![0x03, 0x62, 0xF1, 0x90, 0, 0, 0, 0],
            },
            "the EFF flag must not leak into the stored id"
        );
    }

    #[test]
    fn dropped_assemblies_are_surfaced_in_the_stats() {
        // A capture with holes has to say so, or a gap reads as "the ECU never
        // answered".
        let mut s: SniffSession<Vec<u8>> = SniffSession::new(None, 0, false).unwrap();
        s.on_frame(0x7E8, &[0x10, 0x11, 0x62, 0xF1, 0x90, 1, 2, 3], 0).unwrap();
        s.on_frame(0x7E8, &[0x22, 4, 5, 6, 7, 8, 9, 10], 1000).unwrap(); // seq 1 missed
        assert_eq!(s.stats().dropped, 1);
        assert_eq!(s.stats().pdus, 0);
    }

    #[test]
    fn long_payloads_are_shortened_for_the_terminal_only() {
        let pdu = SnifferPdu { id: 0x7E8, data: (0..40).collect(), frames: 6 };
        let line = format_pdu(0, &pdu);
        assert!(line.contains('…'), "the display is cut: {line}");
        assert!(line.contains("(40B, 6 frames)"), "the true length is stated: {line}");
    }
}
