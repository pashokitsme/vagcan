//! The board's settings record, on the host: every version it has written loads.
//!
//! `crates/dash/vag-dash-fw/src/schema.rs` compiled as it is — the firmware cannot be built
//! for the host, and a board in a car holds a record an older image wrote.

#[path = "../../../../crates/dash/vag-dash-fw/src/schema.rs"]
mod schema;

use schema::{Page, PageKind, SCHEMA_VERSION, decode};

/// A version-1 record as the image before `last_run` wrote it, byte by byte in postcard's
/// words: brightness `200` (a `u8`, one byte), active page `1`, two pages — a values page of
/// cells 0, 1, 300 (`300` a two-byte varint) and a chart of cell 3. Written out rather than
/// serialised here, so the test holds the old layout and not today's derive.
const V1: [u8; 12] = [200, 1, 2, 1, 3, 0, 1, 0xAC, 0x02, 0, 1, 3];

fn cells(list: &[u16]) -> heapless::Vec<u16, { schema::MAX_CELLS }> {
	heapless::Vec::from_slice(list).unwrap()
}

#[test]
fn a_version_one_record_loads_with_every_field_it_had() {
	let config = decode(1, &V1).expect("decodes").expect("a version this image knows");
	assert_eq!(config.brightness, 200);
	assert_eq!(config.active_page, 1);
	assert_eq!(
		config.pages.as_slice(),
		[
			Page {
				kind: PageKind::Values,
				cells: cells(&[0, 1, 300])
			},
			Page {
				kind: PageKind::Chart,
				cells: cells(&[3])
			},
		]
	);
	assert!(config.last_run.is_empty(), "no run was ever kept by an image without the stopwatch");
}

#[test]
fn the_current_record_round_trips_and_is_the_old_one_with_the_run_appended() {
	let mut config = decode(1, &V1).unwrap().unwrap();
	config.last_run.push((60, 5_430)).unwrap();
	config.last_run.push((100, 9_870)).unwrap();
	let bytes = postcard::to_allocvec(&config).unwrap();
	assert_eq!(decode(SCHEMA_VERSION, &bytes).unwrap(), Some(config.clone()));
	assert_eq!(&bytes[..V1.len()], V1, "a field is appended, never inserted");
	// A migrated record saved again is a current one.
	let mut empty = config;
	empty.last_run.clear();
	assert_eq!(postcard::to_allocvec(&empty).unwrap(), [&V1[..], &[0]].concat());
}

#[test]
fn a_version_this_image_does_not_know_is_nothing_and_garbage_is_an_error() {
	assert_eq!(decode(SCHEMA_VERSION + 1, &V1).unwrap(), None, "a newer image's record is not guessed at");
	assert_eq!(decode(0, &V1).unwrap(), None);
	assert!(decode(1, &[200, 1, 9]).is_err(), "nine pages where the board holds eight");
	assert!(decode(SCHEMA_VERSION, &V1).is_err(), "a version-1 record read as version 2 is short");
}
