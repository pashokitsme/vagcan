//! `vagcan faults` — what the car has stored against itself.
//!
//! The fault reader proper: every unit, every confirmed code, and
//! on request the extended data the unit keeps beside it — which is where the
//! occurrence counter and the mileage stamp live on these control units.
//!
//! Two honesty rules run through it:
//!
//! * **Only confirmed codes are called faults.** Asking with status mask
//!   `0xFF` returns everything the unit knows about, including tests that have
//!   simply never run since the memory was last cleared. On the reference car
//!   the body control module answers 508 codes that way, of which three are
//!   actual stored faults.
//! * **A code is printed as a code until something names it.** The texts come
//!   from the run's one source: the ODIS project's own fault tables
//!   ([`crate::odisfaults`]), or the VCDS label files ([`crate::faultnames`]) —
//!   never one filling in for the other. Where the source does not resolve one, the
//!   code stands alone rather than beside a plausible-sounding invention, and `-v`
//!   says why. [`Namers`] is that rule, in one place.
//!
//! What it prints, on stdout, rustc's shape: a block per unit that has codes — its id and
//! name, then each code with its text, and indented under it what the unit recorded —
//! and a summary line ([`render_units`]). On stderr the adapter, where the names come from
//! and the sealed catalogues; the rest at `-v` (`vag_cli_core::verbosity`).
//!
//! Read-only: the service issued is `0x19`, which reads. Clearing faults is
//! `0x14`, which the client's allowlist rejects.

use anyhow::Result;
use vag_cli_core::report::Advice as _;
use vag_cli_core::{marks, note, out, say};
use vag_uds_can::UnitLink;
use vag_uds_client::address::UnitAddress;
use vag_uds_client::dtc::{CONFIRMED, CarTime, FAILED_NOW, FaultContext, UnitStamp};
use vag_uds_client::{AsyncUdsClient, RawDtc, gateway};
use vag_uds_transport::CanId;

/// What one unit reported.
#[derive(Debug, Clone, Default)]
pub struct UnitFaults {
	/// The unit's own component string, when it gave one.
	pub component: Option<String>,
	/// Every code the unit listed, confirmed or not.
	pub all: Vec<RawDtc>,
}

impl UnitFaults {
	pub fn confirmed(&self) -> Vec<&RawDtc> {
		self.all.iter().filter(|d| d.status & CONFIRMED != 0).collect()
	}
}

/// A code's three bytes, as the unit sent them: `047120`.
pub fn code_hex(code: [u8; 3]) -> String {
	format!("{:02X}{:02X}{:02X}", code[0], code[1], code[2])
}

/// The decimal fault number VW's own tools print beside a code.
///
/// **All three bytes are one number**, big-endian. An earlier version of this
/// function split them into a two-byte number and a symptom byte, which was
/// wrong: `00 01 29` is fault 297, not fault 1 symptom 0x29. The refutation is
/// this car's own VCDS scan, which prints `0297` beside the brake unit's code
/// and `291104` beside the steering column's `04 71 20`
/// (`vendor/vcds-ru/Scans/`), matching the 24-bit reading in all four cases
/// checked and the 16-bit one in none.
pub fn fault_number(code: [u8; 3]) -> u32 {
	u32::from_be_bytes([0, code[0], code[1], code[2]])
}

/// How long ago a fault happened, told against the unit's own counters.
///
/// Both halves are differences taken on the same control unit, so neither
/// depends on the car's clock being set right: the stamp is a packed date
/// (`vag_uds_client::dtc::CarTime`) and the difference between two of them is
/// real elapsed time even when the date itself is days out, as it is here.
/// The difference is taken through `seconds_between`, which unpacks first —
/// subtracting the raw stamps overshoots by 4 counts per minute boundary.
pub fn describe_age(context: &FaultContext, now: Option<&UnitStamp>) -> String {
	let mut parts = vec![format!("{} km", context.mileage_km)];
	match context.occurrences {
		0xFF => parts.push("255+ times".to_string()),
		n => parts.push(format!("{n}×")),
	}
	if let Some(now) = now {
		if let Some(seconds) = vag_uds_client::dtc::seconds_between(context.clock, now.clock) {
			let seconds = seconds as f64;
			parts.push(match seconds {
				s if s < 90.0 => format!("{s:.0} s ago"),
				s if s < 5_400.0 => format!("{:.0} min ago", s / 60.0),
				s if s < 172_800.0 => format!("{:.1} h ago", s / 3_600.0),
				s => format!("{:.1} days ago", s / 86_400.0),
			});
		}
		if now.mileage_km >= context.mileage_km {
			parts.push(format!("{} km ago", now.mileage_km - context.mileage_km));
		}
	}
	parts.join(", ")
}

/// When a fault was stored, by the car's own clock: `2026-07-27 00:00:03`, or `—` for a
/// stamp that is not a date.
///
/// Exact, to the second: the stamp is a packed date and time, and two VCDS
/// printouts of this car reproduce field for field (`vag_uds_client::dtc::CarTime`).
/// It is the *car's* clock, which on this car runs some days behind real time —
/// `faults --help` says whose clock the column is.
pub fn describe_when(context: &FaultContext) -> String {
	match CarTime::parse(context.clock) {
		Some(t) => format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second),
		// An unset or corrupt stamp is not a moment.
		None => "—".to_string(),
	}
}

/// Decode the status byte into the states it actually asserts.
///
/// Straight out of ISO 14229-1 table D.1 — every bit is defined there, so
/// nothing here is inferred from this car.
pub fn describe_status(status: u8) -> String {
	const BITS: [(u8, &str); 8] = [
		(0x01, "failed now"),
		(0x02, "failed this cycle"),
		(0x04, "pending"),
		(0x08, "confirmed"),
		(0x10, "not tested since clear"),
		(0x20, "failed since clear"),
		(0x40, "not tested this cycle"),
		(0x80, "warning lamp"),
	];
	let set: Vec<&str> = BITS.iter().filter(|(bit, _)| status & bit != 0).map(|(_, name)| *name).collect();
	if set.is_empty() { format!("{status:02X}") } else { set.join(", ") }
}

/// The status byte in one word for a code's facts line, the most telling state first (ISO 14229-1
/// table D.1): failing now, confirmed, pending, else the byte. `-v` decodes it in full.
pub fn short_status(status: u8) -> String {
	match status {
		s if s & FAILED_NOW != 0 => "failing".to_string(),
		s if s & CONFIRMED != 0 => "confirmed".to_string(),
		s if s & 0x04 != 0 => "pending".to_string(),
		s => format!("{s:02X}"),
	}
}

/// The identifiers by which a unit names its own description file: the ODX
/// name, and the coding index whose first three digits pick the variant
/// (`.archive/research/labels/fault-naming-hop.md` §10.4). Both come off the car.
const ODX_NAME: u16 = 0xF19E;
const ODX_VERSION: u16 = 0xF1A2;

/// Read one identifier value as the padded ASCII these units store.
fn ident_text(bytes: &[u8]) -> String {
	String::from_utf8_lossy(bytes).trim_end_matches(['\0', ' ']).to_string()
}

/// How a code is named, from the run's one source.
///
/// **One source per run** (CLAUDE.md, "ODIS and VCDS are independent sources"). An ODIS
/// project carries a fault table per ECU variant with the text in the clear, and a VCDS
/// installation names a code through a registry, a per-unit catalogue and a text store,
/// each of which can be missing or sealed. A run reads one of them
/// ([`crate::source::run_source`]); a code the source does not name is shown as its number,
/// never in the other source's words. `[faults] language` picks among the source's own
/// languages.
///
/// Either half is absent on a run of the other, and a command with neither still prints
/// the codes.
pub struct Namers {
	/// The run's source, when there is one.
	source: Option<vag_cli_core::source::Source>,
	odis: Option<crate::odisfaults::OdisFaults>,
	vcds: Option<crate::faultnames::Namer>,
	/// Where the VCDS files were looked for, for the note when there are none.
	vcds_root: std::path::PathBuf,
	/// The other source, when it is set up: what a line that cannot be answered here offers.
	other: Option<vag_cli_core::source::Source>,
	/// `[faults] language` naming a language the run's source does not have: a `warning:` and,
	/// where the other source has it, the `help:` that reads it — said at level 0.
	language_warning: Vec<String>,
}

/// What the run's source knows about one unit, looked up once.
pub struct UnitNamers {
	odis: Option<crate::odisfaults::UnitTexts>,
	vcds: Option<vag_data_labels::UnitLookup>,
}

/// What one code is called, taken apart for the listing: the code a tester prints, the
/// words, and what `-v` says about where they came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Named {
	/// `B1168F2`: the project's display code, or VCDS's SAE code and failure type.
	pub dtc: Option<String>,
	/// The text, its line breaks flattened — a fault text is one row on a screen.
	pub text: Option<String>,
	/// Where the words came from when that is worth saying, or why there are none.
	pub note: Option<String>,
	/// Whether there are words: the run's tally counts names, not rows.
	pub named: bool,
	/// Whose words the code shows: [`ODIS`], [`ODIS_GUESSED`]
	/// when the variant is not settled, [`VCDS`]; empty with no words.
	pub src: &'static str,
	/// The variant was picked by family, not by the car's own F1A2: recording F1A2 settles it.
	pub needs_f1a2: bool,
	/// What the text line says after the text when its words are not settled by the car:
	/// [`NOT_SETTLED`].
	pub mark: Option<&'static str>,
}

/// After a text from an ODIS variant the car's identifiers did not settle.
pub const NOT_SETTLED: &str = "(variant not settled)";

/// The project's words, its variant settled by the car.
pub const ODIS: &str = "odis";
/// The project's words from a variant the car's identifiers did not settle — one of
/// several, or picked by family. The listing never presents such a code as settled
/// (`odisfaults::Naming::provenance`).
pub const ODIS_GUESSED: &str = "odis?";
/// The VCDS labels' words.
pub const VCDS: &str = "vcds";

impl Namers {
	/// Open the run's source's fault text (`source`, [`Extracted::run_source`]'s). Never
	/// fails for a shortage of data — see [`Namers::is_empty`] — only for a VCDS pool that
	/// is there and will not open, or for keys named with `--iv-cache` (`given`) that are
	/// not there.
	///
	/// [`Extracted::run_source`]: vag_cli_core::extracted::Extracted::run_source
	pub fn open(source: Option<vag_cli_core::source::Source>, keys: &std::path::Path, given: bool) -> Result<Namers> {
		use vag_cli_core::source::Source;
		let vcds_root = crate::project::rod_pool()?;
		if given && !keys.is_file() {
			return Err(anyhow::anyhow!("--iv-cache {}: no such file", keys.display()).help("leave the flag out to use the VCDS store's rod-keys.json"));
		}
		// Which sources are set up, for the lines that offer the other one: metadata — a source
		// row, a file name — never the other source's text.
		let held = vag_cli_core::source::held_here();
		let other = source.map(Source::other).filter(|other| held.contains(other));
		let mut namers = Namers {
			source,
			odis: None,
			vcds: None,
			vcds_root,
			other,
			language_warning: Vec::new(),
		};
		let setting = crate::config::fault_language(&crate::config::load());
		let vcds_language = || {
			crate::odisfaults::VcdsBuild::of(&namers.vcds_root)
				.language
				.map(|l| crate::config::language_code(&l))
		};
		match source {
			Some(Source::Odis) => {
				namers.odis = crate::odisfaults::OdisFaults::open();
				// The keys open VCDS's text, which an ODIS run does not read: the flag is said to
				// do nothing, rather than seem to.
				if given {
					say!("note: --iv-cache opens VCDS fault text, and this run reads the ODIS project; `--source vcds` reads VCDS");
				}
				if let Some(odis) = &namers.odis {
					let available: Vec<String> = odis.languages().into_iter().map(str::to_string).collect();
					let elsewhere = other.is_some() && setting.as_ref().is_some_and(|s| vcds_language().as_ref() == Some(s));
					namers.language_warning = language_warning(setting.as_deref(), Source::Odis, &available, elsewhere);
				}
			}
			Some(Source::Vcds) => {
				namers.vcds = open_vcds(&namers.vcds_root, keys)?;
				if namers.vcds.is_some() {
					let available: Vec<String> = vcds_language().into_iter().collect();
					let elsewhere = other.is_some()
						&& setting
							.as_ref()
							.is_some_and(|s| crate::odisfaults::OdisFaults::project_languages().contains(s));
					namers.language_warning = language_warning(setting.as_deref(), Source::Vcds, &available, elsewhere);
				}
			}
			None => {}
		}
		Ok(namers)
	}

	/// The level-0 lines when `[faults] language` names a language the run's source lacks.
	pub fn language_warning(&self) -> &[String] {
		&self.language_warning
	}

	/// Why the codes are not named, for the `note:` line: what the run's source lacks.
	pub fn none_because(&self) -> &'static str {
		match self.source {
			Some(vag_cli_core::source::Source::Odis) => "the ODIS project has no fault table",
			Some(vag_cli_core::source::Source::Vcds) => "the VCDS installation has no fault text",
			None => "no label data on this machine",
		}
	}

	/// The hint under that line: what reads the run's source's fault text, or the other
	/// source's flag where it is set up and names them.
	pub fn none_hint(&self) -> String {
		use vag_cli_core::source::Source;
		match (self.source, self.other) {
			(Some(_), Some(other)) => format!("use \"{}\" to name them from {}", other.flag(), other.what()),
			(Some(Source::Odis), None) => "use \"vagcan setup <ODIS project>\" to read it again".to_string(),
			(Some(Source::Vcds), None) => "use \"vagcan setup <VCDS installation>\" to read one".to_string(),
			(None, _) => "use \"vagcan setup\" to read it".to_string(),
		}
	}

	/// Whether there is nothing at all to name a code with.
	pub fn is_empty(&self) -> bool {
		self.odis.is_none() && self.vcds.is_none()
	}

	/// What to say, with `-v`, when nothing names the codes: what the run's source lacks, what
	/// sets it up, and — where the other source is set up — the flag that names them from it.
	pub fn empty_note(&self) -> String {
		use vag_cli_core::source::Source;
		let mut lines = match self.source {
			Some(Source::Odis) => vec![
				"codes below are shown as numbers: the ODIS project has no fault table".to_string(),
				"help: read it again: `vagcan setup <ODIS project>`".to_string(),
			],
			Some(Source::Vcds) => vec![
				"codes below are shown as numbers: the VCDS installation has no fault text".to_string(),
				format!("note: looked in {}", self.vcds_root.display()),
				"help: read one: `vagcan setup <VCDS installation>`".to_string(),
			],
			None => return crate::missing::no_fault_labels(&self.vcds_root),
		};
		if let Some(other) = self.other {
			lines.push(format!("help: or name them from {}: `{}`", other.what(), other.flag()));
		}
		lines.join("\n")
	}

	/// The `names` line above a listing: the run's source that names the codes,
	/// `ODIS SK37X (deu)` or `VCDS`.
	pub fn summary(&self) -> String {
		match (self.odis.as_ref(), self.vcds.is_some()) {
			(Some(odis), _) => odis.short(),
			(None, true) => "VCDS".to_string(),
			(None, false) => String::new(),
		}
	}

	/// The lines that say where the names come from and how many there are, for `-v`.
	pub fn describe(&self) -> Vec<String> {
		let mut out = Vec::new();
		if let Some(odis) = &self.odis {
			out.push(odis.describe());
			if let Some(note) = odis.choice_note() {
				out.push(note);
			}
		}
		if let Some(vcds) = &self.vcds {
			out.push(format!(
				"{} rows of fault registry, {} texts, from {}",
				vcds.registry_rows(),
				vcds.codes_texts(),
				self.vcds_root.display(),
			));
			if let Some(note) = vcds.redirect_note() {
				out.push(note.to_string());
			}
		}
		out
	}

	/// Look a unit up in the run's source by what it said about itself.
	pub fn unit(&mut self, odx_name: &str, version: &str) -> UnitNamers {
		UnitNamers {
			odis: self.odis.as_mut().map(|o| o.unit(odx_name, version)),
			vcds: self.vcds.as_mut().map(|n| n.unit(odx_name, version)),
		}
	}

	/// Why a unit's codes may come out as numbers, one line each, for `-v`.
	pub fn notes(&self, unit: &UnitNamers) -> Vec<String> {
		let mut out = Vec::new();
		if self.odis.is_some() && !unit.odis.as_ref().is_some_and(|u| !u.is_empty()) {
			out.push("the ODIS project has no fault table for this unit — codes only".to_string());
		}
		if let Some(note) = unit.vcds.as_ref().and_then(|lookup| crate::faultnames::unit_note(lookup, "codes only")) {
			out.push(note);
		}
		out
	}

	/// The VCDS catalogue a unit's codes would need opened, if it is sealed.
	pub fn sealed(unit: &UnitNamers) -> Option<std::path::PathBuf> {
		match &unit.vcds {
			Some(vag_data_labels::UnitLookup::Locked { file }) => Some(file.clone()),
			// A better variant sealed, another's text standing in: opening it gives the unit's own.
			Some(vag_data_labels::UnitLookup::Found {
				sealed_before: Some(file), ..
			}) => Some(file.clone()),
			_ => None,
		}
	}

	/// What a code is called. `None` when there is nothing to say beside the number.
	pub fn name(&self, unit: &UnitNamers, code: [u8; 3]) -> Option<Named> {
		if let Some((odis, texts)) = self.odis.as_ref().zip(unit.odis.as_ref()) {
			return odis_named(odis.name(texts, code), !texts.is_empty());
		}
		match (&self.vcds, &unit.vcds) {
			(Some(namer), Some(vag_data_labels::UnitLookup::Found { catalogue, .. })) => {
				let named = vcds_named(&namer.name(catalogue, code));
				(named.named || named.note.is_some()).then_some(named)
			}
			_ => None,
		}
	}
}

/// What a run says, at level 0, when `[faults] language` asks for a language the run's source
/// does not have (`available`, ISO 639-2): a `warning:` naming both, and — where the other
/// source has it (`elsewhere`) — the `help:` that reads it. Never the other source's text in
/// its place: a reader who wants it is told what to type.
fn language_warning(setting: Option<&str>, source: vag_cli_core::source::Source, available: &[String], elsewhere: bool) -> Vec<String> {
	let Some(setting) = setting.map(crate::config::language_code) else {
		return Vec::new();
	};
	if available.iter().any(|l| crate::config::language_code(l) == setting) {
		return Vec::new();
	}
	let short = crate::config::short_language(&setting);
	let mut have: Vec<String> = available.iter().map(|l| crate::config::short_language(l)).collect();
	have.sort_unstable();
	have.dedup();
	let have = if have.is_empty() { "none".to_string() } else { have.join(", ") };
	let whose = match source {
		vag_cli_core::source::Source::Odis => "the ODIS project",
		vag_cli_core::source::Source::Vcds => "the VCDS installation",
	};
	let mut lines = vec![format!("warning: no {short} fault text in {whose} (available: {have})")];
	if elsewhere {
		lines.push(format!("help: run with `{}` for {short}", source.other().flag()));
	}
	lines
}

/// The VCDS half of [`Namers`] on a VCDS run, or `None` where the pool holds no fault labels.
///
/// The run's source is the VCDS store's installation ([`crate::source::run_source`] decided
/// so), so the keys are its own, and missing keys beside a full pool are refused: naming the
/// codes as numbers instead would be a silent loss.
fn open_vcds(root: &std::path::Path, keys: &std::path::Path) -> Result<Option<crate::faultnames::Namer>> {
	if !crate::faultnames::has_fault_labels(root) {
		return Ok(None);
	}
	crate::faultnames::Namer::open(root, keys).map(Some)
}

/// The VCDS chain's answer, taken apart: the code VCDS prints, the words, or why there
/// are none.
fn vcds_named(naming: &crate::faultnames::Naming) -> Named {
	use crate::faultnames::Naming;
	match naming {
		Naming::Named { text, sae, failure_type } => Named {
			dtc: match (sae, failure_type) {
				(Some(sae), Some(ftb)) => Some(format!("{sae}{ftb:02X}")),
				(Some(sae), None) => Some(sae.clone()),
				_ => None,
			},
			text: Some(text.replace('\n', " / ")),
			note: None,
			named: true,
			src: VCDS,
			needs_f1a2: false,
			mark: None,
		},
		other => Named {
			note: other.line(),
			..Named::default()
		},
	}
}

/// The ODIS project's answer for one code, taken apart — the whole of what an ODIS run says
/// about it. Separated from [`Namers::name`] so it is testable without a project on disk.
///
/// **A row with a display code and no text** — two of the reference project's 291,346 rows
/// are shaped that way — keeps its code and has no words: VCDS's words for it are the other
/// source's, and an ODIS run does not borrow them. `project_has_unit`: whether the project
/// has a table for the unit at all, so a number not in it is said rather than left blank.
fn odis_named(naming: Option<crate::odisfaults::Naming>, project_has_unit: bool) -> Option<Named> {
	let Some(naming) = naming else {
		// The project has a table for the unit and this number is not in it: said, rather
		// than left blank.
		return project_has_unit.then(|| Named {
			note: Some("not in this unit's fault table in the ODIS project".to_string()),
			..Named::default()
		});
	};
	let text = naming.text.as_deref().map(|t| t.replace('\n', " / "));
	let mut notes: Vec<String> = Vec::new();
	// `LEVEL` in the file's own word. It matched VCDS's fault priority on both codes it
	// was checked against (`research/odis-dtc/README.md` §2), which is evidence for a
	// decoder and not one — so the number is said and the word stays the file's.
	if naming.level > 0 {
		notes.push(format!("level {}", naming.level));
	}
	if let Some(provenance) = naming.provenance() {
		notes.push(provenance);
	}
	let src = if naming.provenance().is_none() { ODIS } else { ODIS_GUESSED };
	// A guessed variant is the text line's mark, at level 0: it is not the car's own answer.
	let mark = (src == ODIS_GUESSED).then_some(NOT_SETTLED);
	let named = text.is_some();
	if !named {
		// A code without words is not a named code, whatever else the row carries.
		notes.push("no text in the project".to_string());
	}
	Some(Named {
		dtc: naming.display_code.clone(),
		text,
		note: (!notes.is_empty()).then(|| notes.join("; ")),
		named,
		src: if named { src } else { "" },
		// Only a family match leaves F1A2 to be recorded; several variants of the
		// car's own F1A2 disagreeing is a fact of the project, not a missing read.
		needs_f1a2: src == ODIS_GUESSED && !naming.confirmed,
		mark: mark.filter(|_| named),
	})
}
/// The request ids a `--unit` list names, checked before the adapter is opened:
/// the cable is a single-user resource, and holding it open to fail on a typo
/// blocks the next attempt.
fn unit_list(flag: &str, spec: &str) -> Result<Vec<u16>> {
	Ok(
		vag_uds_client::address::parse_list(spec)
			.map_err(|e| anyhow::anyhow!("{flag}: {e}"))?
			.iter()
			.map(|u| u.request)
			.collect(),
	)
}

/// The caveat that used to head every listing, said at `-v` (and in `faults --help`):
/// a stored code is a record, not a diagnosis.
pub const STORED_IS_NOT_FAILING: &str =
	"a stored code is a record that something happened once — not a diagnosis, and not necessarily a fault now; `failing` is failing now";

/// One stored code and what is known about it, as `faults` prints it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row {
	/// The unit's short number, `--` when it has none.
	pub unit: String,
	pub request: u16,
	/// What the unit called itself (`F197`).
	pub name: String,
	/// The unit's part number (`F187`), when it gave one.
	pub part: String,
	pub code: [u8; 3],
	pub dtc: String,
	pub status: u8,
	/// From the first extended-data record that reads as a fault context.
	pub context: Option<FaultContext>,
	pub text: String,
	/// Whose words `text` is: [`ODIS`], [`ODIS_GUESSED`], [`VCDS`], or empty.
	pub src: &'static str,
	/// The freeze frame stored with the code ([`crate::freezeframe::lines`]); empty when
	/// the unit gave none.
	pub freeze: Vec<crate::freezeframe::Line>,
	/// What the text line says after the text when its words are borrowed or guessed:
	/// [`Named::mark`].
	pub mark: Option<&'static str>,
	/// `--details`: the extended-data records as they came, `(record, bytes)`.
	pub records: Vec<(u8, Vec<u8>)>,
}

/// The width a listing wraps its text to when the terminal's is not known.
pub const WIDTH: usize = 80;

/// How far a code's lines are indented under its unit's heading.
const CODE_INDENT: usize = 2;

/// The listing, as `faults` prints it on stdout under its summary: one block per unit that
/// has codes, each code a line of its own under the unit — its mark, [`marks::FAIL`] or
/// [`marks::STORED`], the code and its text — with what is known about it indented
/// beneath: rustc's shape. `width`: the terminal's, when known; [`WIDTH`] otherwise.
///
/// ```text
/// 70C  Lenks.Modul  5Q0953521KM
///   ✗ B145501  Unterbrechung des Signalpfades "Init"
///              failing · 254 times · 214592 km · last 2026-07-27 00:00
/// ```
///
/// A text too long for its line wraps under itself, never cut. Nothing is aligned across
/// units, so no width is guessed at.
pub fn render_units(rows: &[Row], width: Option<usize>) -> String {
	let width = width.unwrap_or(WIDTH);
	let mut blocks: Vec<String> = Vec::new();
	let mut at = 0;
	while at < rows.len() {
		let unit = &rows[at];
		let mut block = heading(unit);
		while at < rows.len() && rows[at].request == unit.request {
			for line in code_lines(&rows[at], width) {
				block.push('\n');
				block.push_str(&line);
			}
			at += 1;
		}
		block.push('\n');
		blocks.push(block);
	}
	blocks.join("\n")
}

/// A unit's heading: its request id — how `--unit` names it too — what it calls itself,
/// and its part number.
fn heading(row: &Row) -> String {
	let mut line = format!("{:03X}", row.request);
	for said in [&row.name, &row.part].into_iter().filter(|said| !said.is_empty()) {
		line.push_str("  ");
		line.push_str(said);
	}
	line
}

/// One code's lines: the code and its text, wrapped under itself, then indented to the
/// text what is known about it and the freeze frame stored with it, then — with `--details` —
/// its records as they came.
fn code_lines(row: &Row, width: usize) -> Vec<String> {
	let code = match row.dtc.is_empty() {
		true => code_hex(row.code),
		false => row.dtc.clone(),
	};
	let mark = match row.status {
		s if s & FAILED_NOW != 0 => marks::FAIL,
		s if s & CONFIRMED != 0 => marks::STORED,
		_ => marks::UNCONFIRMED,
	};
	// Seven wide, an SAE code's width, so a bare number's facts line up with the rest.
	let lead = format!("{}{mark} {code:<7}", " ".repeat(CODE_INDENT));
	let indent = " ".repeat(lead.chars().count() + 2);
	let room = width.saturating_sub(indent.len()).max(20);
	let mut text = row.text.clone();
	if let Some(mark) = row.mark {
		text = match text.is_empty() {
			true => mark.to_string(),
			false => format!("{text} {mark}"),
		};
	}
	let mut wrapped = vag_cli_core::table::wrap(&text, room).into_iter();
	let mut lines = vec![match wrapped.next() {
		Some(first) => format!("{lead}  {first}"),
		None => lead.trim_end().to_string(),
	}];
	lines.extend(wrapped.map(|line| format!("{indent}{line}")));
	lines.push(format!("{indent}{}", facts(row)));
	// The freeze frame under the facts, in its one rendering; `--details`' records after it,
	// in the same key column.
	let mut block = row.freeze.clone();
	block.extend(row.records.iter().map(|(record, bytes)| crate::freezeframe::Line {
		key: format!("record {record:02X}"),
		value: crate::render::hex_spaced(bytes),
	}));
	lines.extend(crate::freezeframe::render(&block, &indent, Some(width)).lines().map(str::to_string));
	lines
}

/// What is known about a code, in one line: its state, how often, the odometer, and when
/// by the car's own clock, to the minute.
fn facts(row: &Row) -> String {
	let mut parts = vec![match short_status(row.status).as_str() {
		"confirmed" => "stored".to_string(),
		word @ ("failing" | "pending") => word.to_string(),
		// None of the three: not a fault yet; `-v` decodes the byte.
		_ => "not confirmed".to_string(),
	}];
	if let Some(c) = &row.context {
		parts.push(match c.occurrences {
			0xFF => "255+ times".to_string(),
			1 => "1 time".to_string(),
			n => format!("{n} times"),
		});
		parts.push(format!("{} km", c.mileage_km));
		let when = describe_when(c);
		if when.len() == 19 {
			parts.push(format!("last {}", &when[..16]));
		}
	}
	parts.join(marks::SEP)
}

/// `--supported`: one block per unit, each code the unit supports a line under it with its
/// decimal number.
pub fn render_supported(units: &[(Row, Vec<[u8; 3]>)]) -> String {
	units
		.iter()
		.filter(|(_, codes)| !codes.is_empty())
		.map(|(unit, codes)| {
			let mut block = heading(unit);
			for code in codes {
				block.push_str(&format!("\n{}{}  {}", " ".repeat(CODE_INDENT), code_hex(*code), fault_number(*code)));
			}
			block.push('\n');
			block
		})
		.collect::<Vec<_>>()
		.join("\n")
}

/// The note under a listing with guessed codes (`src` [`ODIS_GUESSED`]), on stderr: which
/// units, what F1A2 did — the identifier that settles it — and, where it was not read, a
/// `help:` line to read it. `by_family`: the units whose variant was picked by family, and
/// whether each answered F1A2. `None` when every code's source is settled.
pub fn guessed_line(rows: &[Row], by_family: &[(u16, bool)]) -> Option<String> {
	let mut guessed: Vec<u16> = rows.iter().filter(|r| r.src == ODIS_GUESSED).map(|r| r.request).collect();
	guessed.dedup();
	if guessed.is_empty() {
		return None;
	}
	let ids = |ids: &[u16]| ids.iter().map(|r| format!("{r:03X}")).collect::<Vec<_>>().join(", ");
	let silent: Vec<u16> = by_family.iter().filter(|(_, read)| !read).map(|(r, _)| *r).collect();
	let unmatched: Vec<u16> = by_family.iter().filter(|(_, read)| *read).map(|(r, _)| *r).collect();
	let why = match (silent.is_empty(), unmatched.is_empty()) {
		(false, _) => " (F1A2 not read)",
		(true, false) => " (F1A2 matches no variant in the project)",
		(true, true) => "",
	};
	let mut out = format!("note: the ODIS variant of {} is a guess{why}", ids(&guessed));
	if !silent.is_empty() {
		out.push_str("\nhelp: run `vagcan faults` again with the ignition on");
	}
	Some(out)
}

/// The summary line over the listing: how many codes, on how many of the units read, and
/// how many of them fail now.
pub fn summary(stored: usize, failing: usize, with_codes: usize, units: usize, all: bool) -> String {
	let what = if all { "listed" } else { "stored" };
	match (stored, failing) {
		(0, _) => format!("no {what} codes on the {units} {} read", crate::render::plural(units, "unit")),
		(_, 0) => format!(
			"{stored} {what} {} on {with_codes} of {units} {}",
			crate::render::plural(stored, "code"),
			crate::render::plural(units, "unit")
		),
		_ => format!(
			"{stored} {what} {} on {with_codes} of {units} {}{}{failing} failing now",
			crate::render::plural(stored, "code"),
			crate::render::plural(units, "unit"),
			marks::SEP
		),
	}
}

/// Read faults from the car (see the module docs).
///
/// `open` takes the link to the car, and is called only once every argument
/// has been checked.
#[allow(clippy::too_many_arguments)]
pub async fn run<L: UnitLink>(
	open: impl AsyncFnOnce() -> Result<L>,
	only: Option<&str>,
	details: bool,
	all_codes: bool,
	supported: bool,
	extended: bool,
	iv_cache: Option<&str>,
) -> Result<()> {
	// Arguments first: the adapter is a single-user resource, so a typo in
	// --unit must not cost the port before it is reported.
	// The fault texts are opened before the port for the same reason: a missing
	// `Codes.dat` is a mistake to report, not one to make after taking the
	// adapter and reading the whole car.
	//
	// The names come from what `vagcan setup` wrote for the run's one source — the ODIS
	// project's cache, or the shared pool of VCDS files — and from nowhere else. None
	// being there is the ordinary "setup has not run yet" case: the codes are still
	// read and shown as numbers, with a note that names `vagcan setup`.
	// The keys are the VCDS store's (`rod-keys.json`) unless `--iv-cache` names a file.
	// The numbers `vagcan units` lists, for `--unit` and for the headings below.
	crate::labels::install_recorded_units();
	// `--unit` before the project is opened: opening it can upgrade its cache, and a
	// typo is refused touching nothing.
	let requested = only.map(|spec| unit_list("--unit", spec)).transpose()?;
	// Nothing set up at all is the shortage `rescue` offers `setup` for, as before a store
	// was read: `stores` says it.
	let keys = crate::datadir::or_default(iv_cache, || Ok(crate::project::stores()?.vcds.rod_keys()))?;
	// The run's one source (`Extracted::run_source`): its fault text, and the freeze-frame
	// layouts `setup` stored from it, looked up per unit by the same identifiers that name its
	// codes — and on a VCDS run by the car's project, which its VIN says.
	let extracted = crate::extracted::current();
	let mut namers = Namers::open(extracted.run_source(), &keys, iv_cache.is_some())?;
	let freeze_source = extracted.run_source().filter(|_| extracted.knows_freeze_frames());
	let freeze_known = freeze_source.is_some();

	let mut backend = open().await?;
	// One read, and only where a layout could be looked up by it: the project that picks the
	// unit's own variant out of several (`EV_GatewNF_SK37` for an SK37 car).
	let vin = match freeze_known {
		true => {
			let (link, vin) = crate::units::read_vin(backend).await;
			backend = link;
			vin
		}
		false => None,
	};
	let extracted = extracted.for_car(vin.as_deref());

	if extended {
		// An extended session is workshop mode; see `crate::safety`.
		backend = match crate::safety::require_stationary(backend).await {
			Ok(backend) => backend,
			Err((_, why)) => {
				return Err(
					anyhow::anyhow!("`--extended` refused: {why}")
						.note("an extended diagnostic session can make a control unit stop assisting the driver")
						.help("stop the car and run `vagcan faults --extended` again"),
				);
			}
		};
	}

	let order = match requested {
		Some(ids) => ids,
		None => {
			let gw = UnitAddress::from_request(gateway::GATEWAY).expect("the gateway is in VW's block");
			let mut uds = AsyncUdsClient::new(backend.to_unit(CanId::Standard(gw.request), CanId::Standard(gw.response)));
			let listed = match uds.read_data_by_identifier(gateway::INSTALLATION_LIST).await {
				Ok(bitmap) => gateway::decode_installation_list(&bitmap),
				Err(e) => {
					say!("the gateway did not list the car's units ({e}): only the engine, the gearbox and the gateway are read");
					Vec::new()
				}
			};
			backend = L::release(uds.into_transport());
			// The engine, the gearbox and the gateway itself are never in the
			// list; the order is the one the board's fault count walks too.
			gateway::walk_order(&listed)
		}
	};

	// Where the names come from: one line every run, the counts at -v. Where nothing
	// opened, the data has not been set up yet — the codes read fine, and the line says
	// how to name them.
	if !supported {
		match namers.is_empty() {
			true => {
				say!("note: the codes are not named: {}", namers.none_because());
				say!("  ({})", namers.none_hint());
				note!("{}", namers.empty_note());
			}
			false => {
				say!("note: code names from {}", namers.summary());
				for line in namers.language_warning() {
					say!("{line}");
				}
				for line in namers.describe() {
					note!("{line}");
				}
			}
		}
		note!("{STORED_IS_NOT_FAILING}");
	}

	let mut rows: Vec<Row> = Vec::new();
	let mut supported_units: Vec<(Row, Vec<[u8; 3]>)> = Vec::new();
	let mut supported_total = 0usize;
	let mut sealed: Vec<std::path::PathBuf> = Vec::new();
	// Units with a row whose ODIS variant was picked by family, and whether they answered F1A2.
	let mut guessed_by_family: Vec<(u16, bool)> = Vec::new();
	let mut read = 0usize;
	let count = order.len();
	let mut progress = crate::progress::Line::new();
	for (at, request) in order.into_iter().enumerate() {
		progress.update(&format!("reading faults — {request:03X}, unit {} of {count}", at + 1));
		let Some(address) = UnitAddress::from_request(request) else {
			note!("{request:03X}  skipped: no diagnostic address (700-795 or 7E0-7E7)");
			continue;
		};
		let mut uds = AsyncUdsClient::new(backend.to_unit(CanId::Standard(address.request), CanId::Standard(address.response)));
		// See `crate::safety`: an extended session is workshop mode, and this
		// command reads faults perfectly well without one.
		if extended {
			let _ = uds.start_session(0x03).await;
		}

		// The unit names itself; nothing here maps an address to a name.
		let component = uds
			.read_data_by_identifier(0xF197)
			.await
			.ok()
			.map(|b| String::from_utf8_lossy(&b).trim_end_matches(['\0', ' ']).to_string())
			.filter(|s| !s.is_empty());
		let listing = uds.read_dtcs_by_status_mask(0xFF).await;
		// A list that did not come back is not a clean unit: said at level 0, and the unit
		// is not counted as read in the summary.
		match &listing {
			Ok(_) => read += 1,
			Err(e) => say!("warning: {request:03X} fault list not read: {e}"),
		}
		let unit = UnitFaults {
			component,
			all: listing.unwrap_or_default(),
		};
		// The unit's own "now": the same odometer and counter its faults are
		// stamped with, so ages are differences rather than dates.
		let now = uds
			.read_data_by_identifier(UnitStamp::DID)
			.await
			.ok()
			.and_then(|data| UnitStamp::parse(&data));
		// A unit with no short number shows a dash rather than repeating
		// its id, which reads as a rendering fault.
		let number = crate::car::unit_number(vag_uds_client::address::short_number(request));
		let name = unit.component.clone().unwrap_or_default();

		if supported {
			// The unit's whole catalogue of codes, in its own order — which is
			// how the label files store fault names, so the two lists are
			// worth comparing.
			match uds.read_supported_dtcs().await {
				Ok(list) => {
					supported_total += list.len();
					let unit = Row {
						unit: number.clone(),
						request,
						name: name.clone(),
						..Row::default()
					};
					supported_units.push((unit, list.iter().map(|dtc| dtc.code).collect()));
				}
				Err(e) => note!("{request:03X}  no supported list ({e})"),
			}
			backend = L::release(uds.into_transport());
			continue;
		}

		let mut show: Vec<RawDtc> = if all_codes {
			unit.all.clone()
		} else {
			unit.confirmed().into_iter().cloned().collect()
		};
		note!(
			"{request:03X}  {} {} listed, {} confirmed",
			unit.all.len(),
			crate::render::plural(unit.all.len(), "code"),
			unit.confirmed().len()
		);
		// Something failing right now outranks a code stored months ago.
		show.sort_by_key(|d| (d.status & FAILED_NOW == 0, d.code));
		if !show.is_empty() {
			// The unit names its own variant and description file, so they are
			// read only once something has to be named out of them.
			// What the unit answered for F1A2, for the line a guessed variant gets.
			let mut f1a2 = String::new();
			// Read when something can be looked up by them: the names, or the freeze-frame
			// layouts.
			let identity = match namers.is_empty() && !freeze_known {
				true => None,
				false => {
					let read = |data: Option<Vec<u8>>| data.map(|b| ident_text(&b)).unwrap_or_default();
					let odx = read(uds.read_data_by_identifier(ODX_NAME).await.ok());
					let version = read(uds.read_data_by_identifier(ODX_VERSION).await.ok());
					vag_cli_core::wire!("   {request:03X}  names looked up by F19E {odx:?} F1A2 {version:?}");
					f1a2 = version.clone();
					Some((odx, version))
				}
			};
			let lookup = identity
				.as_ref()
				.filter(|_| !namers.is_empty())
				.map(|(odx, version)| namers.unit(odx, version));
			// The unit's own variant's layout, or none: another variant's fields are not this
			// unit's, and its bytes stay bytes.
			let found = match (identity.as_ref(), freeze_source) {
				(Some((odx, version)), Some(source)) => Some((source, extracted.freeze_frames(Some(odx), Some(version)))),
				_ => None,
			};
			let layout = match found {
				Some((source, crate::extracted::FreezeLayout::Unit { variant, fields })) => {
					note!("{request:03X}  freeze frames described by {} {variant}", source.possessive());
					Some(crate::freezeframe::Layout::new(fields, source))
				}
				Some((source, crate::extracted::FreezeLayout::Another { variant })) => {
					note!(
						"{request:03X}  freeze frames: no layout for this variant ({} {variant} is another variant)",
						source.possessive()
					);
					None
				}
				_ => None,
			};
			// The part number, for the unit's heading.
			let part = uds.read_data_by_identifier(0xF187).await.ok().map(|b| ident_text(&b)).unwrap_or_default();
			if let Some(file) = lookup.as_ref().and_then(Namers::sealed)
				&& !sealed.contains(&file)
			{
				sealed.push(file);
			}
			for line in lookup.as_ref().map(|l| namers.notes(l)).unwrap_or_default() {
				note!("{request:03X}  {line}");
			}
			for dtc in &show {
				let named = lookup.as_ref().and_then(|l| namers.name(l, dtc.code)).unwrap_or_default();
				if let Some(why) = &named.note {
					note!("{request:03X}  {}  {why}", code_hex(dtc.code));
				}
				// Extended data carries when it happened: the odometer at the
				// time and how often. Read for every fault, since that is the
				// question a stored code raises.
				// The same answer holds the freeze frame: the unit's own records after the two
				// standard ones (`crate::freezeframe`). No read beyond this one.
				let records = uds.read_dtc_extended(dtc.code).await.unwrap_or_default();
				let mut context = None;
				let mut freeze = Vec::new();
				for record in &records {
					let frame = crate::freezeframe::split(record, layout.as_ref(), now.as_ref());
					if freeze.is_empty() {
						freeze = crate::freezeframe::lines(&frame, layout.as_ref(), "no layout for this variant");
					}
					let parsed = frame.context;
					if let Some(parsed) = &parsed {
						note!(
							"{request:03X}  {} ({})  {}{}{}",
							code_hex(dtc.code),
							fault_number(dtc.code),
							describe_status(dtc.status),
							marks::SEP,
							describe_age(parsed, now.as_ref())
						);
					}
					context = context.or(parsed);
				}
				rows.push(Row {
					unit: number.clone(),
					request,
					name: name.clone(),
					part: part.clone(),
					code: dtc.code,
					dtc: named.dtc.unwrap_or_default(),
					status: dtc.status,
					context,
					text: named.text.unwrap_or_default(),
					src: named.src,
					freeze,
					mark: named.mark,
					records: match details {
						true => records.iter().map(|r| (r.record, r.data.clone())).collect(),
						false => Vec::new(),
					},
				});
				if named.needs_f1a2 && !guessed_by_family.iter().any(|(r, _)| *r == request) {
					guessed_by_family.push((request, !f1a2.is_empty()));
				}
			}
		}
		backend = L::release(uds.into_transport());
	}
	// Off the screen before anything else is written, and for good: a spinner left
	// ticking draws over the summary and over the question below it.
	drop(progress);
	// Every read is done: the link closes now, before the question below and the key
	// search after it, and a `--slcan` board is out of adapter mode by then.
	drop(backend);

	let width = vag_cli_core::table::terminal_width();
	if supported {
		let with_codes = supported_units.iter().filter(|(_, codes)| !codes.is_empty()).count();
		out!(
			"{supported_total} {} supported on {with_codes} of {read} {}\n",
			crate::render::plural(supported_total, "code"),
			crate::render::plural(read, "unit")
		);
		if supported_total > 0 {
			out!("\n{}", render_supported(&supported_units));
		}
		return Ok(());
	}
	let failing = rows.iter().filter(|r| r.status & FAILED_NOW != 0).count();
	let mut with_codes: Vec<u16> = rows.iter().map(|r| r.request).collect();
	with_codes.dedup();
	out!("{}\n", summary(rows.len(), failing, with_codes.len(), read, all_codes));
	if !rows.is_empty() {
		out!("\n{}", render_units(&rows, width));
	}
	// A guessed variant is marked on its text line; what settles it, once, under the summary.
	if let Some(line) = guessed_line(&rows, &guessed_by_family) {
		say!("{line}");
	}
	crate::faultnames::offer_to_unseal(&sealed, &keys)?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn names_come_from_the_one_pool_setup_filled() {
		// The whole point of the copy: after `vagcan setup`, `faults` names codes
		// with no flag, because the raw files live under ~/.vagcan. The pool is
		// shared across projects — a fault registry is a property of a VCDS
		// build, not of one car — while the keys that open it are per project,
		// because a key is a property of one file's bytes (design §4.2).
		assert_eq!(crate::datadir::rod_pool_dir().unwrap(), crate::datadir::vagcan_dir().unwrap().join("rod"));
	}

	#[test]
	fn a_vcds_run_refuses_missing_keys_beside_a_filled_pool_and_an_empty_pool_is_no_text() {
		// On a VCDS run the text is the run's own source's: keys missing beside a full pool
		// are refused, since numbers instead would be a silent loss — and an empty pool is
		// no VCDS text, not an error.
		let base = std::env::temp_dir().join(format!("vagcan-vcds-keys-{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&base);
		std::fs::create_dir_all(base.join("UDS_EV")).unwrap();
		std::fs::write(base.join("UDS_EV").join(vag_data_labels::dtc::REGISTRY_FILE), b"x").unwrap();
		std::fs::write(base.join(crate::setup::CODES_FILES[0]), b"x").unwrap();
		let keys = base.join("project").join("rod-keys.json");
		assert!(crate::faultnames::has_fault_labels(&base), "the fixture is a filled pool");
		let refused = open_vcds(&base, &keys).err().expect("keys missing");
		let refused = vag_cli_core::report::render(&refused, 0, vag_cli_core::report::Colour::Never);
		assert!(refused.contains("rod-keys.json"), "{refused}");
		assert!(open_vcds(&base.join("empty"), &keys).unwrap().is_none());
		let _ = std::fs::remove_dir_all(&base);
	}

	#[test]
	fn an_odis_run_never_opens_the_vcds_text_and_a_vcds_run_never_the_projects() {
		// One source per run: the namers hold the run's source's half and nothing of the
		// other, whatever is on disk. The test root's pool and project hold nothing, so the
		// halves are empty — what matters is which ones can be.
		let keys = std::env::temp_dir().join(format!("vagcan-no-keys-{}", std::process::id()));
		let odis = Namers::open(Some(vag_cli_core::source::Source::Odis), &keys, false).unwrap();
		assert!(odis.vcds.is_none(), "an ODIS run opened VCDS's fault text");
		let vcds = Namers::open(Some(vag_cli_core::source::Source::Vcds), &keys, false).unwrap();
		assert!(vcds.odis.is_none(), "a VCDS run opened the ODIS project's fault tables");
		let none = Namers::open(None, &keys, false).unwrap();
		assert!(none.is_empty());
		// `--iv-cache` naming a file that is not there is a typo, said as one on any run.
		for source in [Some(vag_cli_core::source::Source::Odis), Some(vag_cli_core::source::Source::Vcds), None] {
			let said = Namers::open(source, &keys, true)
				.err()
				.expect("an --iv-cache that is not there")
				.to_string();
			assert!(said.starts_with("--iv-cache"), "{said}");
		}
	}

	#[test]
	fn a_language_the_runs_source_lacks_is_a_warning_with_the_other_source_where_it_has_it() {
		use vag_cli_core::source::Source;
		let eng = ["eng".to_string()];
		let deu = ["deu".to_string()];
		assert!(language_warning(None, Source::Vcds, &eng, true).is_empty(), "unset: nothing to say");
		assert!(
			language_warning(Some("en"), Source::Vcds, &eng, true).is_empty(),
			"honoured, in either spelling"
		);
		// The owner's machine: `language = "en"`, a German ODIS project, an English VCDS build.
		assert_eq!(
			language_warning(Some("en"), Source::Odis, &deu, true),
			[
				"warning: no en fault text in the ODIS project (available: de)",
				"help: run with `--source vcds` for en"
			]
		);
		// The other source does not have it either, or is not set up: the warning alone.
		assert_eq!(
			language_warning(Some("de"), Source::Vcds, &eng, false),
			["warning: no de fault text in the VCDS installation (available: en)"]
		);
		assert_eq!(
			language_warning(Some("de"), Source::Vcds, &[], false),
			["warning: no de fault text in the VCDS installation (available: none)"]
		);
	}

	/// An ODIS row as the reference project's two textless ones are shaped: a
	/// display code, and nothing to read.
	fn textless_odis_row() -> crate::odisfaults::Naming {
		crate::odisfaults::Naming {
			display_code: Some("B1168F2".into()),
			text: None,
			language: Some("deu".into()),
			variant: "EV_Brake_035".into(),
			family: "EV_Brake".into(),
			matched: 1,
			confirmed: true,
			disagrees: false,
			level: 0,
		}
	}

	#[test]
	fn an_odis_row_with_no_text_keeps_its_code_and_borrows_no_words() {
		// Two rows of the reference project's 291,346 carry a display code and no text.
		// An ODIS run shows the code with no words: VCDS's words are the other source's.
		let named = odis_named(Some(textless_odis_row()), true).expect("a code with a display code says something");
		assert_eq!(
			named,
			Named {
				dtc: Some("B1168F2".into()),
				text: None,
				note: Some("no text in the project".into()),
				named: false,
				src: "",
				needs_f1a2: false,
				mark: None,
			}
		);
	}

	#[test]
	fn an_odis_rows_own_words_need_no_marker() {
		let german = crate::odisfaults::Naming {
			text: Some("Lenkwinkelsensor".into()),
			..textless_odis_row()
		};
		let named = odis_named(Some(german), true).unwrap();
		assert_eq!((named.text.as_deref(), named.note, named.src), (Some("Lenkwinkelsensor"), None, ODIS));
		// A number the project's table for the unit does not list is said, not left blank.
		let missing = odis_named(None, true).unwrap();
		assert_eq!(missing.note.as_deref(), Some("not in this unit's fault table in the ODIS project"));
		assert_eq!(odis_named(None, false), None, "no table for the unit: nothing to say beside the number");
	}

	#[test]
	fn a_row_whose_variant_is_not_settled_is_marked_in_the_table_itself() {
		// `odisfaults::Naming::provenance`: the line never presents a guessed variant as
		// settled. The guess is the text line's mark, at level 0.
		let text = |naming: crate::odisfaults::Naming| crate::odisfaults::Naming {
			text: Some("Lenkwinkelsensor".into()),
			..naming
		};
		let settled = odis_named(Some(text(textless_odis_row())), true).unwrap();
		assert_eq!((settled.src, settled.needs_f1a2, settled.mark), (ODIS, false, None));
		let by_family = crate::odisfaults::Naming {
			confirmed: false,
			..text(textless_odis_row())
		};
		let guessed = odis_named(Some(by_family), true).unwrap();
		assert_eq!((guessed.src, guessed.needs_f1a2, guessed.mark), (ODIS_GUESSED, true, Some(NOT_SETTLED)));
		let of_two = crate::odisfaults::Naming {
			matched: 2,
			disagrees: true,
			..text(textless_odis_row())
		};
		let guessed = odis_named(Some(of_two), true).unwrap();
		assert_eq!((guessed.src, guessed.needs_f1a2), (ODIS_GUESSED, false), "the car's own F1A2 picked two");
	}

	#[test]
	fn a_vcds_name_is_the_sae_code_and_the_words_and_a_reason_is_a_note() {
		let named = vcds_named(&crate::faultnames::Naming::Named {
			text: "Line 1\nline 2".into(),
			sae: Some("U1110".into()),
			failure_type: Some(0x00),
		});
		assert_eq!(named.dtc.as_deref(), Some("U111000"));
		assert_eq!(named.text.as_deref(), Some("Line 1 / line 2"), "one row on a screen");
		assert_eq!(named.src, VCDS);
		let unlisted = vcds_named(&crate::faultnames::Naming::NotListed);
		assert_eq!((unlisted.text, unlisted.named), (None, false));
		assert!(unlisted.note.unwrap().contains("not in this unit's fault catalogue"));
	}

	#[test]
	fn only_confirmed_codes_count_as_faults() {
		let unit = UnitFaults {
			all: vec![
				RawDtc {
					code: [0x00, 0x01, 0x07],
					status: 0x10,
				},
				RawDtc {
					code: [0x06, 0x09, 0x01],
					status: 0x08,
				},
			],
			..Default::default()
		};
		let confirmed = unit.confirmed();
		assert_eq!(confirmed.len(), 1);
		assert_eq!(confirmed[0].code, [0x06, 0x09, 0x01]);
	}

	#[test]
	fn the_status_byte_is_read_bit_by_bit_from_the_standard() {
		assert_eq!(describe_status(0x08), "confirmed");
		assert_eq!(describe_status(0x10), "not tested since clear");
		assert_eq!(describe_status(0x09), "failed now, confirmed");
		// Nothing asserted: report the byte rather than an empty line.
		assert_eq!(describe_status(0x00), "00");
		// One word in the table, the most telling one.
		assert_eq!(short_status(0x09), "failing");
		assert_eq!(short_status(0x2C), "confirmed");
		assert_eq!(short_status(0x04), "pending");
		assert_eq!(short_status(0x50), "50");
	}

	#[test]
	fn a_fault_is_dated_by_the_cars_own_counters_not_by_a_calendar() {
		// Real record from the body control module, against the stamp read
		// from the same unit: 9 occurrences, 42 km and 17.9 hours ago.
		let context = FaultContext {
			priority: 6,
			occurrences: 9,
			aging_counter: 0xB8,
			mileage_km: 212_763,
			clock: 0x69F9_044B,
		};
		let now = UnitStamp {
			mileage_km: 212_805,
			clock: 0x69FA_005C,
		};
		let text = describe_age(&context, Some(&now));
		assert!(text.contains("212763 km"), "{text}");
		assert!(text.contains("9×"), "{text}");
		// The two stamps are 2026-07-28 00:18:19 and 2026-07-28 08:01:32 by
		// the car's clock — seven and three quarter hours apart. Reading the
		// stamp as a counter, as this once did, gave 17.9 h.
		assert!(text.contains("7.7 h ago"), "{text}");
		assert!(text.contains("42 km ago"), "{text}");

		// With nothing to compare against, only what the record itself says.
		let alone = describe_age(&context, None);
		assert!(alone.contains("212763 km"));
		assert!(!alone.contains("ago"), "{alone}");
	}

	#[test]
	fn a_fault_is_stamped_with_the_cars_own_date_and_time() {
		// Both anchors come from VCDS printouts of this car: one unit's fault
		// at 2026.07.27 00:00:03 and another's at 2026.07.28 23:50:02.
		// Six fields, two exact matches.
		let at = |clock| {
			describe_when(&FaultContext {
				priority: 1,
				occurrences: 1,
				aging_counter: 0,
				mileage_km: 1,
				clock,
			})
		};
		assert_eq!(at(0x69F6_0003), "2026-07-27 00:00:03");
		assert_eq!(at(0x69F9_7C82), "2026-07-28 23:50:02");
		// An unset stamp is reported as one, not as the year 2000.
		assert_eq!(at(0x0000_0000), "—");
	}

	#[test]
	fn a_saturated_occurrence_count_is_not_reported_as_exactly_255() {
		let context = FaultContext {
			priority: 6,
			occurrences: 0xFF,
			aging_counter: 0xB9,
			mileage_km: 212_795,
			clock: 0x69F9_68D9,
		};
		assert!(describe_age(&context, None).contains("255+ times"));
		let row = Row {
			context: Some(context),
			..Row::default()
		};
		assert!(render_units(&[row], None).contains(" 255+ times "), "the listing's facts line too");
	}

	#[test]
	fn all_three_bytes_are_one_fault_number() {
		// Checked against this car's own VCDS scan, which prints these decimal
		// numbers for these units: reading the first two bytes as the number
		// would give 1, 1137 and 260 instead.
		assert_eq!((code_hex([0x00, 0x01, 0x29]), fault_number([0x00, 0x01, 0x29])), ("000129".into(), 297));
		assert_eq!(
			(code_hex([0x04, 0x71, 0x20]), fault_number([0x04, 0x71, 0x20])),
			("047120".into(), 291_104)
		);
		assert_eq!(
			(code_hex([0x01, 0x04, 0x05]), fault_number([0x01, 0x04, 0x05])),
			("010405".into(), 66_565)
		);
	}

	#[test]
	fn the_listing_is_a_block_per_unit_at_eighty_columns() {
		// Invented units and texts. rustc's shape: the unit, each code with its text, and
		// what the unit recorded indented under the text. Nothing aligned across units.
		let context = |km, n, clock| {
			Some(FaultContext {
				priority: 1,
				occurrences: n,
				aging_counter: 0,
				mileage_km: km,
				clock,
			})
		};
		let rows = [
			Row {
				unit: "16".into(),
				request: 0x70C,
				name: "Lenks.Modul".into(),
				part: "5Q0953521KM".into(),
				code: [0x04, 0x71, 0x20],
				dtc: "B145501".into(),
				status: 0x08,
				context: context(214_592, 254, 0x69F6_0003),
				text: "Unterbrechung des Signalpfades \"Init\"".into(),
				src: ODIS,
				..Row::default()
			},
			Row {
				unit: "17".into(),
				request: 0x714,
				name: "KOMBI".into(),
				code: [0xFF, 0xFF, 0x3C],
				dtc: "U111000".into(),
				status: 0x09,
				context: context(214_634, 13, 0x69F9_7C82),
				text: "BAP_TEL_0x28 keine Kommunikation".into(),
				src: ODIS_GUESSED,
				mark: Some(NOT_SETTLED),
				..Row::default()
			},
			Row {
				unit: "17".into(),
				request: 0x714,
				name: "KOMBI".into(),
				code: [0x00, 0x01, 0x29],
				status: 0x08,
				..Row::default()
			},
		];
		let listing = render_units(&rows, None);
		assert_eq!(
			listing,
			"70C  Lenks.Modul  5Q0953521KM\n\
			 \x20 · B145501  Unterbrechung des Signalpfades \"Init\"\n\
			 \x20            stored · 254 times · 214592 km · last 2026-07-27 00:00\n\
			 \n\
			 714  KOMBI\n\
			 \x20 ✗ U111000  BAP_TEL_0x28 keine Kommunikation (variant not settled)\n\
			 \x20            failing · 13 times · 214634 km · last 2026-07-28 23:50\n\
			 \x20 · 000129\n\
			 \x20            stored\n"
		);
		assert_eq!(render_units(&rows, Some(80)), listing, "eighty columns is the shape itself");
		assert!(listing.lines().all(|l| l.chars().count() <= 80), "{listing}");
		assert!(!listing.contains('\u{2022}'), "never a bullet: {listing}");
		// A guessed variant is said under the listing, with what F1A2 did.
		assert_eq!(
			guessed_line(&rows, &[(0x714, false)]).as_deref(),
			Some("note: the ODIS variant of 714 is a guess (F1A2 not read)\nhelp: run `vagcan faults` again with the ignition on")
		);
		assert_eq!(
			guessed_line(&rows, &[]).as_deref(),
			Some("note: the ODIS variant of 714 is a guess"),
			"several variants of the car's own F1A2: nothing to read again"
		);
		assert_eq!(guessed_line(&rows[..1], &[]), None, "every code settled: no line");
		assert_eq!(summary(3, 1, 2, 18, false), "3 stored codes on 2 of 18 units · 1 failing now");
		assert_eq!(summary(1, 0, 1, 1, true), "1 listed code on 1 of 1 unit", "nothing failing goes unsaid");
		assert_eq!(summary(0, 0, 0, 15, false), "no stored codes on the 15 units read");
	}

	#[test]
	fn a_long_text_wraps_under_itself_and_is_never_cut() {
		let row = Row {
			request: 0x713,
			name: "ESC".into(),
			code: [0x00, 0x01, 0x29],
			dtc: "B1168F2".into(),
			status: 0x08,
			text: "a fault text long enough that one line of an eighty column screen will not hold it, so it goes on".into(),
			..Row::default()
		};
		let listing = render_units(std::slice::from_ref(&row), Some(80));
		assert_eq!(
			listing,
			"713  ESC\n\
			 \x20 · B1168F2  a fault text long enough that one line of an eighty column screen\n\
			 \x20            will not hold it, so it goes on\n\
			 \x20            stored\n"
		);
		assert!(listing.lines().all(|l| l.chars().count() <= 80), "{listing}");
		// A word longer than the room is its own line, whole.
		let word = Row {
			text: format!("short {}", "x".repeat(90)),
			..row.clone()
		};
		let listing = render_units(&[word], Some(80));
		assert!(listing.contains(&"x".repeat(90)), "{listing}");
		// `--details`: the records go under the facts, as they came.
		let detailed = Row {
			records: vec![(0x02, vec![0x01, 0xAB])],
			..row.clone()
		};
		assert!(render_units(&[detailed], Some(80)).ends_with("             stored\n             record 02  01 AB\n"));
		// The freeze frame (#47) goes under the code's facts, in `freezeframe::render`'s
		// column, and `--details`' records after it in the same column.
		let line = |key: &str, value: &str| crate::freezeframe::Line {
			key: key.into(),
			value: value.into(),
		};
		let framed = Row {
			freeze: vec![line("priority", "6"), line("Engine speed", "1000 /min")],
			records: vec![(0x70, vec![0x0F, 0xA0])],
			..row
		};
		let listing = render_units(&[framed], Some(80));
		assert!(
			listing.ends_with(
				"             stored\n\
				 \x20            priority      6\n\
				 \x20            Engine speed  1000 /min\n\
				 \x20            record 70     0F A0\n"
			),
			"{listing}"
		);
	}

	/// The namers over a real machine's data, for a check by hand on a copy of it: every
	/// recorded unit's lookup, and three codes named through it, printed. Run with
	/// `VAGCAN_HOME=<copy> cargo test -p vag-cli-diag -- --ignored --nocapture namers_over`.
	#[test]
	#[ignore = "reads the data dir VAGCAN_HOME names; a check by hand on a copy of real data"]
	fn namers_over_a_real_copy_name_the_recorded_units_codes() {
		let store = crate::project::vcds().unwrap();
		let source = crate::extracted::current().run_source();
		let mut namers = Namers::open(source, &store.rod_keys(), false).unwrap();
		assert!(!namers.is_empty(), "nothing names a code here");
		println!("names {}", namers.summary());
		let mut named = 0;
		for car in crate::units::recorded_cars().unwrap() {
			for unit in car.units {
				let Some(odx) = unit.odx_name.clone() else { continue };
				let lookup = namers.unit(&odx, unit.odx_version.as_deref().unwrap_or(""));
				for code in [[0x00, 0x01, 0x29], [0x04, 0x71, 0x20], [0xFF, 0xFF, 0x3C]] {
					if let Some(n) = namers.name(&lookup, code).filter(|n| n.named) {
						named += 1;
						println!("{odx} {} {:?} {:?} {} {:?}", code_hex(code), n.dtc, n.text, n.src, n.mark);
					}
				}
			}
		}
		assert!(named > 0, "no recorded unit's code was named");
	}

	#[test]
	fn supported_codes_are_a_block_per_unit_with_their_numbers() {
		let unit = Row {
			unit: "03".into(),
			request: 0x713,
			name: "ESC".into(),
			..Row::default()
		};
		assert_eq!(
			render_supported(&[(unit.clone(), vec![[0x00, 0x01, 0x29], [0x04, 0x71, 0x20]]), (unit, Vec::new())]),
			"713  ESC\n  000129  297\n  047120  291104\n"
		);
	}
}
