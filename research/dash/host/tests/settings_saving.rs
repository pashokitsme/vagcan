//! What the board's settings in RAM are against flash, and when a run the stopwatch kept is
//! written: `crates/dash/vag-dash-fw/src/saving.rs` compiled as it is — the firmware cannot be
//! built for the host.

#[path = "../../../../crates/dash/vag-dash-fw/src/saving.rs"]
mod saving;

use saving::{RunWrite, Saving};

#[test]
fn a_run_kept_with_nothing_else_unsaved_writes_the_whole_configuration() {
	let mut saving = Saving::default();
	saving.run_kept();
	assert!(saving.run_pending);
	assert_eq!(saving.at_standstill(false), RunWrite::Whole);
	assert_eq!(saving, Saving::default(), "flash now holds what RAM does");
	assert_eq!(saving.at_standstill(false), RunWrite::Nothing, "written once");
}

#[test]
fn a_run_kept_beside_other_changes_is_added_to_what_flash_holds() {
	let mut saving = Saving::default();
	saving.changed();
	saving.run_kept();
	assert_eq!(saving.at_standstill(false), RunWrite::AddToStored);
	assert!(saving.unsaved, "the other changes stay the person's to save");
	assert!(!saving.run_pending);
}

/// PR #12 review: `erase` left `unsaved` as it was, so the next run's write took the whole
/// configuration in RAM — the erased pages and brightness — back to flash, and the next boot
/// loaded them rather than the defaults.
#[test]
fn after_an_erase_a_run_does_not_write_the_erased_configuration_back() {
	let mut saving = Saving::default();
	saving.erased();
	assert!(saving.unsaved, "RAM holds what flash no longer does");
	saving.run_kept();
	assert_ne!(saving.at_standstill(false), RunWrite::Whole);
	// And with a run already waiting when flash was erased.
	let mut saving = Saving::default();
	saving.run_kept();
	saving.erased();
	assert_eq!(saving.at_standstill(false), RunWrite::Nothing);
}

#[test]
fn a_record_this_image_cannot_read_is_overwritten_only_by_save() {
	let mut saving = Saving::default();
	saving.run_kept();
	assert_eq!(saving.at_standstill(true), RunWrite::Nothing);
	assert!(saving.unsaved, "the run is RAM's alone, for `save` to write");
	assert!(!saving.run_pending);
}

#[test]
fn a_failed_write_and_the_commands_leave_it_as_they_say() {
	let mut saving = Saving::default();
	saving.run_kept();
	assert_eq!(saving.at_standstill(false), RunWrite::Whole);
	saving.not_written();
	assert_eq!(
		saving,
		Saving {
			unsaved: true,
			run_pending: false
		},
		"`save` retries"
	);
	saving.run_kept();
	saving.agreed();
	assert_eq!(saving, Saving::default(), "a save or a load");
	saving.run_kept();
	saving.defaults();
	assert_eq!(
		saving,
		Saving {
			unsaved: true,
			run_pending: false
		}
	);
}

#[test]
fn get_says_a_pending_run_and_what_is_unsaved() {
	let said = |unsaved, run_pending| Saving { unsaved, run_pending }.said();
	assert_eq!(said(false, false), "saved");
	assert_eq!(said(true, false), "UNSAVED");
	assert!(said(false, true).starts_with("saved") && said(false, true).contains("a run waits"));
	assert!(said(true, true).starts_with("UNSAVED") && said(true, true).contains("a run waits"));
}
