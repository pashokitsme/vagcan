//! Which page a short press goes to.
//!
//! Three lines, and they are here rather than in the firmware's `Config` for
//! the same reason the alarm and button machines are: the firmware cannot be
//! built for the host, and a page index that wraps wrong or walks off the end
//! of the list is the kind of bug a test catches in a millisecond and a
//! person on the car reports as "the pages switch strangely".

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

#[cfg(test)]
mod tests {
	use super::*;

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
	fn it_always_stays_in_range() {
		for count in 1..=8u8 {
			for active in 0..=u8::MAX {
				assert!(next(active, count) < count, "next({active}, {count})");
			}
		}
	}
}
