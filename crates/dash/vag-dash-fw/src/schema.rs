//! The settings record as flash holds it, and every version of it the board has written.
//!
//! Pure on purpose: serde, `heapless`, `postcard` and two of the renderer's bounds, nothing of
//! the plan and nothing of the chip. The firmware cannot be built for the host, so this is the
//! one part of the settings a host test can reach — `research/dash/host/tests/settings_schema.rs`
//! compiles this file as it is, loads byte images of every version through [`decode`], and
//! picks the newest of two slots through [`held`], a newer image's record among them.
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

/// One slot of flash whose magic, length and CRC check out: a record some image wrote whole,
/// whether or not this one can read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
	pub generation: u32,
	/// The version its header names.
	pub version: u16,
	/// The record, where this image knows its version and its bytes decode under it.
	pub config: Option<Config>,
}

/// What the slots hold, taken together ([`held`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Held {
	/// The newest record's slot and generation, whatever its version and whether or not this
	/// image reads it. A save goes to another slot under the next generation, so it never goes
	/// out under a lower generation than one in flash and never overwrites the newest record.
	pub newest: Option<(u32, u32)>,
	/// The newest record this image reads: the newest record, or where that is one it cannot
	/// read, the one before it.
	pub config: Option<Config>,
	/// The newest record's version, where this image cannot read it — a newer image wrote it
	/// (or the bytes do not decode under their own version). Only an explicit `save`
	/// overwrites it; a write nobody asked for, like a stopwatch run's, does not.
	pub unreadable: Option<u16>,
}

/// The slots, taken together, in slot order: which is the newest record, and what of flash
/// this image can use.
///
/// Every slot whose header and CRC check out counts for the newest, whatever its version. A
/// record this image did not know used to be dropped with its generation: with a newer
/// image's generations 4 and 5 in the two slots this image started from generation 0, and
/// its first save went into slot 1 as generation 1 — over the newest record, under a lower
/// generation (PR #12 review).
pub fn held(slots: impl IntoIterator<Item = Option<Slot>>) -> Held {
	let mut held = Held::default();
	let mut readable: Option<u32> = None;
	for (index, slot) in slots.into_iter().enumerate() {
		let Some(slot) = slot else { continue };
		if held.newest.is_none_or(|(_, newest)| slot.generation > newest) {
			held.newest = Some((index as u32, slot.generation));
			held.unreadable = slot.config.is_none().then_some(slot.version);
		}
		match slot.config {
			Some(config) if readable.is_none_or(|newest| slot.generation > newest) => {
				readable = Some(slot.generation);
				held.config = Some(config);
			}
			_ => {}
		}
	}
	held
}
