//! Offering to fix the label shortage where it is met, instead of sending the
//! reader away to run another command and start over.
//!
//! `vagcan setup` is a step somebody has to know about. Every command that
//! needs what it makes used to end at [`crate::missing::NoLabelData`] — a good
//! message, naming the fix — and there the run stopped: read the paragraph, run
//! `setup`, then type the original command again. This is the paragraph
//! followed by a question, and an answer of `y` that ends with the original
//! command running rather than with an instruction to run it.
//!
//! ## Why this lives in `diag` and not in `core`
//!
//! [`crate::project::current`] is one of the six sites that meet this shortage,
//! and it is where nearly every command meets it first — so the offer has to be
//! able to reach it. It cannot go beside it: fixing the shortage means
//! [`crate::setup`], `setup` is in `diag`, and `core` must not depend on `diag`.
//! So `project::current` keeps returning its error untouched, and the offer
//! sits one layer out, on the other side of that dependency.
//!
//! ## Why one path and not six
//!
//! The failure is recognised by its *type* — every site `bail!`s a
//! [`NoLabelData`], so `downcast_ref` answers "did this fail for want of label
//! data?" wherever the command's error surfaces. That means the offer can be
//! made once, around the whole dispatch in `vag-cli`, rather than being written
//! into each command that might hit it. Six copies of an offer to download
//! ninety megabytes is six places for one of them to stop asking.
//!
//! **Retrying the command is safe because this shortage is always met before
//! any adapter is opened.** It is a missing file, checked on the way in: `faults`
//! opens its label files before the port and says so in as many words, and the
//! other five sites touch nothing but the filesystem. A retry therefore repeats
//! no read of the car — the thing that would matter — and no output the
//! first attempt already printed.
//!
//! ## What it will not do
//!
//! Ask when there is nobody to answer, and download without an explicit `y`.
//! The fix is ~90 MB over the network and a shell-out to `curl` and `unzip`;
//! the default is no, an empty line is no, and a pipe is not asked at all —
//! there it prints exactly what it always printed and fails exactly as before.

use anyhow::Result;

use crate::missing::NoLabelData;
use crate::ui::menu::Asker;

/// What to tell somebody whose stdin is a pipe. Never reached — the question is
/// not asked without a terminal — but [`crate::ui::menu::Console`] is built
/// with one, and a sentence that cannot be printed still has to be true.
const INSTEAD: &str = "vagcan setup /path/to/VCDS      (or the path to an extracted ODIS project)";

/// The offer, as the person reads it.
///
/// Three facts and no more: what it would do, what it costs, and that it is
/// offline. The shortage itself has already been printed above this — it says
/// what is missing, where it looked, and how to do the same thing by hand — so
/// none of that is repeated here.
const OFFER: &str = "\nvagcan can do that now: fetch a VCDS installation (about 90 MB), read it, and\n\
                     carry on with what you asked for. A few minutes, mostly parsing.\n\
                     Offline — it opens no adapter and touches no car.";

/// The question. `[y/N]` in the text rather than as a default, because the
/// answer is a letter and the default is the empty line.
const QUESTION: &str = "Fetch it and carry on? [y/N]";

/// Whether `err` is a command that failed for want of label data, and whether
/// there is somebody there to be asked about it.
///
/// Both halves, because either one alone is the wrong answer: an offer made to
/// a pipe is a hang, and an offer made about a serial-port error is nonsense.
pub fn worth_offering(err: &anyhow::Error) -> bool {
	// [`crate::ui::can_ask`] is the one predicate for "may this process block
	// on a person", and it is read here rather than spelled out. It is passed
	// down so both branches stay testable: a test's own stdin is whatever
	// `cargo test` was run from, which is a terminal as often as not.
	offerable(err, crate::ui::can_ask())
}

/// The rule behind [`worth_offering`], with the terminal passed in.
fn offerable(err: &anyhow::Error, may_ask: bool) -> bool {
	may_ask && err.downcast_ref::<NoLabelData>().is_some()
}

/// Ask, and if the answer is yes, fetch an installation and read it.
///
/// `true` means the data is now there and the command that failed may be run
/// again. `false` is a plain no — nothing was downloaded, nothing was said that
/// the shortage printed above this did not already say, and the caller fails
/// the way it would have failed anyway.
///
/// Only called once [`worth_offering`] has said yes, so it does not re-check
/// the terminal: the question is [`Asker::line`]'s, which would take its
/// default silently on a pipe, and a silent default is not the refusal this
/// path owes a script.
pub fn offer(archive_base: &str) -> Result<bool> {
	let mut io = crate::ui::Console::new(INSTEAD);
	if !asked(&mut io)? {
		return Ok(false);
	}
	fix(archive_base)?;
	Ok(true)
}

/// The question and what its answer means, behind [`Asker`] so both answers are
/// testable without a terminal.
///
/// Anything but `y`/`yes` is no, an empty line included. That is the same rule
/// [`crate::faultnames::offer_to_unseal`] follows for the same kind of question
/// — an expensive thing offered unprompted — and it is the rule this one needs
/// most: the cost here is a download.
fn asked(io: &mut impl Asker) -> Result<bool> {
	io.say(OFFER)?;
	let typed = io.line(QUESTION, "")?;
	Ok(matches!(typed.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

/// Fetch an installation and read it, asking only what `setup` cannot answer.
///
/// The whole of `setup`, not a piece of it: the download is only how an
/// installation is obtained, and everything after it — the copy, the label
/// cache, the names, the `.rod` keys, the project it all lands in — is what the
/// command that failed was actually short of.
///
/// **Which is why it is not silent, and this used to say it was.** `setup` still
/// reaches [`crate::setup::source::project_id`], which asks what the project
/// should be called and offers a default made from the folder — one question,
/// answered by pressing Enter. Nothing here can answer it instead: the name is
/// the reader's, and a run that invented one would leave them with a directory
/// they did not choose and no line telling them so.
///
/// `refresh` is false. This runs because something was *missing*; redoing work
/// already on disk is a different request, and `vagcan setup --refresh` is
/// still the only way to ask for it.
fn fix(archive_base: &str) -> Result<()> {
	crate::setup::run(crate::setup::Options {
		dir: None,
		refresh: false,
		archive_base,
		download: true,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::ui::menu::{Answer, Scripted};

	/// An error out of one of the six sites, built the way they build it.
	fn shortage() -> anyhow::Error {
		anyhow::Error::from(NoLabelData::new("No car has been set up yet.").looked_in(std::path::Path::new("/x/data")))
	}

	#[test]
	fn the_shortage_survives_being_bailed_as_an_error() {
		// The whole mechanism in one assertion: the sites report this by
		// `bail!`-ing the type, and the dispatcher recognises it by downcasting
		// to the type. A site that rendered it to a string would still print
		// the same paragraph and would silently never be offered a fix, which
		// is the failure worth a test of its own.
		fn like_a_site() -> Result<()> {
			anyhow::bail!(NoLabelData::new("The project `SK37X` has no label cache.").looked_for(std::path::Path::new("/x/c.sqlite")))
		}
		let err = like_a_site().unwrap_err();
		assert!(err.downcast_ref::<NoLabelData>().is_some(), "{err}");
		// And it still prints as it always did, prefix to fix.
		assert!(err.to_string().contains("vagcan setup /path/to/VCDS"), "{err}");
	}

	#[test]
	fn nothing_is_offered_without_a_terminal() {
		// The rule that matters most here: a pipe, a script, CI. No question is
		// asked, so the command reports the shortage and fails exactly as it
		// did before this module existed.
		assert!(!offerable(&shortage(), false));
		// And with somebody there, the same error is worth asking about.
		assert!(offerable(&shortage(), true));
	}

	#[test]
	fn only_this_shortage_is_offered_a_download() {
		// A serial port that is not there, a survey file that will not parse —
		// no amount of VCDS fixes either, and an offer to download ninety
		// megabytes at one of them would be noise in front of the real message.
		let other = anyhow::anyhow!("no adapter at /dev/tty.usbmodem1234");
		assert!(!offerable(&other, true));
	}

	#[test]
	fn the_default_is_no_and_an_empty_line_takes_it() {
		// A download nobody asked for is the one outcome this must never have.
		// Enter — the empty line — is a no, and so is anything that is not a
		// yes: `later`, `n`, a stray path.
		for typed in ["", "n", "no", "N", "later", "/Applications/VCDS"] {
			let mut io = Scripted::new(vec![Answer::Type(typed.to_string())]);
			assert!(!asked(&mut io).unwrap(), "{typed:?} must not start a download");
		}
		// Quitting the question — Ctrl-D — is a no as well.
		let mut io = Scripted::new(vec![Answer::Quit]);
		assert!(!asked(&mut io).unwrap());
	}

	#[test]
	fn a_yes_is_a_yes_however_it_is_typed() {
		for typed in ["y", "Y", "yes", "YES", " y "] {
			let mut io = Scripted::new(vec![Answer::Type(typed.to_string())]);
			assert!(asked(&mut io).unwrap(), "{typed:?} is a yes");
		}
	}

	#[test]
	fn the_question_says_what_it_costs_before_it_asks() {
		// Somebody is about to be asked to spend ninety megabytes and a few
		// minutes. Both figures are in front of the question, and so is the one
		// reassurance that matters in a tool that reads cars: this touches none.
		let mut io = Scripted::new(vec![Answer::Type(String::new())]);
		asked(&mut io).unwrap();
		let said = io.all_said();
		assert!(said.contains("90 MB"), "{said}");
		assert!(said.contains("minutes"), "{said}");
		assert!(said.contains("no car") || said.contains("touches no car"), "{said}");
		// It must not repeat the fix the shortage above it has just spelled
		// out — the reader has that paragraph on screen.
		assert!(!said.contains("vagcan setup /path/to/VCDS"), "{said}");
		// One question, and it defaults to nothing rather than to a yes.
		assert_eq!(io.typed.len(), 1);
		assert_eq!(io.defaults(), vec![String::new()]);
		assert!(io.typed[0].0.contains("[y/N]"), "{:?}", io.typed[0]);
	}
}
