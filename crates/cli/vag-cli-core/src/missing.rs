//! What the tool says when the data it needs has not been made yet.
//!
//! Nothing this tool reads ships with it. The label files are Ross-Tech's and
//! cannot be redistributed, and a project's channels are read out of them or out
//! of an ODIS project. So a fresh checkout is *expected* to be short of both,
//! and the only question is whether it says so.
//!
//! **There are two shortages, and `setup` fixes both — in a different order.**
//!
//! | missing | why | the fix |
//! |---|---|---|
//! | label data | never set up from an ODIS project or a VCDS install | `vagcan setup <DIR>` — offline, one command |
//! | scalings for this car | no ODIS project describes its units, and no VCDS installation has a file for them | [`scalings_path`]: `setup` with an ODIS project, or with a VCDS installation and then `watch`, `measure` or `units --identify` with the car |
//!
//! The second is the one a reader cannot guess: with VCDS alone the channels
//! arrive the first time `watch`, `measure` or `units --identify` runs with the
//! car after `setup` (or `dev dash build`, offline, for a car already recorded),
//! not from `setup` itself, and somebody who already ran `setup` once has no
//! reason to think connecting the car will change anything. That is why the two messages live
//! here side by side rather than being written out at each call site, and why
//! each has a test. Until 2026-09-28 the second was fixed by a drive and
//! `vagcan dev recording calibrate`; that command is gone (owner, 2026-09-28).
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

/// How a car's scalings are got, in the order a person types it.
///
/// Quoted verbatim by every message about a car short of them — [`no_catalog`],
/// which is what `watch` reports, and `vag_cli_measure::messages`, which is what
/// `measure` reports — so that a reader who meets the shortage twice is not left
/// wondering whether the two are the same path. The two wordings once drifted
/// to five commands and three without anybody noticing.
pub fn scalings_path() -> &'static str {
	"    vagcan setup <ODIS project>          every unit the project describes\n  \
     or, with a VCDS installation:\n    \
     vagcan setup <VCDS installation>     names and fault text now; then `vagcan watch`,\n                                         \
     `measure` or `units --identify` with the car records\n                                         \
     the car's units and reads their channels, once"
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
/// It is *not* [`no_catalog`]: that shortage is a car whose units no source
/// read so far describes, and with VCDS its fix ends with `watch`, `measure` or
/// `units --identify` with the car after `setup`, which this one does not. See
/// the module docs.
///
/// **It is an [`std::error::Error`], and that is load-bearing.** Every site
/// below reports it by `bail!`-ing one of these, so the type survives into the
/// `anyhow::Error` a command fails with and `downcast_ref::<NoLabelData>()`
/// answers "did this command fail for want of label data?" — which is what
/// `vag_cli_diag::rescue` asks before offering to fix it. Rendering it to a
/// `String` at the call site would erase that and leave the dispatcher matching
/// on prose.
#[derive(Debug)]
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
             vagcan learns a car from an extracted ODIS-Service project, in one command:\n    \
             vagcan setup <path to the ODIS project folder>\n\n\
             A VCDS installation works too — names and fault text, and the channels of the\n\
             units of every car `vagcan watch`, `measure` or `units --identify` records:\n    \
             vagcan setup <path to the VCDS installation>\n\
             Neither to hand? Leave the path off: it asks which, and can fetch VCDS.\n\n\
             It is offline — no adapter, no car. An ODIS project reads in seconds; a VCDS\n\
             installation takes minutes, most of them searching for keys the first time.\n\
             What it reads lands in a project under ~/.vagcan/data/. An ODIS project can be\n\
             deleted afterwards; keep a VCDS installation: a unit the tool meets later —\n\
             swapped, updated, or asleep the first time — is read from it.\n\n\
             VCDS is Ross-Tech's, and free from them directly:\n    \
             {VCDS_DOWNLOAD}"
		)
	}
}

/// Nothing to add — the whole message is [`Display`](std::fmt::Display). The
/// impl exists so the type reaches an `anyhow::Error` intact; see the type docs.
impl std::error::Error for NoLabelData {}

/// Something `vagcan setup` makes, and this machine has not got.
///
/// `what` names the file in the reader's terms ("the measurement names"), and
/// `needed_for` says what wanted it, because the command that failed is often
/// three steps away from the one that would fix it.
///
/// A named case of [`NoLabelData`] rather than a message of its own: the two
/// call sites — `vcds names` and the fault namer — build the same sentence, and
/// a function is where that sentence gets built once.
///
/// Returns the [`NoLabelData`] rather than its text, so that a `bail!` of it
/// stays recognisable to the offer that can fix it. It still prints itself.
pub fn no_label_data(what: &str, needed_for: &str, path: &Path) -> NoLabelData {
	NoLabelData::new(format!("{what} are not on this machine, and {needed_for} needs them.")).looked_for(path)
}

/// The label shortage, met where the codes are about to be printed anyway.
///
/// A note above output that still happens, not a stop: a headline promising
/// "codes below" above a run that prints no codes is the kind of sentence
/// somebody reads twice and still misreads.
///
/// **Text, where its neighbour returns the [`NoLabelData`] itself.** That one
/// is `bail!`ed and this one is `println!`ed, and the type is what
/// `vag_cli_diag::rescue` recognises a *failed* command by: handing this one
/// back as an error would offer to run `setup` in the middle of a fault read
/// that is going perfectly well.
pub fn no_fault_labels(looked_in: &Path) -> String {
	NoLabelData::new("Codes below are shown as numbers: no fault-name labels on this machine.")
		.looked_in(looked_in)
		.to_string()
}

/// No scalings for the car in front of the tool: no row proven on a car, and
/// no channel read from an ODIS project or a VCDS installation.
///
/// It names `setup`, and the order: with a VCDS installation alone `setup`
/// comes first and the channels arrive with the next `watch`, `measure` or
/// `units --identify`, which reads the installation's registry for the units
/// the car says it has.
pub fn no_catalog(subject: &str, dir: &Path) -> String {
	let mut out = String::new();
	let _ = writeln!(
		out,
		"{subject} has no scalings on this machine: no row proven on a car, and no channel\n\
         read from an ODIS project or a VCDS installation."
	);
	let _ = writeln!(out, "\nLooked in: {}", dir.display());
	let _ = writeln!(
		out,
		"\n\
         A scaling says what an identifier's bytes mean: the raw form, the factor and the\n\
         offset. `vagcan setup` brings them:\n\
         {}",
		scalings_path()
	);
	out
}

/// States on screen as bytes because the project cache predates state ranges.
///
/// A note, not a stop: everything else on the screen is right, and a state read
/// on the lower end of its range is still named. `sources` is the ODIS projects
/// the cache was read from; with exactly one, the command names it.
pub fn state_ranges_note(sources: &[String]) -> String {
	let command = match sources {
		[one] => format!("vagcan setup {}", shell_word(one)),
		_ => "vagcan setup <path to the ODIS project folder>".to_string(),
	};
	format!(
		"Some switch and lever positions may show as raw bytes: this project was read\n\
         before vagcan kept each state's full range. To name them, read it again:\n    \
         {command}"
	)
}

/// A path as one word a POSIX shell reads back unchanged: bare when every
/// character is one no shell treats specially, in single quotes otherwise.
fn shell_word(path: &str) -> String {
	let plain = !path.is_empty() && path.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+,:@%=~".contains(c));
	match plain {
		true => path.to_string(),
		false => format!("'{}'", path.replace('\'', r"'\''")),
	}
}

/// Values are on screen, but as bytes, and the reader has no way to know why.
///
/// Three lines, printed once per run. A screen read at an open driver's door
/// cannot afford a paragraph, and repeating it per row would crowd out the
/// values it is apologising for.
///
/// Said as a condition: a raw channel is an identifier `--did` named that no
/// source declares, and those stay raw whatever `setup` reads. Only what an
/// ODIS project or a VCDS list names can be scaled.
pub fn raw_channels_note(count: usize) -> String {
	format!(
		"{count} channel{} shown as raw bytes: nothing read into this project scales them.\n\
         `vagcan setup` scales an identifier that an ODIS project or a VCDS installation\n\
         lists with a scaling; any other stays raw.",
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
		let m = no_label_data("The measurement names", "`vagcan dev vcds names`", Path::new("/x/n.json")).to_string();
		assert!(m.contains("vagcan setup <path to the ODIS project folder>"), "{m}");
		assert!(m.contains("vagcan setup <path to the VCDS installation>"), "{m}");
		assert!(m.contains("/x/n.json"), "the reader must see which file was looked for:\n{m}");
		assert!(m.contains(VCDS_DOWNLOAD), "no VCDS install is a case, not an oversight:\n{m}");
		assert!(m.contains("Ross-Tech"), "{m}");
		// The one thing it must never do is send a reader down the other path.
		assert!(!m.contains("calibrate"), "a label shortage is not fixed by driving:\n{m}");
	}

	#[test]
	fn the_scalings_shortage_names_setup_and_then_the_commands_that_read_the_car() {
		// Since 2026-09-28 `setup` brings scalings from either source, and the
		// drive and `calibrate` that used to be the fix are gone. What a reader
		// cannot guess is the order with VCDS: `setup`, then one of the three
		// commands that record the car's units and read their channels — named,
		// since "any car command" was not true of `info` or `faults` (found in
		// review, 2026-09-28) — and no survey, which is gone too.
		let m = no_catalog("This car", Path::new("/x/data"));
		assert!(m.contains(scalings_path()), "{m}");
		assert!(m.contains("/x/data"), "the reader must see where it looked:\n{m}");
		assert!(m.contains("vagcan setup <VCDS installation>"), "{m}");
		assert!(
			m.contains("`vagcan watch`") && m.contains("`measure`") && m.contains("`units --identify`"),
			"{m}"
		);
		assert!(!m.contains("car command"), "{m}");
		assert!(!m.contains("survey"), "that command is gone:\n{m}");
		assert!(!m.contains("calibrate"), "that command is gone:\n{m}");
	}

	#[test]
	fn the_two_shortages_cannot_be_mistaken_for_one_another() {
		// Both are fixed by `setup`, and the risk now is the order: a car short
		// of scalings with only VCDS needs `watch`, `measure` or `units --identify`
		// with the car after `setup`, and a project short of label data needs
		// `setup` to have run at all. So only the scalings shortage gives the
		// setup-then-the-car path.
		let label = no_label_data("The names", "this", Path::new("/n")).to_string();
		let catalog = no_catalog("This car", Path::new("/d"));
		assert!(label.contains("vagcan setup <path"));
		assert!(!label.contains(scalings_path()), "{label}");
		assert!(!label.contains("calibrate"), "{label}");
		assert!(catalog.contains(scalings_path()), "{catalog}");
		assert!(!catalog.contains("vagcan setup <path to the ODIS project folder>"), "{catalog}");
		assert!(!catalog.contains("calibrate"), "{catalog}");
	}

	#[test]
	fn the_fault_label_shortage_names_setup_the_copy_and_never_a_drive() {
		// The message for a car whose codes read but cannot be named: setup has
		// not copied the labels in yet. It must send the reader to `setup`, name
		// the directory it looked in, and never to a drive — the numbers are
		// real, only the names are absent, so a drive would be the wrong loop.
		let m = no_fault_labels(Path::new("/home/x/.vagcan/data/extracted"));
		assert!(m.contains("vagcan setup <path to the ODIS project folder>"), "{m}");
		assert!(m.contains("/home/x/.vagcan/data/extracted"), "the reader must see where it looked:\n{m}");
		assert!(m.contains(VCDS_DOWNLOAD), "no VCDS install is a case, not an oversight:\n{m}");
		assert!(m.contains("~/.vagcan/data/"), "the point is that setup copies what it read in:\n{m}");
		assert!(m.contains("deleted afterwards"), "an ODIS project is then disposable:\n{m}");
		assert!(
			m.contains("keep a VCDS installation"),
			"a VCDS installation is not — a unit met later is read from it:\n{m}"
		);
		// The one thing it must never do is send a reader driving.
		assert!(!m.contains("calibrate"), "a missing name is not fixed by a drive:\n{m}");
		assert!(!m.contains("measurement rows"), "{m}");
	}

	#[test]
	fn the_raw_note_is_one_reading_and_says_what_turns_bytes_into_numbers() {
		let one = raw_channels_note(1);
		assert!(one.contains("1 channel is"), "{one}");
		assert!(raw_channels_note(7).contains("7 channels are"));
		assert!(one.contains("vagcan setup"), "{one}");
		assert!(!one.contains("survey"), "no command but setup: {one}");
		// A condition, not a promise: what no source names stays raw.
		assert!(one.contains("any other stays raw"), "{one}");
		// It shares a screen with the values it is about. Three lines, no more.
		assert_eq!(one.lines().count(), 3, "{one}");
	}

	#[test]
	fn the_state_ranges_note_names_the_project_to_read_again() {
		let plain = state_ranges_note(&["/data/AB12X".to_string()]);
		assert!(plain.contains("vagcan setup /data/AB12X"), "{plain}");
		// Anything a shell would act on is quoted, a quote inside included.
		for (path, word) in [
			("/data/ODIS Projects/AB12X", "'/data/ODIS Projects/AB12X'"),
			("/data/a&b", "'/data/a&b'"),
			("/data/owner's", r"'/data/owner'\''s'"),
		] {
			let note = state_ranges_note(&[path.to_string()]);
			assert!(note.contains(&format!("vagcan setup {word}")), "{note}");
		}
		// Two sources, or none recorded: the command cannot pick for the reader.
		for sources in [vec![], vec!["/a".to_string(), "/b".to_string()]] {
			assert!(state_ranges_note(&sources).contains("vagcan setup <path to the ODIS project folder>"));
		}
	}

	#[test]
	fn the_scalings_path_is_quoted_from_one_place() {
		// Two commands print it. A reader who meets it twice must see the same
		// steps, or they will reasonably assume there are two paths.
		assert!(no_catalog("x", Path::new("/d")).contains(scalings_path()));
	}

	/// The fix line, cut out of a rendered message so two of them can be
	/// compared without the part that is meant to differ.
	fn fix_only(message: &str) -> String {
		let at = message.find("vagcan learns a car").unwrap_or_else(|| panic!("no fix in:\n{message}"));
		message[at..].to_string()
	}

	#[test]
	fn every_site_that_says_setup_has_not_run_says_it_in_the_same_words() {
		// The point of the type. Five commands report this shortage — two
		// through the named helpers, three by building it themselves — and the
		// half that tells the reader what to do is one string, so it cannot go
		// stale in one place and stay current in another.
		let sites = [
			no_label_data("The measurement names", "`vagcan dev vcds names`", Path::new("/n.json")).to_string(),
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
		// And what it tells them is still both ways in, ODIS first — the project
		// the tool is heading for — then a VCDS installation, which it can fetch.
		// Each with the argument spelled as what to type, since a bare
		// `vagcan setup` under "point it at an ODIS project" was no instruction.
		let odis = first.find("vagcan setup <path to the ODIS project folder>").expect(&first);
		let vcds = first.find("vagcan setup <path to the VCDS installation>").expect(&first);
		assert!(odis < vcds, "ODIS comes first:\n{first}");
		assert!(first.contains(VCDS_DOWNLOAD), "{first}");
		// And the time it takes is said per source: an ODIS project reads in
		// seconds, and "a few minutes over all the label files" was only ever
		// true of a VCDS installation.
		assert!(first.contains("seconds"), "{first}");
		assert!(!first.contains("few minutes over all the label files"), "{first}");
		for line in first.lines() {
			assert!(line.chars().count() <= 80, "{} columns: {line:?}", line.chars().count());
		}
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
