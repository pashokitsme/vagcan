//! What the tool says when the data it needs has not been made yet.
//!
//! Nothing this tool reads ships with it. The label files are Ross-Tech's and
//! cannot be redistributed; the measurement rows were proven on a car and
//! cannot be invented. So a fresh checkout is *expected* to be short of both,
//! and the only question is whether it says so.
//!
//! **There are two shortages and they have nothing to do with each other.**
//!
//! | missing | why | the fix |
//! |---|---|---|
//! | label data | never parsed a VCDS install | `vagcan setup <VCDS-DIR>` — offline, one command |
//! | a measurement catalog | this car was never calibrated | a drive: `survey`, `watch --out`, `recording calibrate` |
//!
//! Telling somebody to run `setup` when their real problem is that they have
//! never calibrated their car sends them round a loop that cannot help them,
//! and the reverse sends them looking for a VCDS installation they do not need.
//! That is why the two messages live here side by side rather than being
//! written out at each call site, and why each has a test.

use std::fmt::Write as _;
use std::path::Path;

/// Where a VCDS installation comes from. This project cannot ship the data:
/// it is Ross-Tech's, and all the label files are derived from their product.
pub const VCDS_DOWNLOAD: &str = "https://www.ross-tech.com/vcds/download/";

/// The three commands that turn raw bytes into proven numbers, in order.
///
/// Quoted verbatim by every message about a missing catalog, so that a reader
/// who meets the shortage twice — once in `measure`, once in `watch` — is not
/// left wondering whether the two are the same path.
pub fn calibration_path() -> &'static str {
	"    vagcan survey                    once, parked: what every unit answers\n    \
     vagcan watch --out drive.csv     then drive, with the values on screen\n    \
     vagcan recording calibrate --log drive.csv --out <part-number>.json"
}

/// Something `vagcan setup` makes, and this machine has not got.
///
/// `what` names the file in the reader's terms ("the measurement names"), and
/// `needed_for` says what wanted it, because the command that failed is often
/// three steps away from the one that would fix it.
pub fn no_label_data(what: &str, needed_for: &str, path: &Path) -> String {
	let mut out = String::new();
	let _ = writeln!(out, "{what} are not on this machine, and {needed_for} needs them.");
	let _ = writeln!(out, "\nLooked for: {}", path.display());
	let _ = writeln!(
		out,
		"\n\
         They are recovered from a VCDS installation, in one command:\n    \
         vagcan setup /path/to/VCDS\n\n\
         No VCDS installation? Leave the path off and it offers to fetch one for you:\n    \
         vagcan setup\n\n\
         Either way it is offline — no adapter, no car — and takes a few minutes over\n\
         all the label files. It writes everything into a project under ~/.vagcan/data/.\n\n\
         VCDS is Ross-Tech's, and free from them directly: {VCDS_DOWNLOAD}"
	);
	out
}

/// Fault codes read fine, but this machine has nothing to name them with.
///
/// A third shortage, and it must not be mistaken for either of the other two.
/// It is **not** [`no_catalog`] — a drive cannot invent a fault name — and it is
/// its own message rather than [`no_label_data`] because the fix is now sharper:
/// since `vagcan setup` copies the label files into `~/.vagcan`, this is the
/// machine where that copy has not happened. Setup was never run, or was run by
/// a build from before it copied them, and either way one command fixes it.
///
/// Deliberately says the codes are still shown: a reader looking at bare numbers
/// needs to know the numbers are real and only the names are missing.
pub fn no_fault_labels(looked_in: &Path) -> String {
	let mut out = String::new();
	let _ = writeln!(out, "Codes below are shown as numbers: no fault-name labels on this machine.");
	let _ = writeln!(out, "\nLooked in: {}", looked_in.display());
	let _ = writeln!(
		out,
		"\n\
         The names come from a VCDS installation, and `vagcan setup` copies them in:\n    \
         vagcan setup /path/to/VCDS\n\n\
         Haven't got one? Leave the path off and it offers to fetch a copy:\n    \
         vagcan setup\n\n\
         That is offline, and it leaves what it read under ~/.vagcan/ so the\n\
         installation can then be deleted. VCDS is Ross-Tech's, and free from them\n\
         directly: {VCDS_DOWNLOAD}"
	);
	out
}

/// No proven measurement rows for the car in front of the tool.
///
/// Deliberately does **not** mention `setup`. A VCDS installation supplies
/// names and nothing else — the label files provably carries no scaling
/// (`research/labels/rod-labels.md` §4.0c) — so parsing one again cannot
/// produce a single row of what is missing here. Only a drive can.
pub fn no_catalog(subject: &str, dir: &Path) -> String {
	let mut out = String::new();
	let _ = writeln!(out, "{subject} has no proven measurement rows on this machine.");
	let _ = writeln!(out, "\nLooked in: {}", dir.display());
	let _ = writeln!(
		out,
		"\n\
         A row says what an identifier's bytes mean — the raw form, the factor and the\n\
         offset — and every one of them was measured on a car. The label files do not\n\
         carry scaling and never did, so this is not something `vagcan setup` can fix.\n\n\
         What does fix it is a drive:\n\
         {}\n\n\
         The last step fits each unknown identifier against a reading already trusted —\n\
         the standard OBD-II parameters, or a row proven earlier — and writes the ones\n\
         that hold as a catalog under {}.",
		calibration_path(),
		crate::project::measurements_hint()
	);
	out
}

/// Values are on screen, but as bytes, and the reader has no way to know why.
///
/// One line plus the path, printed once per run. A screen read at an open
/// driver's door cannot afford a paragraph, and repeating it per row would
/// crowd out the values it is apologising for.
pub fn raw_channels_note(count: usize) -> String {
	format!(
		"{count} channel{} shown as raw bytes: no proven scaling for this car yet.\n\
         Record a drive and fit them — `vagcan watch --out drive.csv`, then\n\
         `vagcan recording calibrate --log drive.csv --out <part-number>.json`.",
		if count == 1 { " is" } else { "s are" }
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Every one of these is read by somebody who is stuck. The test is that
	/// each says what is missing, and ends somewhere to go.
	#[test]
	fn the_label_shortage_names_setup_and_where_the_data_comes_from() {
		let m = no_label_data("The measurement names", "`vagcan vcds names`", Path::new("/x/n.json"));
		assert!(m.contains("vagcan setup /path/to/VCDS"), "{m}");
		assert!(m.contains("/x/n.json"), "the reader must see which file was looked for:\n{m}");
		assert!(m.contains(VCDS_DOWNLOAD), "no VCDS install is a case, not an oversight:\n{m}");
		assert!(m.contains("Ross-Tech"), "{m}");
		// The one thing it must never do is send a reader down the other path.
		assert!(!m.contains("calibrate"), "a label shortage is not fixed by driving:\n{m}");
	}

	#[test]
	fn the_catalog_shortage_names_the_drive_and_never_names_setup() {
		let m = no_catalog("This car", Path::new("/x/data"));
		for step in ["vagcan survey", "vagcan watch --out drive.csv", "vagcan recording calibrate"] {
			assert!(m.contains(step), "{step} missing from:\n{m}");
		}
		// The distinction that has to survive: somebody whose car has never
		// been calibrated must not be sent to look for a VCDS installation.
		// `setup` is named — but only to rule it out, which is the one thing a
		// reader who already ran it needs to hear.
		assert!(m.contains("not something `vagcan setup` can fix"), "{m}");
		assert!(!m.contains("vagcan setup /path"), "that is an instruction, not a warning:\n{m}");
		assert!(!m.contains(VCDS_DOWNLOAD), "{m}");
	}

	#[test]
	fn the_two_shortages_cannot_be_mistaken_for_one_another() {
		// They are the same shape of failure — "the tool has less data than it
		// wants" — with opposite fixes, and the whole risk is a reader running
		// the wrong one. So neither message may *offer* the other's command.
		let label = no_label_data("The names", "this", Path::new("/n"));
		let catalog = no_catalog("This car", Path::new("/d"));
		assert!(label.contains("vagcan setup /path/to/VCDS"));
		assert!(!label.contains("calibrate"), "{label}");
		assert!(catalog.contains("vagcan recording calibrate"));
		assert!(!catalog.contains("vagcan setup /path"), "{catalog}");
	}

	#[test]
	fn the_fault_label_shortage_names_setup_the_copy_and_never_a_drive() {
		// The message for a car whose codes read but cannot be named: setup has
		// not copied the labels in yet. It must send the reader to `setup`, name
		// the directory it looked in, and never to a drive — the numbers are
		// real, only the names are absent, so `calibrate` would be the wrong loop.
		let m = no_fault_labels(Path::new("/home/x/.vagcan/data/extracted"));
		assert!(m.contains("vagcan setup /path/to/VCDS"), "{m}");
		assert!(m.contains("/home/x/.vagcan/data/extracted"), "the reader must see where it looked:\n{m}");
		assert!(m.contains(VCDS_DOWNLOAD), "no VCDS install is a case, not an oversight:\n{m}");
		assert!(m.contains("copies them in"), "the point is that setup now copies the labels:\n{m}");
		// The one thing it must never do is send a reader driving.
		assert!(!m.contains("calibrate"), "a missing name is not fixed by a drive:\n{m}");
		assert!(!m.contains("measurement rows"), "{m}");
	}

	#[test]
	fn the_raw_note_is_one_reading_and_says_what_turns_bytes_into_numbers() {
		let one = raw_channels_note(1);
		assert!(one.contains("1 channel is"), "{one}");
		assert!(raw_channels_note(7).contains("7 channels are"));
		assert!(one.contains("recording calibrate"), "{one}");
		// It shares a screen with the values it is about. Three lines, no more.
		assert_eq!(one.lines().count(), 3, "{one}");
	}

	#[test]
	fn the_calibration_path_is_quoted_from_one_place() {
		// Two commands print it. A reader who meets it twice must see the same
		// three steps, or they will reasonably assume there are two paths.
		assert!(no_catalog("x", Path::new("/d")).contains(calibration_path()));
	}
}
