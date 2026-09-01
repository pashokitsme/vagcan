//! What the panel shows, and how — the part a person changes.
//!
//! Everything the device could ever *decode* is flashed with the firmware: the
//! catalogs live on the laptop, the image is built for one car, and the board
//! has neither the memory nor the reason to resolve anything at run time. So a
//! cell here is an **index into the plan that is already in the image**. That
//! is not a restriction bolted on for safety; it is the only thing the type can
//! express. A forty-first identifier is not refused, it is unsayable.

use serde::{Deserialize, Serialize};

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
	/// What a device with nothing stored shows. Deliberately not empty: a
	/// panel that boots blank because its settings were never written looks
	/// broken, and "looks broken" is indistinguishable from "is broken".
	fn default() -> Self {
		let mut pages = heapless::Vec::new();
		let mut cells = heapless::Vec::new();
		let _ = cells.extend_from_slice(&[0, 1, 2, 3]);
		let _ = pages.push(Page {
			kind: PageKind::Values,
			cells,
		});
		let mut chart = heapless::Vec::new();
		let _ = chart.push(0);
		let _ = pages.push(Page {
			kind: PageKind::Chart,
			cells: chart,
		});
		Self {
			brightness: 128,
			active_page: 0,
			pages,
		}
	}
}

impl Config {
	/// Rejects what the panel could not render anyway. Called before a save so
	/// that an unusable configuration never reaches flash — the device must be
	/// able to trust what it reads back at boot.
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
		}
		Ok(())
	}
}
