//! What the panel shows, and how — the part a person changes.
//!
//! Everything the device could ever *decode* is flashed with the firmware: the
//! catalogs live on the laptop, the image is built for one car, and the board
//! has neither the memory nor the reason to resolve anything at run time. So a
//! cell here is an **index into the plan that is already in the image**. That
//! is not a restriction bolted on for safety; it is the only thing the type can
//! express. A forty-first identifier is not refused, it is unsayable.

use serde::{Deserialize, Serialize};
use vag_dash_render::plan::Page as PlanPage;

use crate::plan::PLAN;

/// How many pages the panel can hold, and how many cells fit on one. Both are
/// bounded because the storage is: a configuration has to fit in a flash
/// sector with room for its header.
pub const MAX_PAGES: usize = 8;
pub const MAX_CELLS: usize = 8;

/// Bumped whenever the meaning of a field changes. A stored blob whose version
/// is not this one is ignored rather than reinterpreted — a configuration read
/// under the wrong schema is worse than no configuration.
pub const SCHEMA_VERSION: u16 = 1;

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
}

impl Default for Config {
	/// What a device with nothing stored shows: **the plan's own pages**, in
	/// the plan's order. Deliberately not empty — a panel that boots blank
	/// because its settings were never written looks broken, and "looks
	/// broken" is indistinguishable from "is broken" — and deliberately not a
	/// list of indices written here, because the only thing that knows which
	/// indices mean anything is the plan.
	///
	/// A plan with no pages at all (the generator refuses one, but the type
	/// allows it) falls back to one values page of its first four channels;
	/// pages past [`MAX_PAGES`] and cells past [`MAX_CELLS`] are dropped
	/// rather than refused, since the storage is what bounds them.
	fn default() -> Self {
		let mut pages: heapless::Vec<Page, MAX_PAGES> = heapless::Vec::new();
		for page in PLAN.pages {
			let mut cells = heapless::Vec::new();
			let kind = match page {
				PlanPage::Values { cells: indices, .. } => {
					let _ = cells.extend_from_slice(&indices[..indices.len().min(MAX_CELLS)]);
					PageKind::Values
				}
				PlanPage::Chart { channel, .. } => {
					let _ = cells.push(*channel);
					PageKind::Chart
				}
			};
			if pages.push(Page { kind, cells }).is_err() {
				break;
			}
		}
		if pages.is_empty() {
			let mut cells = heapless::Vec::new();
			for index in 0..PLAN.channels.len().min(4) {
				let _ = cells.push(index as u16);
			}
			let _ = pages.push(Page {
				kind: PageKind::Values,
				cells,
			});
		}
		Self {
			brightness: 128,
			active_page: 0,
			pages,
		}
	}
}

impl Config {
	/// Rejects what the panel could not render anyway. Called before a save so
	/// that an unusable configuration never reaches flash, and on what comes
	/// back from flash — a configuration saved against one plan can name a
	/// cell the next image does not have, and the device must be able to
	/// trust what it draws from.
	pub fn validate(&self) -> Result<(), &'static str> {
		if self.pages.is_empty() {
			return Err("no pages");
		}
		if usize::from(self.active_page) >= self.pages.len() {
			return Err("active_page past the end");
		}
		for page in &self.pages {
			if page.cells.is_empty() {
				return Err("a page with no cells");
			}
			if page.kind == PageKind::Chart && page.cells.len() != 1 {
				return Err("a chart page shows exactly one cell");
			}
			if page.cells.iter().any(|&cell| usize::from(cell) >= PLAN.channels.len()) {
				return Err("a cell past the end of the plan");
			}
		}
		Ok(())
	}
}
