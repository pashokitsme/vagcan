//! The settings record as flash holds it, and every version of it the board has written.
//!
//! Pure on purpose: serde, `heapless`, `postcard` and two of the renderer's bounds, nothing of
//! the plan and nothing of the chip. The firmware cannot be built for the host, so this is the
//! one part of the settings a host test can reach — `research/dash/host/tests/settings_schema.rs`
//! compiles this file as it is and loads byte images of every version through [`decode`].
//! What the plan decides about a configuration (its defaults, whether it fits) is
//! [`crate::config`]'s.
//!
//! A version is kept readable, never discarded: a board in a car holds the owner's settings,
//! and a new image that forgot them would be a regression nobody asked for. Each version's
//! record is the one before with fields appended — postcard writes a struct as its fields in
//! order — so an old record is decoded under its own shape and carried forward, and the next
//! save writes the current one.

use serde::{Deserialize, Serialize};
use vag_dash_render::pages::MAX_PAGES;
use vag_dash_render::stopwatch::MAX_MARKS;

/// How many cells fit on one page, bounded because a configuration has to fit in a flash
/// sector with room for its header.
pub const MAX_CELLS: usize = 8;

/// The version [`Config`] is written under. Bumped whenever the record changes; every older
/// one stays readable through [`decode`].
///
/// - 1: brightness, active page, pages.
/// - 2 (2026-09-26): [`Config::last_run`] appended.
pub const SCHEMA_VERSION: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PageKind {
	/// One channel, large, with a sparkline.
	Chart,
	/// Up to four columns: small label over a large number.
	Values,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
	pub kind: PageKind,
	/// Indices into the flashed plan. Not identifiers — indices.
	pub cells: heapless::Vec<u16, MAX_CELLS>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
	/// 0..=255, straight to the panel's contrast register.
	pub brightness: u8,
	/// Which page is showing when the device wakes up.
	pub active_page: u8,
	pub pages: heapless::Vec<Page, MAX_PAGES>,
	/// The stopwatch's last finished run (`todo/dash/19`): each mark in km/h with its time in
	/// milliseconds. Kept by the mark it timed rather than by its place, so a plan whose
	/// marks changed shows a dash for a mark this run never timed, not another mark's time.
	pub last_run: heapless::Vec<(u16, u32), MAX_MARKS>,
}

/// Version 1's record: [`Config`] before `last_run`.
#[derive(Deserialize)]
struct ConfigV1 {
	brightness: u8,
	active_page: u8,
	pages: heapless::Vec<Page, MAX_PAGES>,
}

/// A stored payload under the version its header names: `Ok(None)` for a version this image
/// does not know — one written by a newer image, read as nothing rather than guessed at —
/// and an error where the bytes do not decode under their own version.
pub fn decode(version: u16, payload: &[u8]) -> Result<Option<Config>, postcard::Error> {
	match version {
		SCHEMA_VERSION => postcard::from_bytes(payload).map(Some),
		1 => {
			let old: ConfigV1 = postcard::from_bytes(payload)?;
			Ok(Some(Config {
				brightness: old.brightness,
				active_page: old.active_page,
				pages: old.pages,
				last_run: heapless::Vec::new(),
			}))
		}
		_ => Ok(None),
	}
}
