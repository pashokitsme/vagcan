//! One framed UDS request, or one subscription, to the dash over BLE — a bench
//! check of the board's side of `todo/dash/16`, not the product transport.
//!
//! ```text
//! bleuds 7E0 7E8 22F190                         # one Request, print the Answer
//! bleuds --subscribe 7E0 7E8 F40D 100 5         # read F40D every 100 ms for 5 s
//! bleuds --timing --subscribe 7E0 7E8 F40D 20 5 # the same as a timing subscription
//! bleuds --sweep-response 7E0 7E8 50 F40D 100   # 50 response ids under one request id
//! ```
//!
//! `--timing` marks the subscription timing (`link::Priority::Timing`): the board's
//! planner puts it ahead of the host's other work, and the board allows one at a time.
//!
//! The sweep is the heap attack the board's guard refuses: one request id under many
//! response ids, one subscription each, in one connection. All but the first must come
//! back refused.
//!
//! Connects to the first device named `vagcan-dash` (or `--name <name>`), sends
//! `vag_uds_transport::link` frames on the Nordic UART Service, and prints what
//! comes back. Text the board pushes on the same service (its `state` line) is
//! printed as text.

use anyhow::{Context, Result, bail};
use btleplug::api::{Central, CharPropFlags, Peripheral as _, ScanFilter, WriteType};
use btleplug::platform::{Adapter, Peripheral};
use futures::StreamExt;
use std::time::{Duration, Instant};
use vag_dash_ble::{hex, open_nus};
use vag_uds_transport::link::{self, Message, Outcome, Piece, Priority, Reassembler, Request, Subscribe};

const DEFAULT_NAME: &str = "vagcan-dash";
/// How long to look for the board before giving up.
const SCAN_SECS: u64 = 20;
/// Bytes per write. ATT's default payload, which every central allows before any
/// MTU exchange; the frames this tool sends are a few bytes, so nothing is lost
/// by not asking macOS what it negotiated (btleplug does not say).
const WRITE_CHUNK: usize = 20;
/// How long to wait for an Answer. The board may hold a request behind its rate
/// cap (10 s window) or a unit's response-pending (10 s).
const ANSWER_WAIT: Duration = Duration::from_secs(25);
/// How long `--sweep-response` listens, counted from its first subscription: the 50
/// writes take a few seconds at one ATT round trip each, and the rest is for the
/// accepted subscription's readings.
const SWEEP_LISTEN: Duration = Duration::from_secs(10);

enum Mode {
	Request {
		request: u16,
		response: u16,
		pdu: Vec<u8>,
	},
	Subscribe {
		request: u16,
		response: u16,
		did: u16,
		period_ms: u16,
		seconds: u64,
		priority: Priority,
	},
	/// One request id under `count` response ids, one subscription each, in one
	/// connection: the board must refuse all but the first and hold no more for them.
	SweepResponse {
		request: u16,
		first_response: u16,
		count: u16,
		did: u16,
		period_ms: u16,
	},
}

fn usage() -> ! {
	eprintln!("usage: bleuds [--name NAME] <request id> <response id> <hex pdu>");
	eprintln!("       bleuds [--name NAME] [--timing] --subscribe <request id> <response id> <did> <period ms> <seconds>");
	eprintln!("       bleuds [--name NAME] --sweep-response <request id> <first response id> <count> <did> <period ms>");
	std::process::exit(2);
}

fn hex_u16(text: &str) -> Result<u16> {
	u16::from_str_radix(text.trim_start_matches("0x"), 16).with_context(|| format!("{text:?} is not a hex number"))
}

fn hex_bytes(text: &str) -> Result<Vec<u8>> {
	let digits: String = text.chars().filter(|c| !c.is_whitespace()).collect();
	if digits.is_empty() || !digits.len().is_multiple_of(2) {
		bail!("{text:?} is not whole hex bytes");
	}
	(0..digits.len())
		.step_by(2)
		.map(|i| u8::from_str_radix(&digits[i..i + 2], 16).with_context(|| format!("{text:?} is not hex")))
		.collect()
}

fn parse(args: &[String]) -> Result<(String, Mode)> {
	let mut args = args.to_vec();
	let mut name = DEFAULT_NAME.to_string();
	if let Some(at) = args.iter().position(|a| a == "--name") {
		if at + 1 >= args.len() {
			usage();
		}
		name = args.remove(at + 1);
		args.remove(at);
	}
	let priority = match args.iter().position(|a| a == "--timing") {
		Some(at) => {
			args.remove(at);
			Priority::Timing
		}
		None => Priority::Normal,
	};
	let mode = match args.as_slice() {
		[flag, request, response, did, period, seconds] if flag == "--subscribe" => Mode::Subscribe {
			request: hex_u16(request)?,
			response: hex_u16(response)?,
			did: hex_u16(did)?,
			period_ms: period.parse().context("period is milliseconds")?,
			seconds: seconds.parse().context("seconds is a number")?,
			priority,
		},
		_ if priority == Priority::Timing => usage(),
		[flag, request, first, count, did, period] if flag == "--sweep-response" => Mode::SweepResponse {
			request: hex_u16(request)?,
			first_response: hex_u16(first)?,
			count: count.parse().context("count is a number")?,
			did: hex_u16(did)?,
			period_ms: period.parse().context("period is milliseconds")?,
		},
		[request, response, pdu] => Mode::Request {
			request: hex_u16(request)?,
			response: hex_u16(response)?,
			pdu: hex_bytes(pdu)?,
		},
		_ => usage(),
	};
	Ok((name, mode))
}

#[tokio::main]
async fn main() -> Result<()> {
	let args: Vec<String> = std::env::args().skip(1).collect();
	let (name, mode) = parse(&args)?;

	let adapter = vag_dash_ble::adapter().await?;
	let board = find(&adapter, &name).await?;
	let started = Instant::now();
	let (rx, tx) = open_nus(&board).await?;
	println!("connected to {name} in {} ms", started.elapsed().as_millis());
	if !tx.properties.contains(CharPropFlags::NOTIFY) {
		bail!("the board's TX characteristic cannot notify");
	}
	board.subscribe(&tx).await?;
	let mut notifications = board.notifications().await?;
	let mut reassembler = Reassembler::new();

	let send = async |message: &Message| -> Result<()> {
		let frame = link::encode(message)?;
		for chunk in link::chunks(&frame, WRITE_CHUNK) {
			board.write(&rx, chunk, WriteType::WithResponse).await?;
		}
		Ok(())
	};

	let result: Result<()> = async {
		match mode {
			Mode::Request { request, response, pdu } => {
				println!("> Request {request:03X}/{response:03X} {}", hex(&pdu));
				let sent = Instant::now();
				send(&Message::Request(Request {
					seq: 1,
					request_id: request,
					response_id: response,
					pdu,
				}))
				.await?;
				let deadline = tokio::time::sleep(ANSWER_WAIT);
				tokio::pin!(deadline);
				loop {
					tokio::select! {
						() = &mut deadline => bail!("no Answer in {} s", ANSWER_WAIT.as_secs()),
						n = notifications.next() => {
							let Some(n) = n else { bail!("the board went away") };
							for piece in reassembler.push(&n.value) {
								match piece {
									Piece::Message(Message::Answer(a)) => {
										println!("< Answer seq {} after {} ms: {}", a.seq, sent.elapsed().as_millis(), outcome(&a.outcome));
										return Ok(());
									}
									other => show(other),
								}
							}
						}
					}
				}
			}
			Mode::Subscribe {
				request,
				response,
				did,
				period_ms,
				seconds,
				priority,
			} => {
				const SUB: u16 = 1;
				println!("> Subscribe {request:03X}/{response:03X} {did:04X} every {period_ms} ms for {seconds} s, {priority:?}");
				send(&Message::Subscribe(Subscribe {
					sub: SUB,
					request_id: request,
					response_id: response,
					did,
					period_ms,
					priority,
				}))
				.await?;
				let (mut count, mut first, mut last) = (0u32, None, 0u32);
				let end = tokio::time::sleep(Duration::from_secs(seconds));
				tokio::pin!(end);
				loop {
					tokio::select! {
						() = &mut end => break,
						n = notifications.next() => {
							let Some(n) = n else { bail!("the board went away") };
							for piece in reassembler.push(&n.value) {
								match piece {
									Piece::Message(Message::Reading(r)) => {
										count += 1;
										first.get_or_insert(r.at_ms);
										last = r.at_ms;
										println!("< Reading sub {} at {} ms: {}", r.sub, r.at_ms, outcome(&r.outcome));
									}
									other => show(other),
								}
							}
						}
					}
				}
				send(&Message::Unsubscribe { sub: SUB }).await?;
				report_rate(count, first, last);
				Ok(())
			}
			Mode::SweepResponse {
				request,
				first_response,
				count,
				did,
				period_ms,
			} => {
				println!("> Subscribe {request:03X} under {count} response ids from {first_response:03X}, {did:04X} every {period_ms} ms");
				// Notifications are read while the subscriptions go out, not after:
				// btleplug hands them over through a bounded broadcast channel, and a
				// reader that waits for its own writes to finish loses what the board
				// answered meanwhile (the first run counted 17 of 49 refusals that way).
				let sending = async {
					for n in 0..count {
						send(&Message::Subscribe(Subscribe {
							sub: n + 1,
							request_id: request,
							response_id: first_response.wrapping_add(n) & 0x7FF,
							did,
							period_ms,
							priority: Priority::Normal,
						}))
						.await?;
					}
					anyhow::Ok(())
				};
				let (mut refused, mut readings) = (0u32, 0u32);
				let mut first_refusal = None;
				let collecting = async {
					let end = tokio::time::sleep(SWEEP_LISTEN);
					tokio::pin!(end);
					loop {
						tokio::select! {
							() = &mut end => break,
							n = notifications.next() => {
								let Some(n) = n else { bail!("the board went away") };
								for piece in reassembler.push(&n.value) {
									match piece {
										Piece::Message(Message::Reading(r)) => match r.outcome {
											Outcome::Refused(why) => {
												refused += 1;
												first_refusal.get_or_insert(why);
											}
											_ => readings += 1,
										},
										other => show(other),
									}
								}
							}
						}
					}
					anyhow::Ok(())
				};
				let (sent, collected) = tokio::join!(sending, collecting);
				sent?;
				collected?;
				for n in 0..count {
					send(&Message::Unsubscribe { sub: n + 1 }).await?;
				}
				println!("{refused} subscription(s) refused, {readings} reading(s) for the one accepted");
				if let Some(why) = first_refusal {
					println!("  first refusal: {why}");
				}
				Ok(())
			}
		}
	}
	.await;
	board.unsubscribe(&tx).await.ok();
	board.disconnect().await.ok();
	println!("disconnected");
	result
}

/// The rate of a subscription on the board's clock.
fn report_rate(count: u32, first: Option<u32>, last: u32) {
	match first {
		Some(first) if count > 1 => println!(
			"{count} readings over {} ms of board time: {:.1} Hz",
			last - first,
			f64::from(count - 1) * 1000.0 / f64::from((last - first).max(1))
		),
		_ => println!("{count} reading(s)"),
	}
}

/// Scan until a device with `name` shows up.
async fn find(adapter: &Adapter, name: &str) -> Result<Peripheral> {
	adapter.start_scan(ScanFilter::default()).await?;
	let started = Instant::now();
	let found = loop {
		if started.elapsed() > Duration::from_secs(SCAN_SECS) {
			break None;
		}
		let mut hit = None;
		for peripheral in adapter.peripherals().await? {
			// By name only: a device that speaks NUS is not necessarily the dash.
			let props = peripheral.properties().await?;
			if props.and_then(|p| p.local_name).as_deref() == Some(name) {
				hit = Some(peripheral);
				break;
			}
		}
		if hit.is_some() {
			break hit;
		}
		tokio::time::sleep(Duration::from_millis(250)).await;
	};
	adapter.stop_scan().await.ok();
	match found {
		Some(p) => {
			println!("found {name} after {} ms of scanning", started.elapsed().as_millis());
			Ok(p)
		}
		None => bail!("no {name} on the air in {SCAN_SECS} s"),
	}
}

fn outcome(outcome: &Outcome) -> String {
	match outcome {
		Outcome::Pdu(pdu) => format!("PDU {}", hex(pdu)),
		Outcome::NoAnswer => "NoAnswer".to_string(),
		Outcome::Refused(why) => format!("Refused: {why}"),
		Outcome::BusError(why) => format!("BusError: {why}"),
	}
}

fn show(piece: Piece) {
	match piece {
		Piece::Text(text) => println!("< text {:?}", String::from_utf8_lossy(&text)),
		Piece::Message(other) => println!("< {other:?}"),
		Piece::Error(e) => println!("< a frame that did not decode: {e}"),
	}
}
