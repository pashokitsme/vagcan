//! What `vagcan` says when it is run with no subcommand.
//!
//! It used to say `error: 'vagcan' requires a subcommand but one was not
//! provided`, which is clap telling the truth about its own grammar and nothing
//! about this tool. The first thing somebody types is the bare name, and the
//! answer to it should be what this is, what state the machine is in, and what
//! to type next — in that order, because the third depends on the second.
//!
//! **Every fact on the screen is read, never assumed.** The adapter line is
//! [`device::list`]'s answer and the car line is [`project`]'s; anything that
//! cannot be established cheaply is left off rather than guessed at, which is
//! why [`Adapters`] and [`Cars`] each carry an `Unknown` that prints as an
//! admission instead of as a claim. A confident wrong line here is worse than
//! no line: it is read at an open driver's door by somebody deciding whether
//! the cable or the car is at fault.
//!
//! **An admission carries why.** Every `Unknown` here holds the error that
//! produced it, formatted once at the point it happened, and the screen prints
//! it under the state line as `caused by:` — a permission bit, a missing mount
//! and a truncated file all read as "could not be read" otherwise, and they
//! send somebody to three different places.
//!
//! **Nothing here touches the car.** No adapter is opened, no frame is sent —
//! listing USB serial devices and reading `~/.vagcan` is the whole of it, so
//! this stays instant and stays safe to run with the engine running.

use anyhow::Context;
use vag_cli_core::{datadir, device, project};
use vag_uds_can::AdapterInfo;

/// What is plugged into this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Adapters {
	/// Recognised CAN adapters — the ones `vagcan devices` marks with a `*`.
	Ready(Vec<AdapterInfo>),
	/// Serial devices are connected, and none of them is a CAN adapter this
	/// tool recognises. Kept apart from [`Adapters::None`] because the two need
	/// different advice: nothing to plug in versus something to look at.
	Unrecognised(usize),
	/// Nothing serial is connected at all.
	None,
	/// The listing itself failed, so nothing is known either way. Carries the
	/// reason as one line (see [`cause`]), because "could not be listed" alone
	/// does not say whether the port node is missing or the permission is.
	Unknown(String),
}

/// Which car's data this run would use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cars {
	/// Exactly one project answers for a bare command.
	One(CarData),
	/// Several projects are set up and nothing picks one. The fix is to say
	/// which, so both ways of saying it are offered.
	Ambiguous(Vec<String>),
	/// Something named a project that is not there — a typo in `--project`, in
	/// `VAGCAN_PROJECT` or in `~/.vagcan/config.json`. Kept apart from
	/// [`Cars::Ambiguous`] because the advice there is to name one, which is
	/// exactly what just failed. Carries [`project::current`]'s own words, so
	/// this screen reports the cause every other command reports.
	Misnamed { ids: Vec<String>, why: String },
	/// `vagcan setup` has never run on this machine.
	None,
	/// `~/.vagcan/data/` could not be read, which is not the same as empty.
	/// Carries the reason as one line (see [`cause`]): a permission bit and an
	/// unmounted home directory are both this state and neither is the other.
	Unknown(String),
}

/// How much of a car the project's cache describes, or why that is not known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Described {
	/// `(variants, channels)` from an ODIS source, channels never nought.
	Channels(u64, u64),
	/// A cache with no ODIS side: a VCDS installation carries names and no
	/// scalings, and its rows live in another table entirely.
	None,
	/// There is no cache at all — the project was never built, or its build
	/// stopped before writing one. Apart from [`Described::Unknown`] because
	/// the fix differs: this one is `setup`, that one is a broken file.
	Unbuilt,
	/// The cache is there and would not answer — unreadable, or not this
	/// schema. Apart from [`Described::None`] because "nothing described" and
	/// "could not be read" send somebody to different places, and apart from
	/// [`Described::Unbuilt`] for the same reason one step further: a reader
	/// told to run `setup` on a *corrupt* cache runs it and gets the same
	/// screen back. Carries the reason as one line (see [`cause`]) — sqlite
	/// says which of the two it is, and it is the only thing that can.
	Unknown(String),
}

/// One project, summarised in what can be read in milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarData {
	pub id: String,
	/// Which sources have contributed, from the project's own provenance log.
	/// Both `false` on a project whose log is missing — then the summary says
	/// the name and stops, rather than inventing a provenance for it.
	pub from_vcds: bool,
	pub from_odis: bool,
	/// ECU variants described, and channels across them.
	pub described: Described,
	/// How many catalog files carry rows proven on a car — one file per key,
	/// and the key is a part number *or* an ODX name, whichever the unit
	/// reported (`CatalogStore::for_unit`). Those rows are the only numbers
	/// here no re-parse could recreate, which is why they get a line of their
	/// own rather than being folded into the count above.
	pub proven_catalogs: usize,
}

/// Everything the overview knows, gathered without opening the adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
	pub adapters: Adapters,
	pub cars: Cars,
}

/// Read the machine's state. Infallible on purpose: a failure to establish a
/// fact is one of the states, not an error that replaces the screen.
pub fn gather() -> Facts {
	Facts {
		adapters: adapters(),
		cars: cars(),
	}
}

fn adapters() -> Adapters {
	let found = match device::list().context("listing the serial devices") {
		Ok(found) => found,
		Err(e) => return Adapters::Unknown(cause(&e)),
	};
	let known: Vec<AdapterInfo> = found.iter().filter(|a| a.known).cloned().collect();
	match (known.is_empty(), found.len()) {
		(false, _) => Adapters::Ready(known),
		(true, 0) => Adapters::None,
		(true, n) => Adapters::Unrecognised(n),
	}
}

fn cars() -> Cars {
	// The directory is opened here rather than left to `project::list`, which
	// cannot fail: it answers a `data/` it may not read with an empty list, and
	// an empty list on this screen prints as "nothing has been set up yet" over
	// a store that is sitting right there. Only the difference between the two
	// is taken from `read_dir`; the listing itself stays `project`'s, so the
	// rule about which directory names count as projects lives in one place.
	let dir = match datadir::projects_dir().context("locating ~/.vagcan/data/") {
		Ok(dir) => dir,
		Err(e) => return Cars::Unknown(cause(&e)),
	};
	match store(&dir) {
		Store::Listable => {}
		Store::Missing => return Cars::None,
		Store::Unreadable(why) => return Cars::Unknown(why),
	}
	let ids = match project::list().context("listing the projects under ~/.vagcan/data/") {
		Ok(ids) => ids,
		Err(e) => return Cars::Unknown(cause(&e)),
	};
	if ids.is_empty() {
		return Cars::None;
	}
	// `current()` is the same resolution every other command performs, so what
	// this screen reports is what the next command will actually use. When it
	// refuses, the two reasons need different advice, and the count separates
	// them: with one project set up nothing can be ambiguous, so a refusal is
	// something naming a project that is not there. With several, a typo reads
	// as the ambiguity — the list is right either way, and so is "say which".
	match project::current() {
		Ok(project) => Cars::One(summarise(&project)),
		Err(e) if ids.len() < 2 => Cars::Misnamed { ids, why: format!("{e:#}") },
		Err(_) => Cars::Ambiguous(ids),
	}
}

/// What `~/.vagcan/data/` itself says, before anything that is in it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Store {
	/// It opened; which projects it holds is [`project::list`]'s answer.
	Listable,
	/// It is not there — nothing has been set up on this machine.
	Missing,
	/// It is there and would not open, with `read_dir`'s own reason as one line.
	/// Named apart from [`Store::Missing`] because `read_dir` hands both back the
	/// same way, and this screen must not print the second as the first; the
	/// reason travels because a permission bit and a home directory that is not
	/// mounted are both this variant and want different things done.
	Unreadable(String),
}

fn store(dir: &std::path::Path) -> Store {
	match std::fs::read_dir(dir) {
		Ok(_) => Store::Listable,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => Store::Missing,
		// The path is the context because it is the one thing `io::Error` never
		// carries, and it is what tells somebody *which* directory refused.
		Err(e) => Store::Unreadable(cause(&anyhow::Error::new(e).context(format!("reading {}", dir.display())))),
	}
}

/// What one project holds, in the two queries that answer it cheaply.
fn summarise(project: &project::Project) -> CarData {
	CarData {
		id: project.id.clone(),
		from_vcds: project::has_source(project, "vcds"),
		from_odis: project::has_source(project, "odis"),
		described: described(&project.cache()),
		proven_catalogs: proven_catalogs(&project.measurements_dir()),
	}
}

/// What the project's cache says it describes, with a failure kept as one.
///
/// A cache that will not open and a cache with no ODIS rows in it are both
/// "no channel count", and printing them the same way tells somebody whose
/// `cache.sqlite` is truncated that their VCDS-only project is normal.
fn described(cache: &std::path::Path) -> Described {
	// Asked before opening, because the reader opens read-only and so cannot
	// tell "there is no file" from "the file will not parse" out of one error
	// — and those two send somebody to different places.
	if !cache.is_file() {
		return Described::Unbuilt;
	}
	match vag_data_db::channel_counts(cache).with_context(|| format!("reading {}", cache.display())) {
		Ok((variants, channels)) if channels > 0 => Described::Channels(variants, channels),
		Ok(_) => Described::None,
		Err(e) => Described::Unknown(cause(&e)),
	}
}

/// Somebody else's error, as the one line this screen can afford.
///
/// `{e:#}` and not `{e:?}`: anyhow's alternate `Display` walks the whole chain
/// into `our sentence: what sqlite said`, where `Debug` prints a multi-line
/// `Caused by:` block (and a backtrace when one is enabled). This screen is six
/// lines that are meant to be taken in at a glance, in a car park, at a driver's
/// door — a block pasted into the middle of it is the stack trace the layout
/// exists to avoid. So the chain is flattened, and any newline inside a single
/// link goes with it: a cause gets one line however long it is.
///
/// [`Cars::Misnamed`] deliberately does not come through here. Its `why` is not
/// a cause hung off a state line but `project::current`'s whole message, printed
/// under "Next:" as the instruction it is, and its blank lines are structure.
fn cause(e: &anyhow::Error) -> String {
	format!("{e:#}").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// How many catalog files hold rows proven on a car — one file per key.
///
/// A directory that is not there is nought proven, not an error: a project
/// built this morning has no `measurements/` until the first drive writes one.
fn proven_catalogs(dir: &std::path::Path) -> usize {
	let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
	entries
		.flatten()
		.filter(|entry| entry.path().extension().is_some_and(|ext| ext.eq_ignore_ascii_case("json")))
		.count()
}

/// The screen itself, as one string so that a test can read it.
pub fn render(facts: &Facts) -> String {
	let mut out = String::from(
		"vagcan — read a VAG car (VW / Audi / Škoda / SEAT) over CAN, through the OBD-II port.\n\
		 It only ever reads: no coding, no adaptation, no clearing faults, no flashing.\n\n",
	);
	out.push_str(&state(facts));
	out.push('\n');
	out.push_str(&next(facts));
	out.push_str("\n`vagcan --help` lists every command, and `vagcan help <command>` explains one.\n");
	out
}

/// The two lines of state, each one a fact that was read.
fn state(facts: &Facts) -> String {
	let mut out = String::new();
	match &facts.adapters {
		Adapters::Ready(found) => {
			for (n, adapter) in found.iter().enumerate() {
				let label = if n == 0 { "Adapter " } else { "        " };
				out.push_str(&format!("  {label}  {} — {}\n", adapter.path, adapter.description));
			}
		}
		Adapters::Unrecognised(n) => out.push_str(&format!(
			"  Adapter   none recognised — {n} other serial device(s) connected, see `vagcan devices`\n"
		)),
		Adapters::None => out.push_str("  Adapter   nothing connected\n"),
		Adapters::Unknown(why) => {
			out.push_str("  Adapter   could not be listed — `vagcan devices` says why\n");
			out.push_str(&caused_by(why));
		}
	}
	match &facts.cars {
		Cars::One(car) => {
			out.push_str(&format!("  Car data  {}{}\n", car.id, provenance(car)));
			match &car.described {
				Described::Channels(variants, channels) => {
					out.push_str(&format!("            {channels} channels across {variants} control-unit variants\n"));
				}
				Described::None => {}
				Described::Unbuilt => out.push_str("            not built yet — `vagcan setup` reads a source into it\n"),
				Described::Unknown(why) => {
					out.push_str("            its label cache is there and will not open — `vagcan setup --refresh` rewrites it\n");
					out.push_str(&caused_by(why));
				}
			}
			if car.proven_catalogs > 0 {
				out.push_str(&format!(
					"            {} catalog file(s) with scalings proven on a car\n",
					car.proven_catalogs
				));
			}
		}
		// How to pick belongs to the "Next" block below, and saying it twice
		// trains a reader to skim the state lines.
		Cars::Ambiguous(ids) => out.push_str(&format!("  Car data  set up: {} — none picked for this run\n", ids.join(", "))),
		// Likewise the reason: the state line says what is on disk, and why
		// this run does not land on it is the "Next" block's business.
		Cars::Misnamed { ids, .. } => out.push_str(&format!("  Car data  set up: {} — not what this run names\n", ids.join(", "))),
		Cars::None => out.push_str("  Car data  none — nothing has been set up yet\n"),
		Cars::Unknown(why) => {
			out.push_str("  Car data  could not be read under ~/.vagcan/data/\n");
			out.push_str(&caused_by(why));
		}
	}
	out
}

/// The one line a state gets to say why, under the state it belongs to.
///
/// Our sentence stays first and stays the same width as every other line here;
/// the underlying error follows it, indented to the state block's continuation
/// column, so the screen can still be read down the left edge by somebody who
/// only wants to know which of the four facts is missing.
fn caused_by(why: &str) -> String {
	format!("            caused by: {why}\n")
}

/// Where a project's data came from, as a clause to hang off its name.
///
/// Empty when its provenance log says nothing, because the two sources are not
/// interchangeable — an ODIS project carries scalings and a VCDS installation
/// does not — and guessing which one is behind a name would misdescribe what
/// the tool can do with it.
fn provenance(car: &CarData) -> String {
	match (car.from_vcds, car.from_odis) {
		(true, true) => ", from a VCDS installation and an ODIS project".to_string(),
		(true, false) => ", from a VCDS installation".to_string(),
		(false, true) => ", from an ODIS project".to_string(),
		(false, false) => String::new(),
	}
}

/// What to type next — which depends on what is missing, in that order.
///
/// Data first: without it every reading comes back as a raw identifier and a
/// number, so an adapter with no project is the worse of the two gaps. The car
/// commands are only offered where there is something to run them against.
fn next(facts: &Facts) -> String {
	let mut out = String::from("Next:\n");
	if !matches!(facts.cars, Cars::One(_)) {
		out.push_str(&setup_advice(&facts.cars));
		return out;
	}
	let plugged = matches!(facts.adapters, Adapters::Ready(_) | Adapters::Unknown(_));
	if !plugged {
		out.push_str(
			"  Plug in a USB-CAN adapter, then `vagcan devices` to check it enumerated.\n  \
             Wiring: OBD-II pin 6 → CAN-H, pin 14 → CAN-L, pin 5 → GND, termination OFF.\n\n  \
             Away from the car, these work anyway:\n    \
             vagcan vcds …        VCDS's own files: labels, names, logs\n    \
             vagcan recording …   read back a drive recorded with `watch --out`\n",
		);
		return out;
	}
	out.push_str(
		"  vagcan info      which car is this?\n  \
         vagcan units     which control units does it have?\n  \
         vagcan faults    what is it complaining about?\n  \
         vagcan watch     live values from several units, chosen on screen\n",
	);
	out
}

/// The one instruction a machine with no usable project needs.
///
/// It names both sources, because which one somebody has decides what they get:
/// an ODIS project carries scalings and a VCDS installation carries names, and
/// somebody with neither can be handed one — `setup` downloads an installation
/// itself rather than leaving a person to read a link.
fn setup_advice(cars: &Cars) -> String {
	match cars {
		Cars::Ambiguous(ids) => format!(
			"  More than one car is set up ({}), and nothing says which this run is about.\n  \
             Pick one for a command:   vagcan --project <id> …\n  \
             or for this shell:        export VAGCAN_PROJECT=<id>\n",
			ids.join(", ")
		),
		// Nothing added to it and nothing rephrased: this is the error every
		// other command prints for the same run, and a screen that reworded it
		// would have somebody comparing two accounts of one typo.
		Cars::Misnamed { why, .. } => indented(why),
		// Also the answer when `~/.vagcan/data/` could not be read: `setup` is
		// what creates it, and it reports its own failure better than a screen
		// that only knows the directory would not open.
		_ => "  vagcan setup     learn a car, once, offline. Without it a reading is a bare\n                   \
              identifier and a number: the names and the scalings come from somebody\n                   \
              else's data, which may not be shipped with this tool.\n\n  \
              Point it at an extracted ODIS-Service project (names *and* scalings)\n  \
              or a VCDS installation (names). With no path it asks which, and offers\n  \
              to download a VCDS installation for you.\n"
			.to_string(),
	}
}

/// Somebody else's message, moved under the "Next:" heading unchanged.
///
/// Two spaces on every line that has anything on it, so a blank line inside
/// the message stays blank rather than becoming two spaces of trailing space.
fn indented(text: &str) -> String {
	text
		.lines()
		.map(|line| if line.is_empty() { "\n".to_string() } else { format!("  {line}\n") })
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	fn adapter(path: &str, description: &str) -> AdapterInfo {
		AdapterInfo {
			path: path.to_string(),
			description: description.to_string(),
			known: true,
		}
	}

	/// A cause of the shape `cause()` produces: our sentence, then the real one.
	fn unlisted() -> String {
		"listing the serial devices: Permission denied (os error 13)".to_string()
	}

	fn unreadable() -> String {
		"reading /Users/x/.vagcan/data: Permission denied (os error 13)".to_string()
	}

	fn car() -> CarData {
		CarData {
			id: "SK37X".to_string(),
			from_vcds: true,
			from_odis: true,
			described: Described::Channels(669, 399_283),
			proven_catalogs: 3,
		}
	}

	/// Every screen, whatever state produced it, says what the tool is and that
	/// it does not write.
	#[test]
	fn the_screen_always_says_what_this_is_and_that_it_only_reads() {
		for facts in [
			Facts {
				adapters: Adapters::Ready(vec![adapter("/dev/cu.usbmodem1", "CANable 2.0 (slcan)")]),
				cars: Cars::One(car()),
			},
			Facts {
				adapters: Adapters::None,
				cars: Cars::None,
			},
			Facts {
				adapters: Adapters::Unknown(unlisted()),
				cars: Cars::Unknown(unreadable()),
			},
		] {
			let text = render(&facts);
			assert!(text.contains("OBD-II"), "{text}");
			assert!(text.contains("no coding"), "{text}");
			assert!(text.contains("--help"), "{text}");
		}
	}

	#[test]
	fn a_fresh_machine_is_sent_to_setup_and_told_where_the_data_comes_from() {
		// The state every new user is in. Naming both sources matters: which
		// one they have decides whether they get scalings or only names, and
		// somebody with neither has to be told the download exists at all.
		let text = render(&Facts {
			adapters: Adapters::None,
			cars: Cars::None,
		});
		assert!(text.contains("nothing has been set up yet"), "{text}");
		assert!(text.contains("vagcan setup"), "{text}");
		assert!(text.contains("ODIS"), "{text}");
		assert!(text.contains("VCDS"), "{text}");
		assert!(text.contains("download"), "{text}");
		// And it does not offer commands that cannot answer yet.
		assert!(!text.contains("vagcan info"), "{text}");
	}

	#[test]
	fn a_missing_adapter_is_what_to_plug_in_and_what_still_runs_without_it() {
		// Data but no cable: the commands that need the car are not offered,
		// and the ones that read files are.
		let text = render(&Facts {
			adapters: Adapters::None,
			cars: Cars::One(car()),
		});
		assert!(text.contains("nothing connected"), "{text}");
		assert!(text.contains("pin 6"), "the wiring is on the screen that says to plug it in: {text}");
		assert!(text.contains("vagcan devices"), "{text}");
		assert!(text.contains("vagcan vcds"), "{text}");
		assert!(text.contains("vagcan recording"), "{text}");
		assert!(!text.contains("vagcan info"), "{text}");

		// Serial devices with no CAN adapter among them is a different thing to
		// look at, and says so rather than claiming nothing is connected.
		let other = render(&Facts {
			adapters: Adapters::Unrecognised(2),
			cars: Cars::One(car()),
		});
		assert!(other.contains("none recognised"), "{other}");
		assert!(!other.contains("nothing connected"), "{other}");
	}

	#[test]
	fn a_ready_machine_lists_the_commands_a_person_actually_types() {
		let text = render(&Facts {
			adapters: Adapters::Ready(vec![adapter("/dev/cu.usbmodem2101", "CANable 2.0 (slcan)")]),
			cars: Cars::One(car()),
		});
		assert!(text.contains("/dev/cu.usbmodem2101"), "{text}");
		assert!(text.contains("CANable 2.0 (slcan)"), "{text}");
		assert!(text.contains("SK37X, from a VCDS installation and an ODIS project"), "{text}");
		assert!(text.contains("399283 channels across 669 control-unit variants"), "{text}");
		assert!(text.contains("3 catalog file(s) with scalings proven"), "{text}");
		for command in ["vagcan info", "vagcan units", "vagcan faults", "vagcan watch"] {
			assert!(text.contains(command), "{command} missing from:\n{text}");
		}
		// The setup instruction is for machines that need it, and this one does
		// not — an unconditional "run setup" trains people to ignore the line.
		assert!(!text.contains("vagcan setup"), "{text}");
	}

	#[test]
	fn what_could_not_be_read_is_admitted_rather_than_guessed_at() {
		// The rule this screen lives by. A listing that failed must not print
		// as "nothing connected", and an unreadable data directory must not
		// print as "nothing set up" — both would send somebody looking in the
		// wrong place, and the second would invite a second `setup` over a
		// store that is already there.
		let text = render(&Facts {
			adapters: Adapters::Unknown(unlisted()),
			cars: Cars::Unknown(unreadable()),
		});
		assert!(text.contains("could not be listed"), "{text}");
		assert!(!text.contains("nothing connected"), "{text}");
		assert!(text.contains("could not be read"), "{text}");
		assert!(!text.contains("nothing has been set up"), "{text}");
	}

	#[test]
	fn an_adapter_listing_that_failed_prints_our_sentence_and_then_the_real_error() {
		let text = render(&Facts {
			adapters: Adapters::Unknown(unlisted()),
			cars: Cars::One(car()),
		});
		assert!(text.contains("  Adapter   could not be listed"), "our sentence is first: {text}");
		assert!(
			text.contains("            caused by: listing the serial devices: Permission denied (os error 13)"),
			"and the cause is under it, indented: {text}"
		);
	}

	#[test]
	fn a_data_directory_that_will_not_open_prints_our_sentence_and_then_the_real_error() {
		let text = render(&Facts {
			adapters: Adapters::None,
			cars: Cars::Unknown(unreadable()),
		});
		assert!(text.contains("  Car data  could not be read under ~/.vagcan/data/"), "{text}");
		assert!(
			text.contains("            caused by: reading /Users/x/.vagcan/data: Permission denied (os error 13)"),
			"{text}"
		);
	}

	#[test]
	fn a_cache_that_will_not_open_prints_our_sentence_and_then_the_real_error() {
		let text = render(&Facts {
			adapters: Adapters::Ready(vec![adapter("/dev/cu.usbmodem1", "CANable")]),
			cars: Cars::One(CarData {
				described: Described::Unknown("reading /Users/x/.vagcan/data/SK37X/cache.sqlite: sqlite error: file is not a database".to_string()),
				..car()
			}),
		});
		assert!(text.contains("            its label cache is there and will not open"), "{text}");
		assert!(
			text.contains("            caused by: reading /Users/x/.vagcan/data/SK37X/cache.sqlite: sqlite error: file is not a database"),
			"{text}"
		);
	}

	#[test]
	fn the_cause_a_state_carries_is_the_real_one_and_fits_on_one_line() {
		// A placeholder would satisfy "there is a cause" and help nobody, so the
		// causes are taken from the failures themselves, not written here.
		let root = std::env::temp_dir().join(format!("vagcan-overview-cause-{}-{:?}", std::process::id(), std::thread::current().id()));
		let _ = std::fs::remove_dir_all(&root);
		std::fs::create_dir_all(&root).unwrap();

		// `read_dir` over a plain file: the same branch as a `chmod 000` and it
		// needs no permissions to arrange.
		let file = root.join("not-a-directory");
		std::fs::write(&file, b"x").unwrap();
		let Store::Unreadable(why) = store(&file) else {
			panic!("a file is not a listable directory");
		};
		assert!(why.contains(&file.display().to_string()), "which path refused is in it: {why}");
		assert!(why.to_lowercase().contains("not a directory"), "and what the OS said: {why}");
		assert!(!why.contains('\n'), "one line: {why}");

		// A cache that is not a database: sqlite's own words, not "could not be
		// read", which is the sentence this cause exists to qualify.
		let cache = root.join("cache.sqlite");
		std::fs::write(&cache, b"this is not a database").unwrap();
		let Described::Unknown(why) = described(&cache) else {
			panic!("a truncated cache does not answer");
		};
		assert!(why.contains(&cache.display().to_string()), "{why}");
		assert!(why.contains("not a database"), "sqlite says which kind of broken: {why}");
		assert!(!why.contains('\n'), "one line: {why}");

		let _ = std::fs::remove_dir_all(&root);
	}

	#[test]
	fn a_project_that_cannot_say_where_it_came_from_is_only_named() {
		// `has_source` answers false for a project with no provenance log, and
		// the two sources are not interchangeable — so the name stands alone
		// rather than being credited to a source nobody recorded.
		let text = render(&Facts {
			adapters: Adapters::Ready(vec![adapter("/dev/cu.usbmodem1", "CANable")]),
			cars: Cars::One(CarData {
				from_vcds: false,
				from_odis: false,
				described: Described::None,
				proven_catalogs: 0,
				..car()
			}),
		});
		assert!(text.contains("Car data  SK37X\n"), "{text}");
		assert!(!text.contains("from a"), "{text}");
		// And a cache with no channels in it claims no channels.
		assert!(!text.contains("channels"), "{text}");
	}

	#[test]
	fn several_cars_are_listed_rather_than_chosen_between() {
		// Picking one silently is how somebody reads another platform's
		// scalings off this car. Both ways of saying which are named, because
		// the flag is per command and the variable is per shell.
		let text = render(&Facts {
			adapters: Adapters::Ready(vec![adapter("/dev/cu.usbmodem1", "CANable")]),
			cars: Cars::Ambiguous(vec!["AU21X".to_string(), "SK37X".to_string()]),
		});
		assert!(text.contains("set up: AU21X, SK37X"), "{text}");
		assert!(text.contains("--project"), "{text}");
		assert!(text.contains("VAGCAN_PROJECT"), "{text}");
		// Nothing on the car is offered while it is unclear whose data it would
		// be read with.
		assert!(!text.contains("vagcan info"), "{text}");
	}

	#[test]
	fn a_data_directory_that_will_not_open_is_not_a_machine_with_no_data() {
		// `project::list` answers both with an empty list, and this screen
		// prints an empty list as "nothing has been set up yet" — over a store
		// that is there, next to an invitation to run `setup` a second time
		// over it. So the two are told apart before the listing.
		let root = std::env::temp_dir().join(format!("vagcan-overview-{}-{:?}", std::process::id(), std::thread::current().id()));
		let _ = std::fs::remove_dir_all(&root);
		std::fs::create_dir_all(&root).unwrap();

		let data = root.join("data");
		assert_eq!(store(&data), Store::Missing, "a fresh machine has no data directory");
		std::fs::create_dir_all(&data).unwrap();
		assert_eq!(store(&data), Store::Listable);

		// A plain file where the directory should be is the same branch as the
		// `chmod 000` that prompted this, and needs no permissions to arrange.
		let file = root.join("not-a-directory");
		std::fs::write(&file, b"x").unwrap();
		assert!(matches!(store(&file), Store::Unreadable(_)));

		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o000)).unwrap();
			// Run as root there is nothing to deny, and the assertion would be
			// about the test runner rather than about this code.
			if std::fs::read_dir(&data).is_err() {
				let Store::Unreadable(why) = store(&data) else {
					panic!("a directory that will not open is not listable");
				};
				assert!(why.to_lowercase().contains("permission denied"), "the permission bit is named: {why}");
			}
			std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o755)).unwrap();
		}

		// And the state it produces is an admission, not "nothing set up".
		let text = render(&Facts {
			adapters: Adapters::None,
			cars: Cars::Unknown(unreadable()),
		});
		assert!(text.contains("could not be read under ~/.vagcan/data/"), "{text}");
		assert!(!text.contains("nothing has been set up"), "{text}");

		let _ = std::fs::remove_dir_all(&root);
	}

	#[test]
	fn a_project_named_that_is_not_there_says_so_instead_of_blaming_a_choice() {
		// One car set up and a typo in `--project`: calling that "more than one
		// car is set up" is false, and recommending `--project` is telling
		// somebody to redo the thing that just failed. What `current()` said is
		// what every other command prints for this run, so it is what is shown.
		let text = render(&Facts {
			adapters: Adapters::Ready(vec![adapter("/dev/cu.usbmodem1", "CANable")]),
			cars: Cars::Misnamed {
				ids: vec!["SK37X".to_string()],
				why: "--project names the project \"SK73X\", and there is no /Users/x/.vagcan/data/SK73X.\n\n\
				      Set up here: SK37X"
					.to_string(),
			},
		});
		assert!(text.contains("SK73X"), "the name that was not found is on the screen: {text}");
		assert!(text.contains("Set up here: SK37X"), "{text}");
		assert!(!text.contains("More than one car is set up"), "{text}");
		// Nothing on the car is offered until it resolves.
		assert!(!text.contains("vagcan info"), "{text}");
	}

	#[test]
	fn a_cache_that_will_not_open_is_told_apart_from_a_cache_with_no_channels() {
		// A truncated `cache.sqlite` used to read exactly like a VCDS-only
		// project: the provenance line, no channel line, nothing to act on.
		let text = render(&Facts {
			adapters: Adapters::Ready(vec![adapter("/dev/cu.usbmodem1", "CANable")]),
			cars: Cars::One(CarData {
				described: Described::Unknown("reading /Users/x/.vagcan/data/SK37X/cache.sqlite: sqlite error: file is not a database".to_string()),
				..car()
			}),
		});
		assert!(text.contains("will not open"), "{text}");
		assert!(text.contains("--refresh"), "a corrupt cache is rewritten, not merely built: {text}");
		assert!(!text.contains("channels across"), "{text}");

		// And a project that was never built is not the same event. Both used
		// to print "could not be read", which sends somebody who has simply not
		// run `setup` to look for a broken file.
		let unbuilt = render(&Facts {
			adapters: Adapters::Ready(vec![adapter("/dev/cu.usbmodem1", "CANable")]),
			cars: Cars::One(CarData {
				described: Described::Unbuilt,
				..car()
			}),
		});
		assert!(unbuilt.contains("not built yet"), "{unbuilt}");
		assert!(!unbuilt.contains("will not open"), "an absent cache is not a broken one: {unbuilt}");

		// A project with no ODIS rows in it claims nothing either way.
		let quiet = render(&Facts {
			adapters: Adapters::Ready(vec![adapter("/dev/cu.usbmodem1", "CANable")]),
			cars: Cars::One(CarData {
				described: Described::None,
				..car()
			}),
		});
		assert!(!quiet.contains("could not be read"), "{quiet}");
		assert!(!quiet.contains("channels"), "{quiet}");
	}
}
