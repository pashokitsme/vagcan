//! The cruise lever as buttons, while cruise is off (`todo/dash/19`).
//!
//! A steering-column unit reports its stalks as fields of one identifier, and on
//! the reference car the cruise lever's rocker and its ON/CANCEL/OFF switch are
//! two of them. With the switch OFF the engine ignores the rocker, so the rocker
//! is free to page the panel. Everything that names *which* unit, identifier,
//! field or state that is lives in the owner's `dash.toml` and is resolved by
//! the plan: this module sees only [`StateIndex`]es, and a [`Classify`] the plan
//! provides turns a field's raw reading into one.
//!
//! Two rules, both on the safe side:
//!
//! - **The gate.** The lever is ours only while **both** witnesses say off: the
//!   switch reads its off state, and the engine's own cruise status reads its
//!   off state. A read that is missing or stale is not "off" — a lost read must
//!   never turn a press meant for cruise into a page turn. CANCEL is not off
//!   either (`todo/dash/14` §6a: plus after CANCEL resumes the set speed).
//! - **A press.** The rocker is an analog ladder read with noise, and a
//!   transition between two of its levels can pass through a third for one read.
//!   So a state counts only when **two consecutive reads** agree on it, and a
//!   press fires on the read that confirms an edge into `next`, `previous` or
//!   `measure` — once: holding does not repeat, and a one-read excursion never
//!   fires. The gate has to be open on both reads of the press.
//!
//! Nothing here reads a clock or a bus. The lever is sampled at whatever rate the
//! scheduler gives it; "two reads" is the debounce whatever that rate is.

/// A state of one field: its place in the list of states the plan resolved for
/// that field. Which list, and what each state is called, is the plan's; this
/// module only compares them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateIndex(pub u16);

/// What turns a field's raw reading into a state — the plan's, built from the
/// intervals the ODIS project gives each state. `None` for a reading no state
/// claims, which is treated as no reading at all.
pub trait Classify {
	fn classify(&self, raw: i64) -> Option<StateIndex>;
}

impl<F: Fn(i64) -> Option<StateIndex>> Classify for F {
	fn classify(&self, raw: i64) -> Option<StateIndex> {
		self(raw)
	}
}

/// The states that mean something here, as the plan resolved them from the
/// owner's texts: three of the rocker's, one of the switch's, one of the cruise
/// status's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct States {
	/// The rocker's state that turns to the next page.
	pub next: StateIndex,
	/// The rocker's state that turns to the previous page.
	pub previous: StateIndex,
	/// The rocker's state that switches the stopwatch on and off.
	pub measure: StateIndex,
	/// The switch's off state.
	pub switch_off: StateIndex,
	/// The cruise status's off state.
	pub cruise_off: StateIndex,
}

/// One read of the lever: the rocker and the switch from one answer, and the
/// cruise status as last read. `None` is a field that did not answer, did not
/// classify, or is stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Read {
	pub rocker: Option<StateIndex>,
	pub switch: Option<StateIndex>,
	pub cruise: Option<StateIndex>,
}

impl Read {
	/// A read from raw field values, through the plan's classifiers.
	pub fn classify(
		rocker: Option<i64>,
		switch: Option<i64>,
		cruise: Option<i64>,
		rocker_states: &impl Classify,
		switch_states: &impl Classify,
		cruise_states: &impl Classify,
	) -> Self {
		Read {
			rocker: rocker.and_then(|raw| rocker_states.classify(raw)),
			switch: switch.and_then(|raw| switch_states.classify(raw)),
			cruise: cruise.and_then(|raw| cruise_states.classify(raw)),
		}
	}
}

/// A press of the lever, while it is ours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lever {
	Next,
	Previous,
	Measure,
}

/// The gate and the press detector.
pub struct Stalk {
	states: States,
	/// The rocker's state on the last read, `None` when it had none.
	last: Option<StateIndex>,
	/// The last state two consecutive reads agreed on. Kept across a missing
	/// read, so a press held through a gap in the answers does not fire twice;
	/// `None` until the first, which is only where the lever is.
	held: Option<StateIndex>,
	/// Whether the gate was open on the last read.
	open: bool,
}

impl Stalk {
	pub const fn new(states: States) -> Self {
		Stalk {
			states,
			last: None,
			held: None,
			open: false,
		}
	}

	/// Whether the gate was open on the last read — what the scheduler reads the
	/// lever's rate by.
	pub fn gate_open(&self) -> bool {
		self.open
	}

	/// Whether a read hands the lever to us: both witnesses answered, and both say off.
	pub fn gate(&self, read: &Read) -> bool {
		read.switch == Some(self.states.switch_off) && read.cruise == Some(self.states.cruise_off)
	}

	/// Reading stopped — the board was an adapter, and nothing of the lever was read
	/// meanwhile. The next read pairs with nothing, as after a missing one, and the gate
	/// is closed until a read opens it: a read from before the gap is not half of a press.
	pub fn lost(&mut self) {
		self.last = None;
		self.open = false;
	}

	/// One read. A press, if this read confirmed one.
	///
	/// **Call it exactly once per new answer** of the identifier that carries the
	/// rocker and the switch — never once per frame, and never again with a stored
	/// answer. The debounce is "two consecutive reads", so the same answer fed
	/// twice is a state confirmed by one read: a transitional ladder level caught
	/// once in passing (between rest and `previous`, say) would then fire `measure`.
	/// A lost or stale answer is fed as a `None` rocker, which is what it is.
	/// The cruise status in [`Read::cruise`] is the latest one read, whenever that was.
	///
	/// The rocker's state is tracked whether or not the gate is open, so a press
	/// that began while the lever was cruise's is already held when the gate
	/// opens, and does not fire then.
	pub fn read(&mut self, read: Read) -> Option<Lever> {
		let open = self.gate(&read);
		let was_open = core::mem::replace(&mut self.open, open);
		let rocker = core::mem::replace(&mut self.last, read.rocker);

		let state = read.rocker?;
		if rocker != Some(state) || self.held == Some(state) {
			return None;
		}
		// The first state two reads agree on is where the lever is, not an edge:
		// a lever held when reading starts is not a press.
		let before = self.held.replace(state);
		if before.is_none() || !(open && was_open) {
			return None;
		}
		match state {
			s if s == self.states.next => Some(Lever::Next),
			s if s == self.states.previous => Some(Lever::Previous),
			s if s == self.states.measure => Some(Lever::Measure),
			_ => None,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Neutral states: the rocker rests at 0 and has 1, 2, 3 for its buttons; the
	/// switch is off at 0, on at 1, cancel at 2; the cruise status is off at 0.
	const REST: StateIndex = StateIndex(0);
	const NEXT: StateIndex = StateIndex(1);
	const PREVIOUS: StateIndex = StateIndex(2);
	const MEASURE: StateIndex = StateIndex(3);
	const OFF: StateIndex = StateIndex(0);
	const ON: StateIndex = StateIndex(1);
	const CANCEL: StateIndex = StateIndex(2);
	const CRUISE_OFF: StateIndex = StateIndex(0);
	const CRUISE_PASSIVE: StateIndex = StateIndex(2);

	const STATES: States = States {
		next: NEXT,
		previous: PREVIOUS,
		measure: MEASURE,
		switch_off: OFF,
		cruise_off: CRUISE_OFF,
	};

	/// The rocker at `state`, with both witnesses saying off.
	fn free(state: StateIndex) -> Read {
		Read {
			rocker: Some(state),
			switch: Some(OFF),
			cruise: Some(CRUISE_OFF),
		}
	}

	fn run(stalk: &mut Stalk, reads: &[Read]) -> std::vec::Vec<Option<Lever>> {
		reads.iter().map(|r| stalk.read(*r)).collect()
	}

	#[test]
	fn a_press_held_for_two_reads_fires_once_on_the_second() {
		let mut stalk = Stalk::new(STATES);
		let out = run(&mut stalk, &[free(REST), free(REST), free(NEXT), free(NEXT), free(NEXT), free(NEXT)]);
		assert_eq!(out, [None, None, None, Some(Lever::Next), None, None], "holding does not repeat");
	}

	#[test]
	fn each_button_is_its_own_press() {
		let mut stalk = Stalk::new(STATES);
		let reads = [
			free(REST),
			free(REST),
			free(PREVIOUS),
			free(PREVIOUS),
			free(REST),
			free(REST),
			free(MEASURE),
			free(MEASURE),
			free(REST),
			free(NEXT),
			free(NEXT),
		];
		let fired: std::vec::Vec<Lever> = run(&mut stalk, &reads).into_iter().flatten().collect();
		assert_eq!(fired, [Lever::Previous, Lever::Measure, Lever::Next]);
	}

	#[test]
	fn one_noisy_read_never_fires() {
		// A transition between two ladder levels can pass through a third for one
		// read: rest, one read of `measure`, the press it was on its way to.
		let mut stalk = Stalk::new(STATES);
		let out = run(
			&mut stalk,
			&[
				free(REST),
				free(REST),
				free(MEASURE),
				free(PREVIOUS),
				free(PREVIOUS),
				free(REST),
				free(NEXT),
				free(REST),
			],
		);
		assert_eq!(out, [None, None, None, None, Some(Lever::Previous), None, None, None]);
	}

	#[test]
	fn a_glitch_inside_a_hold_does_not_fire_it_again() {
		let mut stalk = Stalk::new(STATES);
		let out = run(
			&mut stalk,
			&[free(REST), free(REST), free(NEXT), free(NEXT), free(REST), free(NEXT), free(NEXT)],
		);
		assert_eq!(
			out.iter().flatten().count(),
			1,
			"one read of rest is not a release, so the hold is one press: {out:?}"
		);
		// A release two reads long is one, and the next press fires again.
		let out = run(&mut stalk, &[free(REST), free(REST), free(NEXT), free(NEXT)]);
		assert_eq!(out, [None, None, None, Some(Lever::Next)]);
	}

	#[test]
	fn a_missing_read_breaks_the_pair_and_does_not_release_a_hold() {
		let mut stalk = Stalk::new(STATES);
		let gap = Read { rocker: None, ..free(REST) };
		// `next`, a read that did not answer, `next`: never two in a row.
		assert_eq!(run(&mut stalk, &[free(REST), free(REST), free(NEXT), gap, free(NEXT)]), [None; 5]);
		// Held through a gap once it has fired: still one press.
		let mut stalk = Stalk::new(STATES);
		let out = run(
			&mut stalk,
			&[free(REST), free(REST), free(NEXT), free(NEXT), gap, gap, free(NEXT), free(NEXT)],
		);
		assert_eq!(out.iter().flatten().count(), 1, "{out:?}");
	}

	#[test]
	fn a_lever_already_held_when_reading_starts_is_not_a_press() {
		// No state before it, so no edge into it: the first state two reads agree
		// on is where the lever is, not a press.
		let mut stalk = Stalk::new(STATES);
		assert_eq!(run(&mut stalk, &[free(NEXT), free(NEXT), free(NEXT)]), [None; 3]);
		// Released and pressed again, it is.
		assert_eq!(
			run(&mut stalk, &[free(REST), free(REST), free(NEXT), free(NEXT)]),
			[None, None, None, Some(Lever::Next)]
		);
	}

	#[test]
	fn the_gate_is_open_only_while_both_witnesses_say_off() {
		let mut stalk = Stalk::new(STATES);
		let with = |switch, cruise| Read {
			rocker: Some(REST),
			switch,
			cruise,
		};
		let cases = [
			(Some(OFF), Some(CRUISE_OFF), true),
			(Some(ON), Some(CRUISE_OFF), false),
			(Some(CANCEL), Some(CRUISE_OFF), false),
			(Some(OFF), Some(CRUISE_PASSIVE), false),
			(None, Some(CRUISE_OFF), false),
			(Some(OFF), None, false),
			(None, None, false),
		];
		for (switch, cruise, open) in cases {
			stalk.read(with(switch, cruise));
			assert_eq!(stalk.gate_open(), open, "switch {switch:?}, cruise {cruise:?}");
		}
	}

	#[test]
	fn nothing_fires_through_a_closed_gate() {
		for closed in [
			Read {
				switch: Some(ON),
				..free(NEXT)
			},
			Read {
				switch: Some(CANCEL),
				..free(NEXT)
			},
			Read {
				cruise: Some(CRUISE_PASSIVE),
				..free(NEXT)
			},
			Read { switch: None, ..free(NEXT) },
			Read { cruise: None, ..free(NEXT) },
		] {
			let mut stalk = Stalk::new(STATES);
			let out = run(&mut stalk, &[free(REST), free(REST), closed, closed, closed]);
			assert_eq!(out, [None; 5], "{closed:?}");
		}
	}

	#[test]
	fn a_press_the_gate_opened_under_does_not_fire() {
		// Cruise was on while the rocker went to `next`; the witnesses turn off
		// with it still held. The edge happened while the lever was cruise's.
		let mut stalk = Stalk::new(STATES);
		let closed = Read {
			switch: Some(ON),
			cruise: Some(CRUISE_PASSIVE),
			..free(NEXT)
		};
		let out = run(&mut stalk, &[free(REST), free(REST), closed, free(NEXT), free(NEXT)]);
		assert_eq!(out, [None; 5], "the gate must be open on both reads of the press");
		// Released and pressed again with the gate open, it is ours.
		assert_eq!(
			run(&mut stalk, &[free(REST), free(REST), free(NEXT), free(NEXT)]),
			[None, None, None, Some(Lever::Next)]
		);
	}

	#[test]
	fn a_gate_that_closes_on_the_second_read_stops_the_press() {
		let mut stalk = Stalk::new(STATES);
		let closing = Read {
			switch: Some(ON),
			..free(NEXT)
		};
		assert_eq!(run(&mut stalk, &[free(REST), free(REST), free(NEXT), closing, free(NEXT)]), [None; 5]);
	}

	#[test]
	fn raw_readings_go_through_the_plans_classifiers() {
		// A neutral ladder: each state an interval of raw values, as the ODIS
		// project gives them; nothing past the last one is a state.
		let ladder = |raw: i64| match raw {
			0..=49 => Some(REST),
			50..=99 => Some(NEXT),
			100..=149 => Some(PREVIOUS),
			150..=199 => Some(MEASURE),
			_ => None,
		};
		let switch = |raw: i64| (raw < 100).then_some(OFF).or(Some(ON));
		let cruise = |raw: i64| (raw == 0).then_some(CRUISE_OFF).or(Some(CRUISE_PASSIVE));
		let read = |rocker| Read::classify(Some(rocker), Some(10), Some(0), &ladder, &switch, &cruise);
		assert_eq!(read(72), free(NEXT), "±2 of noise around a level is the same state");
		assert_eq!(read(74), free(NEXT));
		assert_eq!(read(250).rocker, None, "no state claims it");
		assert_eq!(
			Read::classify(None, Some(10), Some(0), &ladder, &switch, &cruise).rocker,
			None,
			"no answer"
		);
		let mut stalk = Stalk::new(STATES);
		let out = run(&mut stalk, &[read(20), read(20), read(128), read(131)]);
		assert_eq!(out, [None, None, None, Some(Lever::Previous)]);
	}

	#[test]
	fn a_read_from_before_the_adapter_screen_is_not_half_of_a_press() {
		let mut stalk = Stalk::new(STATES);
		// At rest, then NEXT caught once in passing as the board became an adapter.
		assert_eq!(run(&mut stalk, &[free(REST), free(REST), free(NEXT)]), [None, None, None]);
		stalk.lost();
		assert!(!stalk.gate_open());
		// Minutes later, one read of NEXT is still one read.
		assert_eq!(stalk.read(free(NEXT)), None);
		assert_eq!(stalk.read(free(NEXT)), Some(Lever::Next), "the second read pairs");
	}
}
