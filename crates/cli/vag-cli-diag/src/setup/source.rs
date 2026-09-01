//! What `setup` was told to learn this car from, and how it was told.
//!
//! `setup` used to take one argument and mean one thing by it: a VCDS
//! installation, or nothing and an offer to download one. There are two sources
//! now — a VCDS installation and an extracted ODIS-Service project — and no
//! argument can be read as both, so the question has to be asked. This module
//! is the asking: the four ways in, the copy that tells them apart, the
//! directory each one then needs, and what to say when the directory is the
//! wrong one.
//!
//! **The ODIS project leads, and that is a reversal.** This module first
//! ordered the menu by how many people have one, which put the VCDS
//! installation on top: offering VW's own dealer data first, pre-highlighted
//! and one Enter away, made the default answer the one almost nobody could
//! give. That was right while ODIS was a second source bolted onto a
//! VCDS-shaped tool, and it stopped being right when the tool was rebuilt
//! around it.
//!
//! What decided it is not preference. A VCDS label file carries a
//! measurement's *name* and provably not the join from that name to the
//! identifier it is read from, nor its scaling — refuted structurally, twice
//! (`research/labels/rod-labels.md` §4.0c). An ODIS project carries the whole
//! chain and declares it per ECU variant, and three rows this project had
//! proved by driving came back identical out of the ODIS file with no drive,
//! two of them engine-speed channels with opposite byte order. So the top two
//! rows are both the ODIS project — paired with VCDS wording, then alone.
//!
//! **The two sources compose, and the top row is that composition.** What an
//! ODIS project calls a channel is real but machine-phrased —
//! `Engine_temperature`, `Brake_pedal_information_plausibility` — and each one
//! carries a text id (`MAS06602`) which is *the same key* VCDS's recovered
//! `names.json` is written under. So one source gives the structure and the
//! other gives the wording for the very same rows, and offering that pair as a
//! first-class answer is better than leaving somebody to discover it by running
//! `setup` twice.
//!
//! **What the VCDS line may not claim.** An earlier version of it said VCDS
//! supplies fault names, implying ODIS cannot. ODIS can: the project carries
//! `DTC_*` objects in six figures with their descriptions in the clear. What is
//! missing is a loader, which is a limit of this build and not a property of
//! the format, and shipped copy must not freeze a temporary gap into a claim
//! about the sources. Nor does the line lean on "units ODIS misses" — that was
//! two of fifteen on one car under one project, which is a sample of one. What
//! is durably true is what VCDS is *for*: a car no ODIS project covers, or a
//! person who cannot get one. It also provably does not carry scalings
//! (`research/labels/rod-labels.md` §4.0c) and has no per-variant channel list
//! at all, which is why it is no longer the answer on top.
//!
//! **A wrong directory is the ordinary failure, not an exceptional one.** The
//! two misses seen in practice are pointing at `Labels/` inside an installation
//! and pointing at `~/Downloads` instead of `~/Downloads/SK37X`, and both are a
//! person who is looking straight at the right folder. So a refusal says what
//! was expected *and*, where it can tell, which folder they meant — and then
//! asks again rather than ending the command.
//!
//! **Nothing here reads a file.** A directory is recognised by which names are
//! in it, which is all that can be known before `vag_data_labels::odis` or the label
//! parsers get their hands on it. Being wrong in the permissive direction is
//! cheap — the parser says so, in more detail than this could — and being wrong
//! in the strict direction turns somebody's real project away at the door.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::ui::menu::{Asker, Item};

/// The name an unnamed project gets.
///
/// D3 of the plan: the S42 chassis-type lookup that would derive a real
/// identifier for a VCDS-only car is not built, and nothing here pretends
/// otherwise. It is a placeholder, and it is only ever *offered* — see
/// [`project_id`], which prefers a project that already exists.
const DEFAULT_ID: &str = "default";

/// The most a project name may be. A directory name, not a sentence.
const MAX_ID: usize = 64;

/// The characters a project name may hold, besides letters and digits.
///
/// A project is a directory under `~/.vagcan/data/`, so its name is a
/// filesystem name and the interesting question is which characters would make
/// it something other than one child of that directory.
const ID_EXTRAS: [char; 3] = ['-', '_', '.'];

/// What an extracted ODIS project has in it that nothing else does.
///
/// The string pool, under either spelling: a project shipped compressed and one
/// somebody has already gunzipped are the same project, and `vag_data_labels::odis` is
/// the authority on which it can actually read.
const ODIS_STRINGS: [&str; 2] = ["AStringData.data.gz", "AStringData.data"];

/// The ending of one ODIS object pool's index. A project is ~470 of these.
///
/// `.key` and not `.sd.key`, which is what this first matched on. A pool is
/// named `0.0.0@<PoolID>.<kind>.key`, and `.sd` (service data) is only the
/// commonest of six kinds counted in a real project — `.bv` base variants, `.fg`
/// functional groups, `.pr` protocols, `.cp` com params, `.vi` vehicle info. A
/// project with no `.sd` pool at all cannot be ruled out, and turning a real one
/// away at the door is the exact failure this recognition exists to prevent.
///
/// It is loose on its own — plenty of things end in `.key` — which is why it is
/// never asked on its own: a directory is a project when it has [`ODIS_STRINGS`]
/// **and** one of these, and neither half is evidence by itself.
const ODIS_POOL: &str = ".key";

/// What `setup` was told to learn this car from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
	Odis { dir: PathBuf },
	Vcds { dir: PathBuf },
	DownloadVcds,
}

/// What a directory looks like from the outside, before anything parses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Look {
	Vcds,
	Odis,
}

impl Look {
	/// The other one. There are two, and every refusal that names one has to be
	/// able to name the other.
	fn other(self) -> Look {
		match self {
			Look::Vcds => Look::Odis,
			Look::Odis => Look::Vcds,
		}
	}

	/// The noun, with the article a sentence needs in front of it.
	fn named(self) -> &'static str {
		match self {
			Look::Vcds => "a VCDS installation",
			Look::Odis => "an ODIS project",
		}
	}

	/// How to recognise one, in the words its own refusals use.
	fn expected(self) -> &'static str {
		match self {
			Look::Vcds => "A VCDS installation root is the folder holding `Labels/` and `UDS_EV/`.",
			Look::Odis => "An extracted ODIS project is the folder holding `AStringData.data.gz` and the `<pool>.key` files.",
		}
	}

	/// The question asked to get the directory.
	fn question(self) -> &'static str {
		match self {
			Look::Vcds => "Where is the VCDS installation?",
			Look::Odis => "Where is the ODIS project?",
		}
	}

	fn source(self, dir: PathBuf) -> Source {
		match self {
			Look::Vcds => Source::Vcds { dir },
			Look::Odis => Source::Odis { dir },
		}
	}
}

/// What the menu asks.
const QUESTION: &str = "What should vagcan learn this car from?";

/// The four ways in, the one that can finish the job first.
///
/// Split out from [`choose`] so the copy can be measured: a detail line one
/// column too long is silently cut to `…` by the renderer, and one version of
/// the ODIS line lost the words that said why anyone would pick it. Eighty
/// columns is the terminal to write for — see the test.
///
/// Each detail does two jobs in under sixty columns: say how to recognise the
/// thing, and say what picking it gets you.
fn options<'a>() -> [Item<'a>; 4] {
	MENU.map(|(label, detail, _)| Item { label, detail })
}

/// What picking a row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pick {
	/// Ask for a directory of this kind.
	Dir(Look),
	/// Fetch an installation first; it is read as one afterwards.
	Download,
	/// Ask for an ODIS project, then for somewhere to get the wording.
	OdisAndNames,
}

/// The menu as one table: the label, the line under it, and what picking it
/// does.
///
/// One table rather than a list of items beside a `match` on row numbers. The
/// order has been reversed once and grown once, and a reorder that moves the
/// copy without moving the outcome is the worst kind of silent: every row still
/// works, and every row does the wrong thing.
///
/// **What the two lines about sources may and may not claim.** An ODIS project
/// carries the channels, the scalings *and* the fault codes with their text in
/// the clear; the loader for the last of those is still being written, which is
/// a limit of this build and not a property of the format, so no line here says
/// VCDS is needed for fault names. A VCDS installation carries wording and
/// fault text and provably not scalings (`research/labels/rod-labels.md`
/// §4.0c), and it has no per-variant channel list at all — so its line says
/// what it is *for*, which is a car no ODIS project covers.
const MENU: [(&str, &str, Pick); 4] = [
	(
		"ODIS + VCDS names",
		"channels and scalings from ODIS, wording from VCDS",
		Pick::OdisAndNames,
	),
	(
		"ODIS project",
		"a folder like SK37X — what to read, and how to scale it",
		Pick::Dir(Look::Odis),
	),
	(
		"VCDS installation",
		"Labels/ and UDS_EV/ — when no ODIS project covers the car",
		Pick::Dir(Look::Vcds),
	),
	("Download VCDS", "fetch Ross-Tech's installer, about 90 MB, and read that", Pick::Download),
];

/// What the second question asks, once an ODIS project is in hand.
const NAMES_QUESTION: &str = "Where should the measurement names come from?";

/// What answering the second question does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wording {
	Point,
	Download,
	Skip,
}

/// The second menu: where the wording for the ODIS channels comes from.
///
/// **Skipping is a real answer and it is on the list.** A project holding ODIS
/// structure and no wording is valid and useful — the channels keep their
/// machine phrasing (`Engine_temperature`) and everything reads and scales — so
/// abandoning here must land on that rather than on a failed `setup`. Offering
/// the download here too, rather than sending somebody back to the first menu
/// for it, is the same argument: they have already answered the expensive
/// question.
const NAMES_MENU: [(&str, &str, Wording); 3] = [
	("VCDS installation", "point at one — its text table carries the wording", Wording::Point),
	("Download VCDS", "fetch Ross-Tech's installer, about 90 MB", Wording::Download),
	("Skip the names", "the channels keep the phrasing ODIS gives them", Wording::Skip),
];

/// Where the wording comes from, or `None` for none.
fn names_options<'a>() -> [Item<'a>; 3] {
	NAMES_MENU.map(|(label, detail, _)| Item { label, detail })
}

/// What `setup` was told to read, whole.
///
/// Two fields because one option needs two inputs. `source` is what the run is
/// *about* — the structure, and the thing the project is named after; `names`
/// is where the human wording for those channels comes from, when it comes from
/// somewhere else. `names` is only ever a VCDS source: an ODIS project already
/// names its own channels, and nothing else supplies wording at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
	pub source: Source,
	pub names: Option<Source>,
}

impl Choice {
	/// A source on its own, which is three of the four rows and every path
	/// given on the command line.
	fn only(source: Source) -> Choice {
		Choice { source, names: None }
	}

	/// The "Download VCDS" row, without the menu it is a row of.
	///
	/// The one answer this module can give that nobody has to be asked for: a
	/// caller that already has a `yes` in hand (`setup::Options::download`)
	/// hands it in here rather than re-asking a question it has the answer to.
	/// Built here and not out of the enum directly so the download keeps
	/// arriving as a [`Choice`] — [`super::fetched`] resolves it into the
	/// installation it fetched, whichever of the three asked for it.
	pub fn download() -> Choice {
		Choice::only(Source::DownloadVcds)
	}
}

/// Run the picker.
///
/// `preselected` is the path `setup` was given on the command line. It skips
/// the menu *and* the question of which kind it is: the folder itself says,
/// and asking somebody to classify a directory they are pointing straight at
/// would be a question with a knowable answer.
///
/// `None` means the person left without choosing, which is a successful,
/// zero-exit outcome — the same rule `setup`'s download prompt already follows.
/// An empty line at the directory question means the same thing, and so a
/// redirected stdin (which takes every default, [`Asker::line`]) backs out
/// rather than hanging.
pub fn choose(io: &mut impl Asker, preselected: Option<&str>) -> Result<Option<Choice>> {
	if let Some(given) = preselected {
		return Ok(Some(Choice::only(given_path(given)?)));
	}
	let Some(row) = io.ask(QUESTION, &options(), 0)? else {
		return Ok(None);
	};
	match MENU.get(row).map(|(_, _, pick)| *pick) {
		Some(Pick::OdisAndNames) => odis_and_names(io),
		Some(Pick::Dir(look)) => Ok(ask_for(io, look)?.map(Choice::only)),
		Some(Pick::Download) => Ok(Some(Choice::only(Source::DownloadVcds))),
		// An asker that named a row outside the menu has named nothing. Nobody
		// chose anything, which is the same answer as leaving.
		None => Ok(None),
	}
}

/// The recommended row: an ODIS project, then somewhere to get the wording.
///
/// **The ODIS project is asked for first**, and the order is not arbitrary.
/// It is the source the run is *about*: it decides what can be read at all,
/// and it is what the project is named after (spec §4.1), so the run has no
/// identity until it is answered. Asking for the installation first would be
/// asking somebody to furnish the wording for channels that may turn out not
/// to be there.
///
/// **Giving up on the second question is not giving up on the run.** An empty
/// answer at the first question backs out of the whole thing, because nothing
/// has been decided yet; abandoning the second lands on the ODIS project alone,
/// which is exactly what the row below this one would have produced. A project
/// with structure and no wording reads and scales perfectly well — the channels
/// simply keep the phrasing ODIS gives them — so a half-finished pair is a
/// smaller result, never a failed `setup`.
fn odis_and_names(io: &mut impl Asker) -> Result<Option<Choice>> {
	let Some(source) = ask_for(io, Look::Odis)? else {
		return Ok(None);
	};
	// Quitting the second menu is the same answer as skipping it, and lands in
	// the same place rather than returning early — an early return here skipped
	// the line below that says what was given up, which is the one thing
	// somebody who changed their mind needs to read.
	let names = match io.ask(NAMES_QUESTION, &names_options(), 0)? {
		Some(row) => match NAMES_MENU.get(row).map(|(_, _, wording)| *wording) {
			Some(Wording::Point) => ask_for(io, Look::Vcds)?,
			Some(Wording::Download) => Some(Source::DownloadVcds),
			// A row outside the menu names nothing, which is no wording either.
			Some(Wording::Skip) | None => None,
		},
		None => None,
	};
	if names.is_none() {
		// Wrapped by hand, like every other message this command prints: `say`
		// does not clip, so a sentence left to the terminal comes out ragged.
		io.say(
			"No names source — the channels will read under the phrasing ODIS gives\n\
             them. Adding one later is another `vagcan setup` into this project.",
		)?;
	}
	Ok(Some(Choice { source, names }))
}

/// Ask for the directory of a kind already chosen, until it is one or the
/// person gives up.
///
/// Typed rather than picked, and that is a decision worth the sentence:
/// [`crate::ui::picker::pick_path`] descends a *fixed* number of levels from a
/// *fixed* root, which is right for `~/.vagcan/cars/<vin>/measures` and wrong
/// here — neither the root nor the depth of an installation is knowable
/// (`/Applications/VCDS`, `~/Downloads/SK37X`, an external disk). The path is
/// also already in the person's hands: every file manager copies one, and
/// dropping a folder on a terminal pastes it. [`expand`] is what makes that
/// paste work.
fn ask_for(io: &mut impl Asker, want: Look) -> Result<Option<Source>> {
	io.say("Drag the folder into this window, or paste its path. An empty line goes back.")?;
	loop {
		let typed = io.line(want.question(), "")?;
		if typed.trim().is_empty() {
			return Ok(None);
		}
		let dir = expand(&typed);
		if identify(&dir) == Some(want) {
			return Ok(Some(want.source(dir)));
		}
		// Said, not returned: a wrong folder is a thing to correct, and ending
		// the command over it would cost the person the menu as well.
		io.say(&refused(&dir, want))?;
	}
}

/// The path `setup` was given, read for what it is.
///
/// An error here rather than a question: a path on the command line is a
/// statement, and the run that made it cannot be talked out of it.
fn given_path(given: &str) -> Result<Source> {
	let dir = expand(given);
	match identify(&dir) {
		Some(look) => Ok(look.source(dir)),
		None => bail!(unrecognised(&dir)),
	}
}

/// Which of the two a directory is, if either.
///
/// Order matters only in that the two tests are exclusive in practice: nothing
/// holds both a `UDS_EV/` and an ODIS string pool.
fn identify(dir: &Path) -> Option<Look> {
	if !dir.is_dir() {
		return None;
	}
	if dir.join(super::ODX_DIR).is_dir() {
		return Some(Look::Vcds);
	}
	if ODIS_STRINGS.iter().any(|name| dir.join(name).is_file()) && has_pool(dir) {
		return Some(Look::Odis);
	}
	None
}

/// Whether a directory holds at least one ODIS object pool.
///
/// The string pool alone is not enough: a folder holding only
/// `AStringData.data.gz` is half an extraction, and accepting it would send
/// `vag_data_labels::odis` looking for objects that are not there.
fn has_pool(dir: &Path) -> bool {
	let Ok(listing) = std::fs::read_dir(dir) else { return false };
	listing.flatten().any(|entry| entry.file_name().to_string_lossy().ends_with(ODIS_POOL))
}

/// A directory near the one that was given which *is* one of the two.
#[derive(Debug)]
enum Near {
	/// The given directory is inside this one — they pointed too deep.
	Above(PathBuf, Look),
	/// This one is inside the given directory — they pointed too shallow.
	Inside(PathBuf, Look),
}

/// How far up and how many entries down to look for what they meant.
///
/// Both bounded: an unbounded walk of `/` would stat a filesystem to answer a
/// question about one typo.
const LOOK_UP: usize = 3;
const LOOK_IN: usize = 200;

/// What they probably meant, if anything nearby is one of the two.
///
/// `want` first when there is one, because the kind they picked is the kind
/// they are looking for; the other kind afterwards, because "that is the other
/// one" is still an answer they can act on. With no `want` — a path given on
/// the command line, which says nothing about which was meant — the ODIS
/// project is looked for first, the same order the menu offers them in.
fn nearby(dir: &Path, want: Option<Look>) -> Option<Near> {
	let order = match want {
		Some(look) => [look, look.other()],
		None => [Look::Odis, Look::Vcds],
	};
	for kind in order {
		if let Some(above) = dir.ancestors().skip(1).take(LOOK_UP).find(|up| identify(up) == Some(kind)) {
			return Some(Near::Above(above.to_path_buf(), kind));
		}
		if let Some(inside) = children(dir).into_iter().find(|child| identify(child) == Some(kind)) {
			return Some(Near::Inside(inside, kind));
		}
	}
	None
}

/// The entries of a directory, in name order and bounded.
///
/// Sorted so that a directory holding two candidates names the same one twice
/// rather than whichever the filesystem happened to hand over first.
fn children(dir: &Path) -> Vec<PathBuf> {
	let Ok(listing) = std::fs::read_dir(dir) else { return Vec::new() };
	let mut paths: Vec<PathBuf> = listing.flatten().take(LOOK_IN).map(|entry| entry.path()).collect();
	paths.sort();
	paths
}

/// Why this directory is not the one, and which one probably is.
///
/// Three things, in the order somebody reads them: what is wrong with what they
/// gave, what they most likely meant, and what the right thing looks like. The
/// last one is unconditional — a refusal that only guesses leaves anybody it
/// guessed wrong about with nothing.
fn refused(dir: &Path, want: Look) -> String {
	let shown = dir.display();
	// Being *the other kind* is a complete answer on its own, and a hint after
	// it would only point back at the folder already named.
	if let Some(other) = identify(dir) {
		return format!(
			"{shown} is {}, not {}.\n    Go back and pick that instead, or point at {} here.\n    {}",
			other.named(),
			want.named(),
			want.named(),
			want.expected()
		);
	}
	let mut out = match (dir.exists(), dir.is_dir()) {
		(false, _) => format!("{shown} is not a directory — there is nothing at that path."),
		(true, false) => format!("{shown} is not a directory — it is a file. If that is an archive, unpack it and point at the folder it unpacks to."),
		(true, true) => format!("{shown} is not {}.", want.named()),
	};
	out.push_str(&hint(dir, Some(want)));
	out.push_str(&format!("\n    {}", want.expected()));
	out
}

/// The "did you mean" half of a refusal, or nothing when there is no guess.
fn hint(dir: &Path, want: Option<Look>) -> String {
	match nearby(dir, want) {
		Some(Near::Above(path, look)) if want.is_none_or(|w| w == look) => {
			format!(
				"\n    It is inside {}, which is one. Point at that instead:\n        {}",
				path.display(),
				path.display()
			)
		}
		Some(Near::Above(path, look)) => format!(
			"\n    It is inside {}, and that is {} rather than what you picked.",
			path.display(),
			look.named()
		),
		Some(Near::Inside(path, look)) if want.is_none_or(|w| w == look) => {
			format!("\n    It does hold one. Did you mean:\n        {}", path.display())
		}
		Some(Near::Inside(path, look)) => format!("\n    It holds {}, which is {}.", path.display(), look.named()),
		None => String::new(),
	}
}

/// A path given on the command line that is neither of the two.
///
/// Names both shapes, because nothing was picked and so nothing says which one
/// they were after — and names where a VCDS installation comes from, because
/// "you have neither" is the likeliest reason to be here at all.
fn unrecognised(dir: &Path) -> String {
	let shown = dir.display();
	let head = match (dir.exists(), dir.is_dir()) {
		(false, _) => format!("{shown} is not a directory — there is nothing at that path."),
		(true, false) => format!("{shown} is not a directory — it is a file. If that is an archive, unpack it and point at the folder it unpacks to."),
		(true, true) => format!("{shown} is neither an ODIS project nor a VCDS installation."),
	};
	// The ODIS shape first, the order the menu offers them in.
	format!(
		"{head}{}\n\n    {}\n    {}\n\n\
         With no path at all, `vagcan setup` asks which to read — and offers to download an\n\
         installation if you have neither.\n\
         Ross-Tech's own: {}",
		hint(dir, None),
		Look::Odis.expected(),
		Look::Vcds.expected(),
		crate::missing::VCDS_DOWNLOAD
	)
}

/// A path as a person hands one over.
///
/// Three things happen to it, and each of them is somebody's ordinary paste:
/// a folder dropped on a terminal arrives backslash-escaped, one copied out of
/// a file manager arrives quoted, and one typed by hand arrives with a `~` that
/// no process expands for itself. Getting any of those wrong produces "there is
/// nothing at that path" about a path that is plainly there.
///
/// `~user` is deliberately left alone: this tool does not know where somebody
/// else's home directory is, and a guess would name a path nobody meant.
fn expand(typed: &str) -> PathBuf {
	let trimmed = typed.trim();
	let (bare, quoted) = match unquoted(trimmed) {
		Some(inner) => (inner, true),
		None => (trimmed, false),
	};
	// Inside quotes a backslash is a backslash; outside them it is the terminal
	// escaping the character after it.
	let text = match quoted {
		true => bare.to_string(),
		false => unescape(bare),
	};
	let Some(home) = dirs::home_dir() else { return PathBuf::from(text) };
	match text.strip_prefix('~') {
		Some("") => home,
		Some(rest) => match rest.strip_prefix('/') {
			Some(under) => home.join(under),
			// `~someone/else` — not ours to resolve.
			None => PathBuf::from(text),
		},
		None => PathBuf::from(text),
	}
}

/// What is inside a matched pair of quotes, if the whole string is one.
fn unquoted(text: &str) -> Option<&str> {
	for quote in ['"', '\''] {
		if text.len() >= 2 && text.starts_with(quote) && text.ends_with(quote) {
			return Some(&text[1..text.len() - 1]);
		}
	}
	None
}

/// A backslash before a character means the character.
fn unescape(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	let mut chars = text.chars();
	while let Some(c) = chars.next() {
		match c {
			'\\' => out.extend(chars.next()),
			c => out.push(c),
		}
	}
	out
}

/// Ask what to call this project.
///
/// Returns the ODIS folder name unasked when the source is ODIS (spec §4.1) —
/// that string is already the identifier VW's own tooling uses, and asking
/// would invite a second name for one project. Asks otherwise, because D3 leaves a
/// VCDS-only project with nothing to derive a name from.
///
/// Either way it says what it settled on. A run with no terminal answers this
/// question by taking the default without printing anything ([`Asker::line`]),
/// and a project name that appears nowhere is a directory somebody finds later
/// with no idea how it got there.
///
/// **The folder name is the fallback, not the authority.** A project names
/// itself inside, in `index.xml`'s `<CATALOG><SHORT-NAME>`, and that survives
/// what an unzip does to a folder — `SK37X (1)` on disk is still `SK37X` in
/// there. `vag_data_labels::odis::Project::id()` is what reads it, and this question is
/// asked *before* a project is opened, so it cannot. Two consequences the caller
/// owns: prefer `Project::id()` over this answer once the project is open, and
/// do not create the directory under this name until they have been compared —
/// a store at `SK37X-1` holding data that calls itself `SK37X` is one platform in
/// two places. Nothing here reads `index.xml` itself: one rule with two
/// implementations that can disagree is worse than one that is asked twice.
pub fn project_id(io: &mut impl Asker, source: &Source, existing: &[String]) -> Result<String> {
	let folder = match source {
		Source::Odis { dir } => dir.file_name().map(|name| name.to_string_lossy().into_owned()),
		_ => None,
	};
	// A folder that is already a usable name is the answer. One that is not —
	// "SK 37X (copy)" out of an unzip — still knows what it wants to be called,
	// so it is offered as the default rather than thrown away.
	if let Some(name) = &folder
		&& why_not(name).is_none()
	{
		io.say(&settled(name, existing.iter().any(|id| id == name), true))?;
		return Ok(name.clone());
	}
	let default = match (folder.as_deref().map(clean), existing) {
		// The ODIS folder name, cleaned into something a directory can be
		// called.
		(Some(cleaned), _) if !cleaned.is_empty() => cleaned,
		// One project already here is almost certainly this car: a second VCDS
		// build on one laptop is a different build, not a different platform —
		// and a platform is what a project is, so even a genuinely different car
		// usually belongs in the same one. Offering `default` beside it would
		// split one platform in two for the price of one keystroke.
		(_, [only]) => only.clone(),
		_ => DEFAULT_ID.to_string(),
	};
	io.say(
		// Wrapped by hand at the width every other message in this command is
		// wrapped at. `say` does not clip — it must not, it is not a redraw — so
		// a sentence left to the terminal comes out as a ragged wall.
		"A project is a kind of car rather than one car — VW's own naming puts several\n\
         models under one, so two cars can share it. What is true of exactly one car\n\
         lives under cars/<VIN>/ instead.\n\
         A second source is added to a project rather than replacing what is there.",
	)?;
	// A default that appears out of nowhere is worse than no default: somebody
	// shown `[SK-37X-copy]` with no explanation cannot tell whether the tool
	// read it somewhere or invented it.
	if let Some(name) = &folder
		&& name != &default
	{
		io.say(&format!(
			"The folder is called `{name}`; `{default}` is that name made into one a directory can have."
		))?;
	}
	if !existing.is_empty() {
		io.say(&format!("Projects already here: {}", existing.join(", ")))?;
	}
	loop {
		let typed = io.line("What should this project be called?", &default)?;
		let id = typed.trim().to_string();
		match why_not(&id) {
			None => {
				io.say(&settled(&id, existing.contains(&id), false))?;
				return Ok(id);
			}
			// Asked again rather than refused: a name is one keystroke, and
			// losing the whole run over a slash would be a poor trade.
			// The resolved path goes last because it is the one part whose
			// length is unknown here — anywhere else it would push the rest of
			// the sentence past the width this is wrapped to.
			Some(why) => io.say(&format!(
				"`{id}` cannot be a project name — {why}.\n    \
                 A name may hold letters, digits, `-`, `_` and `.` and nothing else:\n    \
                 it is one folder under {}",
				projects_in()
			))?,
		}
	}
}

/// Where projects live, in words, for a sentence a person reads.
///
/// **A literal path in printed copy is a promise**, and this one has already
/// broken once: `~/.vagcan/data/measured/` outlived the directory it named and
/// sent somebody on a real car to put a proven row where nothing was looking for
/// it. So it is asked of [`crate::datadir`], which owns the layout, rather than
/// written out here — and falls back to the shape only when there is no home
/// directory to resolve against, which is the one case where naming a real path
/// is impossible rather than merely stale.
fn projects_in() -> String {
	match crate::datadir::projects_dir() {
		Ok(dir) => dir.display().to_string(),
		Err(_) => "~/.vagcan/data/".to_string(),
	}
}

/// What the run says about the project it landed on.
///
/// The merge case is the one that has to be said out loud: spec §5 adds a
/// source to an existing project rather than replacing it, and somebody who
/// believes they are starting fresh would otherwise find out from the data.
fn settled(id: &str, already: bool, from_odis: bool) -> String {
	let how = match from_odis {
		true => " — the name ODIS gives this folder",
		false => "",
	};
	// Two lines, because one ran to 110 columns on a real terminal. The id is
	// the only part whose length is unknown, so it goes on the first.
	let what = match already {
		true => "This source is added to it; nothing already in it is replaced.",
		false => "New — nothing has been read into it yet.",
	};
	format!("Project `{id}`{how}.\n{what}")
}

/// Why this cannot be a directory under `~/.vagcan/data/`, if it cannot.
fn why_not(id: &str) -> Option<String> {
	if id.is_empty() {
		return Some("a name with nothing in it is not a name".to_string());
	}
	if id == "." || id == ".." {
		return Some(format!("`{id}` already names a folder: the one above, or the one it is in"));
	}
	if id.chars().count() > MAX_ID {
		return Some(format!("it is {} characters, and {MAX_ID} is the most a name may be", id.chars().count()));
	}
	// A separator is the one worth naming first even when something else came
	// earlier in the string: it is the difference between a bad name and a name
	// that is not in the projects folder at all.
	let bad = id
		.chars()
		.find(|c| matches!(c, '/' | '\\'))
		.or_else(|| id.chars().find(|c| !allowed(*c)))?;
	Some(match bad {
		separator @ ('/' | '\\') => format!("`{separator}` would put it somewhere else entirely"),
		' ' => "a space cannot be in a folder name here".to_string(),
		other => format!("`{other}` cannot be in a folder name here"),
	})
}

/// Whether one character may be in a project name.
fn allowed(c: char) -> bool {
	c.is_ascii_alphanumeric() || ID_EXTRAS.contains(&c)
}

/// The nearest thing to `text` that could be a directory name.
///
/// Everything a name may not hold becomes a `-`, runs collapse, and the ends
/// are trimmed — so "SK 37X (copy)" offers itself as "SK-37X-copy" rather than
/// being dropped for a `default` that says nothing about the car.
fn clean(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	for c in text.chars() {
		match allowed(c) {
			true => out.push(c),
			false if !out.ends_with('-') => out.push('-'),
			false => {}
		}
	}
	out.trim_matches(['-', '.']).chars().take(MAX_ID).collect()
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::ui::menu::{Answer, Scripted};

	/// A stand-in for a VCDS installation: the one directory `setup` recognises
	/// it by, and the one it copies beside it.
	fn vcds(root: &Path) -> PathBuf {
		let dir = root.join("vcds-en");
		std::fs::create_dir_all(dir.join("UDS_EV")).unwrap();
		std::fs::create_dir_all(dir.join("Labels")).unwrap();
		std::fs::write(dir.join("UDS_EV/RD.rod"), b"registry").unwrap();
		std::fs::write(dir.join("Labels/part.lbl"), b"001,1,Engine Speed,,").unwrap();
		dir
	}

	/// A stand-in for an extracted ODIS project: the string pool and one pool
	/// pair, named the way a real one is (`0.0.0@<PoolID>.<kind>.key`).
	/// **No ODIS byte is in this repository** — these are empty files under the
	/// names the real ones use, which is all the picker looks at.
	fn odis(root: &Path, name: &str) -> PathBuf {
		odis_of_kind(root, name, "sd")
	}

	/// The same, with the pool kind named — `sd` service data, `bv` base
	/// variants, `fg` functional groups, `pr` protocols, `cp` com params, `vi`
	/// vehicle info.
	fn odis_of_kind(root: &Path, name: &str, kind: &str) -> PathBuf {
		let dir = root.join(name);
		std::fs::create_dir_all(&dir).unwrap();
		std::fs::write(dir.join("AStringData.data.gz"), b"").unwrap();
		std::fs::write(dir.join(format!("0.0.0@BL_LIBECM.{kind}.key")), b"").unwrap();
		std::fs::write(dir.join(format!("0.0.0@BL_LIBECM.{kind}.db")), b"").unwrap();
		dir
	}

	fn typed(path: &Path) -> Answer {
		Answer::Type(path.display().to_string())
	}

	/// The row a kind is on, so a test says which source it picks rather than
	/// which number. The order has moved once already and should not cost a
	/// test edit when it moves again.
	fn row(look: Look) -> Answer {
		let at = MENU
			.iter()
			.position(|(_, _, pick)| *pick == Pick::Dir(look))
			.expect("every kind of source is on the menu");
		Answer::Pick(at)
	}

	/// The row that fetches an installation rather than pointing at one.
	fn download_row() -> Answer {
		let at = MENU
			.iter()
			.position(|(_, _, pick)| *pick == Pick::Download)
			.expect("the download is on the menu");
		Answer::Pick(at)
	}

	#[test]
	fn the_menu_leads_with_the_pair_that_can_finish_the_job() {
		// Reversed once, then grown. The VCDS installation led while ODIS was a
		// second source bolted onto a VCDS-shaped tool, and that stopped
		// applying when the tool was rebuilt around ODIS. What leads now is the
		// two together: ODIS says which identifiers a variant answers and how
		// to scale them, VCDS supplies the wording for the very same rows, and
		// both sides key on the same text id. The pre-highlighted row should be
		// the one that finishes the job most completely.
		let here = tempfile::tempdir().unwrap();
		let project = odis(here.path(), "SK37X");
		let install = vcds(here.path());
		let mut io = Scripted::new(vec![Answer::Pick(0), typed(&project), wording_row(Wording::Point), typed(&install)]);
		let chosen = choose(&mut io, None).unwrap().unwrap();
		assert_eq!(chosen.source, Source::Odis { dir: project });
		assert_eq!(chosen.names, Some(Source::Vcds { dir: install }));
		assert_eq!(
			io.seen[0].1.iter().map(|(label, _)| label.as_str()).collect::<Vec<_>>(),
			["ODIS + VCDS names", "ODIS project", "VCDS installation", "Download VCDS"]
		);
		assert_eq!(io.highlights[0], 0, "and it is the row the highlight starts on");
	}

	/// The row that asks for both, and the row of the second menu that answers
	/// the wording question — named rather than numbered, for the same reason
	/// [`row`] is.
	fn pair_row() -> Answer {
		let at = MENU
			.iter()
			.position(|(_, _, pick)| *pick == Pick::OdisAndNames)
			.expect("the pair is on the menu");
		Answer::Pick(at)
	}

	fn wording_row(which: Wording) -> Answer {
		let at = NAMES_MENU
			.iter()
			.position(|(_, _, w)| *w == which)
			.expect("every answer is on the names menu");
		Answer::Pick(at)
	}

	#[test]
	fn the_recommended_row_asks_for_the_project_first_and_the_wording_after() {
		// The ODIS project decides what can be read at all and is what the
		// project is named after, so it is the question with an answer worth
		// having on its own. Asking for the installation first would be asking
		// somebody to furnish wording for channels that may not be there.
		let here = tempfile::tempdir().unwrap();
		let project = odis(here.path(), "SK37X");
		let install = vcds(here.path());
		let mut io = Scripted::new(vec![pair_row(), typed(&project), wording_row(Wording::Point), typed(&install)]);
		let chosen = choose(&mut io, None).unwrap().unwrap();
		assert_eq!(chosen.source, Source::Odis { dir: project });
		assert_eq!(chosen.names, Some(Source::Vcds { dir: install }));
		let asked: Vec<&str> = io.typed.iter().map(|(question, _)| question.as_str()).collect();
		assert_eq!(asked, ["Where is the ODIS project?", "Where is the VCDS installation?"], "in that order");
	}

	#[test]
	fn the_wording_can_be_downloaded_without_going_back_to_the_first_menu() {
		// They have already answered the expensive question. Sending them back
		// to the top to pick the download would throw that away.
		let here = tempfile::tempdir().unwrap();
		let project = odis(here.path(), "SK37X");
		let mut io = Scripted::new(vec![pair_row(), typed(&project), wording_row(Wording::Download)]);
		let chosen = choose(&mut io, None).unwrap().unwrap();
		assert_eq!(chosen.source, Source::Odis { dir: project });
		assert_eq!(chosen.names, Some(Source::DownloadVcds));
	}

	#[test]
	fn abandoning_the_wording_leaves_a_project_rather_than_a_failed_run() {
		// A project with structure and no wording reads and scales perfectly
		// well — the channels keep the phrasing ODIS gives them. So skipping,
		// quitting the second menu, and leaving its directory question empty
		// are all the same answer, and none of them is a failed `setup`.
		let here = tempfile::tempdir().unwrap();
		let project = odis(here.path(), "SK37X");
		let alone = Some(Choice::only(Source::Odis { dir: project.clone() }));
		for script in [
			vec![pair_row(), typed(&project), wording_row(Wording::Skip)],
			vec![pair_row(), typed(&project), Answer::Quit],
			vec![pair_row(), typed(&project), wording_row(Wording::Point), Answer::Type(String::new())],
		] {
			let mut io = Scripted::new(script);
			assert_eq!(choose(&mut io, None).unwrap(), alone);
			// Asserted either side of the hand-wrap, never across it.
			let said = io.all_said();
			assert!(said.contains("No names source"), "it says what that means: {said}");
			assert!(said.contains("into this project"), "and how to add one later: {:?}", io.said);
		}
	}

	#[test]
	fn leaving_the_first_question_empty_still_backs_out_of_the_whole_run() {
		// Nothing has been decided yet, so there is nothing to keep.
		let mut io = Scripted::new(vec![pair_row(), Answer::Type(String::new())]);
		assert_eq!(choose(&mut io, None).unwrap(), None);
		assert_eq!(io.seen.len(), 1, "the wording was never asked about: {:?}", io.seen);
	}

	#[test]
	fn every_option_of_both_menus_fits_the_terminal_somebody_actually_has() {
		// Four rows now, and the first label is the longest yet.
		for (question, items) in [(QUESTION, options().to_vec()), (NAMES_QUESTION, names_options().to_vec())] {
			let drawn = crate::ui::menu::screen(question, &items, 0, 80);
			let cut: Vec<&String> = drawn.iter().filter(|line| line.contains('…')).collect();
			assert!(cut.is_empty(), "cut off at 80 columns: {cut:?}");
		}
	}

	#[test]
	fn the_row_a_person_picks_is_the_source_that_row_names() {
		// The reorder hazard, pinned. Label, detail and outcome travel together
		// in one table, so moving the copy cannot leave the outcome behind —
		// which would be silent, because every row would still work and every
		// row would do the wrong thing.
		for (label, _, pick) in MENU {
			match pick {
				Pick::OdisAndNames => assert!(label.contains("ODIS") && label.contains("names"), "{label}"),
				Pick::Dir(Look::Odis) => assert!(label.contains("ODIS") && !label.contains("names"), "{label}"),
				Pick::Dir(Look::Vcds) => assert!(label.contains("VCDS installation"), "{label}"),
				Pick::Download => assert!(label.contains("Download"), "{label}"),
			}
		}
	}

	#[test]
	fn every_option_says_in_its_own_line_what_it_is_and_how_to_recognise_it() {
		// "ODIS project" is two words most owners have never met, so the label
		// cannot carry the offer on its own. Each line has to say both how to
		// recognise the thing and what picking it gets you.
		let mut io = Scripted::new(vec![Answer::Quit]);
		assert_eq!(choose(&mut io, None).unwrap(), None);
		let menu = io.last_menu();
		assert!(menu.contains("What should vagcan learn this car from?"), "{menu}");
		assert!(menu.contains("Labels/"), "the VCDS line says how to recognise one: {menu}");
		assert!(menu.contains("SK37X"), "the ODIS line shows what one is called: {menu}");
		assert!(menu.contains("90 MB"), "the download says what it costs: {menu}");
		// The reason to pick each, not just the way to spot it. The VCDS line
		// matters most: it is second now, and nobody may come away thinking
		// their installation has been made useless.
		// The reason to pick each, not just the way to spot it — and no claim
		// that only VCDS can name a fault, which is a gap in this build rather
		// than a property of the sources.
		assert!(menu.contains("how to scale it"), "the ODIS line says what only it supplies: {menu}");
		assert!(menu.contains("wording from VCDS"), "the pair says what each half brings: {menu}");
		assert!(menu.contains("no ODIS project covers"), "the VCDS line says what it is for: {menu}");
		assert!(!menu.contains("fault names"), "VCDS is not the only source of those: {menu}");
	}

	#[test]
	fn each_option_leads_to_the_source_it_names() {
		let here = tempfile::tempdir().unwrap();
		let project = odis(here.path(), "SK37X");
		let mut io = Scripted::new(vec![row(Look::Odis), typed(&project)]);
		assert_eq!(choose(&mut io, None).unwrap(), Some(Choice::only(Source::Odis { dir: project })));

		// Downloading asks for no directory: there is nothing on disk yet.
		let mut io = Scripted::new(vec![download_row()]);
		assert_eq!(choose(&mut io, None).unwrap(), Some(Choice::only(Source::DownloadVcds)));
		assert!(io.typed.is_empty(), "nothing was asked for: {:?}", io.typed);
	}

	#[test]
	fn an_empty_line_at_the_directory_is_never_mind_rather_than_an_error() {
		let mut io = Scripted::new(vec![row(Look::Vcds), Answer::Type(String::new())]);
		assert_eq!(choose(&mut io, None).unwrap(), None);
	}

	#[test]
	fn a_directory_that_is_neither_is_refused_by_name_and_asked_for_again() {
		let here = tempfile::tempdir().unwrap();
		let music = here.path().join("music");
		std::fs::create_dir_all(&music).unwrap();
		let install = vcds(here.path());
		let mut io = Scripted::new(vec![row(Look::Vcds), typed(&music), typed(&install)]);
		assert_eq!(choose(&mut io, None).unwrap(), Some(Choice::only(Source::Vcds { dir: install })));
		let said = io.all_said();
		assert!(said.contains(&music.display().to_string()), "the path is named: {said}");
		assert!(said.contains("Labels/") && said.contains("UDS_EV/"), "what was expected is named: {said}");
		assert_eq!(io.typed.len(), 2, "a refusal asks again rather than ending the command");
	}

	#[test]
	fn pointing_inside_an_installation_names_the_root_one_level_up() {
		// The common miss: a file manager opened `Labels/` to look at it, and
		// that is the path that got pasted.
		let here = tempfile::tempdir().unwrap();
		let install = vcds(here.path());
		let labels = install.join("Labels");
		let mut io = Scripted::new(vec![row(Look::Vcds), typed(&labels), typed(&install)]);
		choose(&mut io, None).unwrap();
		let said = io.all_said();
		assert!(said.contains("is inside"), "{said}");
		assert!(said.contains(&install.display().to_string()), "it names the root to use: {said}");
	}

	#[test]
	fn pointing_at_the_folder_above_names_the_one_that_does_look_right() {
		// The other common miss: `~/Downloads` instead of `~/Downloads/SK37X`.
		let here = tempfile::tempdir().unwrap();
		let project = odis(here.path(), "SK37X");
		let mut io = Scripted::new(vec![row(Look::Odis), typed(here.path()), typed(&project)]);
		choose(&mut io, None).unwrap();
		let said = io.all_said();
		assert!(said.contains(&project.display().to_string()), "it names what they probably meant: {said}");
	}

	#[test]
	fn pointing_at_the_other_kind_says_which_kind_it_actually_is() {
		let here = tempfile::tempdir().unwrap();
		let install = vcds(here.path());
		let project = odis(here.path(), "SK37X");
		let mut io = Scripted::new(vec![row(Look::Odis), typed(&install), typed(&project)]);
		choose(&mut io, None).unwrap();
		let said = io.all_said();
		assert!(said.contains("is a VCDS installation, not an ODIS project"), "{said}");
	}

	#[test]
	fn a_path_that_is_not_a_directory_says_so_in_the_words_the_docs_use() {
		let here = tempfile::tempdir().unwrap();
		let archive = here.path().join("vcds-en.zip");
		std::fs::write(&archive, b"PK").unwrap();
		let install = vcds(here.path());
		let mut io = Scripted::new(vec![row(Look::Vcds), typed(&archive), typed(&install)]);
		choose(&mut io, None).unwrap();
		let said = io.all_said();
		assert!(said.contains("is not a directory"), "USAGE.md documents this phrase: {said}");
		assert!(said.contains("unpack"), "an archive is a case, not a mystery: {said}");
	}

	#[test]
	fn a_path_given_on_the_command_line_skips_the_menu_and_is_recognised() {
		// `vagcan setup ~/Downloads/SK37X` should work without the person having
		// to say which of the two kinds it is — the folder itself says.
		let here = tempfile::tempdir().unwrap();
		let install = vcds(here.path());
		let project = odis(here.path(), "SK37X");
		let mut io = Scripted::new(vec![]);
		assert_eq!(
			choose(&mut io, Some(&install.display().to_string())).unwrap(),
			Some(Choice::only(Source::Vcds { dir: install }))
		);
		assert_eq!(
			choose(&mut io, Some(&project.display().to_string())).unwrap(),
			Some(Choice::only(Source::Odis { dir: project }))
		);
		assert!(io.seen.is_empty(), "the menu never appeared");
	}

	#[test]
	fn a_given_path_that_is_neither_names_both_shapes_and_where_vcds_comes_from() {
		let mut io = Scripted::new(vec![]);
		let why = choose(&mut io, Some("/definitely/not/here")).unwrap_err().to_string();
		assert!(why.contains("is not a directory"), "{why}");
		assert!(why.contains("Labels/"), "{why}");
		assert!(why.contains("AStringData.data.gz"), "the other kind is named too: {why}");
		assert!(why.contains(crate::missing::VCDS_DOWNLOAD), "{why}");
		assert!(why.contains("offers to download"), "the other way in is named: {why}");
	}

	#[test]
	fn a_given_folder_above_a_project_still_says_what_they_probably_meant() {
		let here = tempfile::tempdir().unwrap();
		let project = odis(here.path(), "SK37X");
		let mut io = Scripted::new(vec![]);
		let why = choose(&mut io, Some(&here.path().display().to_string())).unwrap_err().to_string();
		assert!(why.contains(&project.display().to_string()), "{why}");
	}

	#[test]
	fn a_pool_of_any_kind_makes_a_project_not_just_the_service_data_one() {
		// This first matched on `.sd.key`, which is only the commonest of the
		// six kinds a real project carries. A project whose pools happen to be
		// `.bv`/`.fg`/`.pr`/`.cp`/`.vi` is still a project, and turning a real
		// one away at the door is the failure this recognition exists to stop.
		let here = tempfile::tempdir().unwrap();
		for kind in ["sd", "bv", "fg", "pr", "cp", "vi"] {
			let project = odis_of_kind(here.path(), &format!("SK37X-{kind}"), kind);
			assert_eq!(identify(&project), Some(Look::Odis), "a `.{kind}` pool is a pool");
		}
	}

	#[test]
	fn a_directory_of_keys_with_no_string_pool_is_not_a_project() {
		// `.key` on its own is loose — an SSH directory, a certificate store.
		// The two halves are only evidence together.
		let here = tempfile::tempdir().unwrap();
		let keys = here.path().join("keys");
		std::fs::create_dir_all(&keys).unwrap();
		std::fs::write(keys.join("id_rsa.key"), b"").unwrap();
		assert_eq!(identify(&keys), None);
	}

	#[test]
	fn half_an_extraction_is_not_a_project() {
		// The string pool without a single object pool: `vag_data_labels::odis` would
		// be sent looking for objects that are not there.
		let here = tempfile::tempdir().unwrap();
		let half = here.path().join("SK37X");
		std::fs::create_dir_all(&half).unwrap();
		std::fs::write(half.join("AStringData.data.gz"), b"").unwrap();
		assert_eq!(identify(&half), None);
	}

	#[test]
	fn an_odis_project_is_named_by_the_folder_odis_itself_named() {
		// Spec §4.1: the directory name is already the identifier VW's own
		// tooling uses. Asking would invite a second name for one project.
		let here = tempfile::tempdir().unwrap();
		let project = odis(here.path(), "SK37X");
		let mut io = Scripted::new(vec![]);
		let id = project_id(&mut io, &Source::Odis { dir: project }, &[]).unwrap();
		assert_eq!(id, "SK37X");
		assert!(io.typed.is_empty(), "nothing was asked");
		assert!(io.all_said().contains("SK37X"), "it still says what it landed on: {:?}", io.said);
	}

	#[test]
	fn an_odis_project_landing_on_a_name_already_here_says_it_is_adding_to_it() {
		// Spec §5: a second source is added, not swapped in. Somebody who thinks
		// they are starting fresh has to be told they are not.
		let here = tempfile::tempdir().unwrap();
		let project = odis(here.path(), "SK37X");
		let mut io = Scripted::new(vec![]);
		let id = project_id(&mut io, &Source::Odis { dir: project }, &["SK37X".to_string()]).unwrap();
		assert_eq!(id, "SK37X");
		let said = io.all_said();
		assert!(said.contains("added"), "{said}");
		assert!(said.contains("nothing already in it is replaced"), "{said}");
	}

	#[test]
	fn an_odis_folder_whose_name_is_no_folder_name_is_offered_a_cleaned_one() {
		// A project unpacked into "SK 37X (copy)" still has to land somewhere.
		let here = tempfile::tempdir().unwrap();
		let project = odis(here.path(), "SK 37X (copy)");
		let mut io = Scripted::new(vec![Answer::Type(String::new())]);
		let id = project_id(&mut io, &Source::Odis { dir: project }, &[]).unwrap();
		assert_eq!(io.defaults(), ["SK-37X-copy"], "the offered default is the folder name, cleaned");
		assert_eq!(id, "SK-37X-copy");
		// A default out of nowhere is worse than no default: it has to say
		// where it came from, or nobody can tell it from an invention.
		let said = io.all_said();
		assert!(said.contains("SK 37X (copy)"), "the folder it came from is named: {said}");
		assert!(said.contains("SK-37X-copy"), "{said}");
	}

	#[test]
	fn a_vcds_project_is_asked_for_and_defaults_to_default() {
		// D3: no S42 lookup exists, so there is nothing to derive a name from.
		let here = tempfile::tempdir().unwrap();
		let install = vcds(here.path());
		let mut io = Scripted::new(vec![Answer::Type(String::new())]);
		let id = project_id(&mut io, &Source::Vcds { dir: install }, &[]).unwrap();
		assert_eq!(id, "default");
		assert_eq!(io.defaults(), ["default"]);
	}

	#[test]
	fn with_one_project_already_here_that_is_what_pressing_enter_takes() {
		// Better than `default`: a second VCDS build on the same laptop is
		// almost always the same car, and `default` beside `SK37X` would split
		// one platform's data across two projects for the price of one keystroke.
		let here = tempfile::tempdir().unwrap();
		let install = vcds(here.path());
		let mut io = Scripted::new(vec![Answer::Type(String::new())]);
		let id = project_id(&mut io, &Source::Vcds { dir: install }, &["SK37X".to_string()]).unwrap();
		assert_eq!(id, "SK37X");
		assert!(io.all_said().contains("nothing already in it is replaced"), "{:?}", io.said);
	}

	#[test]
	fn with_several_projects_here_they_are_all_named_so_one_can_be_typed() {
		let here = tempfile::tempdir().unwrap();
		let install = vcds(here.path());
		let mut io = Scripted::new(vec![Answer::Type("SK37X".to_string())]);
		let existing = ["SK37X".to_string(), "default".to_string()];
		assert_eq!(project_id(&mut io, &Source::Vcds { dir: install }, &existing).unwrap(), "SK37X");
		let said = io.all_said();
		assert!(said.contains("SK37X") && said.contains("default"), "both are on screen: {said}");
		assert_eq!(io.defaults(), ["default"], "with more than one there is nothing to guess");
	}

	#[test]
	fn the_refusal_points_at_the_directory_projects_are_actually_in() {
		// A literal path in printed copy is a promise, and this one has already
		// moved once (`projects/` → `data/`). It is resolved from the module
		// that owns the layout rather than spelled out here, so the next move
		// cannot leave a person looking for a directory nobody has.
		let here = tempfile::tempdir().unwrap();
		let install = vcds(here.path());
		let mut io = Scripted::new(vec![Answer::Type("no/pe".to_string()), Answer::Type("ok".to_string())]);
		project_id(&mut io, &Source::Vcds { dir: install }, &[]).unwrap();
		let said = io.all_said();
		assert!(!said.contains(".vagcan/projects"), "the directory that no longer exists: {said}");
		let real = crate::datadir::projects_dir().unwrap();
		assert!(
			said.contains(&real.display().to_string()),
			"it names where a project really lands: {said}"
		);
	}

	#[test]
	fn the_offer_does_not_call_a_project_one_car() {
		// A project is a platform — one *kind* of car. VW's own mapping puts
		// several models under one name, so "one car's data" would promise a
		// separation that is not there, and somebody would go looking for a
		// second project for their second Octavia.
		let here = tempfile::tempdir().unwrap();
		let install = vcds(here.path());
		let mut io = Scripted::new(vec![Answer::Type(String::new())]);
		project_id(&mut io, &Source::Vcds { dir: install }, &[]).unwrap();
		let said = io.all_said();
		assert!(!said.contains("one car's data"), "{said}");
		assert!(said.contains("kind of car"), "it says what a project actually covers: {said}");
		// The one-car store is still a real thing, and it is somewhere else.
		assert!(said.contains("cars/"), "and where what is true of one car lives: {said}");
	}

	#[test]
	fn a_name_that_could_not_be_a_folder_is_refused_and_asked_again() {
		let here = tempfile::tempdir().unwrap();
		let install = vcds(here.path());
		let mut io = Scripted::new(vec![Answer::Type("my car/2".to_string()), Answer::Type("my-car-2".to_string())]);
		let id = project_id(&mut io, &Source::Vcds { dir: install }, &[]).unwrap();
		assert_eq!(id, "my-car-2");
		let said = io.all_said();
		assert!(said.contains("my car/2"), "{said}");
		assert!(said.contains('/'), "the character that would put it elsewhere is named: {said}");
		assert_eq!(io.typed.len(), 2, "it asked again rather than giving up");
	}

	#[test]
	fn a_name_that_would_climb_out_of_the_projects_folder_is_refused() {
		let here = tempfile::tempdir().unwrap();
		let install = vcds(here.path());
		let mut io = Scripted::new(vec![Answer::Type("..".to_string()), Answer::Type("ok".to_string())]);
		assert_eq!(project_id(&mut io, &Source::Vcds { dir: install }, &[]).unwrap(), "ok");
		assert!(io.all_said().contains(".."), "{:?}", io.said);
	}

	#[test]
	fn a_pasted_path_survives_the_way_a_terminal_hands_one_over() {
		// A folder dropped on a terminal arrives backslash-escaped; one pasted
		// out of a file manager arrives quoted; one typed by a person arrives
		// with a `~` that no process expands for itself.
		assert_eq!(expand("'/Users/you/My Downloads/SK37X'"), Path::new("/Users/you/My Downloads/SK37X"));
		assert_eq!(expand("\"/Users/you/SK37X\""), Path::new("/Users/you/SK37X"));
		assert_eq!(expand("/Users/you/My\\ Downloads/SK37X"), Path::new("/Users/you/My Downloads/SK37X"));
		assert_eq!(expand("  /Users/you/SK37X  "), Path::new("/Users/you/SK37X"));
		let home = dirs::home_dir().unwrap();
		assert_eq!(expand("~/Downloads/SK37X"), home.join("Downloads/SK37X"));
		assert_eq!(expand("~"), home);
		// Not a home directory of somebody else's — this tool does not know
		// where that is, and guessing would land on a path nobody meant.
		assert_eq!(expand("~someone/else"), Path::new("~someone/else"));
	}
}
