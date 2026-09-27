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
	/// flash erased, a stored configuration that did not fit this plan, a run that could not be
	/// written, or a record flash holds that this image cannot read. Cleared by a `save` or a
	/// `load`.
	pub unsaved: bool,
	/// A finished run is in RAM and not in flash — until it is: written at a standstill
	/// ([`Saving::at_standstill`]), or by a `save`. Until then a power-off loses it. Set while a
	/// write is declined or fails too (PR #12 review: it was cleared before the write's outcome
	/// was known, and `state` said `run_pending=0` over a run RAM alone held).
	pub run_pending: bool,
	/// The standstill has had its one try at the pending run: whatever came of it, the next
	/// standstill does not try again, and the run waits for `save`. Not reported: it is when
	/// the board writes, not what flash holds.
	pub run_tried: bool,
}

/// Flash, as a run's write at a standstill finds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flash {
	/// Its newest record is a newer image's, which this one cannot read. Only an explicit
	/// `save` writes over it; nothing the board does on its own does.
	Unreadable,
	/// It holds no configuration: a board never saved, or erased. The next boot would run on
	/// the defaults.
	Empty,
	/// It holds a configuration this image reads.
	Holds,
	/// It could not be read for one.
	Unusable,
}

/// What the next standstill writes of a kept run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunWrite {
	/// Nothing: no run waits, the one that did has had its try, or it waits for `save`.
	Nothing,
	/// The whole configuration in RAM: it is what flash holds, with the run.
	Whole,
	/// The run added to the configuration flash holds. RAM's other changes stay unsaved, the
	/// person's to keep or not.
	AddToStored,
	/// The run added to the defaults: flash holds nothing, and the defaults are what the next
	/// boot would run on — so an erased board still boots on them. RAM's other changes stay
	/// unsaved (PR #12 review: with any of them, a board with empty flash never wrote a run).
	AddToDefaults,
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
		*self = Saving {
			unsaved: true,
			..Saving::default()
		};
	}

	/// Flash was erased. What RAM holds stays on the glass and is no longer in flash — so it is
	/// unsaved, and a run is added to the defaults, never written back with the erased pages and
	/// brightness (PR #12 review: left saved, the next boot loaded them rather than the
	/// defaults). A run already waiting stays pending: it is in RAM, and flash holds nothing. It
	/// goes into the defaults at the next standstill if it has not had its try; after its try it
	/// waits for `save`, as before the erase.
	pub fn erased(&mut self) {
		self.unsaved = true;
	}

	/// A finished run was put in RAM, to be written at a standstill. It has its own try.
	pub fn run_kept(&mut self) {
		self.run_pending = true;
		self.run_tried = false;
	}

	/// A kept run waits for its try at a standstill. The stopwatch does not arm meanwhile, so
	/// the write is over before a launch can start (PR #12 review: written just past the
	/// arming, it landed as a driver who saw `GO` set off).
	pub fn write_waits(&self) -> bool {
		self.run_pending && !self.run_tried
	}

	/// The car stands: what to write of a kept run, with flash as it is. One try — whatever
	/// comes of it, the next standstill writes nothing, and the run waits for `save`; it stays
	/// pending until [`Saving::run_written`].
	///
	/// Only an explicit `save` overwrites a record this image cannot read ([`Flash::Unreadable`]):
	/// a write nobody asked for does not, so the run waits for `save`. With nothing else unsaved,
	/// RAM is what flash holds — or, flash empty, the defaults the board booted on — and goes
	/// whole, whatever flash is otherwise.
	pub fn at_standstill(&mut self, flash: Flash) -> RunWrite {
		if !self.write_waits() {
			return RunWrite::Nothing;
		}
		self.run_tried = true;
		match (flash, self.unsaved) {
			(Flash::Unreadable, _) => {
				self.unsaved = true;
				RunWrite::Nothing
			}
			(_, false) => RunWrite::Whole,
			(Flash::Holds, true) => RunWrite::AddToStored,
			(Flash::Empty, true) => RunWrite::AddToDefaults,
			(Flash::Unusable, true) => RunWrite::Nothing,
		}
	}

	/// The run [`Saving::at_standstill`] asked to write is in flash.
	pub fn run_written(&mut self) {
		self.run_pending = false;
		self.run_tried = false;
	}

	/// The run [`Saving::at_standstill`] asked to write was not written: it stays pending, and a
	/// `save` writes it.
	pub fn not_written(&mut self) {
		self.unsaved = true;
	}

	/// `get`'s words for it.
	pub fn said(&self) -> &'static str {
		match (self.unsaved, self.run_pending) {
			(false, false) => "saved",
			(true, false) => "UNSAVED",
			(false, true) => "saved, and a run is in RAM only",
			(true, true) => "UNSAVED, and a run is in RAM only",
		}
	}
}
