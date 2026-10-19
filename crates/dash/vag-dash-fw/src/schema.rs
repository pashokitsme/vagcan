//! The settings record as flash holds it, and every version of it the board has written.
//!
//! Pure on purpose: serde, `heapless`, `postcard` and one of the renderer's bounds, nothing of
//! the plan and nothing of the chip. The firmware cannot be built for the host, so this is the
//! one part of the settings a host test can reach — `research/dash/host/tests/settings_schema.rs`
//! compiles this file as it is, loads byte images of every version through [`decode`], picks
//! the newest of two slots through [`held`], a newer image's record among them, and fits a
//! record to a plan through [`fit`]. The plan's own id and page count are handed in;
//! [`crate::config`] is where they come from.
//!
//! A version is kept readable, never discarded: a board in a car holds the owner's settings,
//! and a new image that forgot them would be a regression nobody asked for. Each version is
//! decoded under its own shape — postcard writes a struct as its fields in order, and an old
//! shape is kept here for that alone — and carried forward as the current one; the next save
//! writes the current one.

use serde::{Deserialize, Serialize};
use vag_dash_render::stopwatch::MAX_MARKS;

/// The version [`Config`] is written under. Bumped whenever the record changes; every older
/// one stays readable through [`decode`].
///
/// - 1: brightness, active page, pages.
/// - 2 (2026-09-26): [`Config::last_run`] appended.
/// - 3 (2026-10-19): the pages dropped, [`Config::plan`] appended. Nothing on the board ever
///   edited the pages; the copy only made every `dash.toml` change read as settings that did
///   not fit, and reset brightness and the page with them.
pub const SCHEMA_VERSION: u16 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
	/// 0..=255, straight to the panel's contrast register.
	pub brightness: u8,
	/// Which page is showing when the device wakes up: an index into the plan's pages.
	pub active_page: u8,
	/// The stopwatch's last finished run (`todo/dash/19`): each mark in km/h with its time in
	/// milliseconds. Kept by the mark it timed rather than by its place, so a plan whose
	/// marks changed shows a dash for a mark this run never timed, not another mark's time.
	pub last_run: heapless::Vec<(u16, u32), MAX_MARKS>,
	/// The id of the plan this was saved against (`vag_cli_core::dash::plan_id`): `None` for
	/// a record from before records named one. What it says is [`fit`]'s to judge.
	pub plan: Option<u64>,
}

/// The pages and cells versions 1 and 2 were bounded by — eight of each, then. Frozen here:
/// they are a property of bytes already in flash, not of any plan.
const OLD_PAGES: usize = 8;
const OLD_CELLS: usize = 8;

/// A page as versions 1 and 2 kept one — its kind, its cells — read past and dropped. A
/// tuple: postcard writes a struct and a tuple alike, its fields in order.
type OldPage = (OldKind, heapless::Vec<u16, OLD_CELLS>);

#[derive(Deserialize)]
enum OldKind {
	Chart,
	Values,
}

/// Version 1's record: brightness, active page, pages.
type ConfigV1 = (u8, u8, heapless::Vec<OldPage, OLD_PAGES>);

/// Version 2's record: version 1's with the run appended.
type ConfigV2 = (u8, u8, heapless::Vec<OldPage, OLD_PAGES>, heapless::Vec<(u16, u32), MAX_MARKS>);

/// A stored payload under the version its header names: `Ok(None)` for a version this image
/// does not know — one written by a newer image, read as nothing rather than guessed at —
/// and an error where the bytes do not decode under their own version. A record from before
/// version 3 comes back with no plan: which plan its pages were is not something it said.
pub fn decode(version: u16, payload: &[u8]) -> Result<Option<Config>, postcard::Error> {
	match version {
		SCHEMA_VERSION => postcard::from_bytes(payload).map(Some),
		2 => {
			let (brightness, active_page, _pages, last_run): ConfigV2 = postcard::from_bytes(payload)?;
			Ok(Some(Config {
				brightness,
				active_page,
				last_run,
				plan: None,
			}))
		}
		1 => {
			let (brightness, active_page, _pages): ConfigV1 = postcard::from_bytes(payload)?;
			Ok(Some(Config {
				brightness,
				active_page,
				last_run: heapless::Vec::new(),
				plan: None,
			}))
		}
		_ => Ok(None),
	}
}

/// Which plan a stored record was saved against, as [`fit`] found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavedAgainst {
	/// The plan this image runs.
	This,
	/// Another plan, by its id: the image was built from another `dash.toml`, or from the
	/// same one by another generator.
	Other(u64),
	/// It does not say: a record from before records named their plan.
	Unknown,
}

/// A stored record made this plan's ([`fit`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fitted {
	pub config: Config,
	pub saved_against: SavedAgainst,
	/// The stored active page, where this plan has no such page and the first is shown instead.
	pub page_reset: Option<u8>,
}

/// A stored record on the plan `plan` with `pages` pages: what the board runs on.
///
/// Nothing in it is dropped for having been saved against another plan. Brightness is the
/// panel's, not the plan's. The run is kept by the marks it timed, so another plan's marks
/// show a dash where this run timed none. The active page is kept where this plan has it and
/// is the first page where it does not — a page index is the one field another plan can make
/// meaningless, and only a page past the end is told apart from one that moved. The record
/// comes back under `plan`, the id the next save writes.
///
/// With no pages at all (the empty plan CI builds, which nobody flashes) page 0 stands: it
/// is what the panel's bounds checks make of no page.
pub fn fit(stored: Config, plan: u64, pages: usize) -> Fitted {
	let saved_against = match stored.plan {
		Some(id) if id == plan => SavedAgainst::This,
		Some(id) => SavedAgainst::Other(id),
		None => SavedAgainst::Unknown,
	};
	let mut config = stored;
	let page_reset = (usize::from(config.active_page) >= pages.max(1)).then_some(config.active_page);
	if page_reset.is_some() {
		config.active_page = 0;
	}
	config.plan = Some(plan);
	Fitted {
		config,
		saved_against,
		page_reset,
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
