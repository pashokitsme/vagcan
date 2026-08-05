//! Naming a VW fault number, end to end.
//!
//! A VAG control unit answers `0x19` with a **VW-internal 24-bit number**, and
//! `Codes.dat` is keyed by an ISO/SAE DTC. `research/labels/fault-naming-hop.md` is
//! the whole of how the two are joined; this module is that chain as code:
//!
//! ```text
//! raw 24-bit number ──▶ UDS_EV/RD.rod [DTC] table, key = the number in decimal
//!                            │  2 to 50 rows, and nothing inside the file picks one
//! the unit's own .rod ──▶ [DTC] ──▶ <row index>,<code>
//!                            │  index-1 is the row, and its table key is the raw number
//!                            ▼
//!                          f0 ──▶ Codes.dat ──▶ the text
//! ```
//!
//! Three things had to be true and each is checked in the writeup:
//!
//! * the registry's key **is** the raw number in decimal (§2);
//! * a unit `.rod`'s `[DTC]` id is a **1-based row number** into the registry,
//!   not an identifier in any shared space (§10) — 85 ids to 85 rows, 518 to
//!   518, and every fault the reference car had stored covered;
//! * the row's fields are written in a substitution the table's own key
//!   generates ([`crate::glyphs::TableAlphabet`], §11).
//!
//! **What this module will not do is guess.** Every failure has its own
//! variant in [`UnitLookup`] and [`FaultName`] so the caller can say *why* a
//! code went unnamed, and a code whose chain breaks anywhere comes back as a
//! number. `research/labels/fault-naming-hop.md` has stayed at zero wrong answers and
//! that is the property worth keeping.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::glyphs::TableAlphabet;
use crate::rod::{decode_rod_recover, IvCache, RodStatus};

/// The section every file in this chain is read from.
pub const DTC_SECTION: &str = "DTC";

/// The global fault registry's own file name, inside a VCDS install.
pub const REGISTRY_FILE: &str = "RD.rod";

/// How many separators a well-formed registry row has, i.e. one less than its
/// field count. Structural, and used to reject a row rather than misread one.
const FIELDS: usize = 7;

/// `UDS_EV/RD.rod`'s `[DTC]` section: every row of the global fault registry,
/// in file order, because the order is what a unit file points into.
///
/// 236 755 rows in 110 767 tables on the corpus this was built against. The
/// decoded text is kept whole and rows are offsets into it — a `String` per
/// row would be a quarter-million allocations for a lookup that touches a
/// handful.
#[derive(Debug, Clone, Default)]
pub struct DtcRegistry {
    text: String,
    rows: Vec<RowSpan>,
}

#[derive(Debug, Clone, Copy)]
struct RowSpan {
    key: u32,
    start: u32,
    end: u32,
}

/// One row of the registry: the table it belongs to, and its enciphered body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DtcRow<'a> {
    /// The table key — the raw 24-bit fault number, in decimal.
    pub key: u32,
    /// The row's fields, still written in the table's own alphabet.
    pub payload: &'a str,
}

impl DtcRegistry {
    /// Parse a decoded `[DTC]` section.
    ///
    /// Rows are `<key>,<payload>` separated by CRLF, the key in plaintext.
    /// Rows whose key is not a number are kept as rows — the section has two
    /// of them and dropping them would shift every row number after them,
    /// which is the one thing this index cannot survive.
    pub fn parse(text: &str) -> Self {
        let mut rows = Vec::new();
        let mut at = 0usize;
        for line in text.split("\r\n") {
            let start = at;
            at += line.len() + 2;
            if line.is_empty() {
                continue;
            }
            let (key, body) = match line.split_once(',') {
                Some((key, body)) => (key.parse::<u32>().unwrap_or(u32::MAX), body),
                None => (u32::MAX, line),
            };
            let body_at = start + (line.len() - body.len());
            rows.push(RowSpan { key, start: body_at as u32, end: (start + line.len()) as u32 });
        }
        Self { text: text.to_string(), rows }
    }

    /// Read a **1-based** row number, the way a unit file points.
    ///
    /// 1-based is not a convention chosen here: read 0-based, the reference
    /// car's steering column collapses 85 ids onto 79 rows and misses two of
    /// the faults it has stored (§10.1).
    pub fn row(&self, index: usize) -> Option<DtcRow<'_>> {
        let span = self.rows.get(index.checked_sub(1)?)?;
        Some(DtcRow { key: span.key, payload: &self.text[span.start as usize..span.end as usize] })
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// What a row names, once its table's alphabet is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaultName {
    /// The `Codes.dat` key carrying the text.
    pub codes_key: u32,
    /// The failure-type byte VCDS prints beside the SAE code.
    pub failure_type: Option<u8>,
}

/// Read one registry row through the alphabet its table key generates.
///
/// `None` when the row does not have the shape a registry row has — seven
/// fields, a numeric first field. Nothing is inferred from a short row.
pub fn read_row(row: &DtcRow<'_>) -> Option<FaultName> {
    let alphabet = TableAlphabet::for_key(row.key);
    let fields: Vec<&str> = row.payload.split(alphabet.separator()).collect();
    if fields.len() != FIELDS {
        return None;
    }
    let codes_key = u32::try_from(alphabet.number(fields[0])?).ok()?;
    Some(FaultName { codes_key, failure_type: alphabet.hex_byte(fields[1]) })
}

/// One control unit's fault catalogue: which registry row each code it can
/// report names.
///
/// The unit's `.rod` `[DTC]` section is `<row index>,<2-character code>` per
/// line. Keyed here by the registry table key of the row it points at, which
/// **is** the raw fault number — so a code the car reports is looked up
/// directly, with no second index to keep in step.
#[derive(Debug, Clone, Default)]
pub struct UnitCatalogue {
    rows: BTreeMap<u32, usize>,
    ids: usize,
}

impl UnitCatalogue {
    /// Build the catalogue from a unit's decoded `[DTC]` section.
    ///
    /// Ids that fall outside the registry are dropped: a file from a corpus of
    /// a different vintage than `RD.rod` is a real possibility and it must
    /// narrow the catalogue, not corrupt it. How many were kept is what
    /// [`UnitCatalogue::is_consistent_with_registry`] then judges.
    pub fn parse(text: &str, registry: &DtcRegistry) -> Self {
        let mut rows = BTreeMap::new();
        let mut ids = 0usize;
        for line in text.split("\r\n") {
            if line.is_empty() {
                continue;
            }
            let index = line.split(',').next().unwrap_or_default();
            let Ok(index) = index.parse::<usize>() else { continue };
            ids += 1;
            let Some(row) = registry.row(index) else { continue };
            rows.insert(row.key, index);
        }
        Self { rows, ids }
    }

    /// Whether this unit file and this registry are the same vintage.
    ///
    /// **A catalogue is injective or it is wrong.** Every id in a unit's
    /// `[DTC]` names a different fault, so the rows they land on must have
    /// distinct table keys: the reference car's steering column has 85 ids and
    /// 85 keys, its ESP 518 and 518, its power steering 750 and 750. Point the
    /// same file at a registry of another vintage and the rows shift — 85 ids
    /// collapse to 63 keys, 518 to 457 — which is exactly the signature this
    /// rejects.
    ///
    /// It is the same test that settled 1-based against 0-based indexing
    /// (`research/labels/fault-naming-hop.md` §10.1), turned into a runtime guard,
    /// and it is here because a shifted catalogue does not fail to answer: it
    /// answers with another fault's name.
    pub fn is_consistent_with_registry(&self) -> bool {
        self.ids > 0 && self.rows.len() == self.ids
    }

    /// How many ids the unit file listed, before any were dropped.
    pub fn listed(&self) -> usize {
        self.ids
    }

    /// The registry row this unit uses for a raw fault number.
    pub fn row_of(&self, raw: u32) -> Option<usize> {
        self.rows.get(&raw).copied()
    }

    /// How many faults this unit can report at all.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Why a control unit's catalogue could not be opened — or the catalogue.
///
/// Every arm is a different answer to "why is this fault a number and not a
/// name", and the caller is expected to print the reason.
#[derive(Debug, Clone)]
pub enum UnitLookup {
    /// The catalogue, and the file it came from.
    Found { file: PathBuf, catalogue: UnitCatalogue },
    /// No file of that ODX name anywhere in the corpus. On the reference car
    /// this is the body control module, whose `F19E` is `EV_BCMMQB` and which
    /// the English corpus simply does not ship.
    NoFile,
    /// The family is there and no member of it carries a `[DTC]` section.
    /// Within a family exactly one file does, and the variants carry an `INC`
    /// reference to it instead (§10.5) — following that reference is not
    /// implemented, so a family whose `[DTC]` holder is missing ends here.
    NoSection { candidates: usize },
    /// The `[DTC]` section is there and its first-block key is not in the
    /// cache. Recoverable, at about 95 s of every core, by
    /// `vagcan vcds rod --features rod-crack`.
    Locked { file: PathBuf },
    /// The catalogue opened and does not belong to this registry: its ids do
    /// not land on distinct faults
    /// ([`UnitCatalogue::is_consistent_with_registry`]). Refused rather than
    /// used, because a shifted catalogue names the wrong fault instead of no
    /// fault.
    Mismatched { file: PathBuf, listed: usize, distinct: usize },
}

/// Find and open the fault catalogue of a unit that identified itself.
///
/// `odx_name` is the unit's `F19E` and `version` its `F1A2`; both come off the
/// car, which is what keeps file selection a lookup the vehicle answers rather
/// than a table about one vehicle. Candidates are tried best match first
/// ([`crate::corpus::find_rod_by_odx_variant`]) and the first with a readable
/// `[DTC]` section wins — which is also how the one file in a family that
/// carries the section gets picked out, without a list of which one it is.
pub fn unit_catalogue(
    root: &Path,
    odx_name: &str,
    version: &str,
    cache: &IvCache,
    registry: &DtcRegistry,
) -> UnitLookup {
    let candidates =
        crate::corpus::find_rod_by_odx_variant(root, odx_name, version).unwrap_or_default();
    if candidates.is_empty() {
        return UnitLookup::NoFile;
    }
    let (mut locked, mut mismatched) = (None, None);
    for (_, path) in &candidates {
        match section_of(path, cache) {
            Section::Text(text) => {
                let catalogue = UnitCatalogue::parse(&text, registry);
                if catalogue.is_consistent_with_registry() {
                    return UnitLookup::Found { file: path.clone(), catalogue };
                }
                mismatched = mismatched.or(Some(UnitLookup::Mismatched {
                    file: path.clone(),
                    listed: catalogue.listed(),
                    distinct: catalogue.len(),
                }));
            }
            Section::Locked => locked = locked.or_else(|| Some(path.clone())),
            Section::Absent => {}
        }
    }
    if let Some(mismatched) = mismatched {
        return mismatched;
    }
    match locked {
        Some(file) => UnitLookup::Locked { file },
        None => UnitLookup::NoSection { candidates: candidates.len() },
    }
}

enum Section {
    Text(String),
    Locked,
    Absent,
}

/// Decode one file's `[DTC]` section, using the cache and never the search.
///
/// Every `[DTC]` section in this corpus carries a nonzero per-record term, so
/// none of them opens without a recovered key — which is why the cache is the
/// mechanism here and not an optimisation.
fn section_of(path: &Path, cache: &IvCache) -> Section {
    let Ok(bytes) = std::fs::read(path) else { return Section::Absent };
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let mut cache = cache.clone();
    for section in decode_rod_recover(&bytes, name, &mut cache, false) {
        if section.tag != DTC_SECTION {
            continue;
        }
        return match (section.status, section.text) {
            (RodStatus::Tea | RodStatus::Zlib, Some(text)) => Section::Text(text),
            _ => Section::Locked,
        };
    }
    Section::Absent
}

/// Load the global registry from a VCDS installation.
///
/// Returns `None` when `RD.rod` is not under `root` or its `[DTC]` section has
/// no key in the cache — in both cases nothing downstream can name anything,
/// and the caller says so once rather than per fault.
pub fn load_registry(root: &Path, cache: &IvCache) -> Option<(PathBuf, DtcRegistry)> {
    let path = find_named(root, REGISTRY_FILE)?;
    match section_of(&path, cache) {
        Section::Text(text) => Some((path, DtcRegistry::parse(&text))),
        _ => None,
    }
}

/// The first file of this name anywhere under `root`, searched breadth-first
/// so a top-level copy wins over one buried in a language subdirectory.
pub fn find_named(root: &Path, name: &str) -> Option<PathBuf> {
    let mut queue = vec![root.to_path_buf()];
    let mut dirs = Vec::new();
    while let Some(dir) = queue.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.eq_ignore_ascii_case(name)) {
                return Some(path);
            }
        }
        if queue.is_empty() {
            queue.append(&mut dirs);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three rows of the global registry, verbatim from `UDS_EV/RD.rod`.
    ///
    /// Table 531 writes its separator as `,` and table 297 writes the same
    /// separator as `0` — which is the whole point of the substitution being
    /// per-table, and why a reader that assumes a comma reads table 297 as one
    /// field.
    const SECTION: &str = "000531,.-0238,3P,,,,,\r\n\
                           000531,.0374730,F0,--4.03,,,,\r\n\
                           000297,.,.9140,,00000\r\n";

    #[test]
    fn a_row_is_found_by_its_one_based_number() {
        let registry = DtcRegistry::parse(SECTION);
        assert_eq!(registry.len(), 3);
        assert_eq!(registry.row(1).unwrap().key, 531);
        assert_eq!(registry.row(1).unwrap().payload, ".-0238,3P,,,,,");
        assert_eq!(registry.row(3).unwrap().key, 297);
        // Row 0 does not exist: a unit file's ids start at 1, and treating one
        // as an offset costs two of the reference car's stored faults.
        assert!(registry.row(0).is_none());
        assert!(registry.row(4).is_none());
    }

    #[test]
    fn a_row_names_a_codes_dat_key_and_its_failure_type() {
        let registry = DtcRegistry::parse(SECTION);
        // The failure-type field is written in *both* alphabets: `3P` is a
        // letter and a digit, and it reads as 4B only because the letters go
        // through their own substitution.
        let name = read_row(&registry.row(1).unwrap()).unwrap();
        assert_eq!(name.codes_key, 120_543);
        assert_eq!(name.failure_type, Some(0x4B));
        // Row 2's name is 10 489 840 = 0xA00FF0, whose own low byte is F0 —
        // the invariant that holds on 11 189 of 11 189 eight-digit rows.
        let name = read_row(&registry.row(2).unwrap()).unwrap();
        assert_eq!(name.codes_key, 10_489_840);
        assert_eq!(name.failure_type, Some(0xF0));
        // And table 297's row, whose separator is a `0`.
        let name = read_row(&registry.row(3).unwrap()).unwrap();
        assert_eq!(name.codes_key, 101_542);
        assert_eq!(name.failure_type, Some(0x00));
    }

    #[test]
    fn a_row_of_the_wrong_shape_is_refused_rather_than_read() {
        // Six fields, not seven. Reading field 0 anyway would produce a
        // confident number from a row this code does not understand.
        let registry = DtcRegistry::parse("000531,.-0238,3P,,,,\r\n");
        assert!(read_row(&registry.row(1).unwrap()).is_none());
    }

    #[test]
    fn a_unit_index_points_at_rows_and_is_keyed_by_the_raw_fault_number() {
        let registry = DtcRegistry::parse(SECTION);
        // The unit can report two faults: registry rows 3 and 1.
        let catalogue = UnitCatalogue::parse("3,F2\r\n1,07\r\n", &registry);
        assert_eq!(catalogue.len(), 2);
        assert_eq!(catalogue.row_of(297), Some(3));
        assert_eq!(catalogue.row_of(531), Some(1));
        // A fault this unit cannot report is absent, not row 0.
        assert_eq!(catalogue.row_of(12_289), None);
    }

    #[test]
    fn an_index_past_the_end_of_the_registry_is_dropped_not_kept() {
        // A unit file from a corpus of a different vintage than `RD.rod` can
        // point past it. That must shrink the catalogue, never index wrongly.
        let registry = DtcRegistry::parse(SECTION);
        let catalogue = UnitCatalogue::parse("9999,F2\r\n1,07\r\n", &registry);
        assert_eq!(catalogue.len(), 1);
        assert_eq!(catalogue.row_of(531), Some(1));
        // …and the file no longer matches the registry, which is the point:
        // 2 ids that name 1 fault is a shifted index, not a catalogue.
        assert!(!catalogue.is_consistent_with_registry());
    }

    #[test]
    fn a_catalogue_that_names_one_fault_twice_is_refused() {
        // Rows 1 and 2 are both table 531. A real unit file never lists one
        // fault twice — measured, this is the signature of a unit file read
        // against a registry of another vintage: the reference car's steering
        // column goes from 85 distinct faults to 63, its ESP from 518 to 457.
        let registry = DtcRegistry::parse(SECTION);
        let shifted = UnitCatalogue::parse("1,07\r\n2,F0\r\n", &registry);
        assert_eq!(shifted.listed(), 2);
        assert_eq!(shifted.len(), 1);
        assert!(!shifted.is_consistent_with_registry());

        let matched = UnitCatalogue::parse("1,07\r\n3,F0\r\n", &registry);
        assert!(matched.is_consistent_with_registry());
    }

    #[test]
    fn a_malformed_registry_row_still_occupies_its_row_number() {
        // The section has two rows whose key is not a number. Dropping them
        // would shift every row after them by one, and every id in every unit
        // file with it.
        let registry = DtcRegistry::parse("1866506087652,0376375821227777\r\n000297,3757_1,_6,71-,,,,\r\n");
        assert_eq!(registry.len(), 2);
        assert_eq!(registry.row(2).unwrap().key, 297);
    }
}
