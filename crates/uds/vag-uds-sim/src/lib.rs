//! A virtual car on a pseudo-terminal: the far side of an slcan USB-CAN adapter, with
//! control units behind it that answer from a profile (`docs/virtual-car.md`).
//!
//! The tester runs its whole real path against it — the serial port opened through
//! `--device`, slcan, ISO-TP with flow control and long transfers, the UDS client with its
//! allowlist, the scheduler — and cannot tell it from a car. Host only, and never on a bus:
//! the only port it has is the pseudo-terminal it creates.
//!
//! Layers, each a state machine over a clock it is handed, so each is tested without a
//! port or a sleep:
//!
//! * [`profile`] — the car as written, checked and resolved;
//! * [`server`] — what a unit answers to one whole request;
//! * [`bus`] — ISO-TP, the car's side, every unit on its ids;
//! * [`adapter`] — slcan, the adapter's side;
//! * [`pty`] — the port, and the loop that serves it.
//!
//! [`capture`] builds a profile from a capture of a real conversation. [`Sim`] starts one in
//! a thread, for a harness that wants a car in-process.

pub mod adapter;
pub mod bus;
pub mod capture;
pub mod profile;
#[cfg(unix)]
pub mod pty;
pub mod server;

pub use profile::{Car, Profile, ProfileError};

/// A car served on its own thread until dropped.
#[cfg(unix)]
pub struct Sim {
	path: std::path::PathBuf,
	stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
	thread: Option<std::thread::JoinHandle<std::io::Result<()>>>,
}

#[cfg(unix)]
impl Sim {
	/// Open a pseudo-terminal and serve `car` on it. `notes` hears what happens, on the
	/// sim's thread.
	pub fn start(car: Car, notes: impl FnMut(pty::Notice) + Send + 'static) -> std::io::Result<Sim> {
		let port = pty::Pty::open()?;
		let path = port.path().to_path_buf();
		let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
		let thread = {
			let stop = stop.clone();
			std::thread::Builder::new().name("vag-uds-sim".into()).spawn(move || {
				let mut adapter = adapter::Adapter::new(bus::Bus::new(car));
				pty::serve(&port, &mut adapter, &stop, notes)
			})?
		};
		Ok(Sim {
			path,
			stop,
			thread: Some(thread),
		})
	}

	/// The port to give the tester: `--device <path>`.
	pub fn path(&self) -> &std::path::Path {
		&self.path
	}

	/// Stop serving, and say how the serving ended.
	pub fn stop(mut self) -> std::io::Result<()> {
		self.halt()
	}

	fn halt(&mut self) -> std::io::Result<()> {
		self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
		match self.thread.take().map(|t| t.join()) {
			Some(Ok(result)) => result,
			Some(Err(_)) => Err(std::io::Error::other("the sim's thread panicked")),
			None => Ok(()),
		}
	}

	/// Serve until the port fails, on this thread: what `vagcan dev sim` runs.
	pub fn wait(mut self) -> std::io::Result<()> {
		match self.thread.take().map(|t| t.join()) {
			Some(Ok(result)) => result,
			Some(Err(_)) => Err(std::io::Error::other("the sim's thread panicked")),
			None => Ok(()),
		}
	}
}

#[cfg(unix)]
impl Drop for Sim {
	fn drop(&mut self) {
		let _ = self.halt();
	}
}
