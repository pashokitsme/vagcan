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
pub use vag_dash_core::schema::{Config, Fitted, SCHEMA_VERSION, SavedAgainst};

/// What a device with nothing stored runs on, on this image's plan ([`Config::defaults`]).
pub fn defaults() -> Config {
	Config::defaults(PLAN_ID)
}

/// A stored record on this image's plan ([`vag_dash_core::schema::fit`]): kept whatever plan it
/// was saved against, its page the first where this plan has no such page.
pub fn fit(stored: Config) -> Fitted {
	vag_dash_core::schema::fit(stored, PLAN_ID, usize::from(PAGE_COUNT))
}
