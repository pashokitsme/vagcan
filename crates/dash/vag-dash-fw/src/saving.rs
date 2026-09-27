//! What the settings in RAM are against what flash holds, and what a run the stopwatch kept
//! at speed does at the next standstill (`todo/dash/19`).
//!
//! Pure on purpose, like [`crate::schema`]: the firmware cannot be built for the host, so the
//! decisions live here, where `research/dash/host/tests/settings_saving.rs` compiles this file
//! as it is. The firmware holds one [`Saving`] beside its configuration and does what it says.

/// The settings in RAM against flash: what `state` reports as `unsaved=` and `run_pending=`,
/// and `get` in words ([`Saving::said`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Saving {
	/// The configuration in RAM is not what flash holds: a change not saved, the defaults, a
	/// flash erased, a stored configuration that did not fit this plan, or a record flash holds
	/// that this image cannot read. Cleared by a `save` or a `load`.
	pub unsaved: bool,
	/// A finished run is in RAM and not yet in flash: the next standstill writes it
	/// ([`Saving::at_standstill`]), or a `save` does. Until then a power-off loses it.
	pub run_pending: bool,
}

/// What the next standstill writes of a kept run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunWrite {
	/// Nothing: no run waits, or the one that did now waits for `save`.
	Nothing,
	/// The whole configuration in RAM: it is what flash holds, with the run.
	Whole,
	/// The run added to the configuration flash holds. RAM's other changes stay unsaved, the
	/// person's to keep or not. Flash holding no configuration to add it to, it waits for
	/// `save`.
	AddToStored,
}

impl Saving {
	/// A setting changed in RAM.
	pub fn changed(&mut self) {
		self.unsaved = true;
	}

	/// RAM and flash agree: a `save` wrote RAM, or a `load` read flash. A run waiting is in
	/// flash by the one and gone by the other.
	pub fn agreed(&mut self) {
		*self = Saving::default();
	}

	/// The defaults are in RAM. A run waiting went with what it would have been written into.
	pub fn defaults(&mut self) {
		self.unsaved = true;
		self.run_pending = false;
	}

	/// Flash was erased. What RAM holds stays on the glass and is no longer in flash — so it is
	/// unsaved, and a run kept later is added to what flash holds, which is nothing: it waits
	/// for `save`. Left saved, that run wrote the whole configuration back, and the next boot
	/// loaded the erased pages and brightness rather than the defaults (PR #12 review). A run
	/// already waiting goes, as with the defaults.
	pub fn erased(&mut self) {
		self.unsaved = true;
		self.run_pending = false;
	}

	/// A finished run was put in RAM, to be written at a standstill.
	pub fn run_kept(&mut self) {
		self.run_pending = true;
	}

	/// The car stands: what to write of a kept run. One try — the run is no longer pending
	/// after it, and one that could not be written waits for `save` ([`Saving::not_written`]).
	///
	/// `flash_unreadable`: the newest record in flash is one this image cannot read — written
	/// by a newer image. Only an explicit `save` overwrites it; a write nobody asked for does
	/// not, so the run waits for `save`.
	pub fn at_standstill(&mut self, flash_unreadable: bool) -> RunWrite {
		if !self.run_pending {
			return RunWrite::Nothing;
		}
		self.run_pending = false;
		if flash_unreadable {
			self.unsaved = true;
			return RunWrite::Nothing;
		}
		match self.unsaved {
			true => RunWrite::AddToStored,
			false => RunWrite::Whole,
		}
	}

	/// The run [`Saving::at_standstill`] asked to write was not written: a `save` writes it.
	pub fn not_written(&mut self) {
		self.unsaved = true;
	}

	/// `get`'s words for it.
	pub fn said(&self) -> &'static str {
		match (self.unsaved, self.run_pending) {
			(false, false) => "saved",
			(true, false) => "UNSAVED",
			(false, true) => "saved, and a run waits for a standstill or `save`",
			(true, true) => "UNSAVED, and a run waits for a standstill or `save`",
		}
	}
}
