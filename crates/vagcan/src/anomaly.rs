//! Stopping when something changes.
//!
//! `SAFETY.md` has said since the first incident: *"Stop when something
//! changes. If a lamp comes on, or a system goes quiet, finish nothing and
//! start nothing."* It was a rule for the person holding the laptop, and the
//! tool did not hold itself to it — the per-unit loop counted a unit that had
//! gone silent as `stats.failed` and swept on to the next identifier, and then
//! to the next unit. On 9 August 2026 that is what happened for the second
//! time: the sweep noticed and carried on, which is worse than not noticing.
//!
//! This module is the rule made executable. A sweep feeds every answer through
//! a [`Monitor`]; the first time a unit that had been talking stops talking, or
//! goes back on an identifier it already answered *in this same run*, the
//! monitor records an [`Anomaly`] and the sweep ends — the whole run, not just
//! that unit, because the thing that made the second incident permanent was
//! carrying on after the first drop-out looked like it had resolved itself.
//!
//! ## Why it is deliberately not sensitive during identification
//!
//! The monitor is seeded with the identifiers a unit *answered* in its
//! identification block and starts judging silences only once the sweep proper
//! begins. Units on the reference car answer `F187` and refuse or ignore half
//! the rest of the block; enforcing there would stop a whole-car run on a unit
//! behaving exactly as it always has. A false halt is not free — it is what
//! teaches somebody to reach for the override — so the bar is "it answered this
//! before, and now it will not", which is a statement about one run and needs no
//! table of what any particular car ought to say.

use std::collections::BTreeSet;

use vag_protocol::address::UnitAddress;
use vag_protocol::uds::UdsError;

/// What one request got back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
	/// Data. The unit is there and implements the identifier.
	Answered,
	/// A negative response. The unit is there and does not implement it —
	/// the ordinary answer across most of the space, and not a change.
	Refused,
	/// Nothing at all: a timeout or a malformed frame.
	Silent,
}

impl Answer {
	/// Classify a read the way the sweep already classifies it for its stats,
	/// so the two can never drift into disagreeing about what happened.
	pub fn of(result: &Result<Vec<u8>, UdsError>) -> Answer {
		match result {
			Ok(_) => Answer::Answered,
			Err(UdsError::NegativeResponse { .. }) => Answer::Refused,
			Err(_) => Answer::Silent,
		}
	}
}

/// How many unanswered requests in a row are a unit going quiet rather than a
/// lost frame.
///
/// One timeout on a CAN bus is ordinary. Three in a row from a unit that was
/// answering a moment ago is the unit, not the wire — and three requests is
/// well under a second, so the sweep stops while whatever it provoked is still
/// the most recent thing that happened to the car.
pub const QUIET_RUN: usize = 3;

/// How often the sweep re-reads an identifier the unit already answered.
///
/// A unit that falls over thirty requests into a sweep must not be discovered
/// at the end of it. The witness is one extra request per this many — under two
/// per cent — and it is the only way to catch a unit that answers nothing new
/// because it has stopped answering *anything*, in a space where "refused" is
/// the expected reply.
pub const WITNESS_EVERY: usize = 64;

/// How a unit stopped behaving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
	/// It had been answering, and then nothing came back, repeatedly.
	WentQuiet { silences: usize },
	/// An identifier it answered earlier in this same run it will not answer
	/// now. Not a claim about what the unit *should* implement — only that it
	/// changed its mind under the tool.
	NoLongerAnswers { now: Answer },
}

/// A unit changing its behaviour mid-sweep: the event that ends a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anomaly {
	/// Diagnostic request id of the unit that changed.
	pub request: u16,
	/// The identifier being asked for when it changed — the "what were you
	/// doing" that `SAFETY.md` step 2 says to write down.
	pub did: u16,
	pub change: Change,
}

impl Anomaly {
	/// How the unit is written on screen: its short number when this project
	/// knows one, its request id otherwise.
	pub fn unit(&self) -> String {
		UnitAddress::from_request(self.request)
			.map(|a| a.label())
			.unwrap_or_else(|| format!("{:03X}", self.request))
	}

	/// The whole notice, for a surface that will not erase it.
	///
	/// Says what happened, what was being asked when it happened, and
	/// `SAFETY.md`'s own next steps — because the moment somebody needs those is
	/// the moment they are least likely to go and read a file.
	pub fn report(&self) -> String {
		let unit = self.unit();
		let what = match self.change {
			Change::WentQuiet { silences } => format!("stopped answering altogether — {silences} requests in a row went unanswered"),
			Change::NoLongerAnswers { now: Answer::Refused } => "is now refusing an identifier it answered earlier in this same run".to_string(),
			Change::NoLongerAnswers { .. } => "no longer answers an identifier it answered earlier in this same run".to_string(),
		};
		format!(
			"\n\
			 STOPPED: control unit {unit} ({:03X}) {what}.\n\
			 The tool was asking it for identifier {:04X}.\n\
			 \n\
			 The run has ended here — every unit, not just this one. A control unit that\n\
			 changes under a sweep is the event that cost the reference car its power\n\
			 steering, and the second time it happened the sweep had noticed and carried on.\n\
			 \n\
			 What to do now (SAFETY.md, \"If a unit stops behaving\"):\n\
			 \n\
			 1. Stop the car if it is moving.\n\
			 2. Do NOT clear the faults. The freeze frame is the evidence.\n\
			      vagcan faults --ecu {unit} --details\n\
			 3. Snapshot the unit and compare it with the one you took before:\n\
			      vagcan survey --only {unit} --out after.jsonl\n\
			      vagcan survey --diff before.jsonl after.jsonl\n\
			 4. Try an ignition cycle: off, wait, on. A unit that crashed and restarted\n\
			    often comes back. One that does not is a different problem.\n\
			 5. Then stop, and take it to someone with the factory tool.\n",
			self.request, self.did
		)
	}
}

/// Watches one control unit for the duration of one sweep.
///
/// Per unit rather than per run: "it answered this before" is a statement about
/// the unit in front of the tool, and two units that behave differently are not
/// evidence about each other. The *halt* is per run — the caller stops
/// everything the moment any monitor fires.
#[derive(Debug)]
pub struct Monitor {
	request: u16,
	/// Identifiers this unit answered, this run. Seeded from the
	/// identification block and added to as the sweep finds more.
	answered: BTreeSet<u16>,
	/// Unanswered requests since the last thing the unit said.
	silences: usize,
	/// The first change seen, if any. Kept so the sweep can return normally and
	/// the caller can ask afterwards what ended it.
	halted: Option<Anomaly>,
}

impl Monitor {
	pub fn new(request: u16) -> Monitor {
		Monitor {
			request,
			answered: BTreeSet::new(),
			silences: 0,
			halted: None,
		}
	}

	/// Record an identifier the unit answered without judging it.
	///
	/// For the identification block, which is the sweep's baseline rather than
	/// part of it — see the module docs on why the block is not policed.
	pub fn seed(&mut self, did: u16) {
		self.answered.insert(did);
	}

	/// The unit said *something*, about no identifier in particular.
	///
	/// Group testing asks for eight identifiers at once, and the reply is about
	/// the span rather than about any one of them: a positive answer does not
	/// say which member answered, and a `requestOutOfRange` says only that none
	/// did. Neither can be recorded against an identifier without inventing a
	/// fact — but both prove the unit is still talking, which is the run of
	/// silences this clears.
	pub fn heard(&mut self) {
		self.silences = 0;
	}

	/// Whether this unit has said anything at all yet.
	///
	/// A unit that never spoke is absent, not changed, and nothing about it can
	/// end a run.
	pub fn spoke(&self) -> bool {
		!self.answered.is_empty()
	}

	/// Feed one outcome. `Some` means the run ends here.
	///
	/// Once it has fired it keeps returning the same anomaly: a caller that
	/// misses the first return must not be able to sweep on because the second
	/// request happened to succeed.
	pub fn saw(&mut self, did: u16, answer: Answer) -> Option<&Anomaly> {
		if self.halted.is_some() {
			return self.halted.as_ref();
		}
		match answer {
			Answer::Answered => {
				self.answered.insert(did);
				self.silences = 0;
			}
			Answer::Refused => {
				self.silences = 0;
				if self.answered.contains(&did) {
					self.halted = Some(Anomaly {
						request: self.request,
						did,
						change: Change::NoLongerAnswers { now: Answer::Refused },
					});
				}
			}
			Answer::Silent => {
				self.silences += 1;
				if !self.spoke() {
					// Never heard from: this is an address with nothing on it,
					// which is the ordinary result of walking a car.
					return None;
				}
				if self.answered.contains(&did) {
					self.halted = Some(Anomaly {
						request: self.request,
						did,
						change: Change::NoLongerAnswers { now: Answer::Silent },
					});
				} else if self.silences >= QUIET_RUN {
					self.halted = Some(Anomaly {
						request: self.request,
						did,
						change: Change::WentQuiet { silences: self.silences },
					});
				}
			}
		}
		self.halted.as_ref()
	}

	/// A whole span of identifiers went unanswered, asked as one group request.
	///
	/// Only the run-of-silences rule applies. "It answered this one before"
	/// cannot be asked of a group reply — the request covers eight identifiers
	/// and the silence belongs to none of them in particular — so a lone lost
	/// frame on a batch that happens to begin at a known-good identifier must
	/// not read as the unit going back on itself. `first` is carried for the
	/// report only, to say where the sweep was.
	pub fn silent_span(&mut self, first: u16) -> Option<&Anomaly> {
		if self.halted.is_some() {
			return self.halted.as_ref();
		}
		self.silences += 1;
		if self.spoke() && self.silences >= QUIET_RUN {
			self.halted = Some(Anomaly {
				request: self.request,
				did: first,
				change: Change::WentQuiet { silences: self.silences },
			});
		}
		self.halted.as_ref()
	}

	/// What ended the run, if anything did.
	pub fn halted(&self) -> Option<&Anomaly> {
		self.halted.as_ref()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_unit_that_goes_quiet_mid_sweep_ends_the_run() {
		// The defect this module exists for: on 9 August 2026 the sweep
		// recorded exactly this as `stats.failed` and carried on to the next
		// identifier, and then to the next unit.
		let mut m = Monitor::new(0x712);
		m.seed(0xF187);
		assert!(m.saw(0x2000, Answer::Refused).is_none(), "a refusal is the ordinary answer");
		assert!(m.saw(0x2001, Answer::Silent).is_none(), "one lost frame is not an event");
		assert!(m.saw(0x2002, Answer::Silent).is_none());
		let anomaly = m.saw(0x2003, Answer::Silent).expect("three in a row is the unit, not the wire").clone();
		assert_eq!(anomaly.request, 0x712);
		assert_eq!(anomaly.did, 0x2003, "the report names what was being asked");
		assert_eq!(anomaly.change, Change::WentQuiet { silences: QUIET_RUN });
	}

	#[test]
	fn refusing_what_it_already_answered_ends_the_run_at_once() {
		// No run-up: the unit answered this identifier minutes ago and will not
		// now. That is not a lost frame under any reading.
		let mut m = Monitor::new(0x712);
		m.seed(0xF187);
		let anomaly = m.saw(0xF187, Answer::Refused).expect("it went back on itself").clone();
		assert_eq!(anomaly.change, Change::NoLongerAnswers { now: Answer::Refused });
		assert_eq!(anomaly.did, 0xF187);

		// And the silent spelling of the same thing.
		let mut m = Monitor::new(0x712);
		m.saw(0xF187, Answer::Answered);
		let anomaly = m.saw(0xF187, Answer::Silent).expect("it stopped answering it").clone();
		assert_eq!(anomaly.change, Change::NoLongerAnswers { now: Answer::Silent });
	}

	#[test]
	fn a_unit_that_never_spoke_is_absent_rather_than_changed() {
		// Walking a car means asking addresses with nothing on them. Fifteen
		// timeouts from an empty address must not stop a survey.
		let mut m = Monitor::new(0x773);
		for did in 0x2000..0x2010 {
			assert!(m.saw(did, Answer::Silent).is_none(), "{did:04X} ended the run");
		}
		assert!(m.halted().is_none());
	}

	#[test]
	fn an_answer_in_between_clears_the_run_of_silences() {
		// Two timeouts, then the unit talks again: nothing changed, and a
		// counter that did not reset would halt the run on the next lone
		// timeout half a sweep later.
		let mut m = Monitor::new(0x712);
		m.seed(0xF187);
		m.saw(0x2000, Answer::Silent);
		m.saw(0x2001, Answer::Silent);
		m.saw(0x2002, Answer::Answered);
		assert!(m.saw(0x2003, Answer::Silent).is_none());
		assert!(m.saw(0x2004, Answer::Silent).is_none());
		assert!(m.halted().is_none());
	}

	#[test]
	fn a_monitor_that_has_fired_stays_fired() {
		// The sweep returns early on the first `Some`, but a caller that polls
		// it afterwards must not be told the car is fine because the next
		// request happened to be answered.
		let mut m = Monitor::new(0x712);
		m.seed(0xF187);
		assert!(m.saw(0xF187, Answer::Refused).is_some());
		assert!(m.saw(0x2000, Answer::Answered).is_some(), "it un-halted itself");
		assert!(m.halted().is_some());
	}

	#[test]
	fn a_lost_group_request_is_not_read_as_the_unit_going_back_on_itself() {
		// Group testing asks eight identifiers at once and the span often
		// begins at one the unit answered during identification. A single lost
		// frame there must not halt a whole-car survey — but three in a row
		// still must.
		let mut m = Monitor::new(0x712);
		m.seed(0xF187);
		assert!(m.silent_span(0xF187).is_none(), "one lost group reply is not an event");
		assert!(m.silent_span(0xF187).is_none());
		let anomaly = m.silent_span(0xF187).expect("three in a row is the unit").clone();
		assert_eq!(anomaly.change, Change::WentQuiet { silences: QUIET_RUN });

		// And a batch that answers clears the run, without claiming anything
		// about which of its eight identifiers did the answering.
		let mut m = Monitor::new(0x712);
		m.seed(0xF187);
		m.silent_span(0x2000);
		m.silent_span(0x2008);
		m.heard();
		assert!(m.silent_span(0x2010).is_none());
		assert!(m.halted().is_none());
	}

	#[test]
	fn the_notice_says_what_happened_and_what_to_do_about_it() {
		// The moment somebody needs SAFETY.md's steps is the moment they are
		// least likely to go and read a file, so the steps are in the message.
		let text = Anomaly {
			request: 0x712,
			did: 0x22FF,
			change: Change::WentQuiet { silences: 3 },
		}
		.report();
		assert!(text.contains("STOPPED"), "{text}");
		assert!(text.contains("22FF"), "it names what was being asked: {text}");
		assert!(text.contains("Do NOT clear the faults"), "{text}");
		assert!(text.contains("--diff"), "the snapshot comparison is the forensic tool: {text}");
		assert!(text.contains("ignition cycle"), "{text}");
	}

	#[test]
	fn answers_are_classified_the_way_the_sweep_counts_them() {
		// The monitor and the statistics must never disagree about what a
		// response was; a refusal counted as silence would halt every run.
		assert_eq!(Answer::of(&Ok(vec![0x01])), Answer::Answered);
		assert_eq!(Answer::of(&Err(UdsError::NegativeResponse { sid: 0x22, nrc: 0x31 })), Answer::Refused);
		assert_eq!(Answer::of(&Err(UdsError::Malformed("short".into()))), Answer::Silent);
	}
}
