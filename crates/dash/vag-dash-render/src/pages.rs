//! Which page a page turn goes to.
//!
//! Three lines, and they are here rather than in the firmware's `Config` for
//! the same reason the alarm and button machines are: the firmware cannot be
//! built for the host, and a page index that wraps wrong or walks off the end
//! of the list is the kind of bug a test catches in a millisecond and a
//! person on the car reports as "the pages switch strangely".

use crate::plan::Page;

/// How many pages the board holds. The plan generator refuses a `dash.toml`
/// with more, so every plan page index — an alarm's page among them — is one
/// the board has, and a page count fits the `u8` its settings keep the active
/// page in. One definition, for both ends.
pub const MAX_PAGES: usize = 8;

/// How many cells a values page shows, side by side. The plan generator refuses a page
/// with more; the board's `state` line is sized by it.
pub const MAX_CELLS: usize = 4;

/// The page after `active`, of `count` pages, wrapping at the end.
///
/// With one page the answer is that page. With none the answer is `0`, which
/// is what the caller's bounds check makes of it. An `active` past the end
/// goes to the first page rather than arithmetic on a number that means
/// nothing — defensive; the board's settings take a stored page past this
/// plan's pages to the first upstream (`vag-dash-fw`'s `schema::fit`).
pub fn next(active: u8, count: u8) -> u8 {
	if active >= count {
		return 0;
	}
	(active + 1) % count
}

/// The page before `active`, of `count` pages, wrapping at the start — what the
/// lever's `previous` turns to (`todo/dash/19`).
///
/// With one page the answer is that page, with none `0`. An `active` past the
/// end goes to the first page, as in [`next`].
pub fn previous(active: u8, count: u8) -> u8 {
	if active >= count {
		return 0;
	}
	match active {
		0 => count - 1,
		_ => active - 1,
	}
}

/// The plan channels a page shows: a values page's cells, or a chart's one channel — what
/// the board reads for a page, and what its `state` line lists as `cells=`.
pub fn cells(page: &Page) -> &[u16] {
	match page {
		Page::Values { cells, .. } => cells,
		Page::Chart { channel, .. } => core::slice::from_ref(channel),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_page_shows_its_cells_or_its_charts_one_channel() {
		let values = Page::Values {
			title: "engine",
			cells: &[0, 1, 2, 3],
			volts: true,
		};
		let chart = Page::Chart {
			title: "",
			channel: 1,
			min: 0.0,
			max: 2.5,
			volts: true,
		};
		assert_eq!(cells(&values), &[0, 1, 2, 3]);
		assert_eq!(cells(&chart), &[1]);
	}

	#[test]
	fn it_advances_by_one() {
		assert_eq!(next(0, 3), 1);
		assert_eq!(next(1, 3), 2);
	}

	#[test]
	fn it_wraps_from_the_last_page_to_the_first() {
		assert_eq!(next(2, 3), 0);
	}

	#[test]
	fn with_one_page_it_stays_on_it() {
		assert_eq!(next(0, 1), 0);
	}

	#[test]
	fn with_no_pages_it_stays_at_zero() {
		assert_eq!(next(0, 0), 0);
	}

	#[test]
	fn an_active_page_past_the_end_goes_to_the_first() {
		assert_eq!(next(7, 3), 0);
		assert_eq!(next(u8::MAX, 8), 0);
	}

	#[test]
	fn previous_steps_back_by_one_and_wraps_from_the_first_to_the_last() {
		assert_eq!(previous(2, 3), 1);
		assert_eq!(previous(1, 3), 0);
		assert_eq!(previous(0, 3), 2);
		assert_eq!(previous(0, 1), 0, "one page stays on it");
		assert_eq!(previous(0, 0), 0, "no pages stays at zero");
		assert_eq!(previous(7, 3), 0, "past the end goes to the first, as `next` does");
	}

	#[test]
	fn previous_undoes_next() {
		for count in 1..=8u8 {
			for active in 0..count {
				assert_eq!(previous(next(active, count), count), active, "{active} of {count}");
				assert!(previous(active, count) < count);
			}
		}
	}

	#[test]
	fn it_always_stays_in_range() {
		for count in 1..=8u8 {
			for active in 0..=u8::MAX {
				assert!(next(active, count) < count, "next({active}, {count})");
			}
		}
	}
}
