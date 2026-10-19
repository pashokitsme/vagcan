//! The board's settings record, on the host: every version it has written loads, and a record
//! saved against another plan keeps what does not depend on the plan.
//!
//! `crates/dash/vag-dash-fw/src/schema.rs` compiled as it is — the firmware cannot be built
//! for the host, and a board in a car holds a record an older image wrote.

#[path = "../../../../crates/dash/vag-dash-fw/src/schema.rs"]
mod schema;

use schema::{Config, Fitted, SCHEMA_VERSION, SavedAgainst, decode, fit};

/// A version-1 record as the image before `last_run` wrote it, byte by byte in postcard's
/// words: brightness `200` (a `u8`, one byte), active page `1`, two pages — a values page of
/// cells 0, 1, 300 (`300` a two-byte varint) and a chart of cell 3. Written out rather than
/// serialised here, so the test holds the old layout and not today's derive.
const V1: [u8; 12] = [200, 1, 2, 1, 3, 0, 1, 0xAC, 0x02, 0, 1, 3];

/// Version 2: version 1's record with a run appended — two marks, 60 km/h in 5430 ms
/// (`0xB6 0x2A`) and 100 km/h in 9870 ms (`0x8E 0x4D`).
const V2: [u8; 19] = [200, 1, 2, 1, 3, 0, 1, 0xAC, 0x02, 0, 1, 3, 2, 60, 0xB6, 0x2A, 100, 0x8E, 0x4D];

/// The plan the board runs in these tests.
const PLAN: u64 = 0x7fa5_728f_35ff_8bcd;

fn run(marks: &[(u16, u32)]) -> heapless::Vec<(u16, u32), 3> {
	heapless::Vec::from_slice(marks).unwrap()
}

fn record(active_page: u8, plan: Option<u64>) -> Config {
	Config {
		brightness: 77,
		active_page,
		last_run: run(&[(60, 5_430)]),
		plan,
	}
}

#[test]
fn a_version_one_record_loads_its_brightness_and_page_and_no_plan() {
	let config = decode(1, &V1).expect("decodes").expect("a version this image knows");
	assert_eq!(
		config,
		Config {
			brightness: 200,
			active_page: 1,
			last_run: run(&[]),
			plan: None,
		},
		"its pages are dropped, and which plan they were it never said"
	);
}

#[test]
fn a_version_two_record_loads_its_run_too() {
	let config = decode(2, &V2).expect("decodes").expect("a version this image knows");
	assert_eq!((config.brightness, config.active_page), (200, 1));
	assert_eq!(config.last_run, run(&[(60, 5_430), (100, 9_870)]));
	assert_eq!(config.plan, None);
}

#[test]
fn the_current_record_round_trips_and_holds_no_pages() {
	let config = record(1, Some(PLAN));
	let bytes = postcard::to_allocvec(&config).unwrap();
	assert_eq!(decode(SCHEMA_VERSION, &bytes).unwrap(), Some(config));
	// Brightness, page, one mark, then `Some` and the id's varint: nothing of the plan's pages.
	assert_eq!(bytes, [77, 1, 1, 60, 0xB6, 0x2A, 1, 0xCD, 0x97, 0xFE, 0xAF, 0xF3, 0xD1, 0xDC, 0xD2, 0x7F]);
}

#[test]
fn a_version_this_image_does_not_know_is_nothing_and_garbage_is_an_error() {
	assert_eq!(decode(SCHEMA_VERSION + 1, &V1).unwrap(), None, "a newer image's record is not guessed at");
	assert_eq!(decode(0, &V1).unwrap(), None);
	assert!(decode(1, &[200, 1, 9]).is_err(), "nine pages where version 1 held eight");
	assert!(decode(2, &V1).is_err(), "a version-1 record read as version 2 is short");
	assert!(decode(SCHEMA_VERSION, &[77, 1]).is_err(), "a version-3 record ends in its run and plan");
}

#[test]
fn saved_against_this_plan_everything_is_as_stored() {
	let stored = record(2, Some(PLAN));
	assert_eq!(
		fit(stored.clone(), PLAN, 3),
		Fitted {
			config: stored,
			saved_against: SavedAgainst::This,
			page_reset: None,
		}
	);
}

/// The fix of 2026-10-19: a `dash.toml` change made every stored record "not fit this plan",
/// and brightness and the page went back to the defaults with the copy of the pages.
#[test]
fn saved_against_another_plan_brightness_the_run_and_a_page_this_plan_has_are_kept() {
	let other = PLAN ^ 1;
	let fitted = fit(record(2, Some(other)), PLAN, 3);
	assert_eq!(fitted.saved_against, SavedAgainst::Other(other));
	assert_eq!(fitted.page_reset, None);
	assert_eq!(fitted.config, record(2, Some(PLAN)), "and the next save names this plan");
}

#[test]
fn a_page_this_plan_does_not_have_is_the_first() {
	let fitted = fit(record(5, Some(PLAN ^ 1)), PLAN, 3);
	assert_eq!(fitted.page_reset, Some(5));
	assert_eq!(fitted.config.active_page, 0);
	assert_eq!(fitted.config.brightness, 77, "the rest stays");
	assert_eq!(fitted.config.last_run, run(&[(60, 5_430)]));
	// The last page is a page.
	assert_eq!(fit(record(2, Some(PLAN ^ 1)), PLAN, 3).page_reset, None);
	assert_eq!(fit(record(3, Some(PLAN ^ 1)), PLAN, 3).page_reset, Some(3));
	// Page 0 of the empty plan stands; nothing else does.
	assert_eq!(fit(record(0, Some(PLAN)), PLAN, 0).page_reset, None);
	assert_eq!(fit(record(1, Some(PLAN)), PLAN, 0).page_reset, Some(1));
}

#[test]
fn an_old_record_says_no_plan_and_keeps_what_it_held() {
	let old = decode(2, &V2).unwrap().unwrap();
	let fitted = fit(old, PLAN, 2);
	assert_eq!(fitted.saved_against, SavedAgainst::Unknown);
	assert_eq!((fitted.config.brightness, fitted.config.active_page), (200, 1));
	assert_eq!(fitted.config.last_run.len(), 2);
	assert_eq!(fitted.config.plan, Some(PLAN));
	// Its page past a plan of one page: the first.
	let fitted = fit(decode(1, &V1).unwrap().unwrap(), PLAN, 1);
	assert_eq!((fitted.config.active_page, fitted.page_reset), (0, Some(1)));
}

fn slot(generation: u32, version: u16) -> Option<schema::Slot> {
	let config = decode(1, &V1).unwrap().unwrap();
	Some(schema::Slot {
		generation,
		version,
		config: (version <= SCHEMA_VERSION).then_some(config),
	})
}

/// PR #12 review: a record of a version this image does not know was dropped with its
/// generation. Slot 0 holding a newer image's generation 4 and slot 1 its generation 5, this
/// image booted on generation 0, and its first save — the automatic one of a stopwatch run —
/// went into slot 1 as generation 1, over the newest record there was.
#[test]
fn a_record_this_image_cannot_read_still_counts_for_where_and_under_what_a_save_goes() {
	let newer = SCHEMA_VERSION + 1;
	let held = schema::held([slot(4, newer), slot(5, newer)]);
	assert_eq!(held.newest, Some((1, 5)), "the next save goes to slot 0, as generation 6");
	assert_eq!(held.config, None, "nothing this image reads");
	assert_eq!(held.unreadable, Some(newer), "and only `save` may write over it");
}

#[test]
fn an_older_record_this_image_reads_is_loaded_behind_a_newer_one_it_cannot() {
	let newer = SCHEMA_VERSION + 1;
	let held = schema::held([slot(4, SCHEMA_VERSION), slot(5, newer)]);
	assert_eq!(held.newest, Some((1, 5)));
	assert!(held.config.is_some(), "generation 4, the last this image could read");
	assert_eq!(held.unreadable, Some(newer));
	// Written after the newer image's record, the readable one is simply the newest.
	let held = schema::held([slot(4, newer), slot(5, SCHEMA_VERSION)]);
	assert_eq!((held.newest, held.unreadable), (Some((1, 5)), None));
	assert!(held.config.is_some());
}

#[test]
fn the_newest_record_is_the_one_with_the_higher_generation_in_either_slot() {
	assert_eq!(schema::held([None, None]), schema::Held::default(), "a new board: nothing");
	let held = schema::held([slot(7, 1), None]);
	assert_eq!((held.newest, held.unreadable), (Some((0, 7)), None));
	assert!(held.config.is_some(), "a version-1 record, carried forward");
	let held = schema::held([slot(9, SCHEMA_VERSION), slot(8, SCHEMA_VERSION)]);
	assert_eq!(held.newest, Some((0, 9)));
}
