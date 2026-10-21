//! A runtime dropped right after its last bus handle goes does not panic.
//!
//! A bus task runs on a blocking-pool thread, and dropping a multi-thread runtime shuts its
//! timers down before it joins that pool: a task still between its inbox and its next timer
//! then polls a timer that is shutting down, and panics — "A Tokio 1.x context was found, but
//! it is being shutdown", at the end of `watch --for` in CI's e2e. `bus::closed` waits the
//! tasks out first; `vagcan`'s `main` calls it before its runtime goes.
//!
//! **Its own test binary, on purpose.** `closed` waits for every bus in the process, and a
//! panic hook is process-wide: neither may see another test's buses.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use vag_cli_core::bus::{Budget, Bus, Class, Unit};
use vag_uds_transport::{AsyncIsoTpTransport, CanId, TransportError};

/// Panics a runtime shutting down caused, from any thread.
static SHUTDOWN_PANICS: AtomicUsize = AtomicUsize::new(0);

/// A link that is always busy with a timer: every answer is waited for in 1 ms sleeps
/// that never end, so the task is on a timer whenever it is not on its inbox.
struct Busy;

impl vag_uds_can::UnitLink for Busy {
	type Channel = Busy;

	fn to_unit(self, _request: CanId, _response: CanId) -> Busy {
		self
	}

	fn release(channel: Busy) -> Busy {
		channel
	}
}

impl AsyncIsoTpTransport for Busy {
	async fn send(&mut self, _pdu: &[u8]) -> Result<(), TransportError> {
		Ok(())
	}

	async fn recv(&mut self, _timeout: Duration) -> Result<Vec<u8>, TransportError> {
		loop {
			tokio::time::sleep(Duration::from_millis(1)).await;
		}
	}
}

const ENGINE: Unit = Unit {
	request: 0x7E0,
	response: 0x7E8,
};

/// One process's life in small: a runtime, a bus with a read out, the command dropping its
/// handles, then — as `main` does — the bus tasks waited out before the runtime drops.
fn one_run() {
	let runtime = tokio::runtime::Builder::new_multi_thread()
		.worker_threads(2)
		.enable_all()
		.build()
		.unwrap();
	runtime.block_on(async {
		let bus = Bus::start(Busy, Budget::default());
		let reader = bus.clone();
		let reading = tokio::spawn(async move { reader.read_once(Class::Foreground, ENGINE, 0xF190).await });
		tokio::time::sleep(Duration::from_millis(5)).await;
		reading.abort();
		let _ = reading.await;
		drop(bus);
		assert!(vag_cli_core::bus::closed(Duration::from_secs(2)).await, "a bus task outlived its handles");
	});
	drop(runtime);
}

#[test]
fn a_runtime_dropped_after_its_buses_are_waited_out_does_not_panic() {
	let previous = std::panic::take_hook();
	std::panic::set_hook(Box::new(move |info| {
		if info.to_string().contains("being shutdown") {
			SHUTDOWN_PANICS.fetch_add(1, Ordering::SeqCst);
		} else {
			previous(info);
		}
	}));
	for _ in 0..200 {
		one_run();
	}
	assert_eq!(SHUTDOWN_PANICS.load(Ordering::SeqCst), 0, "timers polled on a runtime shutting down");
}
