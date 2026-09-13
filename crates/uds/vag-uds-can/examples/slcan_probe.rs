//! Bench bring-up probe for an slcan adapter — **no CAN bus required**.
//!
//! Talks the LAWICEL command set directly over the serial port (deliberately
//! *not* through [`vag_uds_can::SlcanBackend::open`], which would immediately put
//! the channel in normal mode) so the adapter can be validated on the desk:
//! close the channel, ask for version/serial/status, set the bitrate, and open
//! in **listen-only** mode — none of which needs a transceiver to see traffic.
//!
//! Custom commands may follow the port, but never a transmit (`t`, `T`, `r`,
//! `R`, or CAN FD's `d`, `D`, `b`, `B`): this probe writes to the adapter directly, so a frame typed here would
//! reach the bus without passing the UDS allowlist. It refuses them before the
//! port is opened.
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
	// Nothing that transmits is among them — see [`refusal`].
	let custom: Vec<String> = args.collect();
	if let Some(why) = custom.iter().find_map(|c| refusal(c)) {
		eprintln!("{why}");
		std::process::exit(2);
	}

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

/// Why a custom command may not be sent, or `None` when it may.
///
/// `t`/`T` put a frame on the bus, `r`/`R` a remote frame, and `d`/`D`/`b`/`B`
/// a CAN FD frame (the CANable 2 firmware takes all eight). This probe writes
/// straight to the port, past `vag-uds-client`'s allowlist, so a frame typed
/// here is whatever the typist wrote — an ECUReset as easily as a read. The
/// probe is for asking an adapter about itself; frames go through `vagcan`.
/// A command is checked line by line, because one argument can carry several
/// (`$'O\rt7E0…'` in a shell is two commands to the adapter).
fn refusal(cmd: &str) -> Option<String> {
	/// Every slcan command that puts a frame on the bus: classic data and remote
	/// frames, then CAN FD without and with bit-rate switch.
	const TRANSMITS: [char; 8] = ['t', 'T', 'r', 'R', 'd', 'D', 'b', 'B'];
	cmd
		.split(['\r', '\n'])
		.map(str::trim_start)
		.find(|line| line.starts_with(TRANSMITS))
		.map(|line| {
			format!(
				"refusing {line:?}: `t`/`T`/`r`/`R`/`d`/`D`/`b`/`B` transmit a frame, and this probe bypasses the UDS allowlist. \
				 It asks an adapter about itself; put frames on a bus through `vagcan`."
			)
		})
}

#[cfg(test)]
mod tests {
	use super::refusal;

	#[test]
	fn transmit_commands_are_refused() {
		// `t7E0021101` is an ECUReset request: bytes on the bus that the
		// allowlist never sees, because nothing here goes through the client.
		for cmd in ["t7E0021101", "T18DA10F1021101", "r7E00", "R18DA10F10", " t7E0021101", "O\rt7E0021101"] {
			assert!(refusal(cmd).is_some(), "sent {cmd:?}");
		}
		// CAN FD frames: the CANable 2 firmware transmits `d`/`D` (FD) and
		// `b`/`B` (FD with bit-rate switch) as readily as `t`/`T`.
		for cmd in ["d7E0021101", "D18DA10F1021101", "b7E0021101", "B18DA10F1021101", "O\rd7E0021101"] {
			assert!(refusal(cmd).is_some(), "sent {cmd:?}");
		}
	}

	#[test]
	fn setup_and_status_commands_still_go_through() {
		for cmd in ["C", "V", "N", "F", "S6", "M1", "L", "O", "Z0"] {
			assert!(refusal(cmd).is_none(), "refused {cmd:?}");
		}
	}
}
