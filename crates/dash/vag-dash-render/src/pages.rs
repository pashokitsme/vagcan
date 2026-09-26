//! Which page a short press goes to.
//!
//! Three lines, and they are here rather than in the firmware's `Config` for
//! the same reason the alarm and button machines are: the firmware cannot be
//! built for the host, and a page index that wraps wrong or walks off the end
//! of the list is the kind of bug a test catches in a millisecond and a
//! person on the car reports as "the pages switch strangely".
//!
//! Also here: whether a configuration stored on the board still describes the
//! plan's pages ([`mismatch`]), for the same reason.

use crate::plan::Page;

/// How many pages the board holds. Its stored configuration is bounded by a
/// flash sector, so the page list is too; the plan generator refuses a
/// `dash.toml` with more, so every plan page index — an alarm's page among
/// them — is one the board has. One definition, for both ends.
pub const MAX_PAGES: usize = 8;

/// The page after `active`, of `count` pages, wrapping at the end.
///
/// With one page the answer is that page. With none the answer is `0`, which
/// is what the caller's bounds check makes of it. An `active` past the end
/// goes to the first page rather than arithmetic on a number that means
/// nothing — defensive; `Config::validate` rejects such a value upstream.
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

/// One page as a stored configuration holds it: a chart or a values page, and
/// the plan indices it shows. The firmware's `config::Page` in borrowed form —
/// that type cannot be built for the host, and this comparison has to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout<'a> {
	pub chart: bool,
	pub cells: &'a [u16],
}

/// The plan's pages as a configuration holds them: at most `max_pages` of them,
/// at most `max_cells` cells each, a chart as its one channel. What the
/// firmware's `Config::default()` is built from, so the default and the check
/// in [`mismatch`] cannot disagree about what "the plan's pages" are.
pub fn from_plan(plan: &[Page], max_pages: usize, max_cells: usize) -> impl Iterator<Item = Layout<'_>> {
	plan.iter().take(max_pages).map(move |page| match page {
		Page::Values { cells, .. } => Layout {
			chart: false,
			cells: &cells[..cells.len().min(max_cells)],
		},
		Page::Chart { channel, .. } => Layout {
			chart: true,
			cells: core::slice::from_ref(channel),
		},
	})
}

/// How a stored configuration's pages differ from the plan's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mismatch {
	/// A different number of pages.
	Count { stored: usize, plan: usize },
	/// Page `index` (zero-based) is another kind, or shows other cells.
	Page { index: usize },
}

/// Whether a stored configuration's pages are the plan's pages, and if not,
/// the first difference.
///
/// Nothing on the device edits pages — its commands set brightness and the
/// active page, and every page list it has ever saved was
/// [`from_plan`]'s. So a stored list that differs from the plan's is not a
/// person's choice being honoured: it is a configuration saved by an image
/// built from an older `dash.toml`, and honouring it hides whatever the new
/// plan added (a chart page, on 2026-09-13). The plan's pages win. When pages
/// become editable on the device, this is the check that has to become a
/// record of which plan a configuration was saved against.
///
/// A plan with no pages enforces nothing: the generator refuses one, and the
/// firmware's fallback page for it is not the plan's to judge.
pub fn mismatch<'a>(stored: impl IntoIterator<Item = Layout<'a>>, plan: &[Page], max_pages: usize, max_cells: usize) -> Option<Mismatch> {
	if plan.is_empty() {
		return None;
	}
	let mut expected = from_plan(plan, max_pages, max_cells);
	let mut stored = stored.into_iter();
	let mut index = 0;
	loop {
		match (stored.next(), expected.next()) {
			(None, None) => return None,
			(Some(got), Some(want)) if got == want => index += 1,
			(Some(_), Some(_)) => return Some(Mismatch::Page { index }),
			(got, want) => {
				return Some(Mismatch::Count {
					stored: index + usize::from(got.is_some()) + stored.count(),
					plan: index + usize::from(want.is_some()) + expected.count(),
				});
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const MAX_PAGES: usize = 8;
	const MAX_CELLS: usize = 8;

	/// `dash.toml` before the owner added a chart: one values page.
	const BEFORE: &[Page] = &[Page::Values {
		title: "engine",
		cells: &[0, 1, 2, 3],
	}];
	/// And after: the same values page, then a chart.
	const AFTER: &[Page] = &[
		Page::Values {
			title: "engine",
			cells: &[0, 1, 2, 3],
		},
		Page::Chart {
			channel: 1,
			min: 0.0,
			max: 2.5,
		},
	];

	#[test]
	fn a_config_saved_before_a_chart_page_was_added_does_not_match() {
		// 2026-09-13, "I never managed to get the charts on screen": the
		// stored configuration still listed only the old values page, passed
		// every cell check, and the plan's new chart page never appeared.
		let stored: std::vec::Vec<Layout<'_>> = from_plan(BEFORE, MAX_PAGES, MAX_CELLS).collect();
		assert_eq!(
			mismatch(stored, AFTER, MAX_PAGES, MAX_CELLS),
			Some(Mismatch::Count { stored: 1, plan: 2 })
		);
	}

	#[test]
	fn a_page_of_another_kind_or_other_cells_does_not_match() {
		let chart_became_values = [
			Layout {
				chart: false,
				cells: &[0, 1, 2, 3],
			},
			Layout { chart: false, cells: &[1] },
		];
		assert_eq!(
			mismatch(chart_became_values, AFTER, MAX_PAGES, MAX_CELLS),
			Some(Mismatch::Page { index: 1 })
		);
		let other_cells = [
			Layout {
				chart: false,
				cells: &[0, 1, 2, 4],
			},
			Layout { chart: true, cells: &[1] },
		];
		assert_eq!(mismatch(other_cells, AFTER, MAX_PAGES, MAX_CELLS), Some(Mismatch::Page { index: 0 }));
	}

	#[test]
	fn the_plans_own_pages_match() {
		assert_eq!(mismatch(from_plan(AFTER, MAX_PAGES, MAX_CELLS), AFTER, MAX_PAGES, MAX_CELLS), None);
	}

	#[test]
	fn a_plan_past_the_storage_bounds_matches_its_truncated_self() {
		// The default drops pages past `max_pages` and cells past `max_cells`,
		// so a saved default must still match — or it could never be saved.
		let wide: &[Page] = &[
			Page::Values {
				title: "a",
				cells: &[0, 1, 2],
			},
			Page::Chart {
				channel: 0,
				min: 0.0,
				max: 1.0,
			},
			Page::Chart {
				channel: 1,
				min: 0.0,
				max: 1.0,
			},
		];
		let stored: std::vec::Vec<Layout<'_>> = from_plan(wide, 2, 2).collect();
		assert_eq!(stored.len(), 2);
		assert_eq!(stored[0].cells, &[0, 1]);
		assert_eq!(mismatch(stored.iter().copied(), wide, 2, 2), None);
	}

	#[test]
	fn a_plan_with_no_pages_enforces_no_layout() {
		// The generator refuses such a plan; the firmware falls back to a
		// values page of its own. Refusing that fallback would refuse every save.
		let fallback = [Layout { chart: false, cells: &[0] }];
		assert_eq!(mismatch(fallback, &[], MAX_PAGES, MAX_CELLS), None);
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
