//! Reading a measurement the way VCDS does: a unit's `[MWB]` list, into the
//! global measurement registry `RM.rod`.
//!
//! ```text
//! the unit's F19E/F1A2 ──▶ its own .rod ──▶ [MWB]
//!                                   └──▶ [INC] ──▶ IV_…_M<n>.rod ──▶ [MWB]
//! [MWB] row "<n>,<code>" ──▶ UDS_EV/RM.rod [MWB], row n counted from 1 ──▶ "<key>,<payload>"
//! the payload, in the alphabet the key generates ──▶ DID, bit layout, scaling, unit, name
//! ```
//!
//! `research/vcds-registry/README.md` is the evidence. On the reference car all
//! twelve gearbox identifiers proven on a drive land on rows the gearbox lists —
//! at 1-based numbering and not at 0-based — with their lengths, byte orders and
//! factors, and the thirteen `IDE–ENG` pairs its gearbox logs print land too.
//!
//! This is [`crate::dtc`]'s chain for the other registry: the same framing, the
//! same 1-based rows, the same per-key alphabet ([`TableAlphabet::for_key`]). Two
//! things differ. Most units carry no `[MWB]` of their own and name, in `[INC]`,
//! the shared store that holds it. And a registry row can say things this reader
//! does not decode; those come back as [`Kind::Undecoded`], never as a guess.
//!
//! Every file is read from **one** install: a unit's list and the registry it
//! points into are only meaningful together, and each VCDS release re-encrypts
//! and reorders both.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::dtc::{DtcRegistry, DtcRow, find_named};
use crate::glyphs::TableAlphabet;
use crate::rod::{IvCache, KeyCost, RodStatus, decode_rod_recover, key_cost, recover_classic_key};

/// The section a unit's measurement list, and the registry, are read from.
pub const MWB_SECTION: &str = "MWB";

/// The section naming the shared stores a unit includes.
pub const INC_SECTION: &str = "INC";

/// The global measurement registry's own file name, inside a VCDS install.
pub const REGISTRY_FILE: &str = "RM.rod";

/// `f2`'s low six bits: the row's type. The bit layout of `f2` was read off the
/// registry against an ODIS project that declares the same rows
/// (`research/vcds-registry/README.md` §1) — a property of the file format, not
/// of any car.
const TYPE_MASK: u64 = 63;
/// `f2`'s bit 6: the value's bytes run least-significant first.
const LITTLE_ENDIAN: u64 = 64;
/// `f2`'s bit 7: the value is two's-complement signed.
const SIGNED: u64 = 128;

/// The row types this reader decodes, by `f2 & TYPE_MASK`.
const LINEAR: u64 = 0;
const IDENTITY: u64 = 2;
const TEXT_TABLE: u64 = 3;
const OBD_FORMULA: u64 = 4;
const RAW: u64 = 7;
const ASCII: u64 = 8;

/// `UDS_EV/RM.rod`'s `[MWB]` section: every measurement row, in file order,
/// because the order is what a unit's list points into.
///
/// 280,932 rows under 36,926 keys in VCDS 26.3. `RD.rod` frames its rows the
/// same way — `<key>,<payload>`, CRLF, a row per line and every line kept — so
/// one row table serves both.
#[derive(Debug, Clone, Default)]
pub struct MeasurementRegistry {
	rows: DtcRegistry,
}

impl MeasurementRegistry {
	/// Parse a decoded `[MWB]` section of `RM.rod`.
	pub fn parse(text: &str) -> Self {
		Self {
			rows: DtcRegistry::parse(text),
		}
	}

	/// Read a **1-based** row number, the way a unit's list points.
	///
	/// Not a convention chosen here: read 0-based, the reference gearbox's
	/// twelve proven identifiers become eleven, because the neighbouring rows of
	/// one key are near-duplicates and an off-by-one lands on a plausible row.
	pub fn row(&self, index: usize) -> Option<DtcRow<'_>> {
		self.rows.row(index)
	}

	pub fn len(&self) -> usize {
		self.rows.len()
	}

	pub fn is_empty(&self) -> bool {
		self.rows.is_empty()
	}
}

/// One registry row, read.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryRow {
	/// The row's key: the `TTTEXT` record of the measurement, whose tail names
	/// its ODX id (`IDE#####`, `MAS#####`). One key holds every variant of the
	/// measurement; the unit's list picks the row.
	pub key: u32,
	/// The identifier `0x22` asks for.
	pub did: u16,
	/// Bits into the positive response after `62 <DID>`, counted as ODX counts
	/// them: `byte * 8 + bit` (`f7 * 8 + f8`).
	pub bit_offset: u32,
	/// How many bits the value occupies (`f9`).
	pub bit_length: u32,
	/// Whether the bits are a two's-complement signed quantity.
	pub signed: bool,
	/// Whether the bytes run most-significant first.
	pub big_endian: bool,
	/// How the raw value becomes what VCDS shows.
	pub kind: Kind,
	/// `UNIT.ROD`'s id for the engineering unit, when the row has one (`f6`).
	pub unit_id: Option<u32>,
	/// The `TTTEXT` record of the name VCDS shows (`f10`) — the `ENG######` its
	/// logs print beside the ODX id.
	pub name_id: Option<u32>,
}

/// How a registry row turns a raw value into a reading.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
	/// `value = raw × factor + offset`. Type 0, and type 2 (identity), whose
	/// fields are empty or say ×1 +0. The row stores `(raw · f4 + f3) / f5`,
	/// read here as `factor = f4 / f5`, `offset = f3 / f5` — the reading that
	/// held on all 142 bridged rows with a nonzero offset.
	Linear { factor: f64, offset: f64 },
	/// A text table: the raw value names a state in `TTDOP.rod`'s table `table`
	/// (`f1`). Type 3.
	Enum { table: u32 },
	/// An OBD-II PID's SAE J1979 formula. Type 4. Not decoded here.
	ObdFormula,
	/// Raw bytes, shown as they are. Type 7.
	Raw,
	/// ASCII text. Type 8.
	Ascii,
	/// A type this reader does not decode — 1, 5, 6, 10, 12 and 18–40 occur in
	/// VCDS 26.3, about one row in eight.
	Undecoded(u8),
}

/// Why a registry row did not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowError {
	/// Not the 12 or 13 fields a registry row has.
	Shape { fields: usize },
	/// A field holds a glyph the key's alphabet does not have.
	Glyph { field: usize },
	/// A field that must be a number is empty or is not one.
	Number { field: usize },
	/// No identifier: about 620 rows in VCDS 26.3 have none, and nothing can ask for them.
	NoDid,
	/// A linear row whose scaling is missing, or divides by zero.
	Scaling,
	/// A text-table row with no table.
	NoTable,
	/// A row of a type this reader does not decode ([`Kind::Undecoded`]) whose
	/// layout does not read either — such types may leave it empty. A row of
	/// that type all the same, which is what a caller counting rows says.
	Undecoded(u8),
}

/// Read one registry row through the alphabet its key generates.
pub fn read_row(row: &DtcRow<'_>) -> Result<RegistryRow, RowError> {
	let alphabet = TableAlphabet::for_key(row.key);
	let fields: Vec<&str> = row.payload.split(alphabet.separator()).collect();
	if !(12..=13).contains(&fields.len()) {
		return Err(RowError::Shape { fields: fields.len() });
	}
	// Empty is a field with nothing in it, which several types have; a field
	// with a glyph outside the alphabet is a row this key did not write.
	let text = |field: usize| -> Result<Option<String>, RowError> {
		match fields[field] {
			"" => Ok(None),
			glyphs => alphabet.decode(glyphs).map(Some).ok_or(RowError::Glyph { field }),
		}
	};
	let whole = |field: usize| -> Result<Option<u64>, RowError> { text(field)?.map(|t| t.parse().map_err(|_| RowError::Number { field })).transpose() };
	let real = |field: usize| -> Result<Option<f64>, RowError> { text(field)?.map(|t| t.parse().map_err(|_| RowError::Number { field })).transpose() };
	let required = |field: usize| whole(field)?.ok_or(RowError::Number { field });

	let did = whole(0)?.ok_or(RowError::NoDid)?;
	let did = u16::try_from(did).map_err(|_| RowError::Number { field: 0 })?;
	let flags = required(2)?;
	let kind = match flags & TYPE_MASK {
		kind @ (LINEAR | IDENTITY) => match (real(3)?, real(4)?, real(5)?) {
			(offset, Some(numerator), Some(denominator)) if denominator != 0.0 => Kind::Linear {
				factor: numerator / denominator,
				offset: offset.unwrap_or(0.0) / denominator,
			},
			(None, None, None) if kind == IDENTITY => Kind::Linear { factor: 1.0, offset: 0.0 },
			_ => return Err(RowError::Scaling),
		},
		TEXT_TABLE => Kind::Enum {
			table: whole(1)?.and_then(|t| u32::try_from(t).ok()).ok_or(RowError::NoTable)?,
		},
		OBD_FORMULA => Kind::ObdFormula,
		RAW => Kind::Raw,
		ASCII => Kind::Ascii,
		other => Kind::Undecoded(other as u8),
	};
	let small = |value: u64, field: usize| u32::try_from(value).map_err(|_| RowError::Number { field });
	let layout = || -> Result<(u32, u32), RowError> {
		let byte = small(required(7)?, 7)?;
		let bit = small(required(8)?, 8)?;
		let offset = byte
			.checked_mul(8)
			.and_then(|b| b.checked_add(bit))
			.ok_or(RowError::Number { field: 7 })?;
		Ok((offset, small(required(9)?, 9)?))
	};
	let (bit_offset, bit_length) = match (layout(), &kind) {
		(Ok(layout), _) => layout,
		(Err(_), Kind::Undecoded(kind)) => return Err(RowError::Undecoded(*kind)),
		(Err(e), _) => return Err(e),
	};
	Ok(RegistryRow {
		key: row.key,
		did,
		bit_offset,
		bit_length,
		signed: flags & SIGNED != 0,
		big_endian: flags & LITTLE_ENDIAN == 0,
		kind,
		unit_id: whole(6)?.map(|u| small(u, 6)).transpose()?,
		name_id: whole(10)?.map(|n| small(n, 10)).transpose()?,
	})
}

/// One unit's measurement list: the registry rows it reads, in file order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitRows {
	/// 1-based registry rows.
	pub rows: Vec<usize>,
	/// How many rows the list named that the registry does not have.
	pub outside: usize,
	/// A plain-TEA list's first row, dropped rather than read: a nonzero
	/// per-record `product` leaves its first eight bytes wrong, which is the row
	/// number itself, and nothing in the list can say which row it was.
	pub first_row_dropped: bool,
}

impl UnitRows {
	/// Parse a unit's decoded `[MWB]` against the registry it points into.
	fn parse(text: &str, registry_len: usize, tea: bool) -> Self {
		let mut out = Self::default();
		let mut lines = text.split("\r\n").filter(|line| !line.is_empty());
		if tea && lines.next().is_some() {
			out.first_row_dropped = true;
		}
		for line in lines {
			let Some((row, _code)) = line.split_once(',') else { continue };
			let Ok(row) = row.parse::<usize>() else { continue };
			match (1..=registry_len).contains(&row) {
				true => out.rows.push(row),
				false => out.outside += 1,
			}
		}
		out
	}
}

/// What a unit's measurement list resolved to — or why it did not.
///
/// Every arm is a different answer to "why does this unit have no channels
/// from VCDS", and the caller is expected to say which.
#[derive(Debug, Clone, PartialEq)]
pub enum UnitMeasurements {
	/// The list: from the unit's own file, or from the store its `[INC]` names.
	Found { file: PathBuf, store: Option<PathBuf>, rows: UnitRows },
	/// No file of that ODX name in the install. On the reference car this is
	/// the body control module, `EV_BCMMQB`, in every install checked.
	NoFile,
	/// The family is there and no member lists measurements, directly or
	/// through `[INC]`.
	NoList { candidates: usize },
	/// The list is in a classic section whose key is not cached: one search,
	/// minutes. `setup` runs it.
	Locked { file: PathBuf, section: String },
	/// The list is in a shifted file. VCDS's runtime key opens it and nothing
	/// offline does (`.archive/research/labels/tttext2.md` §3.3a).
	Shifted { file: PathBuf, section: String },
	/// `[INC]` names no store this install has, or its first row could not be
	/// repaired against the install's file names.
	Unresolved { file: PathBuf },
	/// The list names rows the registry does not have: a file from another
	/// install than the registry.
	Mismatched { file: PathBuf, listed: usize, outside: usize },
}

/// Find and read the measurement list of a unit that identified itself.
///
/// `odx_name` is the unit's `F19E` and `version` its `F1A2`; both come off the
/// car, which keeps the file choice a lookup the vehicle answers
/// ([`crate::label_files::find_rod_by_odx_variant`]). Candidates are tried best
/// match first and the first whose list opens wins. With `crack`, a classic
/// section whose key is not cached is searched for (minutes, recorded in
/// `cache`); a shifted one never is.
pub fn unit_measurements(
	root: &Path,
	odx_name: &str,
	version: &str,
	cache: &mut IvCache,
	crack: bool,
	registry: &MeasurementRegistry,
) -> UnitMeasurements {
	let candidates = crate::label_files::find_rod_by_odx_variant(root, odx_name, version).unwrap_or_default();
	if candidates.is_empty() {
		return UnitMeasurements::NoFile;
	}
	// Why no candidate answered, best reason first: a list from another install
	// is a warning, a lock is minutes away, a shift is not.
	let mut why: Option<UnitMeasurements> = None;
	let mut keep = |answer: UnitMeasurements| {
		let rank = |a: &UnitMeasurements| match a {
			UnitMeasurements::Mismatched { .. } => 0,
			UnitMeasurements::Locked { .. } => 1,
			UnitMeasurements::Shifted { .. } => 2,
			UnitMeasurements::Unresolved { .. } => 3,
			_ => 4,
		};
		if why.as_ref().is_none_or(|w| rank(&answer) < rank(w)) {
			why = Some(answer);
		}
	};
	for (_, file) in &candidates {
		match list_of(file, file, None, cache, crack, registry) {
			Listed::Found(found) => return found,
			Listed::Failed(answer) => keep(answer),
			Listed::Absent => {}
		}
		match read_section(file, INC_SECTION, cache, crack) {
			SectionRead::Text { text, tea } => {
				let (stores, unread) = included(&text, tea, &stems_beside(file));
				let lists: Vec<PathBuf> = stores
					.into_iter()
					.filter(|s| s.section == 'M')
					.map(|s| file.with_file_name(format!("{}.rod", s.stem)))
					.collect();
				if lists.is_empty() {
					match unread {
						Some(Unread::Shifted) => keep(UnitMeasurements::Shifted {
							file: file.clone(),
							section: INC_SECTION.to_string(),
						}),
						Some(Unread::Missing) => keep(UnitMeasurements::Unresolved { file: file.clone() }),
						None => {}
					}
				}
				for store in lists {
					match list_of(&store, file, Some(&store), cache, crack, registry) {
						Listed::Found(found) => return found,
						Listed::Failed(answer) => keep(answer),
						Listed::Absent => keep(UnitMeasurements::Unresolved { file: file.clone() }),
					}
				}
			}
			SectionRead::Locked => keep(UnitMeasurements::Locked {
				file: file.clone(),
				section: INC_SECTION.to_string(),
			}),
			SectionRead::Shifted => keep(UnitMeasurements::Shifted {
				file: file.clone(),
				section: INC_SECTION.to_string(),
			}),
			SectionRead::Absent => {}
		}
	}
	why.unwrap_or(UnitMeasurements::NoList {
		candidates: candidates.len(),
	})
}

/// The outcome of reading one `[MWB]` list.
enum Listed {
	Found(UnitMeasurements),
	Failed(UnitMeasurements),
	Absent,
}

/// Read the `[MWB]` list in `path`, which is `unit`'s own file or a store it includes.
fn list_of(path: &Path, unit: &Path, store: Option<&Path>, cache: &mut IvCache, crack: bool, registry: &MeasurementRegistry) -> Listed {
	match read_section(path, MWB_SECTION, cache, crack) {
		SectionRead::Text { text, tea } => {
			let rows = UnitRows::parse(&text, registry.len(), tea);
			match rows.outside {
				0 => Listed::Found(UnitMeasurements::Found {
					file: unit.to_path_buf(),
					store: store.map(Path::to_path_buf),
					rows,
				}),
				outside => Listed::Failed(UnitMeasurements::Mismatched {
					file: path.to_path_buf(),
					listed: rows.rows.len() + outside,
					outside,
				}),
			}
		}
		SectionRead::Locked => Listed::Failed(UnitMeasurements::Locked {
			file: path.to_path_buf(),
			section: MWB_SECTION.to_string(),
		}),
		SectionRead::Shifted => Listed::Failed(UnitMeasurements::Shifted {
			file: path.to_path_buf(),
			section: MWB_SECTION.to_string(),
		}),
		SectionRead::Absent => Listed::Absent,
	}
}

/// A store a unit includes: a file of its install, and which of its sections
/// the unit takes from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Included {
	/// The store's file stem, e.g. `IV_EV_Unit_VW48_M13`.
	pub stem: String,
	/// The section it supplies, by the letter its stem ends in: `M` measurements
	/// (`[MWB]`), `D` faults (`[DTC]`), `A` adaptations, `G`, `S`, `F`.
	pub section: char,
}

/// Why an `[INC]` row named no store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unread {
	/// It names a store this install does not have, or its first row did not
	/// repair against the install's file names.
	Missing,
	/// A plain-TEA first row whose key's first three digits did not survive.
	/// Those bytes come from the tag alone and a nonzero `product` never reaches
	/// them; only the shifted regime's per-file mask does, so the file is
	/// shifted, and only VCDS's runtime key reads that row.
	Shifted,
}

/// Read a unit's decoded `[INC]`: one `<key>,<store>` row per included store,
/// the store's name written in the alphabet the key generates.
///
/// A plain-TEA section's first eight bytes read wrong when its per-record
/// `product` is nonzero — the key's last three digits, the comma and the name's
/// first letter. The rest of that row is exact, so the row is repaired against
/// the one list that can confirm it, the install's own file names: every key
/// the three surviving digits allow is tried, and the row is kept only when
/// exactly one existing file answers. The second value is why a row named no
/// store, when one did not — a shifted first row before anything else.
fn included(text: &str, tea: bool, stems: &BTreeSet<String>) -> (Vec<Included>, Option<Unread>) {
	let mut out = Vec::new();
	let mut unread = None;
	for (index, line) in text.split("\r\n").filter(|line| !line.is_empty()).enumerate() {
		let direct = line.split_once(',').and_then(|(key, name)| {
			let key = key.parse::<u32>().ok()?;
			let stem = TableAlphabet::for_key(key).decode(name)?;
			stems.contains(&stem).then_some(stem)
		});
		let stem = match direct {
			Some(stem) => Some(stem),
			None if tea && index == 0 => repair_first(line, stems),
			None => None,
		};
		match stem.and_then(|stem| Some((stem.rsplit('_').next()?.chars().next()?, stem))) {
			Some((section, stem)) => out.push(Included { stem, section }),
			None if tea && index == 0 && !line.chars().take(3).all(|c| c.is_ascii_digit()) => unread = Some(Unread::Shifted),
			None => {
				unread.get_or_insert(Unread::Missing);
			}
		}
	}
	(out, unread)
}

/// Repair a TEA section's first `<key>,<store>` row against the install's file
/// names. `None` unless exactly one file answers.
fn repair_first(line: &str, stems: &BTreeSet<String>) -> Option<String> {
	let chars: Vec<char> = line.chars().collect();
	let prefix: u32 = chars.get(..3)?.iter().collect::<String>().parse().ok()?;
	let tail: String = chars.get(8..)?.iter().collect();
	let mut answers = BTreeSet::new();
	for last in 0..1000 {
		let Some(read) = TableAlphabet::for_key(prefix * 1000 + last).decode(&tail) else {
			continue;
		};
		answers.extend(stems.iter().filter(|stem| stem.get(1..) == Some(read.as_str())).cloned());
	}
	match answers.len() {
		1 => answers.into_iter().next(),
		_ => None,
	}
}

/// The `.rod` stems in the directory holding `path`.
fn stems_beside(path: &Path) -> BTreeSet<String> {
	let Some(dir) = path.parent() else { return BTreeSet::new() };
	let Ok(entries) = std::fs::read_dir(dir) else { return BTreeSet::new() };
	entries
		.flatten()
		.map(|entry| entry.path())
		.filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("rod")))
		.filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_string))
		.collect()
}

/// One section of one file, as far as it opened.
enum SectionRead {
	/// The text, and whether it was plain TEA (whose first eight bytes may be wrong).
	Text { text: String, tea: bool },
	/// A classic section whose key is not cached.
	Locked,
	/// A shifted section.
	Shifted,
	/// No such section, or no such file.
	Absent,
}

/// Open `tag` in `path` with the keys in `cache`, first searching for its key
/// when `crack` is set and the section is classic.
fn read_section(path: &Path, tag: &str, cache: &mut IvCache, crack: bool) -> SectionRead {
	let Ok(bytes) = std::fs::read(path) else { return SectionRead::Absent };
	let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
	if crack {
		recover_classic_key(&bytes, name, tag, cache);
	}
	for section in decode_rod_recover(&bytes, name, cache, false) {
		if section.tag != tag {
			continue;
		}
		return match (section.status, section.text) {
			(RodStatus::Tea, Some(text)) => SectionRead::Text { text, tea: true },
			(RodStatus::Zlib, Some(text)) => SectionRead::Text { text, tea: false },
			(RodStatus::SearchDeclined, _) => SectionRead::Shifted,
			_ => match key_cost(&bytes, tag) {
				Some(KeyCost::AnchorSweep) => SectionRead::Shifted,
				_ => SectionRead::Locked,
			},
		};
	}
	SectionRead::Absent
}

/// The words registry rows are shown with: `TTTEXT`'s records, by id.
///
/// A row names two records: its key, the measurement, whose tail carries the
/// ODX id (`IDE#####`, `MAS#####`); and `f10`, the name VCDS shows — the
/// `ENG######` of its logs.
#[derive(Debug, Clone, Default)]
pub struct Words {
	by_id: std::collections::HashMap<u32, (String, Option<String>)>,
}

impl Words {
	/// Index a read `TTTEXT` table.
	pub fn from_table(table: &crate::tttext::Table) -> Self {
		Self {
			by_id: table
				.texts
				.iter()
				.map(|t| (t.id, (t.name.clone(), t.tail.as_ref().and_then(|tail| tail.odx_id()))))
				.collect(),
		}
	}

	/// A record's text.
	pub fn name(&self, id: u32) -> Option<&str> {
		self.by_id.get(&id).map(|(name, _)| name.as_str())
	}

	/// The ODX id a record is the name of, when its tail says.
	pub fn odx_id(&self, id: u32) -> Option<&str> {
		self.by_id.get(&id).and_then(|(_, odx)| odx.as_deref())
	}
}

/// Why a registry row is not a channel the cache can hold.
#[derive(Debug, Clone, PartialEq)]
pub enum NotAChannel {
	/// A kind the cache has no scaling for: an OBD-II formula, raw bytes, ASCII,
	/// or a type not decoded.
	Kind(Kind),
	/// A text-table row whose table is not in `TTDOP.rod`.
	NoTable(u32),
	/// A text-table row none of whose states has words.
	NoStates(u32),
}

/// A registry row as the cache holds a channel, or why it is not one.
///
/// Named by `f10` — what VCDS shows — else by its key, else by its ODX id, else
/// by its identifier; the last two are labels, not guesses. A text-table state
/// with no words is left out, so it reads as unknown rather than as a number
/// dressed as a state.
pub fn to_reading(
	row: &RegistryRow,
	words: &Words,
	units: &crate::unit_strings::UnitStrings,
	tables: &crate::ttdop::TextTables,
) -> Result<crate::odis::Reading, NotAChannel> {
	// The unit a scaling brings with it, for a row whose own unit id names none.
	let mut unit_of_scaling = None;
	let scaling = match &row.kind {
		Kind::Linear { factor, offset } => crate::Scaling::Linear(crate::LinearScale {
			factor: *factor,
			offset: *offset,
		}),
		// A type-4 row is an OBD-II mode-01 parameter at its UDS mirror
		// `F400 + PID`, converted by SAE J1979's public formula: protocol, not
		// this car, and already in `crate::obd`. Taken only where the row's own
		// layout is the one J1979 gives the parameter; VCDS writes a unit's own
		// non-standard `F4xx` mirror as type 0, not 4.
		Kind::ObdFormula => {
			let standard = (row.did >> 8 == 0xF4)
				.then(|| crate::obd::pid((row.did & 0xFF) as u8))
				.flatten()
				.filter(|pid| crate::RawForm::for_field(row.bit_offset, row.bit_length, row.signed, row.big_endian) == Some(pid.form))
				.ok_or(NotAChannel::Kind(Kind::ObdFormula))?;
			unit_of_scaling = Some(standard.unit);
			crate::Scaling::Linear(crate::LinearScale {
				factor: standard.factor,
				offset: standard.offset,
			})
		}
		Kind::Enum { table } => {
			let states = tables.table(*table).ok_or(NotAChannel::NoTable(*table))?;
			let levels: Vec<crate::Level> = states
				.iter()
				.filter_map(|state| {
					let (lower, upper) = (i32::try_from(state.lower).ok()?, i32::try_from(state.upper).ok()?);
					Some(crate::Level::range(lower, upper, words.name(state.text_id)?))
				})
				.collect();
			if levels.is_empty() {
				return Err(NotAChannel::NoStates(*table));
			}
			crate::Scaling::Enum { levels }
		}
		other => return Err(NotAChannel::Kind(other.clone())),
	};
	// The parameter's own ODX id is the tail of `f10`'s record; the key's tail
	// names the whole identifier's response. A DID packing several fields has
	// one key and a name per field — on the reference ESC, `1822`'s key names
	// the whole response (`IDE02271`) and `f10` names one field (`IDE03660`) —
	// and ODIS keys a channel by the field's id.
	let odx = row.name_id.and_then(|id| words.odx_id(id)).or_else(|| words.odx_id(row.key));
	let name = row
		.name_id
		.and_then(|id| words.name(id))
		.or_else(|| words.name(row.key))
		.map(str::to_string)
		.or_else(|| odx.map(str::to_string))
		.unwrap_or_else(|| format!("{:04X}", row.did));
	Ok(crate::odis::Reading {
		did: row.did,
		name,
		unit: row
			.unit_id
			.and_then(|id| units.get(id))
			.or(unit_of_scaling)
			.filter(|unit| !unit.is_empty())
			.map(str::to_string),
		bit_offset: row.bit_offset,
		bit_length: row.bit_length,
		signed: row.signed,
		big_endian: row.big_endian,
		scaling,
		text_id: odx.map(str::to_string),
	})
}

/// What opening the registry came to.
#[derive(Debug, Clone)]
pub enum RegistryLoad {
	Found {
		file: PathBuf,
		registry: MeasurementRegistry,
	},
	/// No `RM.rod` in the install.
	NoFile,
	/// `RM.rod`'s key is not cached: one search, about 82 CPU-s on 26.3.
	Locked {
		file: PathBuf,
	},
	/// `RM.rod` is shifted in this install. It was classic in every install
	/// checked; this arm exists so a future one says so rather than looks empty.
	Shifted {
		file: PathBuf,
	},
}

/// Open the global measurement registry of a VCDS install.
pub fn load_registry(root: &Path, cache: &mut IvCache, crack: bool) -> RegistryLoad {
	let Some(file) = find_named(root, REGISTRY_FILE) else {
		return RegistryLoad::NoFile;
	};
	match read_section(&file, MWB_SECTION, cache, crack) {
		SectionRead::Text { text, .. } => RegistryLoad::Found {
			registry: MeasurementRegistry::parse(&text),
			file,
		},
		SectionRead::Shifted => RegistryLoad::Shifted { file },
		SectionRead::Locked | SectionRead::Absent => RegistryLoad::Locked { file },
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::rod::testkit::section;

	/// A registry line: the key in plaintext, the fields in its alphabet.
	fn line(key: u32, fields: &str) -> String {
		format!("{key:06},{}", TableAlphabet::for_key(key).encipher(fields))
	}

	fn read(key: u32, fields: &str) -> Result<RegistryRow, RowError> {
		let text = line(key, fields);
		let registry = MeasurementRegistry::parse(&text);
		read_row(&registry.row(1).unwrap())
	}

	// The rows below are invented, and so are their keys, unit ids, tables and
	// records: the registry is Ross-Tech's. What a real row reads as is the
	// gated test at the end, against the install on this machine.

	#[test]
	fn an_identity_row_reads_times_one_with_its_flags_layout_unit_and_name() {
		// The shape of the reference gearbox's input shaft speed: 194 = type 2,
		// little-endian, signed.
		let m = read(930_011, "19489,,194,,,,901,0,0,16,930012,80,0010").unwrap();
		assert_eq!(
			m,
			RegistryRow {
				key: 930_011,
				did: 0x4C21,
				bit_offset: 0,
				bit_length: 16,
				signed: true,
				big_endian: false,
				kind: Kind::Linear { factor: 1.0, offset: 0.0 },
				unit_id: Some(901),
				name_id: Some(930_012),
			}
		);
	}

	#[test]
	fn a_linear_row_is_numerator_over_denominator_with_the_offset_divided_too() {
		let m = read(930_021, "19506,,64,0,1,100,902,0,0,16,930022,80,0010").unwrap();
		assert_eq!(m.kind, Kind::Linear { factor: 0.01, offset: 0.0 });
		assert!(!m.big_endian && !m.signed);
		let m = read(4242, "61445,,0,-40,1,1,3,0,0,8,,8081BF,0033").unwrap();
		assert_eq!(m.kind, Kind::Linear { factor: 1.0, offset: -40.0 });
		let m = read(4243, "8233,2,0,20,1,4,4,1,2,16,,8083B0,0010").unwrap();
		assert_eq!(m.kind, Kind::Linear { factor: 0.25, offset: 5.0 });
		assert_eq!(m.bit_offset, 10, "byte 1, bit 2");
		assert!(m.big_endian);
	}

	#[test]
	fn a_text_table_row_names_its_table_and_other_types_are_named_not_guessed() {
		assert_eq!(
			read(930_031, "19523,6051,3,,,,,0,0,8,930032,80,0033").unwrap().kind,
			Kind::Enum { table: 6051 }
		);
		assert_eq!(read(1, "4096,,4,,,,,0,0,8,,80,0033").unwrap().kind, Kind::ObdFormula);
		assert_eq!(read(2, "4097,,7,,,,,0,0,32,,80,0033").unwrap().kind, Kind::Raw);
		assert_eq!(read(3, "4098,,8,,,,,0,0,136,,80,0033").unwrap().kind, Kind::Ascii);
		assert_eq!(read(4, "8200,7,6,,,,,0,0,16,,80,0010").unwrap().kind, Kind::Undecoded(6));
		// A type this reader does not decode may leave its layout empty. The row
		// is still a row of that type, not one that failed to read.
		assert_eq!(read(10, "8200,,12,,,,,0,0,,,80,0010").unwrap_err(), RowError::Undecoded(12));
		// Twelve fields is the other shape a row comes in.
		assert!(read(5, "8200,,2,,,,,0,0,16,,0010").is_ok());
	}

	#[test]
	fn a_row_that_cannot_be_read_says_why() {
		assert_eq!(read(6, "8200,,2,,,,").unwrap_err(), RowError::Shape { fields: 7 });
		assert_eq!(read(7, ",,2,,,,,0,0,12,,80,0010").unwrap_err(), RowError::NoDid);
		assert_eq!(
			read(8, "8200,,0,0,1,0,1,0,0,16,,80,0010").unwrap_err(),
			RowError::Scaling,
			"divides by zero"
		);
		assert_eq!(
			read(9, "8200,,0,,,,1,0,0,16,,80,0010").unwrap_err(),
			RowError::Scaling,
			"a linear row with no scaling"
		);
		assert_eq!(read(10, "8200,,3,,,,,0,0,8,,80,0033").unwrap_err(), RowError::NoTable);
		assert_eq!(read(11, "70000,,2,,,,,0,0,16,,80,0010").unwrap_err(), RowError::Number { field: 0 });
		assert_eq!(read(12, "8200,,2,,,,,,0,16,,80,0010").unwrap_err(), RowError::Number { field: 7 });
	}

	// --- a row as a channel --------------------------------------------------

	/// A `TTTEXT` record for a test: id, text, and the tail's kind and value.
	type Record<'a> = (u32, &'a str, Option<(&'a str, &'a str)>);

	fn words(texts: &[Record<'_>]) -> Words {
		Words::from_table(&crate::tttext::Table {
			texts: texts
				.iter()
				.map(|(id, name, tail)| crate::tttext::Text {
					id: *id,
					name: name.to_string(),
					tail: tail.map(|(kind, value)| crate::tttext::Tail {
						kind: kind.to_string(),
						value: value.to_string(),
					}),
				})
				.collect(),
			..Default::default()
		})
	}

	fn units() -> crate::unit_strings::UnitStrings {
		let line = |id: u32, unit: &str| format!("{id:06},{}\r\n", TableAlphabet::for_key(id).encipher(unit));
		crate::unit_strings::UnitStrings::parse((line(901, "/min") + &line(902, "km/h")).as_bytes(), crate::codes::CodePage::Windows1252)
	}

	fn tables() -> crate::ttdop::TextTables {
		let row = |table: u32, fields: &str| format!("{table:06},{}", TableAlphabet::for_key(table).encipher(fields));
		crate::ttdop::TextTables::parse(
			&[
				row(6051, "1,1,7001,"),
				row(6051, "5,5,7002,"),
				row(6051, "6,6,7999,"),
				row(9, "0,0,7999,"),
			]
			.join("\r\n"),
		)
	}

	#[test]
	fn a_linear_row_becomes_a_channel_named_as_vcds_shows_it_with_its_odx_id_and_unit() {
		let words = words(&[
			(930_011, "Invented shaft speed", Some(("2", "90011"))),
			(930_012, "Invented Shaft Speed Probe", None),
		]);
		let row = read(930_011, "19489,,194,,,,901,0,0,16,930012,80,0010").unwrap();
		let reading = to_reading(&row, &words, &units(), &tables()).unwrap();
		assert_eq!(reading.name, "Invented Shaft Speed Probe", "f10, what VCDS shows");
		assert_eq!(reading.text_id.as_deref(), Some("IDE90011"), "the key's tail");
		assert_eq!(reading.unit.as_deref(), Some("/min"));
		assert_eq!(
			(reading.did, reading.bit_length, reading.signed, reading.big_endian),
			(0x4C21, 16, true, false)
		);
		assert_eq!(reading.scaling, crate::Scaling::Linear(crate::LinearScale { factor: 1.0, offset: 0.0 }));
	}

	#[test]
	fn a_fields_odx_id_is_its_names_tail_before_the_identifiers() {
		// As on the reference ESC's `1822`: the key names the whole response, `f10` the field.
		let named = words(&[
			(930_041, "Invented response status", Some(("2", "90041"))),
			(930_042, "Invented field display", Some(("2", "90042"))),
		]);
		let row = read(930_041, "19250,4,3,,,,,2,0,2,930042,80,0033").unwrap();
		let tables = crate::ttdop::TextTables::parse(&format!("000004,{}", TableAlphabet::for_key(4).encipher("0,0,930042,")));
		assert_eq!(to_reading(&row, &named, &units(), &tables).unwrap().text_id.as_deref(), Some("IDE90042"));
		// With no tail on `f10`'s record, the key's id is what there is.
		let unnamed = words(&[
			(930_041, "Invented response status", Some(("2", "90041"))),
			(930_042, "Invented field display", None),
		]);
		assert_eq!(
			to_reading(&row, &unnamed, &units(), &tables).unwrap().text_id.as_deref(),
			Some("IDE90041")
		);
	}

	#[test]
	fn a_rows_name_falls_back_to_its_key_then_its_identifier() {
		let row = read(930_051, "19540,,64,0,1,100,902,0,0,16,,80,0010").unwrap();
		let named = |words: &Words| to_reading(&row, words, &units(), &tables()).unwrap().name;
		assert_eq!(
			named(&words(&[(930_051, "Invented road speed", Some(("2", "90051")))])),
			"Invented road speed"
		);
		assert_eq!(named(&words(&[])), "4C54");
	}

	#[test]
	fn a_text_table_row_keeps_the_states_that_have_words_and_nothing_else() {
		let words = words(&[(7001, "Invented state one", None), (7002, "Invented state two", None)]);
		let row = read(930_031, "19523,6051,3,,,,,0,0,8,930032,80,0033").unwrap();
		let crate::Scaling::Enum { levels } = to_reading(&row, &words, &units(), &tables()).unwrap().scaling else {
			panic!()
		};
		let states: Vec<(i32, &str)> = levels.iter().map(|l| (l.lower(), l.name())).collect();
		assert_eq!(
			states,
			[(1, "Invented state one"), (5, "Invented state two")],
			"6 has no words and reads as unknown"
		);

		let row = read(1, "4096,9,3,,,,,0,0,8,,80,0033").unwrap();
		assert_eq!(to_reading(&row, &words, &units(), &tables()).unwrap_err(), NotAChannel::NoStates(9));
		let row = read(1, "4096,77,3,,,,,0,0,8,,80,0033").unwrap();
		assert_eq!(to_reading(&row, &words, &units(), &tables()).unwrap_err(), NotAChannel::NoTable(77));
	}

	#[test]
	fn an_obd_formula_row_takes_the_standards_conversion_where_its_layout_is_the_standards() {
		// PID 05, coolant: J1979's `A - 40`, one byte.
		let row = read(3, "62469,,4,,,,,0,0,8,,80,0033").unwrap();
		let reading = to_reading(&row, &Words::default(), &units(), &tables()).unwrap();
		assert_eq!(reading.scaling, crate::Scaling::Linear(crate::LinearScale { factor: 1.0, offset: -40.0 }));
		assert_eq!(reading.unit.as_deref(), Some("°C"), "the standard's unit where the row names none");
		// The same parameter at a width J1979 does not give it is not taken.
		let row = read(3, "62469,,4,,,,,0,0,16,,80,0010").unwrap();
		assert_eq!(
			to_reading(&row, &Words::default(), &units(), &tables()).unwrap_err(),
			NotAChannel::Kind(Kind::ObdFormula)
		);
		// Nor is a type-4 row outside the OBD mirror.
		let row = read(3, "8200,,4,,,,,0,0,8,,80,0033").unwrap();
		assert_eq!(
			to_reading(&row, &Words::default(), &units(), &tables()).unwrap_err(),
			NotAChannel::Kind(Kind::ObdFormula)
		);
	}

	#[test]
	fn a_kind_the_cache_has_no_scaling_for_is_not_a_channel() {
		let row = read(2, "4097,,7,,,,,0,0,32,,80,0033").unwrap();
		assert_eq!(
			to_reading(&row, &Words::default(), &units(), &tables()).unwrap_err(),
			NotAChannel::Kind(Kind::Raw)
		);
	}

	// --- the chain through files --------------------------------------------

	/// Registry rows whose neighbours differ, so a list read one row off lands
	/// on the wrong identifier rather than a copy of the right one.
	const REGISTRY_ROWS: &[(u32, &str)] = &[
		(501, "4097,,0,0,1,1,1,0,0,8,,80,0033"),
		(502, "4098,,0,0,1,10,1,0,0,8,,80,0033"),
		(503, "4099,,0,0,1,100,1,0,0,8,,80,0033"),
		(504, "4100,,2,,,,1,0,0,16,,80,0010"),
		(505, "4101,,66,,,,1,0,0,16,,80,0010"),
		(506, "4102,,0,-40,1,1,3,0,0,8,,80,0033"),
	];

	/// A throwaway VCDS install under the system temp dir, removed when dropped.
	struct Install(PathBuf);

	impl Install {
		fn new(name: &str) -> Self {
			let root = std::env::temp_dir().join(format!("vagcan-registry-{name}-{}", std::process::id()));
			let _ = std::fs::remove_dir_all(&root);
			std::fs::create_dir_all(root.join("UDS_EV")).unwrap();
			let registry: Vec<String> = REGISTRY_ROWS.iter().map(|(key, fields)| line(*key, fields)).collect();
			let install = Self(root);
			install.file("RM.rod", &[section("MWB", registry.join("\r\n").as_bytes(), true, None)]);
			install
		}

		fn file(&self, name: &str, sections: &[Vec<u8>]) {
			std::fs::write(self.0.join("UDS_EV").join(name), sections.concat()).unwrap();
		}

		fn registry(&self) -> MeasurementRegistry {
			match load_registry(&self.0, &mut IvCache::default(), false) {
				RegistryLoad::Found { registry, .. } => registry,
				other => panic!("registry did not open: {other:?}"),
			}
		}

		fn lookup(&self, odx: &str, version: &str) -> UnitMeasurements {
			unit_measurements(&self.0, odx, version, &mut IvCache::default(), false, &self.registry())
		}
	}

	impl Drop for Install {
		fn drop(&mut self) {
			let _ = std::fs::remove_dir_all(&self.0);
		}
	}

	/// A unit's `[MWB]` list: 1-based rows, each with a two-glyph code.
	fn list(rows: &[usize]) -> Vec<u8> {
		rows.iter().map(|row| format!("{row:06},00\r\n")).collect::<String>().into_bytes()
	}

	fn dids(install: &Install, found: &UnitMeasurements) -> Vec<u16> {
		let UnitMeasurements::Found { rows, .. } = found else {
			panic!("not found: {found:?}")
		};
		let registry = install.registry();
		rows.rows.iter().map(|row| read_row(&registry.row(*row).unwrap()).unwrap().did).collect()
	}

	#[test]
	fn a_units_own_list_reads_its_rows_counted_from_one() {
		let install = Install::new("own");
		install.file("EV_UnitA_001.rod", &[section("MWB", &list(&[3, 5]), true, None)]);
		let found = install.lookup("EV_UnitA", "001017");
		assert_eq!(dids(&install, &found), vec![4099, 4101], "rows 3 and 5, not 2 and 4 (4098, 4100)");
		let UnitMeasurements::Found { store, rows, .. } = found else {
			unreachable!()
		};
		assert_eq!(store, None);
		assert!(!rows.first_row_dropped);
	}

	#[test]
	fn a_list_held_in_an_included_store_is_followed_and_a_damaged_first_row_repaired() {
		let install = Install::new("inc");
		install.file("IV_EV_UnitB_M1.rod", &[section("MWB", &list(&[1, 6]), true, None)]);
		install.file("IV_EV_UnitB_D2.rod", &[section("DTC", b"000001,00\r\n", true, None)]);
		// A plain-TEA `[INC]` under a nonzero product: its first eight bytes
		// decode wrong, which is the case the repair exists for.
		let inc = format!("{}\r\n{}\r\n", line(123_456, "IV_EV_UnitB_M1"), line(654_321, "IV_EV_UnitB_D2"));
		install.file("EV_UnitB.rod", &[section("INC", inc.as_bytes(), false, Some([9, 8, 7, 6, 5]))]);
		let found = install.lookup("EV_UnitB", "");
		assert_eq!(dids(&install, &found), vec![4097, 4102]);
		let UnitMeasurements::Found { store, .. } = found else { unreachable!() };
		assert_eq!(store.unwrap().file_name().unwrap(), "IV_EV_UnitB_M1.rod");
	}

	#[test]
	fn the_included_section_letter_is_read_off_the_store_name() {
		let stems: BTreeSet<String> = ["IV_EV_X_M13", "IV_EV_X_D2", "IV_EV_Y_AU27_M"].map(str::to_string).into();
		let text = format!(
			"{}\r\n{}\r\n{}\r\n",
			line(1, "IV_EV_X_M13"),
			line(2, "IV_EV_X_D2"),
			line(3, "IV_EV_Y_AU27_M")
		);
		let (stores, unread) = included(&text, false, &stems);
		assert_eq!(unread, None);
		assert_eq!(stores.iter().map(|s| s.section).collect::<String>(), "MDM");
		let (stores, unread) = included(&format!("{}\r\n", line(4, "IV_EV_GONE_M1")), false, &stems);
		assert!(
			stores.is_empty() && unread == Some(Unread::Missing),
			"a store the install lacks is unresolved, not skipped silently"
		);
	}

	#[test]
	fn an_included_first_row_whose_key_did_not_survive_is_shifted_not_missing() {
		// A nonzero `product` spoils the key's last three digits and never its
		// first three; a first row that lost those is the shifted regime's mask,
		// and telling the person a store is missing would send them looking for a
		// file that is there.
		let install = Install::new("inc-shifted");
		install.file("IV_EV_UnitS_M1.rod", &[section("MWB", &list(&[1]), true, None)]);
		let inc = format!("\u{1}\u{7}\u{2}456,Q{}\r\n", &line(123_456, "IV_EV_UnitS_M1")[8..]);
		install.file("EV_UnitS.rod", &[section("INC", inc.as_bytes(), false, None)]);
		assert!(matches!(install.lookup("EV_UnitS", ""), UnitMeasurements::Shifted { section, .. } if section == "INC"));

		// Whereas a first row that kept its digits and names nothing here is a
		// store the install lacks.
		let inc = format!("{}\r\n", line(123_456, "IV_EV_Gone_M1"));
		install.file("EV_UnitG.rod", &[section("INC", inc.as_bytes(), false, None)]);
		assert!(matches!(install.lookup("EV_UnitG", ""), UnitMeasurements::Unresolved { .. }));
	}

	#[test]
	fn a_plain_tea_lists_first_row_is_dropped_not_guessed() {
		let install = Install::new("tea");
		install.file("EV_UnitT.rod", &[section("MWB", &list(&[2, 3, 4]), false, Some([1, 2, 3, 4, 5]))]);
		let found = install.lookup("EV_UnitT", "");
		assert_eq!(dids(&install, &found), vec![4099, 4100]);
		let UnitMeasurements::Found { rows, .. } = found else { unreachable!() };
		assert!(rows.first_row_dropped);
	}

	#[test]
	fn each_way_a_list_fails_to_open_is_answered_apart() {
		let install = Install::new("fail");
		assert_eq!(install.lookup("EV_Nowhere", ""), UnitMeasurements::NoFile);

		install.file("EV_Blocked.rod", &[section("MWB", &list(&[1]), true, Some([1, 2, 3, 4, 5]))]);
		assert!(matches!(install.lookup("EV_Blocked", ""), UnitMeasurements::Locked { section, .. } if section == "MWB"));

		// A shifted section: the file's mask moves the first block's IV, so its
		// first two bytes no longer read as the zlib header.
		let mut shifted = section("MWB", &list(&[1]), true, None);
		let body = shifted.iter().position(|&b| b == b'\n').unwrap() + 1 + 6;
		shifted[body] ^= 0x5a;
		install.file("EV_Shifted.rod", &[shifted]);
		assert!(matches!(install.lookup("EV_Shifted", ""), UnitMeasurements::Shifted { section, .. } if section == "MWB"));

		install.file("EV_Elsewhere.rod", &[section("MWB", &list(&[2, 99]), true, None)]);
		assert!(matches!(
			install.lookup("EV_Elsewhere", ""),
			UnitMeasurements::Mismatched { listed: 2, outside: 1, .. }
		));

		install.file("EV_Silent.rod", &[section("DTC", b"000001,00\r\n", true, None)]);
		assert_eq!(install.lookup("EV_Silent", ""), UnitMeasurements::NoList { candidates: 1 });
	}

	#[test]
	fn a_classic_key_is_recovered_for_one_section_and_a_shifted_one_is_never_searched() {
		let open = section("MWB", &list(&[1]), true, None);
		assert!(
			recover_classic_key(&open, "EV_Open.rod", "MWB", &mut IvCache::default()),
			"not blocked: opens as it is"
		);
		assert!(!recover_classic_key(&open, "EV_Open.rod", "INC", &mut IvCache::default()), "absent");

		let mut shifted = open.clone();
		let body = shifted.iter().position(|&b| b == b'\n').unwrap() + 1 + 6;
		shifted[body] ^= 0x5a;
		let started = std::time::Instant::now();
		assert!(!recover_classic_key(&shifted, "EV_Shifted.rod", "MWB", &mut IvCache::default()));
		assert!(started.elapsed() < std::time::Duration::from_secs(1), "declined, not searched");

		// The cache holds the section's IV bytes 3..8, not the product that made them.
		let product = [1, 2, 3, 4, 5];
		let key: [u8; 5] = crate::rod::testkit::iv_for_product(b"MWB", product)[3..8].try_into().unwrap();
		let mut cache = IvCache::default();
		cache.insert("EV_Blocked.rod", "MWB", key);
		let blocked = section("MWB", &list(&[1]), true, Some(product));
		assert!(
			recover_classic_key(&blocked, "EV_Blocked.rod", "MWB", &mut cache),
			"the key is already cached"
		);
	}

	// --- the reference car, when this machine has its private data ----------

	/// The reference car's VCDS install, its section keys and its proven rows,
	/// or why not. None of it is in the repository: the install is Ross-Tech's
	/// (`~/vcds-en`, VCDS 26.3), the keys are what `setup` or `dev vcds rod`
	/// recovered (`$VAGCAN_ROD_KEYS`, else the project's `rod-keys.json`), and
	/// the rows are one owner's drives (`~/.vagcan/data/SK37X/measurements`).
	fn reference_car() -> Result<(PathBuf, IvCache, PathBuf), String> {
		let home = PathBuf::from(std::env::var("HOME").map_err(|_| "no $HOME".to_string())?);
		let root = home.join("vcds-en");
		let project = home.join(".vagcan/data/SK37X");
		let keys = std::env::var("VAGCAN_ROD_KEYS")
			.map(PathBuf::from)
			.unwrap_or_else(|_| project.join("rod-keys.json"));
		let rows = project.join("measurements");
		for (what, path) in [("a VCDS install", &root), ("a key cache", &keys), ("proven rows", &rows)] {
			if !path.exists() {
				return Err(format!("no {what} at {}", path.display()));
			}
		}
		Ok((root, IvCache::load(&keys), rows))
	}

	/// Give up on a test that needs the reference car's private data.
	macro_rules! need_car {
		($opened:expr) => {
			match $opened {
				Ok(opened) => opened,
				Err(why) => {
					eprintln!("skipped: {why}");
					return;
				}
			}
		};
	}

	fn scales_alike(kind: &Kind, scaling: &crate::Scaling) -> bool {
		match (kind, scaling) {
			(Kind::Linear { factor, offset }, crate::Scaling::Linear(s)) => (factor - s.factor).abs() < 1e-9 && (offset - s.offset).abs() < 1e-9,
			(Kind::Enum { .. }, crate::Scaling::Enum { .. }) => true,
			_ => false,
		}
	}

	/// For one unit of the reference car: which proven rows its list carries,
	/// and which of those it carries whole — DID, `RawForm`, scaling — reading
	/// each listed number `shift` rows on from where 1-based numbering puts it
	/// (`shift = 1` is 0-based numbering).
	fn proven_against_list(
		root: &Path,
		cache: &mut IvCache,
		registry: &MeasurementRegistry,
		odx: &str,
		version: &str,
		proven: &Path,
		shift: usize,
	) -> Result<(BTreeSet<u16>, BTreeSet<u16>), String> {
		let listed = match unit_measurements(root, odx, version, cache, false, registry) {
			UnitMeasurements::Found { rows, .. } => rows.rows,
			other => return Err(format!("{odx}'s list did not open: {other:?}")),
		};
		let rows: Vec<RegistryRow> = listed
			.iter()
			.filter_map(|&n| registry.row(n + shift))
			.filter_map(|row| read_row(&row).ok())
			.collect();
		let proven = crate::MeasurementCatalog::from_json(&std::fs::read_to_string(proven).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
		let (mut present, mut whole) = (BTreeSet::new(), BTreeSet::new());
		for def in &proven.defs {
			let crate::ReadId::Uds(did) = def.address;
			let mut same_did = rows.iter().filter(|m| m.did == did).peekable();
			if same_did.peek().is_some() {
				present.insert(did);
			}
			if same_did.any(|m| {
				crate::RawForm::for_field(m.bit_offset, m.bit_length, m.signed, m.big_endian) == Some(def.raw_form) && scales_alike(&m.kind, &def.scaling)
			}) {
				whole.insert(did);
			}
		}
		Ok((present, whole))
	}

	#[test]
	fn the_reference_cars_proven_rows_are_the_rows_its_units_list_counted_from_one() {
		let (root, mut cache, rows) = need_car!(reference_car());
		let registry = match load_registry(&root, &mut cache, false) {
			RegistryLoad::Found { registry, .. } => registry,
			other => {
				eprintln!("skipped: the registry did not open: {other:?}");
				return;
			}
		};
		let mut check =
			|odx: &str, version: &str, file: &str, shift: usize| proven_against_list(&root, &mut cache, &registry, odx, version, &rows.join(file), shift);
		let set = |dids: &[u16]| dids.iter().copied().collect::<BTreeSet<u16>>();

		// The gearbox lists its twelve proven rows in its own file.
		let (present, whole) = need_car!(check("EV_TCMDQ200021", "", "0CW300041G.json", 0));
		assert_eq!(present.len(), 12, "{present:X?}");
		assert_eq!(whole, present, "every proven gearbox row, whole");
		// The engine's list is in a store its [INC] names.
		let (present, whole) = need_car!(check("EV_ECM18TFS0208V0906264H", "", "8V0906264H.json", 0));
		assert_eq!(present, set(&[0x206E, 0x2029, 0x202A]));
		assert_eq!(whole, present, "every proven engine row, whole");
		// The cluster's list, also through [INC], carries three of its eight
		// proven rows: the clock's five are in neither VCDS's list nor ODIS's
		// variant. Of the three, `22D2` is nine bits in VCDS and was read as
		// sixteen on the drive — the open question in label-lookup/02.
		let (present, whole) = need_car!(check("EV_DashBoardVDDMQBAB", "009", "5E0920740D.json", 0));
		assert_eq!(present, set(&[0x2203, 0x22B8, 0x22D2]));
		// `22B8` is type 7 in VCDS, raw bytes, where the drive proved a metre
		// counter — what a proven row outranks VCDS for.
		assert_eq!(whole, set(&[0x2203]));

		// Counted from zero, the gearbox's list lands on its neighbours and loses rows.
		let (_, whole) = need_car!(check("EV_TCMDQ200021", "", "0CW300041G.json", 1));
		assert!(whole.len() < 12, "0-based numbering must not agree as well — it did: {whole:X?}");

		// The gearbox's two proven text tables: every raw value the drive saw
		// named is a state of the table VCDS gives the row.
		let tables = match crate::ttdop::load_text_tables(&root, &mut cache, false) {
			crate::ttdop::TablesLoad::Found { tables, .. } => tables,
			other => {
				eprintln!("skipped the text tables: {other:?}");
				return;
			}
		};
		let gearbox = match unit_measurements(&root, "EV_TCMDQ200021", "", &mut cache, false, &registry) {
			UnitMeasurements::Found { rows, .. } => rows.rows,
			other => panic!("the gearbox's list: {other:?}"),
		};
		let listed: Vec<RegistryRow> = gearbox
			.iter()
			.filter_map(|&n| registry.row(n))
			.filter_map(|row| read_row(&row).ok())
			.collect();
		let proven = crate::MeasurementCatalog::from_json(&std::fs::read_to_string(rows.join("0CW300041G.json")).unwrap()).unwrap();
		// Every proven row's unit is the one VCDS names for the row the gearbox
		// lists: `/min`, `km/h`, `%`, `mm`, and none for a text table.
		let units = match crate::unit_strings::load_unit_strings(&root, &mut cache, false, crate::codes::CodePage::Windows1252) {
			crate::unit_strings::UnitsLoad::Found { units, .. } => units,
			other => {
				eprintln!("skipped the units: {other:?}");
				return;
			}
		};
		for def in &proven.defs {
			let crate::ReadId::Uds(did) = def.address;
			let named: Vec<&str> = listed
				.iter()
				.filter(|m| m.did == did)
				.map(|m| m.unit_id.and_then(|u| units.get(u)).unwrap_or(""))
				.collect();
			assert!(
				named.contains(&def.unit.as_ref()),
				"{did:04X}: proven unit {:?}, VCDS's {named:?}",
				def.unit
			);
		}

		let mut tables_checked = 0;
		for def in &proven.defs {
			let (crate::ReadId::Uds(did), crate::Scaling::Enum { levels }) = (def.address, &def.scaling) else {
				continue;
			};
			let table = listed
				.iter()
				.find_map(|m| match m.kind {
					Kind::Enum { table } if m.did == did => Some(table),
					_ => None,
				})
				.unwrap_or_else(|| panic!("{did:04X} is no text-table row of the gearbox's list"));
			let states = tables
				.table(table)
				.unwrap_or_else(|| panic!("table {table} of {did:04X} is not in TTDOP"));
			for level in levels {
				assert!(
					states
						.iter()
						.any(|s| s.lower <= i64::from(level.lower()) && i64::from(level.upper()) <= s.upper),
					"{did:04X}: raw {} ({}) is no state of table {table}",
					level.lower(),
					level.name()
				);
			}
			tables_checked += 1;
		}
		assert_eq!(tables_checked, 2, "the gear and the selector lever");

		// VCDS's own log names each column by `IDE#####-ENG######`: the ENG is
		// `f10` of a row the unit lists, and the IDE is its key's tail. The
		// pairs are this car's gearbox logs.
		let words = match find_named(&root, "TTTEXT.ROD").and_then(|file| {
			let bytes = std::fs::read(&file).ok()?;
			let text = decode_rod_recover(&bytes, "TTTEXT.ROD", &mut cache, false)
				.into_iter()
				.find(|s| s.tag == "TXT")?
				.text?;
			let raw: Vec<u8> = text.chars().map(|c| c as u32 as u8).collect();
			Some(Words::from_table(&crate::tttext::read(&raw, crate::codes::CodePage::Windows1252)))
		}) {
			Some(words) => words,
			None => {
				eprintln!("skipped the logged pairs: TTTEXT did not open");
				return;
			}
		};
		let logged = [
			(22, 103074),
			(23, 99005),
			(75, 99967),
			(86, 98363),
			(130, 103124),
			(2735, 100209),
			(2794, 120857),
			(2803, 120861),
			(3174, 100415),
			(6244, 120895),
			(6247, 120898),
			(6281, 120909),
			(6282, 120910),
		];
		for (ide, eng) in logged {
			let ide = format!("IDE{ide:05}");
			assert!(
				listed.iter().any(|m| m.name_id == Some(eng) && words.odx_id(m.key) == Some(ide.as_str())),
				"the gearbox lists no row named ENG{eng} whose key is {ide}"
			);
		}
		// The name is Ross-Tech's words and stays out of the repository: what is
		// pinned is where it comes from — `f10`'s record, what VCDS shows — and
		// that it is a name, not the identifier standing in for one.
		let input = listed.iter().find(|m| m.did == 0x380A).unwrap();
		let reading = to_reading(input, &words, &units, &tables).unwrap();
		assert_eq!(input.name_id, Some(103074));
		assert_eq!(Some(reading.name.as_str()), words.name(103074), "380A is named by f10's record");
		assert!(!reading.name.trim().is_empty(), "380A has an empty name");
		assert_ne!(reading.name, format!("{:04X}", input.did), "380A fell back to its identifier");
		assert_eq!(reading.text_id.as_deref(), Some("IDE00022"));
	}
}
