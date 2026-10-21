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
//!   from the ODIS project's own fault tables first ([`crate::odisfaults`])
//!   and from the VCDS label files where the project has none
//!   ([`crate::faultnames`]); where neither resolves one, the text column is
//!   empty rather than a plausible-sounding invention, and `-v` says why.
//!   [`Namers`] is the order, in one place for both the live and the recorded path.
//!
//! What it prints: one table, a row per code (`unit id name code dtc status km count
//! last seen text`), and a summary line, on stdout; the adapter, where the names come
//! from and the sealed catalogues on stderr; the rest at `-v` (`vag_cli_core::verbosity`).
//!
//! Read-only: the service issued is `0x19`, which reads. Clearing faults is
//! `0x14`, which the client's allowlist rejects.

use anyhow::Result;
use vag_cli_core::marks::SEP;
use vag_cli_core::report::Advice as _;
use vag_cli_core::table::Table;
use vag_cli_core::{note, out, say};
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

/// The status byte in one word for the table, the most telling state first (ISO 14229-1
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

/// Both ways of naming a code, in the order they are asked.
///
/// **The project first, the label files second.** An ODIS project carries a
/// fault table per ECU variant with the text in the clear, and a VCDS
/// installation names a code through a registry, a per-unit catalogue and a
/// text store, each of which can be missing or sealed. The project is asked
/// first because it answers for the exact variant the unit named; the chain
/// is asked where the project has no table for the unit, or where
/// `[faults] language` names a language only the VCDS build declares.
///
/// Either half may be absent — a project set up from one source has one —
/// and a command with neither still prints the codes.
pub struct Namers {
	odis: Option<crate::odisfaults::OdisFaults>,
	vcds: Option<crate::faultnames::Namer>,
	/// Where the VCDS files were looked for, for the note when there are none.
	vcds_root: std::path::PathBuf,
}

/// What both halves know about one unit, looked up once.
pub struct UnitNamers {
	odis: Option<crate::odisfaults::UnitTexts>,
	vcds: Option<vag_data_labels::UnitLookup>,
}

/// What one code is called, taken apart for the table: the code a tester prints, the
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
	/// Whose words the row shows, in the table's `src` column: [`ODIS`], [`ODIS_GUESSED`]
	/// when the variant is not settled, [`VCDS`]; empty with no words.
	pub src: &'static str,
	/// The variant was picked by family, not by the car's own F1A2: recording F1A2 settles it.
	pub needs_f1a2: bool,
}

/// The project's words, its variant settled by the car.
pub const ODIS: &str = "odis";
/// The project's words from a variant the car's identifiers did not settle — one of
/// several, or picked by family. The table never presents such a row as settled
/// (`odisfaults::Naming::provenance`).
pub const ODIS_GUESSED: &str = "odis?";
/// The VCDS labels' words.
pub const VCDS: &str = "vcds";

impl Namers {
	/// Open whatever this machine has. Never fails for a shortage of data —
	/// see [`Namers::is_empty`] — only for a VCDS pool that is there and will
	/// not open, or for keys named with `--iv-cache` (`given`) that are not there.
	pub fn open(keys: &std::path::Path, given: bool) -> Result<Namers> {
		let vcds_root = crate::project::rod_pool()?;
		let mut odis = crate::odisfaults::OdisFaults::open();
		let without_vcds = crate::project::current().is_ok_and(|p| without_vcds(&p, keys));
		let vcds = open_vcds(&vcds_root, keys, given, without_vcds)?;
		// The VCDS text's language is the pool's own build, read off its files —
		// not any source row's: the pool is shared by every project and holds the
		// build read last, and a stale row can name another. None where the chain
		// did not open.
		if let Some(odis) = &mut odis {
			odis.set_vcds(vcds.as_ref().map(|_| crate::odisfaults::VcdsBuild::of(&vcds_root)));
		}
		Ok(Namers { odis, vcds, vcds_root })
	}

	/// Whether there is nothing at all to name a code with.
	pub fn is_empty(&self) -> bool {
		self.odis.is_none() && self.vcds.is_none()
	}

	/// Where the VCDS files were looked for.
	pub fn vcds_root(&self) -> &std::path::Path {
		&self.vcds_root
	}

	/// The `names` line above a listing: which sources name the codes, in the order they
	/// are asked — `ODIS SK37X (deu) · VCDS (fallback)`.
	pub fn summary(&self) -> String {
		let odis = self.odis.as_ref().map(|o| o.short());
		let vcds_first = self.odis.as_ref().is_some_and(|o| o.prefers_vcds());
		let parts: Vec<String> = match (odis, self.vcds.is_some()) {
			(Some(odis), true) if vcds_first => vec!["VCDS".to_string(), format!("{odis} (fallback)")],
			(Some(odis), true) => vec![odis, "VCDS (fallback)".to_string()],
			(Some(odis), false) => vec![odis],
			(None, true) => vec!["VCDS".to_string()],
			(None, false) => Vec::new(),
		};
		parts.join(SEP)
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
				"{} rows of fault registry, {} texts, from {}{}",
				vcds.registry_rows(),
				vcds.codes_texts(),
				self.vcds_root.display(),
				match &self.odis {
					Some(odis) if odis.prefers_vcds() => " — asked first",
					Some(_) => " — the fallback",
					None => "",
				}
			));
			if let Some(note) = vcds.redirect_note() {
				out.push(note.to_string());
			}
		}
		out
	}

	/// Look a unit up in both halves by what it said about itself.
	pub fn unit(&mut self, odx_name: &str, version: &str) -> UnitNamers {
		UnitNamers {
			odis: self.odis.as_mut().map(|o| o.unit(odx_name, version)),
			vcds: self.vcds.as_mut().map(|n| n.unit(odx_name, version)),
		}
	}

	/// Why a unit's codes may come out as numbers, one line each, for `-v`.
	///
	/// The VCDS chain's own reasons are only worth saying when that chain is
	/// what the unit's codes will be named from — a sealed catalogue is no
	/// loss for a unit the project names in full.
	pub fn notes(&self, unit: &UnitNamers) -> Vec<String> {
		let mut out = Vec::new();
		let project_has_it = unit.odis.as_ref().is_some_and(|u| !u.is_empty());
		if self.odis.is_some() && !project_has_it {
			out.push(match &self.vcds {
				Some(_) => "the ODIS project has no fault table for this unit — VCDS text".to_string(),
				None => "the ODIS project has no fault table for this unit — codes only".to_string(),
			});
		}
		let vcds_matters = !project_has_it || self.odis.as_ref().is_some_and(|o| o.prefers_vcds());
		let instead = if project_has_it { "the ODIS project's text" } else { "codes only" };
		if vcds_matters && let Some(note) = unit.vcds.as_ref().and_then(|lookup| crate::faultnames::unit_note(lookup, instead)) {
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
		let odis = self.odis.as_ref().zip(unit.odis.as_ref()).and_then(|(o, u)| o.name(u, code));
		let vcds = match (&self.vcds, &unit.vcds) {
			(Some(namer), Some(vag_data_labels::UnitLookup::Found { catalogue, .. })) => Some(namer.name(catalogue, code)),
			_ => None,
		};
		let project_has_it = unit.odis.as_ref().is_some_and(|u| !u.is_empty());
		compose(odis, vcds, self.odis.as_ref().is_some_and(|o| o.prefers_vcds()), project_has_it)
	}
}

/// Whether the project has no VCDS installation among its sources — asked of its
/// cache's `source` table, not of the files: a VCDS source dropped with `setup --drop`
/// leaves the shared pool and this project's keys on disk, and its text must not go
/// on naming codes.
///
/// A cache whose sources do not read — an older vagcan's, or one a running `setup`
/// holds locked — says nothing either way, and is not taken as "no VCDS": that printed
/// bare numbers with no word why. The rule from before the sources were asked holds
/// instead — VCDS text where the project's keys are — and that is said once.
fn without_vcds(project: &crate::project::Project, keys: &std::path::Path) -> bool {
	match crate::project::has_source(project, vag_data_db::VCDS) {
		Ok(held) => !held,
		Err(e) => {
			crate::project::say_unreadable(project, &e, "VCDS fault text is used where this project's keys are");
			!keys.is_file()
		}
	}
}

/// The VCDS half of [`Namers`], or `None` where this project has no VCDS text.
///
/// The pool of raw VCDS files is shared by every project and the keys that open
/// it are per project ([`crate::project::Project::rod_keys`]), so a project with
/// no VCDS installation among its sources (`without_vcds`) meets a full pool —
/// another project's — and no keys, or the keys of a VCDS source dropped from
/// it. That is "this project has no VCDS text", not a refusal: its faults are
/// named from ODIS, or shown as numbers. Only keys named with `--iv-cache`
/// override it.
///
/// Everywhere else missing keys are still refused. A project whose sources
/// include VCDS has VCDS text of its own, and `[faults] language` may be what
/// sends a code to it: naming from ODIS instead would print another language
/// with nothing said. Keys named with `--iv-cache` (`given`) that are not there
/// are a typo, said as one before anything else.
fn open_vcds(root: &std::path::Path, keys: &std::path::Path, given: bool, without_vcds: bool) -> Result<Option<crate::faultnames::Namer>> {
	if given && !keys.is_file() {
		return Err(anyhow::anyhow!("--iv-cache {}: no such file", keys.display()).help("leave the flag out to use this project's rod-keys.json"));
	}
	if !crate::faultnames::has_fault_labels(root) || (without_vcds && !given) {
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
		},
		other => Named {
			note: other.line(),
			..Named::default()
		},
	}
}

/// The order the two chains are asked in, as one function over their answers.
///
/// Separated from [`Namers::name`] so the order is testable without a project
/// and a VCDS installation on disk: everything above it is lookup, this is the
/// policy.
///
/// The project wins, except where it has nothing to say. **A row with a
/// display code and no text is one of those** — two of the reference
/// project's 291,346 rows are shaped that way. The code is still the
/// project's and is kept; the words come from VCDS, and the note says whose
/// they are. Printing no words over a name that exists one lookup away, and
/// counting it as named, was the bug.
fn compose(
	odis: Option<crate::odisfaults::Naming>,
	vcds: Option<crate::faultnames::Naming>,
	prefer_vcds: bool,
	project_has_unit: bool,
) -> Option<Named> {
	let vcds = vcds.as_ref().map(vcds_named);
	if prefer_vcds && let Some(named) = vcds.clone().filter(|n| n.named) {
		return Some(named);
	}
	if let Some(naming) = odis {
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
		let odis_src = if naming.provenance().is_none() { ODIS } else { ODIS_GUESSED };
		// Whose words these are is the `src` column's, at level 0: a borrowed or a
		// fallen-back text is another source's words, likely in another language.
		let (text, named, src) = match (text, vcds.as_ref().and_then(|v| v.text.clone())) {
			// VCDS was asked first, for its language, and had nothing: these are the
			// project's words, likely in another language.
			(Some(text), _) if prefer_vcds => {
				notes.push("text from the ODIS project".to_string());
				(Some(text), true, odis_src)
			}
			(Some(text), _) => (Some(text), true, odis_src),
			(None, Some(borrowed)) => {
				notes.push("text from the VCDS labels".to_string());
				(Some(borrowed), true, VCDS)
			}
			// A code without words anywhere is not a named code, whatever else the
			// row carries.
			(None, None) => {
				notes.push("no text in the project".to_string());
				(None, false, "")
			}
		};
		return Some(Named {
			dtc: naming.display_code.clone(),
			text,
			note: (!notes.is_empty()).then(|| notes.join("; ")),
			named,
			src,
			// Only a family match leaves F1A2 to be recorded; several variants of the
			// car's own F1A2 disagreeing is a fact of the project, not a missing read.
			needs_f1a2: src == ODIS_GUESSED && !naming.confirmed,
		});
	}
	if let Some(named) = vcds {
		return (named.named || named.note.is_some()).then_some(named);
	}
	// The project has a table for the unit and this number is not in it,
	// and nothing else can be asked: said, rather than left blank.
	project_has_unit.then(|| Named {
		note: Some("not in this unit's fault table in the ODIS project".to_string()),
		..Named::default()
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

/// One row of the table: a code and what is known about it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row {
	/// The unit's short number, `--` when it has none.
	pub unit: String,
	pub request: u16,
	/// What the unit called itself (`F197`).
	pub name: String,
	pub code: [u8; 3],
	pub dtc: String,
	pub status: u8,
	/// From the first extended-data record that reads as a fault context.
	pub context: Option<FaultContext>,
	pub text: String,
	/// Whose words `text` is: [`ODIS`], [`ODIS_GUESSED`], [`VCDS`], or empty.
	pub src: &'static str,
}

/// The fault table, as `faults` prints it on stdout. `width`: the terminal's, when known.
///
/// One row per code, the text last. Nothing but the text is ever shortened, and the text is
/// never cut: with too little room it goes whole on an indented line under its row
/// ([`Table::render`]). When even the columns before the text are wider than the screen —
/// an 80-column terminal — each code takes two lines and its text a third
/// ([`render_narrow`]), so no line wraps.
pub fn render_rows(rows: &[Row], width: Option<usize>) -> String {
	let mut table = Table::new(&["unit", "id", "name", "code", "dtc", "status", "km", "count", "last seen", "src", "text"])
		.right(6)
		.right(7);
	for row in rows {
		let (km, count, when) = context_cells(row, describe_when);
		let mut cells = head_cells(row);
		cells.extend([short_status(row.status), km, count, when, row.src.to_string(), row.text.clone()]);
		table.row(cells);
	}
	match width {
		Some(width) if table.width_before_last() > width => render_narrow(rows, width),
		_ => table.render(width),
	}
}

/// What names a code: unit, id, name, code, dtc.
fn head_cells(row: &Row) -> Vec<String> {
	vec![
		row.unit.clone(),
		format!("{:03X}", row.request),
		row.name.clone(),
		code_hex(row.code),
		row.dtc.clone(),
	]
}

/// A code's record — km, count, when — with `when` written by `when`.
fn context_cells(row: &Row, when: fn(&FaultContext) -> String) -> (String, String, String) {
	match &row.context {
		Some(c) => (
			c.mileage_km.to_string(),
			match c.occurrences {
				0xFF => "255+".to_string(),
				n => n.to_string(),
			},
			when(c),
		),
		None => (String::new(), String::new(), String::new()),
	}
}

/// When a fault was stored, to the minute: the two-line layout's `last seen`.
fn describe_when_minute(context: &FaultContext) -> String {
	let full = describe_when(context);
	// `YYYY-MM-DD HH:MM:SS` loses its seconds; `—` stays as it is.
	match full.len() {
		19 => full[..16].to_string(),
		_ => full,
	}
}

/// The table for a screen narrower than one row: per code a line of what names it, an
/// indented line of its record, and its text under them, wrapped at spaces to the screen.
fn render_narrow(rows: &[Row], width: usize) -> String {
	let mut head = Table::new(&["unit", "id", "name", "code", "dtc"]);
	let mut record = Table::new(&["status", "km", "count", "last seen", "src"]).right(1).right(2);
	for row in rows {
		head.row(head_cells(row));
		let (km, count, when) = context_cells(row, describe_when_minute);
		record.row(vec![short_status(row.status), km, count, when, row.src.to_string()]);
	}
	let indent = "      ";
	let head = head.render(None);
	let record = record.render(None);
	let mut out = String::new();
	for (at, (first, second)) in head.lines().zip(record.lines()).enumerate() {
		out.push_str(first);
		out.push('\n');
		out.push_str(indent);
		out.push_str(second);
		out.push('\n');
		// The header pair has no text under it; row `at - 1` does.
		if let Some(row) = at.checked_sub(1).and_then(|n| rows.get(n)) {
			for line in vag_cli_core::table::wrap(&row.text, width.saturating_sub(indent.len()).max(20)) {
				out.push_str(indent);
				out.push_str(&line);
				out.push('\n');
			}
		}
	}
	out
}
/// The one stderr line under a table with guessed rows (`src` [`ODIS_GUESSED`]): which
/// units, and where the variant was picked by family, what F1A2 did — the identifier that
/// settles it. `by_family`: those units, and whether each answered F1A2. `None` when every
/// row's source is settled.
pub fn guessed_line(rows: &[Row], by_family: &[(u16, bool)]) -> Option<String> {
	let mut guessed: Vec<u16> = rows.iter().filter(|r| r.src == ODIS_GUESSED).map(|r| r.request).collect();
	guessed.dedup();
	if guessed.is_empty() {
		return None;
	}
	let ids = |ids: &[u16]| ids.iter().map(|r| format!("{r:03X}")).collect::<Vec<_>>().join(" ");
	let mut line = format!("{ODIS_GUESSED}    {}: the ODIS variant is a guess", ids(&guessed));
	let silent: Vec<u16> = by_family.iter().filter(|(_, read)| !read).map(|(r, _)| *r).collect();
	let unmatched: Vec<u16> = by_family.iter().filter(|(_, read)| *read).map(|(r, _)| *r).collect();
	if !silent.is_empty() {
		line.push_str(&format!(
			"; F1A2 not read from {}: run `vagcan faults` again with the ignition on",
			ids(&silent)
		));
	}
	if !unmatched.is_empty() {
		line.push_str(&format!("; F1A2 of {} matches no variant in the project", ids(&unmatched)));
	}
	Some(line)
}

/// The summary line under the table.
pub fn summary(stored: usize, failing: usize, units: usize, all: bool) -> String {
	let what = if all { "listed" } else { "stored" };
	format!(
		"{stored} {what}{SEP}{failing} failing now{SEP}{units} {} read",
		crate::render::plural(units, "unit")
	)
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
	// The names come from what `vagcan setup` wrote — the project's cache and
	// the shared pool of VCDS files — and from nowhere else. Neither being
	// there is the ordinary "setup has not run yet" case: the codes are still
	// read and shown as numbers, with a note that names `vagcan setup`.
	// The keys are per project (`rod-keys.json`) unless `--iv-cache` names a file.
	// The numbers `vagcan units` lists, for `--unit` and for the headings below.
	crate::labels::install_recorded_units();
	// `--unit` before the project is opened: opening it can upgrade its cache, and a
	// typo is refused touching nothing.
	let requested = only.map(|spec| unit_list("--unit", spec)).transpose()?;
	let keys = crate::datadir::or_default(iv_cache, || Ok(crate::project::current()?.rod_keys()))?;
	let mut namers = Namers::open(&keys, iv_cache.is_some())?;

	let mut backend = open().await?;

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
				say!("names    none — `vagcan setup` names the codes");
				note!("{}", crate::missing::no_fault_labels(namers.vcds_root()));
			}
			false => {
				say!("names    {}", namers.summary());
				for line in namers.describe() {
					note!("{line}");
				}
			}
		}
		note!("{STORED_IS_NOT_FAILING}");
	}

	let mut rows: Vec<Row> = Vec::new();
	let mut supported_table = Table::new(&["unit", "id", "name", "code", "number"]).right(4);
	let mut supported_total = 0usize;
	let mut raw_table = Table::new(&["id", "code", "record", "bytes"]);
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
			Err(e) => say!("{request:03X}  fault list not read: {e}"),
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
					for dtc in &list {
						supported_table.row(vec![
							number.clone(),
							format!("{request:03X}"),
							name.clone(),
							code_hex(dtc.code),
							fault_number(dtc.code).to_string(),
						]);
					}
					supported_total += list.len();
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
			let lookup = match namers.is_empty() {
				true => None,
				false => {
					let read = |data: Option<Vec<u8>>| data.map(|b| ident_text(&b)).unwrap_or_default();
					let odx = read(uds.read_data_by_identifier(ODX_NAME).await.ok());
					let version = read(uds.read_data_by_identifier(ODX_VERSION).await.ok());
					vag_cli_core::wire!("   {request:03X}  names looked up by F19E {odx:?} F1A2 {version:?}");
					f1a2 = version.clone();
					Some(namers.unit(&odx, &version))
				}
			};
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
				let records = uds.read_dtc_extended(dtc.code).await.unwrap_or_default();
				let mut context = None;
				for record in &records {
					let parsed = FaultContext::parse(&record.data);
					if let Some(parsed) = &parsed {
						note!(
							"{request:03X}  {}  {}{SEP}{}",
							code_hex(dtc.code),
							describe_status(dtc.status),
							describe_age(parsed, now.as_ref())
						);
					}
					context = context.or(parsed);
					if details {
						raw_table.row(vec![
							format!("{request:03X}"),
							code_hex(dtc.code),
							format!("{:02X}", record.record),
							crate::render::hex_spaced(&record.data),
						]);
					}
				}
				rows.push(Row {
					unit: number.clone(),
					request,
					name: name.clone(),
					code: dtc.code,
					dtc: named.dtc.unwrap_or_default(),
					status: dtc.status,
					context,
					text: named.text.unwrap_or_default(),
					src: named.src,
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
		if !supported_table.is_empty() {
			out!("{}\n", supported_table.render(width));
		}
		out!(
			"{supported_total} {} supported{SEP}{read} {} read\n",
			crate::render::plural(supported_total, "code"),
			crate::render::plural(read, "unit")
		);
		return Ok(());
	}
	let failing = rows.iter().filter(|r| r.status & FAILED_NOW != 0).count();
	if !rows.is_empty() {
		out!("{}\n", render_rows(&rows, width));
	}
	if details && !raw_table.is_empty() {
		out!("{}\n", raw_table.render(width));
	}
	out!("{}\n", summary(rows.len(), failing, read, all_codes));
	// A guessed variant is marked in the table; what settles it, once, under the summary.
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
	fn missing_keys_are_no_vcds_text_only_for_a_project_from_odis_alone_and_only_by_default() {
		// The pool of raw VCDS files is shared by every project and the keys are per
		// project, so a project set up from ODIS alone meets another project's pool
		// and no `rod-keys.json`: its faults are named from ODIS, no refusal.
		let base = std::env::temp_dir().join(format!("vagcan-odis-only-{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&base);
		std::fs::create_dir_all(base.join("UDS_EV")).unwrap();
		std::fs::write(base.join("UDS_EV").join(vag_data_labels::dtc::REGISTRY_FILE), b"x").unwrap();
		std::fs::write(base.join(crate::setup::CODES_FILES[0]), b"x").unwrap();
		let keys = base.join("project").join("rod-keys.json");
		assert!(crate::faultnames::has_fault_labels(&base), "the fixture is a filled pool");

		assert!(
			open_vcds(&base, &keys, false, true).expect("ODIS alone: no refusal").is_none(),
			"a project from ODIS alone, another project's pool"
		);
		// A project whose sources include VCDS, its keys deleted: that VCDS text is the
		// project's own, and printing ODIS's in another language instead would be silent.
		let refused = open_vcds(&base, &keys, false, false).err().expect("ODIS and VCDS, keys missing");
		let refused = vag_cli_core::report::render(&refused, 0, vag_cli_core::report::Colour::Never);
		assert!(refused.contains("rod-keys.json"), "{refused}");
		// `--iv-cache` naming a file that is not there is a typo, said as one — whatever
		// the project was set up from, and whether or not the pool has anything in it.
		for (pool, odis_alone) in [(base.clone(), true), (base.clone(), false), (base.join("empty"), true)] {
			let said = open_vcds(&pool, &keys, true, odis_alone)
				.err()
				.expect("an --iv-cache that is not there")
				.to_string();
			assert!(said.starts_with("--iv-cache"), "{said}");
			assert!(said.contains(&keys.display().to_string()), "{said}");
		}
		// An empty pool, by default, is no VCDS text either way.
		assert!(open_vcds(&base.join("empty"), &keys, false, false).unwrap().is_none());
		// A VCDS source dropped from the project leaves its keys behind: still no VCDS
		// text, because the project has no VCDS source to name codes from.
		std::fs::create_dir_all(keys.parent().unwrap()).unwrap();
		std::fs::write(&keys, b"{}").unwrap();
		assert!(
			open_vcds(&base, &keys, false, true).unwrap().is_none(),
			"the dropped source's keys named codes"
		);
		let _ = std::fs::remove_dir_all(&base);
	}

	#[test]
	fn the_vcds_text_is_asked_of_the_projects_sources_and_not_of_a_sources_json() {
		// What the owner met (2026-10-20): `sources.json` said one thing and the cache
		// another. Whether a project has VCDS text is the cache's `source` table now —
		// a project with a VCDS source and no `sources.json` has it.
		let base = std::env::temp_dir().join(format!("vagcan-no-sources-json-{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&base);
		let project = crate::project::Project {
			id: "SK37X".into(),
			dir: base.join("SK37X"),
		};
		std::fs::create_dir_all(&project.dir).unwrap();
		assert!(!crate::project::has_source(&project, vag_data_db::VCDS).unwrap(), "no cache, no source");
		vag_data_db::record_language(&project.cache(), vag_data_db::VCDS, &base.join("vcds-en/Labels").to_string_lossy(), "eng").unwrap();
		assert!(!project.dir.join("sources.json").exists());
		let keys = project.rod_keys();
		std::fs::write(&keys, b"{}").unwrap();
		assert!(
			!without_vcds(&project, &keys),
			"a VCDS source in the cache is VCDS text, with no sources.json"
		);
		// Dropped, it is gone for `faults` too, whatever is left on disk.
		let id = crate::project::sources(&project).unwrap()[0].id;
		vag_data_db::drop_source(&project.cache(), id).unwrap();
		assert!(without_vcds(&project, &keys));
		let _ = std::fs::remove_dir_all(&base);
	}

	#[test]
	fn a_cache_whose_sources_do_not_read_is_not_taken_for_one_without_vcds() {
		// Review of #37: a cache an older vagcan wrote (or one a running setup holds)
		// made `without_vcds` true, and every code printed as a bare number. It says
		// nothing either way: the keys decide, as before the sources were asked.
		let base = std::env::temp_dir().join(format!("vagcan-unreadable-sources-{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&base);
		let project = crate::project::Project {
			id: "SK37X".into(),
			dir: base.join("SK37X"),
		};
		std::fs::create_dir_all(&project.dir).unwrap();
		std::fs::write(project.cache(), b"this is not an SQLite file, and it is longer than a header").unwrap();
		assert!(crate::project::has_source(&project, vag_data_db::VCDS).is_err(), "an error, not a no");
		let keys = project.rod_keys();
		assert!(without_vcds(&project, &keys), "no keys: no VCDS text");
		std::fs::write(&keys, b"{}").unwrap();
		assert!(!without_vcds(&project, &keys), "the keys are there: VCDS text, as before");
		let _ = std::fs::remove_dir_all(&base);
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
	fn an_odis_row_with_no_text_borrows_the_vcds_words_under_its_own_code() {
		// Two rows of the reference project's 291,346 carry a display code and
		// no text. The project still writes the code, but it has no name to
		// give — so the VCDS chain is asked rather than "(no text in the
		// project)" winning over a name that exists one lookup away.
		let vcds = crate::faultnames::Naming::Named {
			text: "Steering angle sensor".into(),
			sae: Some("B1168".into()),
			failure_type: Some(0xF2),
		};
		let named = compose(Some(textless_odis_row()), Some(vcds), false, true).expect("a code with a display code says something");
		assert_eq!(
			named,
			Named {
				dtc: Some("B1168F2".into()),
				text: Some("Steering angle sensor".into()),
				note: Some("text from the VCDS labels".into()),
				named: true,
				src: VCDS,
				needs_f1a2: false,
			},
			"a borrowed name is still a name, under the project's own code"
		);
	}

	#[test]
	fn with_vcds_asked_first_its_english_wins_and_the_project_answers_the_rest() {
		// Unset language, a German project and an English VCDS build:
		// `prefers_vcds` is true, so a code VCDS names reads in its English, and
		// one it cannot name still gets the project's German rather than nothing.
		let german = crate::odisfaults::Naming {
			text: Some("Lenkwinkelsensor".into()),
			..textless_odis_row()
		};
		let english = crate::faultnames::Naming::Named {
			text: "Steering angle sensor".into(),
			sae: Some("B1168".into()),
			failure_type: Some(0xF2),
		};
		let named = compose(Some(german.clone()), Some(english), true, true).unwrap();
		assert_eq!(
			(named.dtc.as_deref(), named.text.as_deref()),
			(Some("B1168F2"), Some("Steering angle sensor"))
		);
		assert!(named.named);
		let named = compose(Some(german.clone()), Some(crate::faultnames::Naming::NotListed), true, true).unwrap();
		assert_eq!(named.text.as_deref(), Some("Lenkwinkelsensor"));
		assert_eq!(
			named.note.as_deref(),
			Some("text from the ODIS project"),
			"the project's words where VCDS was asked first say whose they are"
		);
		// With the project first, as usual, its own words need no marker.
		let named = compose(Some(german), None, false, true).unwrap();
		assert_eq!((named.text.as_deref(), named.note), (Some("Lenkwinkelsensor"), None));
	}

	#[test]
	fn a_row_whose_variant_is_not_settled_is_marked_in_the_table_itself() {
		// `odisfaults::Naming::provenance`: the line never presents a guessed variant as
		// settled. The guess and whose words a row has are the `src` column's, at level 0.
		let text = |naming: crate::odisfaults::Naming| crate::odisfaults::Naming {
			text: Some("Lenkwinkelsensor".into()),
			..naming
		};
		let settled = compose(Some(text(textless_odis_row())), None, false, true).unwrap();
		assert_eq!((settled.src, settled.needs_f1a2), (ODIS, false));
		let by_family = crate::odisfaults::Naming {
			confirmed: false,
			..text(textless_odis_row())
		};
		let guessed = compose(Some(by_family), None, false, true).unwrap();
		assert_eq!((guessed.src, guessed.needs_f1a2), (ODIS_GUESSED, true));
		let of_two = crate::odisfaults::Naming {
			matched: 2,
			disagrees: true,
			..text(textless_odis_row())
		};
		let guessed = compose(Some(of_two), None, false, true).unwrap();
		assert_eq!((guessed.src, guessed.needs_f1a2), (ODIS_GUESSED, false), "the car's own F1A2 picked two");
		// A borrowed text is the other source's, said in the same column.
		let vcds = crate::faultnames::Naming::Named {
			text: "Steering angle sensor".into(),
			sae: None,
			failure_type: None,
		};
		assert_eq!(compose(Some(textless_odis_row()), Some(vcds), false, true).unwrap().src, VCDS);
	}

	#[test]
	fn a_code_nothing_has_words_for_is_not_counted_as_named() {
		// The display code alone is how a tester writes the number, not what
		// went wrong. Counting it as named was what made the run's tally say
		// every code had a name.
		let named = compose(Some(textless_odis_row()), None, false, true).unwrap();
		assert_eq!(named.dtc.as_deref(), Some("B1168F2"));
		assert_eq!(named.text, None);
		assert_eq!(named.note.as_deref(), Some("no text in the project"));
		assert!(!named.named);
	}

	#[test]
	fn a_vcds_name_is_the_sae_code_and_the_words_and_a_reason_is_a_note() {
		let named = compose(
			None,
			Some(crate::faultnames::Naming::Named {
				text: "Line 1\nline 2".into(),
				sae: Some("U1110".into()),
				failure_type: Some(0x00),
			}),
			false,
			false,
		)
		.unwrap();
		assert_eq!(named.dtc.as_deref(), Some("U111000"));
		assert_eq!(named.text.as_deref(), Some("Line 1 / line 2"), "one row on a screen");
		let unlisted = compose(None, Some(crate::faultnames::Naming::NotListed), false, false).unwrap();
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
			cycle_counter: 0x02B8,
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
				cycle_counter: 0,
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
			cycle_counter: 0x02B9,
			mileage_km: 212_795,
			clock: 0x69F9_68D9,
		};
		assert!(describe_age(&context, None).contains("255+ times"));
		let row = Row {
			context: Some(context),
			..Row::default()
		};
		assert!(render_rows(&[row], None).contains(" 255+ "), "the table's count column too");
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
	fn the_table_is_the_reference_layout() {
		// Invented units and texts in the layout the owner gave (2026-10-20).
		let context = |km, n, clock| {
			Some(FaultContext {
				priority: 1,
				occurrences: n,
				cycle_counter: 0,
				mileage_km: km,
				clock,
			})
		};
		let rows = [
			Row {
				unit: "16".into(),
				request: 0x70C,
				name: "Lenks.Modul".into(),
				code: [0x04, 0x71, 0x20],
				dtc: "B145501".into(),
				status: 0x08,
				context: context(214_592, 254, 0x69F6_0003),
				text: "Unterbrechung des Signalpfades \"Init\"".into(),
				src: ODIS,
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
			},
		];
		assert_eq!(
			render_rows(&rows, None),
			"unit  id   name         code    dtc      status         km  count  last seen            src    text\n\
			 16    70C  Lenks.Modul  047120  B145501  confirmed  214592    254  2026-07-27 00:00:03  odis   Unterbrechung des Signalpfades \"Init\"\n\
			 17    714  KOMBI        FFFF3C  U111000  failing    214634     13  2026-07-28 23:50:02  odis?  BAP_TEL_0x28 keine Kommunikation\n"
		);
		// With room for 30 columns of text, only the text is cut, to a known width only.
		let cut = render_rows(&rows, Some(125));
		assert!(cut.lines().all(|l| l.chars().count() <= 125), "{cut}");
		assert!(cut.contains("2026-07-27 00:00:03  odis   Unterbrechung des Signalpfade…"), "{cut}");
		// At 100 columns the row fits and the text goes whole under it, never cut inside
		// the columns before it (both reviews of PR #38).
		let wide = render_rows(&rows, Some(100));
		assert_eq!(
			wide,
			"unit  id   name         code    dtc      status         km  count  last seen            src\n\
			 16    70C  Lenks.Modul  047120  B145501  confirmed  214592    254  2026-07-27 00:00:03  odis\n\
			 \x20     Unterbrechung des Signalpfades \"Init\"\n\
			 17    714  KOMBI        FFFF3C  U111000  failing    214634     13  2026-07-28 23:50:02  odis?\n\
			 \x20     BAP_TEL_0x28 keine Kommunikation\n"
		);
		assert!(wide.lines().all(|l| l.chars().count() <= 100), "{wide}");
		// At 80 the columns before the text alone are wider than the screen: two lines a
		// code, the seconds dropped, the text under them — no line wraps.
		let narrow = render_rows(&rows, Some(80));
		assert_eq!(
			narrow,
			"unit  id   name         code    dtc\n\
			 \x20     status         km  count  last seen         src\n\
			 16    70C  Lenks.Modul  047120  B145501\n\
			 \x20     confirmed  214592    254  2026-07-27 00:00  odis\n\
			 \x20     Unterbrechung des Signalpfades \"Init\"\n\
			 17    714  KOMBI        FFFF3C  U111000\n\
			 \x20     failing    214634     13  2026-07-28 23:50  odis?\n\
			 \x20     BAP_TEL_0x28 keine Kommunikation\n"
		);
		assert!(narrow.lines().all(|l| l.chars().count() <= 80), "{narrow}");
		// A long text is wrapped at spaces to the screen, not run past it.
		let long = Row {
			text: "a fault text long enough that one indented line of an eighty column screen will not hold it".into(),
			..rows[0].clone()
		};
		let wrapped = render_rows(&[long], Some(80));
		assert!(wrapped.lines().all(|l| l.chars().count() <= 80), "{wrapped}");
		assert!(wrapped.contains("hold it"), "{wrapped}");
		// A guessed variant is said under the table, with what F1A2 did.
		assert_eq!(
			guessed_line(&rows, &[(0x714, false)]).as_deref(),
			Some("odis?    714: the ODIS variant is a guess; F1A2 not read from 714: run `vagcan faults` again with the ignition on")
		);
		assert_eq!(
			guessed_line(&rows, &[]).as_deref(),
			Some("odis?    714: the ODIS variant is a guess"),
			"several variants of the car's own F1A2: nothing to read again"
		);
		assert_eq!(guessed_line(&rows[..1], &[]), None, "every row settled: no line");
		assert_eq!(summary(2, 1, 18, false), "2 stored · 1 failing now · 18 units read");
		assert_eq!(summary(0, 0, 1, true), "0 listed · 0 failing now · 1 unit read");
	}
}
