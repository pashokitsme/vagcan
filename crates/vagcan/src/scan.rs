//! `vagcan scan-dids` — ask an ECU what it will actually give us.
//!
//! VCDS only reads the identifiers its label files name. A sweep of the
//! `ReadDataByIdentifier` space asks the ECU directly, so it finds values no
//! label mentions — an independent crib next to the passive sniffer, and the
//! one source that does not depend on reversing the `.rod` field codec.
//!
//! Read-only by construction: the only service issued is `0x22`, which the UDS
//! client's allowlist already restricts us to.

use std::ops::RangeInclusive;
use std::time::Duration;

use vag_protocol::uds::UdsError;
use vag_protocol::AsyncUdsClient;
use vag_transport::AsyncIsoTpTransport;

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
/// engine ECU (`research/rod-labels.md` §4.0a/§4.0b), plus the standard
/// identification block. The default, because a full `0000-FFFF` sweep is
/// 65,536 requests — minutes at best, and most of it is refusals.
pub const DEFAULT_RANGES: &str = "7400-7500,A000-A100,F100-F200";

/// How many identifiers a range list covers.
pub fn total_dids(ranges: &[RangeInclusive<u16>]) -> usize {
    ranges.iter().map(|r| *r.end() as usize - *r.start() as usize + 1).sum()
}

/// Sweep `ranges`, calling `on_hit` for every identifier that answers.
///
/// `on_hit` is invoked as results arrive rather than at the end, so an
/// interrupted sweep keeps everything it found. A `TesterPresent` goes out
/// every `keepalive_every` identifiers to hold the session open through the
/// long stretches of refusals; pass `0` to disable it.
pub async fn scan_dids<T, F>(
    uds: &mut AsyncUdsClient<T>,
    ranges: &[RangeInclusive<u16>],
    delay: Duration,
    keepalive_every: usize,
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
            stats.asked += 1;
            match uds.read_data_by_identifier(did).await {
                Ok(data) => {
                    stats.hits += 1;
                    on_hit(&DidHit { did, data })?;
                }
                // A refusal is the normal answer for an identifier the ECU
                // does not implement — that is what the sweep is measuring.
                Err(UdsError::NegativeResponse { .. }) => stats.refused += 1,
                Err(_) => stats.failed += 1,
            }
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }
    }
    Ok(stats)
}

/// One report line for a hit: `A058  55 55` plus ASCII when the bytes look
/// like text (part numbers and component names are the common case).
pub fn format_hit(hit: &DidHit) -> String {
    let hex: Vec<String> = hit.data.iter().map(|b| format!("{b:02X}")).collect();
    // Only call it text when there is enough of it to mean something: part
    // numbers and component names run 10+ characters, while a two-byte
    // measurement like 0x5555 is "UU" by coincidence and printing that as a
    // string would invite reading a number as a name.
    let printable = hit.data.len() >= 4 && hit.data.iter().all(|&b| (0x20..0x7F).contains(&b));
    let text = if printable {
        format!("  \"{}\"", String::from_utf8_lossy(&hit.data))
    } else {
        String::new()
    };
    format!("{:04X}  {}{}", hit.did, hex.join(" "), text)
}

/// Sweep one control unit's identifiers against a real adapter (the `vagcan
/// scan` command).
pub async fn run(
    device_path: &str,
    baud: u32,
    ecu: u8,
    range: &str,
    out: Option<&str>,
    delay_ms: u64,
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use std::io::Write;
    use std::time::Instant;
    use vag_can::{IsoTpCan, SlcanBackend, SlcanBitrate};

    let ranges = parse_ranges(range).map_err(|e| anyhow::anyhow!("--range: {e}"))?;
    let total = total_dids(&ranges);

    let mut sink: Option<std::io::BufWriter<std::fs::File>> = match out {
        Some(path) => {
            let file = std::fs::File::create(path)
                .with_context(|| format!("creating results file {path:?}"))?;
            Some(std::io::BufWriter::new(file))
        }
        None => None,
    };

    let backend = SlcanBackend::open(device_path, baud, SlcanBitrate::Rate500k)
        .await
        .with_context(|| format!("opening the adapter at {device_path}"))?;
    let mut uds = AsyncUdsClient::new(IsoTpCan::for_ecu(backend, ecu));

    println!("scanning control unit {:02} — {total} identifiers ({range})\n", ecu + 1);
    let started = Instant::now();
    let stats = scan_dids(
        &mut uds,
        &ranges,
        Duration::from_millis(delay_ms),
        // Roughly a keep-alive every couple of seconds at the default pace.
        400,
        |hit: &DidHit| {
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
        },
    )
    .await?;
    if let Some(w) = sink.as_mut() {
        w.flush()?;
    }

    println!(
        "\n{} of {} identifiers answered ({} refused, {} unanswered) in {:.1}s",
        stats.hits,
        stats.asked,
        stats.refused,
        stats.failed,
        started.elapsed().as_secs_f64()
    );
    if stats.failed == stats.asked && stats.asked > 0 {
        println!(
            "\nNothing answered at all. Check the ignition, the wiring (OBD-II pin 6 → CAN-H, \
             pin 14 → CAN-L), the termination jumper being OFF, and that --ecu names a control \
             unit this car has."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vag_transport::MockAsyncTransport;

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

    #[test]
    fn ranges_parse_from_hex_spans() {
        assert_eq!(parse_ranges("7400-7402").unwrap(), vec![0x7400..=0x7402]);
        assert_eq!(
            parse_ranges("A058, F190-F19A").unwrap(),
            vec![0xA058..=0xA058, 0xF190..=0xF19A]
        );
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
        let stats = scan_dids(&mut uds, &[0xA058..=0xA05A], Duration::ZERO, 0, |hit| {
            hits.push(hit.clone());
            Ok(())
        })
        .await
        .unwrap();

        assert_eq!(stats.asked, 3);
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.refused, 1);
        assert_eq!(stats.failed, 0);
        assert_eq!(hits[0], DidHit { did: 0xA058, data: vec![0x55, 0x55] });
        assert_eq!(hits[1], DidHit { did: 0xA05A, data: vec![0x01] });
    }

    #[tokio::test]
    async fn hits_are_reported_as_they_arrive_not_at_the_end() {
        // The callback must see the first hit before the sweep reaches the
        // second, so an interrupted run keeps what it found.
        let script = vec![
            (req(0x0001), resp(0x0001, &[0xAA])),
            (req(0x0002), resp(0x0002, &[0xBB])),
        ];
        let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(script));

        let mut seen_at = Vec::new();
        let mut n = 0usize;
        scan_dids(&mut uds, &[0x0001..=0x0002], Duration::ZERO, 0, |hit| {
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

        let stats = scan_dids(&mut uds, &[0x0001..=0x0003], Duration::ZERO, 2, |_| Ok(()))
            .await
            .unwrap();

        assert_eq!(stats.asked, 3);
        assert_eq!(stats.hits, 2);
        assert!(uds.into_transport().is_exhausted(), "the scripted exchange ran exactly");
    }

    #[test]
    fn hits_print_as_hex_and_as_text_when_the_bytes_are_printable() {
        assert_eq!(
            format_hit(&DidHit { did: 0xA058, data: vec![0x55, 0x55] }),
            "A058  55 55",
        );
        // A part number reads as text — the common shape of an identity DID.
        assert_eq!(
            format_hit(&DidHit { did: 0xF187, data: b"8V0906264H".to_vec() }),
            "F187  38 56 30 39 30 36 32 36 34 48  \"8V0906264H\"",
        );
    }
}
