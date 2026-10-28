//! Every advertisement heard from one board, on a clock: `bleadv [name] [seconds]`. Prints each
//! gap longer than a second and a summary — whether a board that a scan misses is gone from the
//! air or only unheard. Connects to nothing.

use anyhow::Result;
use btleplug::api::{Central, CentralEvent, Peripheral as _, ScanFilter};
use futures::StreamExt;
use std::time::Duration;
use tokio::time::Instant;

/// A gap worth a line: the board advertises every 160 ms.
const GAP: Duration = Duration::from_secs(1);

#[tokio::main]
async fn main() -> Result<()> {
	let mut args = std::env::args().skip(1);
	let name = args.next().unwrap_or_else(|| "vagcan-dash".into());
	let seconds: u64 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(120);

	let adapter = vag_dash_ble::adapter().await?;
	let mut events = adapter.events().await?;
	adapter.start_scan(ScanFilter::default()).await?;
	let start = Instant::now();
	let end = start + Duration::from_secs(seconds);
	println!("listening for {name} for {seconds} s; gaps over {} s:", GAP.as_secs());

	let mut heard = 0usize;
	let mut last: Option<Instant> = None;
	let mut longest = Duration::ZERO;
	while let Ok(Some(event)) = tokio::time::timeout_at(end, events.next()).await {
		let (CentralEvent::DeviceDiscovered(id) | CentralEvent::DeviceUpdated(id)) = event else {
			continue;
		};
		let Ok(peripheral) = adapter.peripheral(&id).await else { continue };
		let Ok(Some(props)) = peripheral.properties().await else { continue };
		if props.local_name.as_deref() != Some(name.as_str()) {
			continue;
		}
		let now = Instant::now();
		let gap = now - last.unwrap_or(start);
		if gap > GAP {
			println!(
				"{:7.2} s  heard after {:5.2} s  rssi {:?}",
				(now - start).as_secs_f64(),
				gap.as_secs_f64(),
				props.rssi
			);
		}
		longest = longest.max(gap);
		heard += 1;
		last = Some(now);
	}
	adapter.stop_scan().await.ok();
	let tail = end - last.unwrap_or(start);
	longest = longest.max(tail);
	println!(
		"heard {heard} times in {seconds} s · longest gap {:.2} s · silent at the end {:.2} s",
		longest.as_secs_f64(),
		tail.as_secs_f64()
	);
	Ok(())
}
