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
//! A press comes out as a [`Command`] — `next` as [`Command::Next`], `previous` as
//! [`Command::Previous`], `measure` as [`Command::Stopwatch`] — the same command a `[[button]]`
//! on a pin gives, so the screen treats a press the same whatever was pressed
//! ([`control`](crate::control)).
//!
//! Nothing here reads a clock or a bus. The lever is sampled at whatever rate the
//! scheduler gives it; "two reads" is the debounce whatever that rate is.

use crate::control::Command;

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
		open(&self.states, read)
	}

	/// Reading stopped — the board was an adapter, and nothing of the lever was read
	/// meanwhile. The next read pairs with nothing, as after a missing one, and the gate
	/// is closed until a read opens it: a read from before the gap is not half of a press.
	pub fn lost(&mut self) {
		self.last = None;
		self.open = false;
	}

	/// One read. A press, if this read confirmed one, as the command it asks for.
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
	pub fn read(&mut self, read: Read) -> Option<Command> {
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
			s if s == self.states.next => Some(Command::Next),
			s if s == self.states.previous => Some(Command::Previous),
			s if s == self.states.measure => Some(Command::Stopwatch),
			_ => None,
		}
	}
}

/// How long the gate may go unseen open before the lever closes the stopwatch
/// ([`Close::Stale`]): the gate closed for lack of data, not because cruise was taken.
///
/// Why 3 s. Long enough that a hiccup in the lever's data leaves a run alone: the cruise
/// status counts as stale 600 ms after its last answer (three of its periods), and each
/// exchange a unit does not answer holds the bus for the board's answer timeout (500 ms), so
/// two or three reads lost in a row come to a second or more. Short enough that a lever unit
/// gone silent for good — the case this rule is for — gives the gauge pages back within
/// seconds: with the lever the only input that has `Stopwatch`, nothing else can close it,
/// and a silent unit never opens the gate again.
pub const STALE_CLOSE_MS: u64 = 3_000;

/// Why the lever's reads close the stopwatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Close {
	/// Two consecutive reads said cruise is engaged: the switch in a state other than its
	/// off, or the cruise status in one other than its off. Positive evidence, never a
	/// missing read.
	Engaged,
	/// The gate has not been seen open for over [`STALE_CLOSE_MS`]: the lever's data is
	/// missing — stale, unanswered, refused, a reading no state claims, its unit silent — or
	/// never says the same thing twice in a row.
	Stale,
}

/// When the lever's witnesses take the stopwatch away (owner, 2026-09-27). The stopwatch
/// opens and closes on a `Stopwatch` command, and the lever gives one only through an open
/// gate; so a stopwatch up when the gate closes could stay on the glass, holding the gauge
/// pages, until the gate opened again — never, if the lever's unit went silent. Two rules,
/// whatever opened the stopwatch, a lever or a button:
///
/// - **Cruise taken → it closes at once.** Two consecutive reads that say cruise is engaged
///   ([`Close::Engaged`]) — the same two-read confirmation as a press, so one noisy read of
///   the switch's ladder never ends a run. A read that says nothing breaks the pair. The
///   cruise status in a read is the latest one answered, as for the gate, so one answer of it
///   may stand in both reads: it is the engine's own enumerated state, not a ladder.
/// - **The gate closed for lack of data → it stays**, a run in progress included, until the
///   gate has not been seen open for over [`STALE_CLOSE_MS`] ([`Close::Stale`]). A read that
///   shows the gate open starts the time over; an engaged read that finds no pair does not,
///   so a unit answering every other time with cruise on still ends it.
///
/// Level, not edge: every read or tick for which a rule holds says so, and the caller closes
/// the stopwatch if it is up (`Screen::close_stopwatch`). These rules only ever close the
/// stopwatch: they never open it and never turn a page — whether a press pages is still
/// [`Stalk`]'s gate alone. With no `[stalk]` in the plan ([`Closer::new`] with `None`) nothing
/// ever closes. No clock is read here: `now_ms` is a parameter, as in the stopwatch.
#[derive(Debug, Clone, Copy)]
pub struct Closer {
	/// The plan's states, `None` with no `[stalk]`.
	states: Option<States>,
	/// The last read said cruise is engaged: half of a pair.
	engaged: bool,
	/// When the gate was last seen open — or, where no read has shown it open yet, when the
	/// closer first looked. `None` before that first look.
	seen_open: Option<u64>,
}

impl Closer {
	pub const fn new(states: Option<States>) -> Self {
		Closer {
			states,
			engaged: false,
			seen_open: None,
		}
	}

	/// One read of the lever — the same read, at the same moment, [`Stalk::read`] gets: once per
	/// answer of the rocker's identifier, and once for an answer that did not come.
	pub fn read(&mut self, read: &Read, now_ms: u64) -> Option<Close> {
		let states = self.states?;
		let engaged = read.switch.is_some_and(|s| s != states.switch_off) || read.cruise.is_some_and(|c| c != states.cruise_off);
		let pair = core::mem::replace(&mut self.engaged, engaged) && engaged;
		if open(&states, read) {
			self.seen_open = Some(now_ms);
		}
		if pair {
			return Some(Close::Engaged);
		}
		self.tick(now_ms)
	}

	/// The clock alone, with no read: a unit gone silent answers nothing, and still has to end
	/// the stopwatch. Call it between reads; [`Closer::due`] says when it next matters.
	pub fn tick(&mut self, now_ms: u64) -> Option<Close> {
		self.states?;
		let since = *self.seen_open.get_or_insert(now_ms);
		(now_ms.saturating_sub(since) > STALE_CLOSE_MS).then_some(Close::Stale)
	}

	/// The first moment a [`Closer::tick`] would say [`Close::Stale`], if nothing is read before
	/// it — for the caller's sleep. `None` before the first look, and with no `[stalk]`.
	pub fn due(&self) -> Option<u64> {
		self.states?;
		Some(self.seen_open? + STALE_CLOSE_MS + 1)
	}

	/// Reading stopped — the board was an adapter, which ended the stopwatch. The next read
	/// pairs with nothing, and the gate's time starts over at the next look: the minutes as an
	/// adapter are not the lever's data going missing.
	pub fn lost(&mut self) {
		self.engaged = false;
		self.seen_open = None;
	}
}

/// Whether a read hands the lever to us: both witnesses answered, and both say off.
fn open(states: &States, read: &Read) -> bool {
	read.switch == Some(states.switch_off) && read.cruise == Some(states.cruise_off)
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

	fn run(stalk: &mut Stalk, reads: &[Read]) -> std::vec::Vec<Option<Command>> {
		reads.iter().map(|r| stalk.read(*r)).collect()
	}

	#[test]
	fn a_press_held_for_two_reads_fires_once_on_the_second() {
		let mut stalk = Stalk::new(STATES);
		let out = run(&mut stalk, &[free(REST), free(REST), free(NEXT), free(NEXT), free(NEXT), free(NEXT)]);
		assert_eq!(out, [None, None, None, Some(Command::Next), None, None], "holding does not repeat");
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
		let fired: std::vec::Vec<Command> = run(&mut stalk, &reads).into_iter().flatten().collect();
		assert_eq!(fired, [Command::Previous, Command::Stopwatch, Command::Next]);
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
		assert_eq!(out, [None, None, None, None, Some(Command::Previous), None, None, None]);
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
		assert_eq!(out, [None, None, None, Some(Command::Next)]);
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
			[None, None, None, Some(Command::Next)]
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
			[None, None, None, Some(Command::Next)]
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
		assert_eq!(out, [None, None, None, Some(Command::Previous)]);
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
		assert_eq!(stalk.read(free(NEXT)), Some(Command::Next), "the second read pairs");
	}

	// The stopwatch closed by the lever's witnesses (owner, 2026-09-27).

	/// The switch on: cruise taken, as plainly as the lever says it.
	fn switched_on() -> Read {
		Read {
			switch: Some(ON),
			..free(REST)
		}
	}

	/// The engine's cruise status saying anything but off, the switch still reading off.
	fn cruise_passive() -> Read {
		Read {
			cruise: Some(CRUISE_PASSIVE),
			..free(REST)
		}
	}

	/// No answer of the lever's identifier, the cruise status still fresh and off.
	fn unanswered() -> Read {
		Read {
			rocker: None,
			switch: None,
			..free(REST)
		}
	}

	/// Each read of `reads` fed `step_ms` apart from `from_ms`; what each said.
	fn close_run(closer: &mut Closer, from_ms: u64, step_ms: u64, reads: &[Read]) -> std::vec::Vec<Option<Close>> {
		reads
			.iter()
			.enumerate()
			.map(|(i, read)| closer.read(read, from_ms + i as u64 * step_ms))
			.collect()
	}

	#[test]
	fn two_engaged_reads_in_a_row_close_on_the_second() {
		for engaged in [switched_on(), cruise_passive()] {
			let mut closer = Closer::new(Some(STATES));
			assert_eq!(
				close_run(&mut closer, 0, 100, &[free(REST), free(REST), engaged, engaged]),
				[None, None, None, Some(Close::Engaged)],
				"{engaged:?}"
			);
		}
		// Either witness makes a read engaged: one of each is a pair too.
		let mut closer = Closer::new(Some(STATES));
		assert_eq!(
			close_run(&mut closer, 0, 100, &[free(REST), switched_on(), cruise_passive()]),
			[None, None, Some(Close::Engaged)]
		);
		// CANCEL is not off: it is the switch in a state other than its off.
		let cancel = Read {
			switch: Some(CANCEL),
			..free(REST)
		};
		let mut closer = Closer::new(Some(STATES));
		assert_eq!(close_run(&mut closer, 0, 100, &[cancel, cancel]), [None, Some(Close::Engaged)]);
	}

	#[test]
	fn one_noisy_engaged_read_never_closes() {
		let mut closer = Closer::new(Some(STATES));
		let reads = [
			free(REST),
			free(REST),
			switched_on(),
			free(REST),
			cruise_passive(),
			free(REST),
			free(REST),
		];
		assert_eq!(close_run(&mut closer, 0, 100, &reads), [None; 7]);
	}

	#[test]
	fn a_read_that_says_nothing_breaks_an_engaged_pair() {
		// Engaged, a read with neither witness (no answer, the cruise status stale), engaged:
		// never two engaged reads in a row. Short of the stale close, nothing closes.
		let nothing = Read {
			rocker: None,
			switch: None,
			cruise: None,
		};
		let mut closer = Closer::new(Some(STATES));
		let reads = [free(REST), switched_on(), nothing, switched_on(), unanswered(), cruise_passive()];
		assert_eq!(close_run(&mut closer, 0, 100, &reads), [None; 6]);
	}

	#[test]
	fn the_gate_closed_for_lack_of_data_for_2_9_s_then_seen_open_does_not_close() {
		for missing in [
			unanswered(),
			Read { cruise: None, ..free(REST) },
			Read {
				switch: None,
				cruise: None,
				..free(REST)
			},
		] {
			let mut closer = Closer::new(Some(STATES));
			// Open at 0; the lack of data from 100 ms to 2.9 s; open again at 2.9 s.
			assert_eq!(closer.read(&free(REST), 0), None);
			for t in (100..2_900).step_by(100) {
				assert_eq!(closer.read(&missing, t), None, "{missing:?} at {t} ms");
			}
			assert_eq!(closer.read(&free(REST), 2_900), None, "seen open again");
			// The timer starts over there: another 2.9 s of it is still short.
			for t in (3_000..5_800).step_by(100) {
				assert_eq!(closer.read(&missing, t), None, "{missing:?} at {t} ms, after the gate opened at 2.9 s");
			}
			assert_eq!(closer.tick(5_800), None);
		}
	}

	#[test]
	fn the_gate_closed_for_lack_of_data_for_3_1_s_closes() {
		let mut closer = Closer::new(Some(STATES));
		assert_eq!(closer.read(&free(REST), 0), None);
		for t in (100..=STALE_CLOSE_MS).step_by(100) {
			assert_eq!(closer.read(&unanswered(), t), None, "at {t} ms: not yet over 3 s");
		}
		assert_eq!(closer.read(&unanswered(), 3_100), Some(Close::Stale));
	}

	#[test]
	fn a_rocker_unit_gone_silent_closes_after_3_s_on_ticks_alone() {
		// The unit stops answering: the board drops its subscription and feeds one read with no
		// answer; nothing of the lever is read after it, and only the clock moves.
		let mut closer = Closer::new(Some(STATES));
		assert_eq!(closer.read(&free(REST), 1_000), None);
		assert_eq!(closer.read(&unanswered(), 1_600), None);
		assert_eq!(closer.due(), Some(1_000 + STALE_CLOSE_MS + 1), "when a tick would close");
		assert_eq!(closer.tick(3_000), None);
		assert_eq!(closer.tick(1_000 + STALE_CLOSE_MS), None, "3 s exactly is not over 3 s");
		assert_eq!(closer.tick(1_000 + STALE_CLOSE_MS + 1), Some(Close::Stale));
	}

	#[test]
	fn engaged_reads_that_never_pair_still_close_once_the_gate_is_unseen_for_3_s() {
		// Cruise on, and the lever's unit answering every other time: no two engaged reads in a
		// row, and the gate never open. The stale close is the way out.
		let mut closer = Closer::new(Some(STATES));
		assert_eq!(closer.read(&free(REST), 0), None);
		let mut out = std::vec::Vec::new();
		for (i, t) in (100..=3_100).step_by(100).enumerate() {
			let read = if i % 2 == 0 { switched_on() } else { unanswered() };
			out.push(closer.read(&read, t));
		}
		assert_eq!(out.last(), Some(&Some(Close::Stale)), "{out:?}");
		assert!(out[..out.len() - 1].iter().all(Option::is_none), "{out:?}");
	}

	#[test]
	fn with_no_stalk_in_the_plan_nothing_ever_closes() {
		let mut closer = Closer::new(None);
		let reads = [switched_on(), switched_on(), cruise_passive(), cruise_passive(), unanswered()];
		assert_eq!(close_run(&mut closer, 0, 100, &reads), [None; 5]);
		assert_eq!(closer.tick(60_000), None);
		assert_eq!(closer.due(), None);
	}

	#[test]
	fn after_adapter_mode_the_closer_starts_over() {
		let mut closer = Closer::new(Some(STATES));
		assert_eq!(close_run(&mut closer, 0, 100, &[free(REST), switched_on()]), [None, None]);
		closer.lost();
		// Minutes later: one engaged read is one read, and the gate's clock starts again at the
		// first look rather than at the last open read before the gap.
		assert_eq!(closer.read(&switched_on(), 300_000), None, "one read, not the second of a pair");
		assert_eq!(closer.tick(300_000 + STALE_CLOSE_MS), None);
		assert_eq!(closer.tick(300_000 + STALE_CLOSE_MS + 1), Some(Close::Stale));
		// Nothing read yet after the gap: the first tick is where the clock starts.
		let mut closer = Closer::new(Some(STATES));
		closer.lost();
		assert_eq!(closer.due(), None, "nothing seen yet, nothing due");
		assert_eq!(closer.tick(10_000), None);
		assert_eq!(closer.due(), Some(10_000 + STALE_CLOSE_MS + 1));
	}
}
