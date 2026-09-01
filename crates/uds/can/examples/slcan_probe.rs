//! Bench bring-up probe for an slcan adapter — **no CAN bus required**.
//!
//! Talks the LAWICEL command set directly over the serial port (deliberately
//! *not* through [`vag_uds_can::SlcanBackend::open`], which would immediately put
//! the channel in normal mode) so the adapter can be validated on the desk:
//! close the channel, ask for version/serial/status, set the bitrate, and open
//! in **listen-only** mode — none of which needs a transceiver to see traffic.
//!
//! Run:
//! ```text
//! cargo run -p vag-uds-can --features slcan --example slcan_probe -- /dev/cu.usbmodemXXXX
//! ```
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::SerialPortBuilderExt;

/// One command exchange: what to send, and what it means.
struct Step {
	cmd: String,
	what: String,
}

/// The default bench script, as `(command, meaning)` pairs.
const STEPS: &[(&str, &str)] = &[
	("C\r", "close channel (may BEL if already closed)"),
	("V\r", "hardware/software version"),
	("N\r", "serial number"),
	("F\r", "status flags"),
	("S6\r", "bitrate 500 kbit/s"),
	("L\r", "open LISTEN-ONLY (safe with no bus)"),
	("C\r", "close channel again"),
];

#[tokio::main(flavor = "current_thread")]
async fn main() {
	let mut args = std::env::args().skip(1);
	let path = args.next().unwrap_or_else(|| {
		eprintln!("usage: slcan_probe <serial-port> [cmd ...]");
		std::process::exit(2);
	});
	// Extra args override the default script: each is one command (CR added).
	let custom: Vec<String> = args.collect();

	let mut port = tokio_serial::new(&path, 115_200)
		.timeout(Duration::from_millis(200))
		.open_native_async()
		.unwrap_or_else(|e| {
			eprintln!("open {path} failed: {e}");
			std::process::exit(1);
		});
	println!("opened {path}\n");

	// Flush whatever the adapter had queued before the first command.
	drain(&mut port, Duration::from_millis(200)).await;

	let script: Vec<Step> = if custom.is_empty() {
		STEPS
			.iter()
			.map(|(c, w)| Step {
				cmd: c.to_string(),
				what: w.to_string(),
			})
			.collect()
	} else {
		custom
			.iter()
			.map(|c| Step {
				cmd: format!("{c}\r"),
				what: "custom".to_string(),
			})
			.collect()
	};

	let mut acked = 0usize;
	for step in &script {
		port.write_all(step.cmd.as_bytes()).await.expect("write");
		port.flush().await.expect("flush");
		let reply = drain(&mut port, Duration::from_millis(300)).await;
		let verdict = classify(&reply);
		if verdict.starts_with("ACK") {
			acked += 1;
		}
		println!("{:<5} {:<44} -> {:<28} {}", step.cmd.trim_end(), step.what, escape(&reply), verdict);
	}

	println!("\n{acked}/{} commands acknowledged", script.len());
	if acked >= 5 {
		println!("adapter speaks slcan — ready for the car");
	} else {
		println!("adapter did NOT answer as expected — check the firmware");
	}
}

/// Read everything the adapter sends until `quiet` passes with no new bytes.
async fn drain(port: &mut tokio_serial::SerialStream, quiet: Duration) -> Vec<u8> {
	let mut out = Vec::new();
	let mut chunk = [0u8; 256];
	while let Ok(Ok(n)) = tokio::time::timeout(quiet, port.read(&mut chunk)).await {
		if n == 0 {
			break;
		}
		out.extend_from_slice(&chunk[..n]);
	}
	out
}

/// slcan answers: `\r` = OK, `\x07` (BEL) = error, anything else = data + `\r`.
fn classify(reply: &[u8]) -> String {
	match reply.first() {
		None => "NO REPLY".to_string(),
		Some(0x07) => "BEL (command rejected)".to_string(),
		Some(b'\r') => "ACK".to_string(),
		Some(_) if reply.contains(&b'\r') => "ACK + data".to_string(),
		Some(_) => "data, no terminator".to_string(),
	}
}

/// Printable form of a raw reply.
fn escape(bytes: &[u8]) -> String {
	bytes
		.iter()
		.map(|&b| match b {
			b'\r' => "<CR>".to_string(),
			0x07 => "<BEL>".to_string(),
			0x20..=0x7e => (b as char).to_string(),
			_ => format!("<{b:02X}>"),
		})
		.collect()
}
