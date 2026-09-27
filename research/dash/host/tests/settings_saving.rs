//! What the board's settings in RAM are against flash, and when a run the stopwatch kept is
//! written: `crates/dash/vag-dash-fw/src/saving.rs` compiled as it is — the firmware cannot be
//! built for the host.

#[path = "../../../../crates/dash/vag-dash-fw/src/saving.rs"]
mod saving;

use saving::{Flash, RunWrite, Saving};

#[test]
fn a_run_kept_with_nothing_else_unsaved_writes_the_whole_configuration() {
	let mut saving = Saving::default();
	saving.run_kept();
	assert!(saving.run_pending && saving.write_waits());
	assert_eq!(saving.at_standstill(Flash::Holds), RunWrite::Whole);
	assert!(saving.run_pending, "pending until it is in flash");
	assert!(!saving.write_waits(), "its one try is under way");
	saving.run_written();
	assert_eq!(saving, Saving::default(), "flash now holds what RAM does");
	assert_eq!(saving.at_standstill(Flash::Holds), RunWrite::Nothing, "written once");
}

#[test]
fn a_run_kept_beside_other_changes_is_added_to_what_flash_holds() {
	let mut saving = Saving::default();
	saving.changed();
	saving.run_kept();
	assert_eq!(saving.at_standstill(Flash::Holds), RunWrite::AddToStored);
	saving.run_written();
	assert!(saving.unsaved, "the other changes stay the person's to save");
	assert!(!saving.run_pending);
}

/// PR #12 review: a board whose flash holds nothing — never saved, or erased — with any setting
/// changed (one page turn of the lever) added the run to "what flash holds", found nothing, and
/// kept the run in RAM only: lost at ignition off on a board with no `save` (no BLE).
#[test]
fn on_a_board_whose_flash_holds_nothing_a_run_goes_into_the_defaults() {
	let mut saving = Saving::default();
	saving.changed();
	saving.run_kept();
	assert_eq!(saving.at_standstill(Flash::Empty), RunWrite::AddToDefaults);
	saving.run_written();
	assert!(!saving.run_pending, "in flash");
	assert!(saving.unsaved, "the page turn is still the person's to save");
	// With nothing changed, RAM is the defaults the board booted on: all of it.
	let mut saving = Saving::default();
	saving.run_kept();
	assert_eq!(saving.at_standstill(Flash::Empty), RunWrite::Whole);
}

/// PR #12 review: `erase` left `unsaved` as it was, so the next run's write took the whole
/// configuration in RAM — the erased pages and brightness — back to flash, and the next boot
/// loaded them rather than the defaults. It goes into the defaults instead.
#[test]
fn after_an_erase_a_run_does_not_write_the_erased_configuration_back() {
	let mut saving = Saving::default();
	saving.erased();
	assert!(saving.unsaved, "RAM holds what flash no longer does");
	saving.run_kept();
	assert_eq!(saving.at_standstill(Flash::Empty), RunWrite::AddToDefaults);
	// A run already waiting when flash was erased is still in RAM only, and still said so.
	let mut saving = Saving::default();
	saving.run_kept();
	saving.erased();
	assert!(saving.run_pending, "the run is in RAM, and flash holds nothing");
	assert_eq!(saving.at_standstill(Flash::Empty), RunWrite::AddToDefaults);
}

#[test]
fn a_record_this_image_cannot_read_is_overwritten_only_by_save() {
	for unsaved in [false, true] {
		let mut saving = Saving {
			unsaved,
			..Saving::default()
		};
		saving.run_kept();
		assert_eq!(saving.at_standstill(Flash::Unreadable), RunWrite::Nothing);
		assert!(saving.unsaved, "the run is RAM's alone, for `save` to write");
		assert!(saving.run_pending, "and `state` says so");
		assert!(!saving.write_waits(), "the stopwatch arms: the run waits for `save`, not for it");
	}
}

/// PR #12 review: the run was no longer pending before its write was known to succeed, so a
/// declined or failed write left it in RAM alone with `state` saying `run_pending=0`.
#[test]
fn a_run_stays_pending_until_it_is_in_flash_and_the_standstill_tries_once() {
	let mut saving = Saving::default();
	saving.run_kept();
	assert_eq!(saving.at_standstill(Flash::Holds), RunWrite::Whole);
	saving.not_written();
	assert!(saving.run_pending, "the write failed: RAM alone holds the run");
	assert!(saving.unsaved, "`save` writes it");
	assert!(!saving.write_waits());
	assert_eq!(saving.at_standstill(Flash::Holds), RunWrite::Nothing, "one try");
	// Flash that cannot be read for the configuration a run is to be added to: declined.
	let mut saving = Saving::default();
	saving.changed();
	saving.run_kept();
	assert_eq!(saving.at_standstill(Flash::Unusable), RunWrite::Nothing);
	assert!(saving.run_pending && !saving.write_waits());
	// Nothing else unsaved, flash is not read: the whole of RAM goes.
	let mut saving = Saving::default();
	saving.run_kept();
	assert_eq!(saving.at_standstill(Flash::Unusable), RunWrite::Whole);
	// A new run kept after a failed try gets a try of its own.
	let mut saving = Saving::default();
	saving.run_kept();
	saving.at_standstill(Flash::Holds);
	saving.not_written();
	saving.run_kept();
	assert!(saving.write_waits());
	assert_eq!(saving.at_standstill(Flash::Holds), RunWrite::AddToStored, "unsaved since the failure");
	// A `save` writes it, and a `load` or the defaults drop it with the configuration it was in.
	for command in [Saving::agreed as fn(&mut Saving), Saving::defaults] {
		let mut saving = Saving::default();
		saving.run_kept();
		saving.at_standstill(Flash::Unreadable);
		command(&mut saving);
		assert!(!saving.run_pending && !saving.write_waits());
	}
}

#[test]
fn a_failed_write_and_the_commands_leave_it_as_they_say() {
	let mut saving = Saving::default();
	saving.run_kept();
	assert_eq!(saving.at_standstill(Flash::Holds), RunWrite::Whole);
	saving.not_written();
	assert_eq!(
		(saving.unsaved, saving.run_pending, saving.write_waits()),
		(true, true, false),
		"`save` retries"
	);
	saving.run_kept();
	saving.agreed();
	assert_eq!(saving, Saving::default(), "a save or a load");
	saving.run_kept();
	saving.defaults();
	assert_eq!((saving.unsaved, saving.run_pending, saving.write_waits()), (true, false, false));
}

#[test]
fn get_says_a_pending_run_and_what_is_unsaved() {
	let said = |unsaved, run_pending| {
		Saving {
			unsaved,
			run_pending,
			..Saving::default()
		}
		.said()
	};
	assert_eq!(said(false, false), "saved");
	assert_eq!(said(true, false), "UNSAVED");
	assert!(said(false, true).starts_with("saved") && said(false, true).contains("a run is in RAM only"));
	assert!(said(true, true).starts_with("UNSAVED") && said(true, true).contains("a run is in RAM only"));
}
