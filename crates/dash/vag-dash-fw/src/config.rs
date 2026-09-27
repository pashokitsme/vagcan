//! What the panel shows, and how — the part a person changes.
//!
//! Everything the device could ever *decode* is flashed with the firmware: the
//! catalogs live on the laptop, the image is built for one car, and the board
//! has neither the memory nor the reason to resolve anything at run time. So a
//! cell here is an **index into the plan that is already in the image**. That
//! is not a restriction bolted on for safety; it is the only thing the type can
//! express. A forty-first identifier is not refused, it is unsayable.

use vag_dash_render::pages::{self, Layout, Mismatch};

use crate::plan::PLAN;

// The record itself — its fields, its versions, how flash bytes become one — is
// `schema`'s, where a host test can reach it. What the plan decides about it is here.
pub use crate::schema::{Config, MAX_CELLS, Page, PageKind, SCHEMA_VERSION};
// How many pages the panel can hold: defined beside the plan type, because the
// generator refuses a plan with more pages than this.
pub use vag_dash_render::pages::MAX_PAGES;

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
		// `from_plan` already bounds both counts, so neither push can fail.
		for layout in pages::from_plan(PLAN.pages, MAX_PAGES, MAX_CELLS) {
			let mut cells = heapless::Vec::new();
			let _ = cells.extend_from_slice(layout.cells);
			let kind = if layout.chart { PageKind::Chart } else { PageKind::Values };
			let _ = pages.push(Page { kind, cells });
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
			last_run: heapless::Vec::new(),
		}
	}
}

impl Config {
	/// How this configuration's pages differ from the plan's, if they do — see
	/// [`vag_dash_render::pages::mismatch`] for why a difference means the
	/// configuration is stale rather than chosen.
	pub fn plan_mismatch(&self) -> Option<Mismatch> {
		let stored = self.pages.iter().map(|page| Layout {
			chart: page.kind == PageKind::Chart,
			cells: &page.cells,
		});
		pages::mismatch(stored, PLAN.pages, MAX_PAGES, MAX_CELLS)
	}

	/// Rejects what the panel could not render anyway. Called before a save so
	/// that an unusable configuration never reaches flash, and on what comes
	/// back from flash — a configuration saved against one plan can name a
	/// cell the next image does not have, or lack a page the next image's plan
	/// added, and the device must be able to trust what it draws from.
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
		if self.plan_mismatch().is_some() {
			return Err("its pages are not this plan's pages");
		}
		Ok(())
	}
}
