//! A Ctrl-C closes the link a bus owns even while the command's thread is blocked.
//!
//! `measure` holds a bus across its questions, and a question blocks its thread in a read:
//! a command blocked there is never dropped, so its handles cannot close the link. The bus
//! task does it itself, on its own thread, the moment the process is ending — which is what
//! takes a `--slcan` dash board out of adapter mode before the exit (review of PR #48).
//!
//! **Its own test binary, on purpose.** Ending is a process-wide flag that stays set: in a
//! binary shared with other tests it would stop every other bus there.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use vag_cli_core::bus::{Budget, Bus, Class, Unit};
use vag_uds_transport::{AsyncIsoTpTransport, CanId, TransportError};

/// A link that never answers and says when it is dropped: what the slcan backend's close
/// command hangs off.
struct Link {
	closed: Arc<AtomicBool>,
}

impl Drop for Link {
	fn drop(&mut self) {
		self.closed.store(true, Ordering::SeqCst);
	}
}

impl vag_uds_can::UnitLink for Link {
	type Channel = Link;

	fn to_unit(self, _request: CanId, _response: CanId) -> Link {
		self
	}

	fn release(channel: Link) -> Link {
		channel
	}
}

impl AsyncIsoTpTransport for Link {
	async fn send(&mut self, _pdu: &[u8]) -> Result<(), TransportError> {
		Ok(())
	}

	async fn recv(&mut self, _timeout: Duration) -> Result<Vec<u8>, TransportError> {
		std::future::pending().await
	}
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ctrl_c_closes_the_link_while_the_command_is_blocked() {
	let closed = Arc::new(AtomicBool::new(false));
	let bus = Bus::start(Link { closed: Arc::clone(&closed) }, Budget::default());
	// A read under way, so the task is in an exchange rather than idle when the Ctrl-C
	// comes; it is never answered.
	let reading = {
		let bus = bus.clone();
		let engine = Unit {
			request: 0x7E0,
			response: 0x7E8,
		};
		tokio::spawn(async move { bus.read_once(Class::Foreground, engine, 0xF190).await })
	};
	tokio::time::sleep(Duration::from_millis(100)).await;
	assert!(!closed.load(Ordering::SeqCst), "closed before the Ctrl-C");
	vag_cli_core::interrupt::end_for_test();
	// This thread blocks the way a question's read does: no await, the handle held.
	let started = Instant::now();
	while !closed.load(Ordering::SeqCst) && started.elapsed() < Duration::from_secs(2) {
		std::thread::sleep(Duration::from_millis(5));
	}
	assert!(closed.load(Ordering::SeqCst), "the bus task did not close its link");
	drop(bus);
	reading.abort();
}
