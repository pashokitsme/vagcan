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
//!
//! **The first row of that table is [`NoLabelData`], and it is one type because
//! it was five strings.** Six call sites run into the same shortage — `vcds
//! names`, `vcds stats`, `faults`, the fault namer, `labels`, and
//! [`crate::project`] itself, which is where nearly every command meets it
//! first — and each of them
//! used to word it, and word the fix, on its own. They no longer do: a call site
//! writes the one sentence only it can write, and the instruction comes from
//! here. The second row is [`no_catalog`], which stays deliberately apart.
//!
//! The module lives in `core` rather than in `diag` with most of its callers for
//! exactly one reason: [`crate::project`] is one of them, `core` cannot depend on
//! `diag`, and the alternative was writing the fix out a sixth time to get it
//! across the crate boundary. `diag` re-exports it, so `crate::missing::…` there
//! is unchanged.

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

/// The label shortage, in the one wording every command reports it in.
///
/// **This is the string that used to be written five times.** `vcds names`,
/// `vcds stats`, the fault namer, `faults`, [`crate::project`] and `labels` —
/// six call sites over five wordings — each said "the label data is not here,
/// run `vagcan setup`" in its own words, and five wordings of
/// one fact is five chances for one of them to go stale — which had already
/// happened: two of the five still described a `setup` that did not copy the
/// label files in, and two never mentioned that an ODIS project will do instead
/// of a VCDS installation.
///
/// What genuinely differs between the sites is the **first line** — what was
/// wanted, and by what — and **which path was looked at**. Those are the two
/// things this takes. Everything after them is fixed text, written below, once.
///
/// It is *not* [`no_catalog`]: that shortage is fixed by a drive and naming
/// `setup` at it sends a reader round a loop. See the module docs.
pub struct NoLabelData {
	/// What was wanted and by what, as one sentence ending in a full stop.
	headline: String,
	/// The preposition the reader needs — "Looked for" a file, "Looked in" a
	/// directory — and the path, when the site has one worth showing.
	looked: Option<(&'static str, String)>,
}

impl NoLabelData {
	/// `headline` is the one sentence only this call site can write: what is
	/// absent, and what wanted it. It ends in a full stop and never in an
	/// instruction — the instruction is the same for everybody and is added here.
	pub fn new(headline: impl Into<String>) -> NoLabelData {
		NoLabelData {
			headline: headline.into(),
			looked: None,
		}
	}

	/// A file that would have held it. "Looked **for**", because a reader who
	/// sees a full path to a named file wants to know it was a file.
	pub fn looked_for(mut self, path: &Path) -> NoLabelData {
		self.looked = Some(("Looked for", path.display().to_string()));
		self
	}

	/// A directory that would have held it. "Looked **in**" for the same reason.
	pub fn looked_in(mut self, path: &Path) -> NoLabelData {
		self.looked = Some(("Looked in", path.display().to_string()));
		self
	}
}

impl std::fmt::Display for NoLabelData {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		writeln!(f, "{}", self.headline)?;
		if let Some((preposition, path)) = &self.looked {
			writeln!(f, "\n{preposition}: {path}")?;
		}
		// The fix. One copy, and the only copy — a second one anywhere is the
		// bug this type exists to have already fixed.
		writeln!(
			f,
			"\n\
             The label data is recovered from a VCDS installation, in one command:\n    \
             vagcan setup /path/to/VCDS\n\n\
             No VCDS installation? Leave the path off and it offers to fetch one for you —\n\
             or point it at an extracted ODIS-Service project instead:\n    \
             vagcan setup\n\n\
             Either way it is offline — no adapter, no car — and takes a few minutes over\n\
             all the label files. It copies what it read into a project under\n\
             ~/.vagcan/data/, so the installation can then be deleted.\n\n\
             VCDS is Ross-Tech's, and free from them directly: {VCDS_DOWNLOAD}"
		)
	}
}

/// Something `vagcan setup` makes, and this machine has not got.
///
/// `what` names the file in the reader's terms ("the measurement names"), and
/// `needed_for` says what wanted it, because the command that failed is often
/// three steps away from the one that would fix it.
///
/// A named case of [`NoLabelData`] rather than a message of its own: the two
/// call sites — `vcds names` and the fault namer — build the same sentence, and
/// a function is where that sentence gets built once.
pub fn no_label_data(what: &str, needed_for: &str, path: &Path) -> String {
	NoLabelData::new(format!("{what} are not on this machine, and {needed_for} needs them."))
		.looked_for(path)
		.to_string()
}

/// Fault codes read fine, but this machine has nothing to name them with.
///
/// A named case of [`NoLabelData`] and not of [`no_catalog`]: a drive cannot
/// invent a fault name, and only `vagcan setup` can put one here. What it adds
/// over [`no_label_data`] is the first line — it deliberately says the codes are
/// still shown, because a reader looking at bare numbers needs to know the
/// numbers are real and only the names are missing.
pub fn cannot_name_faults(looked_in: &Path) -> String {
	NoLabelData::new("Fault names are not on this machine, and naming a recorded survey needs them.")
		.looked_in(looked_in)
		.to_string()
}

/// The same shortage, met where the codes are about to be printed anyway.
///
/// Apart from [`cannot_name_faults`] because the two are different events: this
/// one is a note above output that still happens, that one is a stop. A headline
/// promising "codes below" above a run that prints no codes is the kind of
/// sentence somebody reads twice and still misreads.
pub fn no_fault_labels(looked_in: &Path) -> String {
	NoLabelData::new("Codes below are shown as numbers: no fault-name labels on this machine.")
		.looked_in(looked_in)
		.to_string()
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
		assert!(m.contains("copies what it read"), "the point is that setup copies the labels in:\n{m}");
		assert!(m.contains("can then be deleted"), "and that the installation is then disposable:\n{m}");
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

	/// The fix line, cut out of a rendered message so two of them can be
	/// compared without the part that is meant to differ.
	fn fix_only(message: &str) -> String {
		let at = message
			.find("The label data is recovered")
			.unwrap_or_else(|| panic!("no fix in:\n{message}"));
		message[at..].to_string()
	}

	#[test]
	fn every_site_that_says_setup_has_not_run_says_it_in_the_same_words() {
		// The point of the type. Five commands report this shortage — two
		// through the named helpers, three by building it themselves — and the
		// half that tells the reader what to do is one string, so it cannot go
		// stale in one place and stay current in another.
		let sites = [
			no_label_data("The measurement names", "`vagcan vcds names`", Path::new("/n.json")),
			no_fault_labels(Path::new("/rod")),
			NoLabelData::new("No car has been set up yet.").looked_in(Path::new("/data")).to_string(),
			NoLabelData::new("The project `SK37X` has no label cache.")
				.looked_for(Path::new("/c.sqlite"))
				.to_string(),
			NoLabelData::new("There is no label cache to count.")
				.looked_for(Path::new("/c.sqlite"))
				.to_string(),
		];
		let first = fix_only(&sites[0]);
		for site in &sites[1..] {
			assert_eq!(fix_only(site), first, "a second wording of the fix:\n{site}");
		}
		// And what it tells them is still both ways in: a VCDS installation,
		// which it can fetch, or an ODIS project — the two `setup` accepts.
		assert!(first.contains("vagcan setup /path/to/VCDS"), "{first}");
		assert!(first.contains("ODIS"), "an ODIS project is the other half of `setup`:\n{first}");
		assert!(first.contains(VCDS_DOWNLOAD), "{first}");
		// Never the other shortage's fix. That is the mistake this module exists
		// to prevent, and it must hold for the sites that build their own
		// headline as much as for the two named ones.
		for site in &sites {
			assert!(!site.contains("calibrate"), "a label shortage is not fixed by driving:\n{site}");
		}
	}

	#[test]
	fn a_site_keeps_its_own_first_line_and_the_path_it_looked_at() {
		// The two things a call site knows and this module cannot: what was
		// wanted, and where it was not. Both must survive into the message, or
		// five sites sharing one fix would have cost the reader the diagnosis.
		let file = NoLabelData::new("The project `SK37X` has no label cache.").looked_for(Path::new("/x/cache.sqlite"));
		assert!(file.to_string().starts_with("The project `SK37X` has no label cache.\n"), "{file}");
		assert!(file.to_string().contains("Looked for: /x/cache.sqlite"), "{file}");
		// A directory is looked *in*. Same message, and the preposition is the
		// reader's only clue whether to expect a file at the end of the path.
		let dir = NoLabelData::new("No car has been set up yet.").looked_in(Path::new("/x/data"));
		assert!(dir.to_string().contains("Looked in: /x/data"), "{dir}");
		// And a site with no path worth showing simply has none, rather than an
		// empty line where one would have been.
		let bare = NoLabelData::new("No car has been set up yet.").to_string();
		assert!(!bare.contains("Looked"), "{bare}");
		assert_eq!(fix_only(&bare), fix_only(&dir.to_string()));
	}
}
