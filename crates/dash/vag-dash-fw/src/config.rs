//! What the panel shows, and how — the part a person changes: brightness and which page is
//! up, and the stopwatch's last run beside them.
//!
//! The pages themselves are the plan's, flashed with the firmware, and nothing on the board
//! edits them; the settings name the plan they were saved against instead
//! ([`Config::plan`]), so a new `dash.toml` keeps them.

use crate::plan::{PAGE_COUNT, PLAN_ID};

// The record itself — its fields, its versions, how flash bytes become one, how a record
// saved against another plan becomes this one's — is `schema`'s, where a host test can reach
// it. What this plan makes of it is here.
pub use crate::schema::{Config, Fitted, SCHEMA_VERSION, SavedAgainst};

impl Default for Config {
	/// What a device with nothing stored runs on: half brightness, the plan's first page, no
	/// run, and this plan.
	fn default() -> Self {
		Self {
			brightness: 128,
			active_page: 0,
			last_run: heapless::Vec::new(),
			plan: Some(PLAN_ID),
		}
	}
}

impl Config {
	/// A stored record on this image's plan ([`crate::schema::fit`]): kept whatever plan it
	/// was saved against, its page the first where this plan has no such page.
	pub fn fit(self) -> Fitted {
		crate::schema::fit(self, PLAN_ID, usize::from(PAGE_COUNT))
	}
}
