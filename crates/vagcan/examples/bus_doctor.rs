//! Find out why a CAN bus looks dead: sweep bitrates × addressing schemes.
//!
//! On MQB the OBD-II diagnostic line carries no periodic broadcast, so
//! listening proves nothing on its own — a correctly wired bus and a
//! disconnected one look identical. This probe therefore *asks*: at every
//! plausible bitrate it listens briefly, then sends a `ReadDataByIdentifier`
//! for the VIN both physically (`0x7E0`) and functionally (`0x7DF`, which every
//! ECU on the bus must answer), and reports whatever comes back.
//!
//! Read-only: the only service issued is UDS `0x22`.
//!
//! ```text
//! cargo run -p vagcan --example bus_doctor -- /dev/cu.usbmodemXXXX
//! ```
use std::time::Duration;

use vag_can::{CanBackend, IsoTpCan, SlcanBackend, SlcanBitrate, SlcanMode};
use vag_protocol::AsyncUdsClient;
use vag_transport::CanId;

/// Bitrates worth trying on a VAG car, most likely first.
const RATES: &[(&str, SlcanBitrate)] = &[
    ("500k", SlcanBitrate::Rate500k),
    ("125k", SlcanBitrate::Rate125k),
    ("100k", SlcanBitrate::Rate100k),
    ("250k", SlcanBitrate::Rate250k),
    ("1M", SlcanBitrate::Rate1m),
];

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: bus_doctor <serial-port>");
        std::process::exit(2);
    });

    let mut any_traffic = false;
    let mut any_reply = false;

    for (name, rate) in RATES {
        println!("\n=== {name} ===");

        // 1. Listen. Traffic at a given bitrate is proof of both the wiring and
        //    the rate; silence is inconclusive on a diagnostic-only line.
        match SlcanBackend::open_mode(&path, 115_200, *rate, SlcanMode::Silent).await {
            Ok(mut backend) => {
                let mut seen = 0usize;
                let deadline = std::time::Instant::now() + Duration::from_millis(1500);
                while std::time::Instant::now() < deadline {
                    match backend.recv_frame(Duration::from_millis(300)).await {
                        Ok((id, data)) => {
                            if seen < 3 {
                                println!("  heard {id:03X}  {data:02X?}");
                            }
                            seen += 1;
                        }
                        Err(_) => break,
                    }
                }
                let _ = backend.close_channel().await;
                if seen > 0 {
                    any_traffic = true;
                    println!("  listen: {seen} frames — the bus runs at this rate");
                } else {
                    println!("  listen: silence (expected on a diagnostic-only line)");
                }
            }
            Err(e) => {
                println!("  cannot open the adapter: {e}");
                continue;
            }
        }

        // 2. Ask. Physical addressing first, then the functional broadcast that
        //    every ECU is required to answer.
        for (label, tx, rx) in [
            ("physical 7E0/7E8", 0x7E0u16, 0x7E8u16),
            ("functional 7DF/7E8", 0x7DF, 0x7E8),
        ] {
            let backend = match SlcanBackend::open_mode(&path, 115_200, *rate, SlcanMode::Normal).await
            {
                Ok(b) => b,
                Err(e) => {
                    println!("  {label}: cannot open: {e}");
                    continue;
                }
            };
            let channel =
                IsoTpCan::new(backend, CanId::Standard(tx), CanId::Standard(rx));
            let mut uds = AsyncUdsClient::new(channel);
            match uds.read_data_by_identifier(0xF190).await {
                Ok(data) => {
                    any_reply = true;
                    println!(
                        "  {label}: ANSWERED — VIN {:?}",
                        String::from_utf8_lossy(&data)
                    );
                }
                Err(e) => println!("  {label}: {e}"),
            }
            let mut backend = uds.into_transport().into_backend();
            let _ = backend.close_channel().await;
        }
    }

    println!("\n---");
    match (any_traffic, any_reply) {
        (_, true) => println!("The car answered. Note the bitrate and addressing that worked."),
        (true, false) => println!(
            "Frames were heard but nothing answered a request — the adapter is on a live bus \
             whose ECUs are not at these addresses, or transmission is not reaching them."
        ),
        (false, false) => println!(
            "Nothing heard and nothing answered at any bitrate. That points at the physical \
             connection rather than at software:\n  \
             - measure OBD-II pin 6 to pin 14 with the ignition OFF and the adapter unplugged: \
             ~60 Ω means the CAN pair really is on those pins\n  \
             - check the 120R jumper is OPEN\n  \
             - check each wire is clamped on bare copper, not on insulation\n  \
             - try swapping CAN-H and CAN-L: a reversed pair behaves exactly like this"
        ),
    }
}
