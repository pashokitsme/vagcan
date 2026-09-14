//! Which answer ids a CAN controller hands up, and when that has to change.
//!
//! A controller's receive queue is finite — the ESP32-C3's async TWAI driver
//! queues 32 frames and drops the next — and a car's powertrain bus carries
//! thousands of other people's frames a second. Unfiltered, the queue fills
//! between a request and its answer, and the answer is what falls off. So the
//! board narrows its controller to the ids it expects answers on
//! (`research/dash/can-bring-up.md` §2.1).
//!
//! The panel's answer ids are known when the image is built; a request relayed
//! for a host (BLE) may go to any unit. [`FilterFollower`] keeps the plan's
//! filter while the exchanges it covers go on, and moves the filter to exactly
//! the one answer id an exchange needs when the current filter would not pass it.
//!
//! The filter is the 11-bit code-and-mask shape a single standard acceptance
//! filter holds (SJA1000 and every controller descended from it): the bits set in
//! `must_match` are compared with `code`, the rest are ignored. Pure data, no
//! hardware: the firmware converts it to its driver's type.

/// An 11-bit acceptance filter: `id` passes when `id & must_match == code & must_match`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandardFilter {
	pub code: u16,
	/// Bits that must equal `code`'s. `0x7FF` is one id exactly; `0` is every id.
	pub must_match: u16,
}

/// An 11-bit id is all a standard filter holds.
const SFF: u16 = 0x7FF;

impl StandardFilter {
	/// Passes `id` and nothing else.
	pub fn exact(id: u16) -> Self {
		StandardFilter {
			code: id & SFF,
			must_match: SFF,
		}
	}

	/// The narrowest filter that passes every id in `ids`, `None` for none.
	///
	/// A bit set in one id and clear in another cannot be insisted on, so it is
	/// dropped from the mask: `7E8`/`7E9` cost one bit, ids scattered further make
	/// the filter a superset. A superset is harmless to correctness — ISO-TP
	/// compares the id exactly — and only costs queue room.
	pub fn covering(ids: impl IntoIterator<Item = u16>) -> Option<Self> {
		let (mut common, mut any, mut seen) = (SFF, 0u16, false);
		for id in ids {
			common &= id & SFF;
			any |= id & SFF;
			seen = true;
		}
		seen.then_some(StandardFilter {
			code: common,
			must_match: !(any & !common) & SFF,
		})
	}

	/// Whether a frame on `id` passes.
	pub fn accepts(&self, id: u16) -> bool {
		(id & SFF) & self.must_match == self.code & self.must_match
	}
}

/// Decides, before each exchange, whether the controller's filter has to move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterFollower {
	plan: Option<StandardFilter>,
	/// What the controller holds now; `None` while it holds no filter at all.
	current: Option<StandardFilter>,
}

impl FilterFollower {
	/// `plan` is the filter the controller was started with (`None`: started
	/// unfiltered, as with a plan that polls nobody).
	pub fn new(plan: Option<StandardFilter>) -> Self {
		FilterFollower { plan, current: plan }
	}

	/// The filter to put on the controller before an exchange answered on
	/// `response`, or `None` when the one it holds already passes it.
	///
	/// - the current filter passes it: keep it (no reconfiguration);
	/// - the plan's filter passes it: go back to the plan's, so the panel's own
	///   exchanges that follow need nothing more;
	/// - otherwise: exactly `response`, the narrowest there is.
	///
	/// An unfiltered controller is never taken as passing: on a live bus it is
	/// the state that loses answers, so the first exchange narrows it.
	pub fn before(&mut self, response: u16) -> Option<StandardFilter> {
		if self.current.is_some_and(|f| f.accepts(response)) {
			return None;
		}
		let next = match self.plan {
			Some(plan) if plan.accepts(response) => plan,
			_ => StandardFilter::exact(response),
		};
		self.current = Some(next);
		Some(next)
	}

	/// The filter the controller holds now.
	pub fn current(&self) -> Option<StandardFilter> {
		self.current
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn exact_passes_one_id() {
		let f = StandardFilter::exact(0x77A);
		assert!(f.accepts(0x77A));
		assert!(!f.accepts(0x77B));
		assert!(!f.accepts(0x7E8));
	}

	#[test]
	fn covering_the_iso_pair_costs_one_bit() {
		let f = StandardFilter::covering([0x7E8, 0x7E9]).unwrap();
		assert_eq!(f.must_match, 0x7FE);
		assert!(f.accepts(0x7E8) && f.accepts(0x7E9));
		assert!(!f.accepts(0x7EA) && !f.accepts(0x77A) && !f.accepts(0x100));
	}

	#[test]
	fn covering_scattered_ids_is_a_superset_that_passes_them_all() {
		let ids = [0x7E8, 0x77A, 0x710];
		let f = StandardFilter::covering(ids).unwrap();
		for id in ids {
			assert!(f.accepts(id), "{id:03X}");
		}
		assert_eq!(StandardFilter::covering([]), None);
	}

	#[test]
	fn the_plans_filter_stays_while_its_units_are_asked() {
		let plan = StandardFilter::covering([0x7E8, 0x7E9]);
		let mut follower = FilterFollower::new(plan);
		assert_eq!(follower.before(0x7E8), None);
		assert_eq!(follower.before(0x7E9), None);
		assert_eq!(follower.current(), plan);
	}

	#[test]
	fn a_unit_outside_the_plan_moves_the_filter_to_its_id_and_the_plan_brings_it_back() {
		let plan = StandardFilter::covering([0x7E8, 0x7E9]);
		let mut follower = FilterFollower::new(plan);
		assert_eq!(follower.before(0x77A), Some(StandardFilter::exact(0x77A)));
		assert_eq!(follower.before(0x77A), None, "a second request to the gateway needs nothing");
		assert_eq!(follower.before(0x7E9), plan, "the panel's exchange restores the plan's filter");
		assert_eq!(follower.before(0x7E8), None);
	}

	#[test]
	fn an_unfiltered_controller_is_narrowed_by_its_first_exchange() {
		let mut follower = FilterFollower::new(None);
		assert_eq!(follower.before(0x7E8), Some(StandardFilter::exact(0x7E8)));
		assert_eq!(follower.before(0x7E8), None);
		assert_eq!(follower.before(0x77A), Some(StandardFilter::exact(0x77A)));
	}
}
