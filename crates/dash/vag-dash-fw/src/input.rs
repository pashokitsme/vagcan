//! Every input's commands, on their way to the one task that applies them (`todo/dash/19`,
//! "Input backends").
//!
//! The `[[button]]` pins are polled and `dashsim`'s presses taken by the input task, and the
//! cruise lever is read by the bus task; each turns what it read into a [`Command`]
//! ([`vag_dash_render::control`]) and offers it here. One task takes the commands in order
//! and applies each to the screen and the settings — the locks, the page cursor, the log
//! line, the state push — so no input waits on a lock, and the bus task never holds an
//! exchange up behind a flash write that holds the settings.
//!
//! **Bounded, and full means dropped.** At most [`CAPACITY`] commands wait. One that finds
//! the queue full is thrown away, and [`CommandQueue::offer`] says so for the caller to log. A
//! person presses a few times a second at most, so a full queue means the consumer is held
//! up — a flash write has the settings — and a page turn queued behind that would land
//! seconds late, which is worse than one that never happened. The newest is the one
//! dropped: those already waiting were pressed first.
//!
//! Nothing here touches hardware, so it is compiled on the host as well
//! (`research/dash/host/tests/input_queue.rs`): this crate cannot be built there.

use core::fmt;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use vag_dash_render::control::Command;

/// How many commands may wait for the consumer.
pub const CAPACITY: usize = 4;

/// Which input gave a command — for the log line only: the screen never sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
	/// `dashsim`'s press, over the cable.
	Sim,
	/// A `[[button]]`, by its GPIO.
	Pin(u8),
	/// The cruise lever.
	Lever,
}

impl fmt::Display for Source {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Source::Sim => f.write_str("dashsim"),
			Source::Pin(pin) => write!(f, "GPIO{pin}"),
			Source::Lever => f.write_str("lever"),
		}
	}
}

/// The queue between every input and the one consumer.
pub struct CommandQueue {
	queue: Channel<CriticalSectionRawMutex, (Source, Command), CAPACITY>,
}

impl CommandQueue {
	pub const fn new() -> Self {
		CommandQueue { queue: Channel::new() }
	}

	/// Queue a command without waiting. `false` when the queue was full and the command was
	/// dropped.
	pub fn offer(&self, source: Source, command: Command) -> bool {
		self.queue.try_send((source, command)).is_ok()
	}

	/// The next command, in the order they were offered.
	pub async fn next(&self) -> (Source, Command) {
		self.queue.receive().await
	}
}

impl Default for CommandQueue {
	fn default() -> Self {
		Self::new()
	}
}
