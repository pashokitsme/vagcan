//! SQLite cache for the VCDS label files.
//!
//! Parsing every `.lbl` and decrypting+parsing every `.clb` file (see
//! `vag_data_labels::load_label_files`) is the expensive part of loading the label files; this
//! crate persists the *parsed* result to SQLite so later runs can skip that
//! work entirely. This is a fast-load cache only: `REDIRECT` chain resolution
//! stays in the existing, reviewed [`vag_data_labels::LabelDb`] — this crate just
//! reconstructs the same `Vec<LabelFile>` that `load_label_files` would produce and
//! hands it to `LabelDb::new`.
//!
//! `vag-data-labels` stays pure-Rust; this crate is the only place in the workspace
//! that depends on `rusqlite`.

use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use vag_data_labels::label::{LabelFile, Measurement, Record};
use vag_data_labels::{LabelDb, Scaling, load_label_files};

/// Errors from building or loading a vag-data-db SQLite cache.
#[derive(Debug)]
pub enum Error {
	Sqlite(rusqlite::Error),
	Io(std::io::Error),
	/// A cache an older vagcan wrote: it lacks a table or a column this build reads.
	/// Nothing upgrades it in place; `vagcan setup --refresh` rebuilds it.
	OlderCache {
		cache: String,
		/// What it lacks, as a phrase: "the `fault` table", "the column `reading_level.upper`".
		missing: String,
	},
	/// A file where the cache should be that SQLite says is not a database at all.
	/// `vagcan setup --refresh` replaces it.
	NotACache {
		cache: String,
	},
	/// No source with this id in the cache.
	NoSuchSource {
		id: i64,
	},
}

impl std::fmt::Display for Error {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Error::Sqlite(e) => write!(f, "sqlite error: {e}"),
			Error::Io(e) => write!(f, "io error: {e}"),
			Error::OlderCache { cache, missing } => write!(
				f,
				"{cache}: this cache was written by an older vagcan (it has no {missing}) — run `vagcan setup --refresh` \
				 on the folders it was read from; `vagcan setup --refresh` with no folder offers to download a VCDS installation"
			),
			Error::NotACache { cache } => write!(
				f,
				"{cache} is not a database — `vagcan setup --refresh` replaces it with a cache built from the folders it reads"
			),
			Error::NoSuchSource { id } => write!(f, "no source {id} in this cache — `vagcan setup --list` lists them"),
		}
	}
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
	fn from(e: rusqlite::Error) -> Self {
		Error::Sqlite(e)
	}
}

impl From<std::io::Error> for Error {
	fn from(e: std::io::Error) -> Self {
		Error::Io(e)
	}
}

/// Row counts written by a successful [`build_db`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BuildStats {
	pub files: usize,
	pub measurements: usize,
	pub redirects: usize,
	pub adaptations: usize,
	pub long_codings: usize,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS label_file (
    id           INTEGER PRIMARY KEY,
    name         TEXT NOT NULL UNIQUE,
    -- What the file's own header says the unit is: its diagnostic address and
    -- the label files' name for it. Cached like everything else here so a lookup
    -- costs no re-parse.
    unit_address INTEGER,
    unit_name    TEXT,
    -- The VCDS source whose label files these are, so dropping that source takes
    -- them with it. NULL only in a cache migrated from before the column whose
    -- owner could not be told (`migrate`).
    source_id    INTEGER REFERENCES source(id)
);
CREATE TABLE IF NOT EXISTS measurement (
    file_id     INTEGER NOT NULL REFERENCES label_file(id),
    block       INTEGER NOT NULL,
    field       INTEGER NOT NULL,
    name        TEXT NOT NULL,
    location    TEXT NOT NULL,
    description TEXT NOT NULL,
    unit        TEXT,
    range_min   REAL,
    range_max   REAL,
    -- Which source put this row here. One cache now holds rows from a VCDS
    -- installation and from an ODIS project, and a row that cannot say which
    -- cannot be replaced when that source is parsed again.
    source_id   INTEGER REFERENCES source(id)
);
CREATE TABLE IF NOT EXISTS redirect (
    file_id   INTEGER NOT NULL REFERENCES label_file(id),
    selector  TEXT,
    target    TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS adaptation (
    file_id     INTEGER NOT NULL REFERENCES label_file(id),
    channel     TEXT NOT NULL,
    idx         TEXT NOT NULL,
    name        TEXT NOT NULL,
    location    TEXT NOT NULL,
    description TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS long_coding (
    file_id  INTEGER NOT NULL REFERENCES label_file(id),
    byte     TEXT NOT NULL,
    bits     TEXT NOT NULL,
    value    TEXT NOT NULL,
    meaning  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_measurement_lookup ON measurement(file_id, block, field);
CREATE INDEX IF NOT EXISTS idx_redirect_selector  ON redirect(selector);
CREATE INDEX IF NOT EXISTS idx_label_file_name    ON label_file(name);
-- read_files() queries each child table `WHERE file_id = ?` once per label
-- file; without these, redirect/adaptation/long_coding reloads are full
-- table scans per file (O(files x rows) over the ~2900-file label_files).
CREATE INDEX IF NOT EXISTS idx_redirect_file      ON redirect(file_id);
CREATE INDEX IF NOT EXISTS idx_adaptation_file    ON adaptation(file_id);
CREATE INDEX IF NOT EXISTS idx_long_coding_file   ON long_coding(file_id);
-- Which directory a row came from, and what read it.
--
-- An mtime answers "is this old?" and cannot answer "is this even about these
-- files?" — and a cache built from the Russian tree is not *stale* for the
-- English one, it is *wrong*, which is the failure that shows a reader
-- confident answers in a language they did not ask for. It lives inside the
-- cache rather than in a note beside it because a cache that cannot say what
-- it holds is not self-describing, and a loose file recording a path is one
-- more thing to keep in step with the file it describes.
--
-- **One row per source, not one row.** It used to be `CHECK (id = 0)`, because
-- one cache came from one VCDS installation. A project's cache now holds a
-- VCDS parse *and* an ODIS parse of the same car, and forcing them into one row
-- would make the second erase the first's provenance.
--
-- `language` is the source's own declaration — an ODIS project's
-- `<LANGUAGE>deu</LANGUAGE>`, a VCDS build's `Codes.dat` (`eng`) or
-- `Code-RUS.dat` (`rus`) — as an ISO 639-2 code, and `NULL` where the source
-- did not say. It is a property of the *source*, not of a row: the ODIS object
-- model has no language field anywhere, and a project carries one text per
-- fault in whatever language its supplier wrote. So a second project in another
-- language is a second source row, and `[faults] language` in config.toml
-- chooses between sources, never between rows.
--
-- **This table is the project's list of sources, and the only one.** The
-- `sources.json` beside the cache used to keep a second list, and the two
-- disagreed: an entry deleted there left its rows here, still read. What that
-- file held and this table lacked is these columns:
--
-- - `root`: the folder `setup` was pointed at. For a VCDS installation that is
--   the installation, while `dir` is its `Labels` directory; for an ODIS project
--   the two are the same folder.
-- - `version`: what the source calls its own version — a VCDS build's fault-text
--   file name, an ODIS project's `VWMCD_ProjectVersionInfo`.
-- - `detail`: one line more, as `setup` wrote it.
-- - `parsed_at`: when `setup` last read it, RFC 3339 in UTC.
-- - `adaptations_at`: when `setup` last read an ODIS project's adaptation
--   channels, even none; NULL for a project read before they were kept (schema 5).
--
-- `dir` is stored absolute. A relative one is a row an older vagcan wrote from a
-- checkout; nothing resolves it against the working directory (`stored_key`).
CREATE TABLE IF NOT EXISTS source (
    id        INTEGER PRIMARY KEY,
    kind      TEXT NOT NULL,
    dir       TEXT NOT NULL,
    language  TEXT,
    root      TEXT,
    version   TEXT,
    detail    TEXT,
    parsed_at TEXT,
    adaptations_at TEXT,
    UNIQUE (kind, dir)
);
-- One fault code of one ECU variant, as an ODIS project describes it.
--
-- Keyed the way the car keys it: `code` is the 24-bit number a `0x19` response
-- carries, read big-endian as one integer — the same number `vagcan faults`
-- prints in decimal. `display` is the SAE-style code the file carries beside
-- it (`P150B00`); it is a separate string, not an encoding of `code`, and on
-- the reference project the two agree for 1,515 pairs of 43,378. `level` and
-- `temporary` are the object's `LEVEL` and `IS-TEMPORARY`, recorded and not
-- interpreted. A `dop` names which of a variant's fault tables the row is
-- from; nearly every variant has one, `DTCDOP_VAGUDS`.
CREATE TABLE IF NOT EXISTS fault (
    source_id  INTEGER NOT NULL REFERENCES source(id),
    variant    TEXT NOT NULL,
    dop        TEXT NOT NULL,
    code       INTEGER NOT NULL,
    display    TEXT,
    text       TEXT,
    text_id    TEXT,
    short_name TEXT,
    level      INTEGER NOT NULL,
    temporary  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_fault_lookup ON fault(variant, code);
-- One readable channel of one ECU variant, as an ODIS project describes it.
--
-- **A separate table from `measurement`, deliberately (D1).** `measurement` is
-- addressed the way VCDS addresses things — a measuring block and a field
-- within it — and `reading` the way ODIS and UDS do, by identifier. Forcing one
-- table would mean inventing a block number for a DID or a DID for a block, and
-- both inventions are car-specific data in code. They coexist; nothing reading
-- either has to know the other exists.
CREATE TABLE IF NOT EXISTS reading (
    id           INTEGER PRIMARY KEY,
    source_id    INTEGER NOT NULL REFERENCES source(id),
    -- The ECU variant this channel belongs to. One project describes hundreds,
    -- and which one a car is comes from what the car answers, not from here.
    variant      TEXT NOT NULL,
    did          INTEGER NOT NULL,
    name         TEXT NOT NULL,
    unit         TEXT,
    -- Bits into the positive response, after the three-byte `62 hi lo` header.
    bit_offset   INTEGER NOT NULL,
    bit_length   INTEGER NOT NULL,
    signed       INTEGER NOT NULL,
    -- Whether the bytes run most-significant first.
    --
    -- Stored, not assumed, and not derivable from anything else in the row.
    -- UDS payloads are big-endian by convention and the reference car's own
    -- proven row is not: DID 0x380A is `u16` little-endian
    -- (`.archive/research/labels/rod-labels.md:433`, established byte by byte against a
    -- log), and the ODIS file agrees. A reader that assumed big-endian would
    -- report 690 /min as 45570 — so a cache that dropped this column would
    -- throw away the parser's correctness at the storage layer.
    big_endian   INTEGER NOT NULL DEFAULT 1,
    text_id      TEXT,
    -- The scaling, in columns rather than one serialised blob. A blob would
    -- need a serialiser this crate does not depend on, and a column can be read
    -- by somebody holding `sqlite3` and no Rust — which is most of what a cache
    -- being inspectable is worth.
    scaling      TEXT NOT NULL,
    factor       REAL,
    offset       REAL,
    anchor_raw   INTEGER,
    anchor_value REAL
);
-- The levels of a `TEXTTABLE` scaling: each interval of raw values means one
-- thing, and there is no scale between them. A child table because a gear
-- selector has as many rows as it has positions and a linear channel has none.
CREATE TABLE IF NOT EXISTS reading_level (
    reading_id INTEGER NOT NULL REFERENCES reading(id),
    -- The interval's lower end, both ends included.
    raw        INTEGER NOT NULL,
    meaning    TEXT NOT NULL,
    -- Its upper end: `raw` again for a level one value wide. NULL only in a row
    -- an older vagcan added the column to in place, which kept the lower end
    -- alone — see `levels_predate_bounds`.
    upper      INTEGER
);
CREATE INDEX IF NOT EXISTS idx_reading_lookup ON reading(variant, did);
CREATE INDEX IF NOT EXISTS idx_reading_level  ON reading_level(reading_id);
-- One adaptation channel of one ECU variant, as an ODIS project describes it: a field
-- of what the variant's `ReadDataByIdentCalibData` service reads with `0x22`.
-- `reading`'s columns with `reading`'s meanings, in a table of its own so that no
-- reader of channels meets an adaptation, nor the other way round. Only ODIS writes
-- here; a VCDS installation's adaptations are its `RA.rod` rows in `registry_row`.
CREATE TABLE IF NOT EXISTS adaptation_channel (
    id           INTEGER PRIMARY KEY,
    source_id    INTEGER NOT NULL REFERENCES source(id),
    variant      TEXT NOT NULL,
    did          INTEGER NOT NULL,
    name         TEXT NOT NULL,
    unit         TEXT,
    bit_offset   INTEGER NOT NULL,
    bit_length   INTEGER NOT NULL,
    signed       INTEGER NOT NULL,
    big_endian   INTEGER NOT NULL DEFAULT 1,
    text_id      TEXT,
    scaling      TEXT NOT NULL,
    factor       REAL,
    offset       REAL,
    anchor_raw   INTEGER,
    anchor_value REAL
);
-- An adaptation channel's text-table states, as `reading_level` holds a channel's.
CREATE TABLE IF NOT EXISTS adaptation_level (
    adaptation_id INTEGER NOT NULL REFERENCES adaptation_channel(id),
    raw           INTEGER NOT NULL,
    meaning       TEXT NOT NULL,
    upper         INTEGER
);
CREATE INDEX IF NOT EXISTS idx_adaptation_lookup ON adaptation_channel(variant, did);
CREATE INDEX IF NOT EXISTS idx_adaptation_level  ON adaptation_level(adaptation_id);
-- One row a VCDS unit selects from a global registry other than RM.rod:
-- adaptations (ADP, RA.rod), basic settings (GES, RG.rod), output tests (SOT,
-- RS.rod), freeze-frame fields (FFMUX, RF.rod), XPL bits (RX.rod), and the MUX.rod
-- cases a row of type 6 defers to (MUX). `vag_data_labels::registries` reads them;
-- research/vcds-registry/README.md §9 says what each column was established
-- against. Stored, not acted on: basic settings and output tests are names only,
-- since their services (0x31, 0x2F) are outside the allowlist for good.
--
-- `variant` is the unit's .rod file stem, as in `reading`. `line` is the row's
-- 1-based number in its registry and `key` its key there. `identifier` is the
-- DID (ADP), routine id (GES) or input/output DID (SOT). For a MUX case,
-- `identifier` is the MUX key and `case_lower`/`case_upper` its raw range, both
-- NULL for the default case. `lead` is an FFMUX row's first field as written: 100
-- plus the record's length in bytes in 917 of 1,001 records, which is not enough to
-- store it as a length. `table_id` is the TTDOP table of a type-3 row and
-- the MUX key of a type-6 row. `raw` is every field deciphered, comma-joined:
-- the columns nobody established a meaning for live there and nowhere else.
CREATE TABLE IF NOT EXISTS registry_row (
    id            INTEGER PRIMARY KEY,
    source_id     INTEGER NOT NULL REFERENCES source(id),
    variant       TEXT NOT NULL,
    section       TEXT NOT NULL,
    line          INTEGER NOT NULL,
    key           INTEGER NOT NULL,
    identifier    INTEGER,
    ordinal       INTEGER,
    lead          INTEGER,
    case_lower    INTEGER,
    case_upper    INTEGER,
    kind          INTEGER,
    signed        INTEGER NOT NULL,
    big_endian    INTEGER NOT NULL,
    bit_offset    INTEGER,
    bit_length    INTEGER,
    factor        REAL,
    offset        REAL,
    table_id      INTEGER,
    constant      TEXT,
    unit          TEXT,
    name          TEXT,
    text_id       TEXT,
    raw           TEXT NOT NULL
);
-- A type-3 registry row's states, as `reading_level` holds a channel's.
CREATE TABLE IF NOT EXISTS registry_level (
    registry_row_id INTEGER NOT NULL REFERENCES registry_row(id),
    raw             INTEGER NOT NULL,
    upper           INTEGER NOT NULL,
    meaning         TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_registry_row_lookup ON registry_row(variant, section, identifier);
CREATE INDEX IF NOT EXISTS idx_registry_level      ON registry_level(registry_row_id);
"#;

/// Create what [`SCHEMA`] makes, in one transaction: a batch that fails part way leaves no
/// table behind, so no half-made cache can be mistaken for an older one.
fn create_schema(conn: &Connection, db_path: &Path) -> Result<(), Error> {
	refuse_older(conn, db_path)?;
	let tx = conn.unchecked_transaction()?;
	tx.execute_batch(SCHEMA)?;
	tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
	tx.commit()?;
	Ok(())
}

/// The shape [`SCHEMA`] makes, in `PRAGMA user_version`. A cache below it is brought up
/// to it in place by [`migrate`], once.
///
/// 1 (2026-10-20): the source metadata `sources.json` kept moved into `source`, label
/// files name their source, and a source is one row per directory.
/// 2 (2026-10-20): a source row that owns no row at all goes. A cache upgraded to 1 is
/// brought to 2 by the same [`migrate`]: every step of it is a no-op where done.
/// 3 (2026-10-20): `registry_row` and `registry_level`, the rows of VCDS's other
/// registries ([`MIGRATED_TABLES`]), created empty; the next read of a VCDS
/// installation fills them.
/// 4 (2026-10-21): `adaptation_channel` and `adaptation_level`, an ODIS project's
/// adaptation channels, created empty; the next `vagcan setup` of the project fills them.
const SCHEMA_VERSION: i64 = 5;

/// The tables [`migrate`] creates in a cache that predates them. Nothing in them is
/// derived from what the cache holds, so a missing one is made, empty, rather than the
/// cache refused: the read that fills them runs with the next read of the source —
/// the VCDS installation for the registry rows, the ODIS project for the adaptations.
const MIGRATED_TABLES: &[&str] = &["registry_row", "registry_level", "adaptation_channel", "adaptation_level"];

/// The columns [`migrate`] adds to a cache that predates them: the five of the shape before
/// [`SCHEMA_VERSION`] 1, and `source.adaptations_at` of the shape before 5. A
/// cache lacking only these is upgraded rather than refused.
const MIGRATED: &[(&str, &str)] = &[
	("label_file", "source_id"),
	("source", "root"),
	("source", "version"),
	("source", "detail"),
	("source", "parsed_at"),
	("source", "adaptations_at"),
];

/// Refuse a cache an older vagcan wrote: one holding some of this schema's tables but
/// not every table, or not every column of one.
///
/// **The shape just before this one is upgraded instead** ([`migrate`]): it lacks only
/// [`MIGRATED`], and asking for a re-read of every source to add five columns would be
/// minutes of work for nothing. Anything older is not upgraded in place. `vagcan setup
/// --refresh` removes such a cache ([`remove_if_older`]) and rebuilds it from the sources it
/// reads. A file with none of the tables is a cache not written yet and passes: a write
/// creates the tables, and a read fails at its first query. A file that is not a database
/// is refused as one ([`Error::NotACache`]).
fn refuse_older(conn: &Connection, db_path: &Path) -> Result<(), Error> {
	let cache = || db_path.display().to_string();
	let missing = match missing(conn) {
		Ok(missing) => missing,
		Err(e) if not_a_database(&e) => return Err(Error::NotACache { cache: cache() }),
		Err(e) => return Err(e.into()),
	};
	let migratable = |(table, column): &(String, Option<String>)| match column.as_deref() {
		Some(column) => MIGRATED.iter().any(|(t, c)| t == table && *c == column),
		None => MIGRATED_TABLES.contains(&table.as_str()),
	};
	if let Some(older) = missing.iter().find(|item| !migratable(item)) {
		return Err(Error::OlderCache {
			cache: cache(),
			missing: phrase(older),
		});
	}
	if !missing.is_empty() || (user_version(conn)? < SCHEMA_VERSION && !missing_everything(conn)?) {
		migrate(conn)?;
	}
	Ok(())
}

fn user_version(conn: &Connection) -> rusqlite::Result<i64> {
	conn.pragma_query_value(None, "user_version", |row| row.get(0))
}

/// Whether the file holds none of [`SCHEMA`]'s tables: a cache not written yet.
fn missing_everything(conn: &Connection) -> rusqlite::Result<bool> {
	for (table, _) in schema_shape() {
		if !columns(conn, table)?.is_empty() {
			return Ok(false);
		}
	}
	Ok(true)
}

/// Bring a cache of the shape before [`SCHEMA_VERSION`] 1 up to it, in one transaction:
///
/// - the [`MIGRATED`] columns are added, and the [`MIGRATED_TABLES`] created;
/// - two source rows naming one directory are made one ([`collapse_duplicates`]);
/// - the label files are given their owner: the one source their measurements name, or
///   the one VCDS source there is. Neither — no measurement names one, and more than one
///   VCDS source — leaves them owned by nobody, and dropping the last VCDS source takes
///   them ([`drop_source`]).
///
/// What `sources.json` held is not this crate's to read: the project layer adopts it
/// ([`adopt_meta`]).
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
	// Immediate: the write lock first, so two processes opening one cache at once queue on
	// it rather than both reading under a shared lock and failing to upgrade it. The one
	// that waited then finds the work done, and does nothing.
	let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
	if user_version(&tx)? >= SCHEMA_VERSION && missing(&tx)?.is_empty() {
		return tx.commit();
	}
	for (table, column) in MIGRATED {
		let held = columns(&tx, table)?;
		if !held.is_empty() && !held.iter().any(|c| c == column) {
			let kind = match *column {
				"source_id" => "INTEGER REFERENCES source(id)",
				_ => "TEXT",
			};
			// Both names are the constant above, never anything a caller supplied.
			tx.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {kind}"))?;
		}
	}
	// Every statement of the schema is `IF NOT EXISTS`: this makes the tables a cache
	// lacks, and their indexes, and leaves the rest as they are.
	tx.execute_batch(SCHEMA)?;
	collapse_duplicates(&tx)?;
	let measured: Vec<i64> = {
		let mut stmt = tx.prepare("SELECT DISTINCT source_id FROM measurement WHERE source_id IS NOT NULL")?;
		stmt.query_map([], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?
	};
	let vcds: Vec<i64> = {
		let mut stmt = tx.prepare("SELECT id FROM source WHERE kind = ?1")?;
		stmt.query_map([VCDS], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?
	};
	let owner = match (measured.as_slice(), vcds.as_slice()) {
		([one], _) => Some(*one),
		([], [one]) => Some(*one),
		_ => None,
	};
	if let Some(owner) = owner {
		tx.execute("UPDATE label_file SET source_id = ?1 WHERE source_id IS NULL", params![owner])?;
	}
	// A source row that owns no row at all is a note of a folder and nothing more — the
	// reference cache had one, a relative path an earlier run in a checkout left. It
	// holds nothing to lose, and kept, it counts as a source of its kind: `faults` and
	// `watch` go on asking for VCDS, and `--drop vcds` is two sources. No path is guessed
	// to delete it. Label files no source owns keep every VCDS row: one could be theirs.
	let unowned_labels: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM label_file WHERE source_id IS NULL)", [], |row| row.get(0))?;
	let ids: Vec<(i64, String)> = {
		let mut stmt = tx.prepare("SELECT id, kind FROM source")?;
		stmt
			.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
			.collect::<rusqlite::Result<_>>()?
	};
	for (id, kind) in ids {
		if kind == VCDS && unowned_labels {
			continue;
		}
		let mut holds = false;
		for table in owning_tables() {
			// The table names are the schema's own.
			let any: bool = tx.query_row(
				&format!("SELECT EXISTS (SELECT 1 FROM {table} WHERE source_id = ?1)"),
				params![id],
				|row| row.get(0),
			)?;
			holds |= any;
		}
		if !holds {
			tx.execute("DELETE FROM source WHERE id = ?1", params![id])?;
		}
	}
	tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
	tx.commit()
}

/// Make every set of source rows that name one directory one row: the oldest, which keeps
/// its place in source order, takes the others' rows ([`merge_into`]).
///
/// A relative `dir` is never compared by where it resolves: it was true in the directory
/// an older vagcan ran in, and resolving it against this process's working directory would
/// be a guess ([`stored_key`]). It stays its own row, which `vagcan setup --list` shows as
/// stale.
fn collapse_duplicates(conn: &Connection) -> rusqlite::Result<()> {
	let rows: Vec<(i64, String, String)> = {
		let mut stmt = conn.prepare("SELECT id, kind, dir FROM source ORDER BY id")?;
		stmt
			.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
			.collect::<rusqlite::Result<_>>()?
	};
	let mut kept: Vec<(i64, String, String)> = Vec::new();
	for (id, kind, dir) in rows {
		let Some(key) = stored_key(&dir) else { continue };
		match kept.iter().find(|(_, k, d)| *k == kind && *d == key) {
			Some((keep, _, _)) => merge_into(conn, *keep, id)?,
			None => kept.push((id, kind, key)),
		}
	}
	for (id, _, key) in kept {
		conn.execute("UPDATE source SET dir = ?1 WHERE id = ?2", params![key, id])?;
	}
	Ok(())
}

/// Fold source `other` into source `keep`, which names the same directory.
///
/// Per table: where `keep` already has rows, `other`'s are a second copy of the same
/// directory's and are deleted; where it has none, `other`'s are repointed to it. So no
/// table ends with two copies under one source, and none loses the only copy it had.
/// `keep` takes whatever metadata it lacks from `other`, and `other`'s row goes.
fn merge_into(conn: &Connection, keep: i64, other: i64) -> rusqlite::Result<()> {
	for table in owning_tables() {
		// The table names are the schema's own, never anything a caller supplied.
		let held: bool = conn.query_row(
			&format!("SELECT EXISTS (SELECT 1 FROM {table} WHERE source_id = ?1)"),
			params![keep],
			|row| row.get(0),
		)?;
		match held {
			false => {
				conn.execute(&format!("UPDATE {table} SET source_id = ?1 WHERE source_id = ?2"), params![keep, other])?;
			}
			true => {
				delete_rows(conn, table, "source_id = ?1", other)?;
			}
		}
	}
	conn.execute(
		"UPDATE source SET \
		     language  = COALESCE(language,  (SELECT language  FROM source WHERE id = ?2)), \
		     root      = COALESCE(root,      (SELECT root      FROM source WHERE id = ?2)), \
		     version   = COALESCE(version,   (SELECT version   FROM source WHERE id = ?2)), \
		     detail    = COALESCE(detail,    (SELECT detail    FROM source WHERE id = ?2)), \
		     parsed_at = COALESCE(parsed_at, (SELECT parsed_at FROM source WHERE id = ?2)), \
		     adaptations_at = COALESCE(adaptations_at, (SELECT adaptations_at FROM source WHERE id = ?2)) \
		 WHERE id = ?1",
		params![keep, other],
	)?;
	conn.execute("DELETE FROM source WHERE id = ?1", params![other])?;
	Ok(())
}

/// Delete the rows of `table` that `owned` (an SQL condition over `?1`) picks, with
/// every row that hangs off them, however deep: a reading's levels, a label file's
/// records. Returns how many rows of `table` itself went.
///
/// The rows that hang off a table are found from the schema's own foreign keys
/// ([`schema_links`]), not from a list written here: a table added with a key into
/// one of these is deleted with it, and one that names a source is dropped with it.
fn delete_rows(conn: &Connection, table: &str, owned: &str, id: i64) -> rusqlite::Result<usize> {
	// `table` and the columns are the schema's own, and `owned` is this file's: nothing
	// a caller supplied is formatted into the SQL.
	for (child, column, _) in schema_links().iter().filter(|(child, _, parent)| parent == table && child != table) {
		delete_rows(conn, child, &format!("{column} IN (SELECT id FROM {table} WHERE {owned})"), id)?;
	}
	conn.execute(&format!("DELETE FROM {table} WHERE {owned}"), params![id])
}

/// Every foreign key in [`SCHEMA`], as `(table, column, parent table)` — asked of SQLite
/// once, from the schema itself, like [`schema_shape`].
fn schema_links() -> &'static [(String, String, String)] {
	static LINKS: std::sync::OnceLock<Vec<(String, String, String)>> = std::sync::OnceLock::new();
	LINKS.get_or_init(|| {
		let conn = Connection::open_in_memory().expect("an in-memory database opens");
		conn.execute_batch(SCHEMA).expect("the schema is valid SQL");
		let mut links = Vec::new();
		for (table, _) in schema_shape() {
			let mut stmt = conn
				.prepare("SELECT \"from\", \"table\" FROM pragma_foreign_key_list(?1)")
				.expect("foreign keys are queryable");
			let keys = stmt
				.query_map(params![table], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
				.and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
				.expect("a table's foreign keys read");
			links.extend(keys.into_iter().map(|(column, parent)| (table.clone(), column, parent)));
		}
		links
	})
}

/// Every table whose rows name the source they came from.
fn owning_tables() -> impl Iterator<Item = &'static str> {
	schema_links()
		.iter()
		.filter(|(_, column, parent)| column == "source_id" && parent == "source")
		.map(|(table, _, _)| table.as_str())
}

/// The tables only a VCDS installation writes, whose rows a cache migrated from before
/// [`SCHEMA_VERSION`] 1 can hold with no source named ([`migrate`]).
const VCDS_ONLY: &[&str] = &["label_file", "measurement"];

fn not_a_database(e: &rusqlite::Error) -> bool {
	matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::NotADatabase)
}

/// Whether the cache at `db_path` is one every reader and writer here refuses — an older
/// vagcan's, or no database at all. `false` for a file that is not there.
#[cfg(test)]
fn is_older(db_path: &Path) -> Result<bool, Error> {
	match check(db_path) {
		Ok(()) => Ok(false),
		Err(Error::OlderCache { .. } | Error::NotACache { .. }) => Ok(true),
		Err(e) => Err(e),
	}
}

/// The refusal every reader and writer here would give the cache at `db_path`, asked
/// without reading or writing anything else. `Ok` for a current cache and for a file
/// that is not there.
pub fn check(db_path: &Path) -> Result<(), Error> {
	if !db_path.is_file() {
		return Ok(());
	}
	let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
	refuse_older(&conn, db_path)
}

/// Remove the cache at `db_path` when every reader and writer here refuses it — an older
/// vagcan wrote it, or it is not a database — with any journal it left. What `vagcan setup
/// --refresh` does first, so the run rebuilds the cache rather than meeting the refusal.
///
/// `Some` when it removed one, holding the `(kind, dir)` of every source the cache said it
/// was read from (none for a file that is not a database): a run rebuilds only what it
/// reads, so the caller says which of these it will not. `None` for a current cache, or
/// none, which is left alone.
pub fn remove_if_older(db_path: &Path) -> Result<Option<Vec<(String, String)>>, Error> {
	let sources = match check(db_path) {
		Ok(()) => return Ok(None),
		Err(Error::OlderCache { .. }) => {
			let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
			held_sources(&conn)?
		}
		Err(Error::NotACache { .. }) => Vec::new(),
		Err(e) => return Err(e),
	};
	std::fs::remove_file(db_path)?;
	let mut journal = db_path.as_os_str().to_owned();
	journal.push("-journal");
	match std::fs::remove_file(std::path::PathBuf::from(journal)) {
		Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
		_ => {}
	}
	Ok(Some(sources))
}

/// The `(kind, dir)` rows of an older cache's `source` table. Every shape of it holds `dir`;
/// the first one had no `kind`, and was a VCDS parse, the only source there was.
fn held_sources(conn: &Connection) -> rusqlite::Result<Vec<(String, String)>> {
	let held = columns(conn, "source")?;
	let sql = match (held.iter().any(|c| c == "kind"), held.iter().any(|c| c == "dir")) {
		(true, true) => "SELECT kind, dir FROM source ORDER BY id",
		(false, true) => "SELECT 'vcds', dir FROM source ORDER BY id",
		_ => return Ok(Vec::new()),
	};
	let mut stmt = conn.prepare(sql)?;
	let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
	rows.collect()
}

/// Every table and column of [`SCHEMA`] the cache lacks, in schema order: `(table, None)`
/// for a whole table, `(table, Some(column))` for one column. Empty when it has them all
/// or has none of the tables.
fn missing(conn: &Connection) -> rusqlite::Result<Vec<(String, Option<String>)>> {
	let mut held = Vec::new();
	for (table, _) in schema_shape() {
		held.push(columns(conn, table)?);
	}
	let mut out = Vec::new();
	if held.iter().all(Vec::is_empty) {
		return Ok(out);
	}
	for ((table, wanted), held) in schema_shape().iter().zip(&held) {
		if held.is_empty() {
			out.push((table.clone(), None));
			continue;
		}
		for column in wanted.iter().filter(|column| !held.contains(column)) {
			out.push((table.clone(), Some(column.clone())));
		}
	}
	Ok(out)
}

/// One [`missing`] item as [`Error::OlderCache`] says it.
fn phrase((table, column): &(String, Option<String>)) -> String {
	match column {
		None => format!("the `{table}` table"),
		Some(column) => format!("the column `{table}.{column}`"),
	}
}

/// Every table [`SCHEMA`] creates, with its columns — asked of SQLite once, from the
/// schema itself, so the check cannot drift from what the batch makes.
fn schema_shape() -> &'static [(String, Vec<String>)] {
	static SHAPE: std::sync::OnceLock<Vec<(String, Vec<String>)>> = std::sync::OnceLock::new();
	SHAPE.get_or_init(|| {
		let conn = Connection::open_in_memory().expect("an in-memory database opens");
		conn.execute_batch(SCHEMA).expect("the schema is valid SQL");
		let mut stmt = conn
			.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
			.expect("sqlite_master is queryable");
		let tables: Vec<String> = stmt
			.query_map([], |row| row.get(0))
			.and_then(|rows| rows.collect())
			.expect("the table names read");
		tables
			.into_iter()
			.map(|table| {
				let held = columns(&conn, &table).expect("a table's columns read");
				(table, held)
			})
			.collect()
	})
}

/// A table's column names, or nothing at all if there is no such table.
fn columns(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
	let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
	let names = stmt.query_map([], |row| row.get::<_, String>(1))?;
	names.collect()
}

/// The kind a source is, as `source.kind` spells it.
pub const VCDS: &str = "vcds";
/// The kind an ODIS project is.
pub const ODIS: &str = "odis";

/// One directory, spelled the one way this table keys on.
///
/// **A path is normalised before it becomes a key, never only before it is
/// read.** `~/Downloads/SK37X/` and `~/Downloads/SK37X` name one directory and
/// every reader in this project already treats them as one — but `source` was
/// `UNIQUE (kind, dir)` on the raw string, so importing under both spellings
/// wrote two source rows and two full copies of the project: 621,468 rows for
/// 310,734 channels, every one of them offered twice to anything that looked
/// them up. That is the whole bug, and it hid behind a check of the *reading*
/// path, which was correct and irrelevant.
///
/// [`std::fs::canonicalize`] rather than trimming a slash: `.`, `..`, a symlink
/// and a relative path are the same class of alias and there is no reason to fix
/// one spelling and wait for the next. It needs the directory to exist, which it
/// does at the moment anything is imported from it; when it does not — a
/// recorded path whose directory has since gone — the raw string minus its
/// trailing separators is kept, so an old row still matches itself.
pub fn normalise_dir(dir: &str) -> String {
	if let Ok(real) = std::fs::canonicalize(dir) {
		return real.to_string_lossy().into_owned();
	}
	let trimmed = dir.trim_end_matches(std::path::is_separator);
	match trimmed.is_empty() {
		// All separators: that is the root, and trimming it away would make it
		// the empty string, which names nothing.
		true => dir.to_owned(),
		false => trimmed.to_owned(),
	}
}

/// The directory a **stored** `source.dir` names, in [`normalise_dir`]'s spelling, or
/// `None` when it is relative.
///
/// A path being imported is resolved where it was typed, which is this process; a path
/// read back out of the cache was typed by some earlier run, in a directory nobody
/// recorded. Resolving that one against this process's working directory is a guess — it
/// is right in a checkout and wrong everywhere else — so a relative stored path matches
/// nothing but its own spelling, and `vagcan setup --list` calls it stale.
fn stored_key(dir: &str) -> Option<String> {
	Path::new(dir).is_absolute().then(|| normalise_dir(dir))
}

/// Find or make the `source` row for one directory, and return its id.
///
/// Rows already written under another spelling of the same directory are folded into
/// the oldest ([`merge_into`]): where it has rows of a table, the other's are a second
/// copy and go, and where it has none, the other's become its own. A cache that already
/// holds the duplicate has no other way back, and every caller of this function is an
/// import about to write that directory's contents again.
///
/// Done here as well as in [`migrate`] because it is the spelling rule that decides
/// which rows are the same, and the rule lives here.
fn source_id(conn: &Connection, kind: &str, dir: &str) -> rusqlite::Result<i64> {
	match held_source_id(conn, kind, dir)? {
		Some(id) => Ok(id),
		None => {
			let wanted = normalise_dir(dir);
			conn.execute("INSERT INTO source (kind, dir) VALUES (?1, ?2)", params![kind, wanted])?;
			conn.query_row("SELECT id FROM source WHERE kind = ?1 AND dir = ?2", params![kind, wanted], |row| {
				row.get(0)
			})
		}
	}
}

/// [`source_id`] for a source that must already be there: `None`, and nothing made, when
/// it is not. Other spellings of the directory are folded in as there.
fn held_source_id(conn: &Connection, kind: &str, dir: &str) -> rusqlite::Result<Option<i64>> {
	let wanted = normalise_dir(dir);
	let mut same: Vec<i64> = {
		let mut stmt = conn.prepare("SELECT id, dir FROM source WHERE kind = ?1 ORDER BY id")?;
		let rows = stmt.query_map(params![kind], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?;
		rows
			.collect::<rusqlite::Result<Vec<_>>>()?
			.into_iter()
			.filter(|(_, stored)| *stored == wanted || stored_key(stored).as_deref() == Some(wanted.as_str()))
			.map(|(id, _)| id)
			.collect()
	};
	let Some(keep) = same.first().copied() else {
		return Ok(None);
	};
	same.remove(0);
	for other in same {
		merge_into(conn, keep, other)?;
	}
	// The survivor is rewritten in the spelling this function will look for next
	// time, so the merge happens once rather than on every run.
	conn.execute("UPDATE source SET dir = ?1 WHERE id = ?2", params![wanted, keep])?;
	Ok(Some(keep))
}

/// Insert every file + its records into `conn`, in one transaction. The
/// `Record::Other` variant is not persisted — it's non-semantic (comments /
/// unrecognized record kinds).
/// `labels_dir` is recorded as this build's `source` row **inside the same
/// transaction**, so a cache that failed half way through claims nothing: the
/// row and the files it describes commit together or neither does.
fn insert_files(conn: &mut Connection, files: &[LabelFile], labels_dir: &str) -> Result<BuildStats, Error> {
	let mut stats = BuildStats::default();
	let tx = conn.transaction()?;
	let source = source_id(&tx, VCDS, labels_dir)?;
	{
		// Idempotent rebuild: clear any existing rows before inserting, so
		// running `build_db` twice against the same path overwrites cleanly
		// instead of tripping the `label_file.name` UNIQUE constraint.
		// Children first to respect the FK references to `label_file`.
		tx.execute_batch(
			"DELETE FROM measurement;\
             DELETE FROM redirect;\
             DELETE FROM adaptation;\
             DELETE FROM long_coding;\
             DELETE FROM label_file;",
		)?;

		let mut insert_file = tx.prepare("INSERT INTO label_file (name, unit_address, unit_name, source_id) VALUES (?1, ?2, ?3, ?4)")?;
		let mut insert_measurement = tx.prepare(
			"INSERT INTO measurement \
                (file_id, block, field, name, location, description, unit, range_min, range_max, source_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
		)?;
		let mut insert_redirect = tx.prepare("INSERT INTO redirect (file_id, selector, target) VALUES (?1, ?2, ?3)")?;
		let mut insert_adaptation = tx.prepare(
			"INSERT INTO adaptation (file_id, channel, idx, name, location, description) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
		)?;
		let mut insert_long_coding = tx.prepare(
			"INSERT INTO long_coding (file_id, byte, bits, value, meaning) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
		)?;

		for lf in files {
			insert_file.execute(params![
				lf.source,
				lf.unit.as_ref().map(|u| u.address),
				lf.unit.as_ref().map(|u| u.name.clone()),
				source,
			])?;
			let file_id = tx.last_insert_rowid();
			stats.files += 1;

			for r in &lf.records {
				match r {
					Record::Measurement(m) => {
						let range_min = m.range.map(|r| r[0]);
						let range_max = m.range.map(|r| r[1]);
						insert_measurement.execute(params![
							file_id,
							m.block,
							m.field,
							m.name,
							m.location,
							m.description,
							m.unit,
							range_min,
							range_max,
							source
						])?;
						stats.measurements += 1;
					}
					Record::Redirect { target, selector, .. } => {
						insert_redirect.execute(params![file_id, selector, target])?;
						stats.redirects += 1;
					}
					Record::Adaptation {
						channel,
						index,
						name,
						location,
						description,
					} => {
						insert_adaptation.execute(params![file_id, channel, index, name, location, description])?;
						stats.adaptations += 1;
					}
					Record::LongCoding { byte, bits, value, meaning } => {
						insert_long_coding.execute(params![file_id, byte, bits, value, meaning])?;
						stats.long_codings += 1;
					}
					Record::Other { .. } => {} // not persisted
				}
			}
		}
	}
	tx.commit()?;
	Ok(stats)
}

/// Read every file + its records back out of `conn`, reconstructing
/// `Record::Measurement`/`Redirect`/`Adaptation`/`LongCoding` values.
/// `Record::Other` and `Redirect.comment` cannot round-trip (neither is
/// persisted by [`insert_files`]).
fn read_files(conn: &Connection) -> Result<Vec<LabelFile>, Error> {
	let mut file_stmt = conn.prepare("SELECT id, name, unit_address, unit_name FROM label_file ORDER BY id")?;
	let file_rows: Vec<(i64, String, Option<u8>, Option<String>)> = file_stmt
		.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
		.collect::<rusqlite::Result<_>>()?;
	drop(file_stmt);

	let mut meas_stmt = conn.prepare(
		"SELECT block, field, name, location, description, unit, range_min, range_max \
         FROM measurement WHERE file_id = ?1 ORDER BY rowid",
	)?;
	let mut redirect_stmt = conn.prepare("SELECT selector, target FROM redirect WHERE file_id = ?1 ORDER BY rowid")?;
	let mut adapt_stmt = conn.prepare(
		"SELECT channel, idx, name, location, description \
         FROM adaptation WHERE file_id = ?1 ORDER BY rowid",
	)?;
	let mut lc_stmt = conn.prepare("SELECT byte, bits, value, meaning FROM long_coding WHERE file_id = ?1 ORDER BY rowid")?;

	let mut files = Vec::with_capacity(file_rows.len());
	for (file_id, name, address, unit_name) in file_rows {
		let mut records = Vec::new();

		let measurements = meas_stmt.query_map(params![file_id], |row| {
			let range_min: Option<f64> = row.get(6)?;
			let range_max: Option<f64> = row.get(7)?;
			Ok(Record::Measurement(Measurement {
				block: row.get(0)?,
				field: row.get(1)?,
				name: row.get(2)?,
				location: row.get(3)?,
				description: row.get(4)?,
				unit: row.get(5)?,
				range: match (range_min, range_max) {
					(Some(min), Some(max)) => Some([min, max]),
					_ => None,
				},
			}))
		})?;
		for m in measurements {
			records.push(m?);
		}

		let redirects = redirect_stmt.query_map(params![file_id], |row| {
			Ok(Record::Redirect {
				selector: row.get(0)?,
				target: row.get(1)?,
				comment: None,
			})
		})?;
		for r in redirects {
			records.push(r?);
		}

		let adaptations = adapt_stmt.query_map(params![file_id], |row| {
			Ok(Record::Adaptation {
				channel: row.get(0)?,
				index: row.get(1)?,
				name: row.get(2)?,
				location: row.get(3)?,
				description: row.get(4)?,
			})
		})?;
		for a in adaptations {
			records.push(a?);
		}

		let long_codings = lc_stmt.query_map(params![file_id], |row| {
			Ok(Record::LongCoding {
				byte: row.get(0)?,
				bits: row.get(1)?,
				value: row.get(2)?,
				meaning: row.get(3)?,
			})
		})?;
		for lc in long_codings {
			records.push(lc?);
		}

		let unit = address
			.zip(unit_name)
			.map(|(address, name)| vag_data_labels::label::UnitLabel { address, name });
		files.push(LabelFile { source: name, records, unit });
	}

	Ok(files)
}

/// Build (or overwrite) a SQLite DB at `db_path` from a labels directory.
/// Returns row counts. Wraps all inserts in one transaction for speed.
pub fn build_db(labels_dir: &Path, db_path: &Path) -> Result<BuildStats, Error> {
	let load = load_label_files(labels_dir)?;
	let mut conn = Connection::open(db_path)?;
	create_schema(&conn, db_path)?;
	insert_files(&mut conn, &load.files, &labels_dir.to_string_lossy())
}

/// Open an existing cache to read it, with no power to create one.
///
/// [`Connection::open`] carries SQLite's `CREATE` flag, so every question asked
/// of a cache that is not there used to *answer it into existence*: a zero-byte
/// `cache.sqlite` left behind by a bare `vagcan`, which then reads as "this
/// project has labels" to everything that tests for the file, and fails at the
/// first query with `no such table: label_file`. A missing database is an error
/// here, never an empty one — only the paths that build or write a cache
/// ([`build_db`], [`put_readings`]) may create the file.
///
/// **Read-write, not read-only, and that is what makes it safe to read.** A
/// `setup` killed inside one of its write transactions leaves
/// `cache.sqlite-journal` behind — a hot journal — and SQLite refuses to read
/// the file until something rolls that journal back. A `SQLITE_OPEN_READ_ONLY`
/// connection is not allowed to: every read failed with "attempt to write a
/// readonly database" (`SQLITE_READONLY_ROLLBACK`) until the next setup, and
/// the callers that treat an unopenable cache as an empty one then printed
/// faults with no ODIS names and a watch with no ODIS channels, saying
/// nothing. A read-write connection rolls the journal back on open. Besides that
/// it writes only the one-time upgrade of the shape before this one ([`migrate`]);
/// the readers that open through here issue no write, and the two writers that do —
/// [`drop_source`], [`adopt_meta`] — act on a cache that must already exist.
///
/// Still no `SQLITE_OPEN_CREATE`, so the property above holds — and a file the
/// process may not write is opened read-only by SQLite itself.
///
/// A cache an older vagcan wrote is refused here, as on every write ([`refuse_older`]).
fn open_existing(db_path: &Path) -> Result<Connection, Error> {
	let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
	refuse_older(&conn, db_path)?;
	Ok(conn)
}

/// The label-file directory `db_path` was built from, if it says.
///
/// `None` for a cache an older vagcan wrote, one that cannot be
/// opened, or one holding no VCDS source at all — a pure-ODIS project's cache is
/// the third case, and it is not stale, it is simply about nothing this question
/// concerns. The caller decides what to do with that (see the freshness rule in
/// `vagcan::labels`, which trusts a cache whose source directory is gone).
///
/// The source the label files name as theirs, not the first VCDS row: a stale row an
/// older vagcan left first in the table used to answer here, and no rebuild could ever
/// match it, so every `setup` parsed the label files again.
pub fn source_of(db_path: &Path) -> Option<String> {
	let conn = open_existing(db_path).ok()?;
	conn
		.query_row(
			"SELECT s.dir FROM label_file l JOIN source s ON s.id = l.source_id WHERE s.kind = ?1 LIMIT 1",
			[VCDS],
			|row| row.get::<_, String>(0),
		)
		.ok()
}

/// Every source that has ever written into this cache, oldest first.
pub fn sources_of(db_path: &Path) -> Result<Vec<(String, String)>, Error> {
	let conn = open_existing(db_path)?;
	let mut stmt = conn.prepare("SELECT kind, dir FROM source ORDER BY id")?;
	let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
	Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Every source with the language it declared: `(kind, dir, language)`,
/// oldest first. The input to the policy that chooses between sources of
/// fault text when there is more than one.
pub fn source_languages(db_path: &Path) -> Result<Vec<(String, String, Option<String>)>, Error> {
	let conn = open_existing(db_path)?;
	let mut stmt = conn.prepare("SELECT kind, dir, language FROM source ORDER BY id")?;
	let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
	Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// One row of the `source` table: one folder `setup` read into this cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
	pub id: i64,
	/// [`VCDS`] or [`ODIS`].
	pub kind: String,
	/// The directory the rows are keyed by: a VCDS installation's `Labels`, an ODIS
	/// project's folder.
	pub dir: String,
	pub language: Option<String>,
	pub meta: Meta,
}

impl Source {
	/// A row whose directory is relative: written by an older vagcan from wherever it ran,
	/// and resolvable against nothing ([`stored_key`]).
	pub fn is_stale(&self) -> bool {
		!Path::new(&self.dir).is_absolute()
	}

	/// The folder to name to a person: what `setup` was pointed at, else the directory.
	pub fn path(&self) -> &str {
		self.meta.root.as_deref().unwrap_or(&self.dir)
	}
}

/// What `setup` writes about a source besides its rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Meta {
	/// The folder `setup` was pointed at — a VCDS installation's root.
	pub root: Option<String>,
	pub version: Option<String>,
	pub detail: Option<String>,
	/// RFC 3339, UTC.
	pub parsed_at: Option<String>,
}

/// How many rows of each kind one source holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rows {
	pub label_files: u64,
	pub measurements: u64,
	/// Channels by identifier: an ODIS project's, or a VCDS registry's.
	pub readings: u64,
	pub faults: u64,
}

impl Rows {
	pub fn total(&self) -> u64 {
		self.label_files + self.measurements + self.readings + self.faults
	}
}

/// Every source in this cache, oldest first.
pub fn sources(db_path: &Path) -> Result<Vec<Source>, Error> {
	let conn = open_existing(db_path)?;
	read_sources(&conn)
}

fn read_sources(conn: &Connection) -> Result<Vec<Source>, Error> {
	let mut stmt = conn.prepare("SELECT id, kind, dir, language, root, version, detail, parsed_at FROM source ORDER BY id")?;
	let rows = stmt.query_map([], |row| {
		Ok(Source {
			id: row.get(0)?,
			kind: row.get(1)?,
			dir: row.get(2)?,
			language: row.get(3)?,
			meta: Meta {
				root: row.get(4)?,
				version: row.get(5)?,
				detail: row.get(6)?,
				parsed_at: row.get(7)?,
			},
		})
	})?;
	Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// How many rows each source holds, by source id, and how many rows belong to no
/// source — label files a migration could not give an owner ([`migrate`]).
///
/// One scan per table: the whole cache is asked once, for a listing.
pub fn source_rows(db_path: &Path) -> Result<(std::collections::BTreeMap<i64, Rows>, Rows), Error> {
	let conn = open_existing(db_path)?;
	let mut by_source: std::collections::BTreeMap<i64, Rows> = std::collections::BTreeMap::new();
	let mut unowned = Rows::default();
	for table in ["label_file", "measurement", "reading", "fault"] {
		// The table names are the constant above, never anything a caller supplied.
		let mut stmt = conn.prepare(&format!("SELECT source_id, COUNT(*) FROM {table} GROUP BY source_id"))?;
		let counts = stmt.query_map([], |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, i64>(1)?)))?;
		for count in counts {
			let (id, n) = count?;
			let rows = match id {
				Some(id) => by_source.entry(id).or_default(),
				None => &mut unowned,
			};
			let n = n.max(0) as u64;
			match table {
				"label_file" => rows.label_files = n,
				"measurement" => rows.measurements = n,
				"reading" => rows.readings = n,
				_ => rows.faults = n,
			}
		}
	}
	Ok((by_source, unowned))
}

/// Delete one source and every row that came from it, in one transaction, and say how
/// many rows went.
///
/// Every table that names a source is cleared of it, with what hangs off those rows: a
/// reading's levels, a label file's records. Dropping the **last** VCDS source also takes
/// the label rows that belong to no source ([`source_rows`]'s second half) — nothing else
/// could claim them, and a project with no VCDS source must not keep VCDS label files.
/// What is left is exactly what the other sources wrote, so every rule that ranks one
/// source over another ranks what remains.
pub fn drop_source(db_path: &Path, id: i64) -> Result<Rows, Error> {
	let mut conn = open_existing(db_path)?;
	let tx = conn.transaction()?;
	let kind: Option<String> = tx
		.query_row("SELECT kind FROM source WHERE id = ?1", params![id], |row| row.get(0))
		.optional()?;
	let Some(kind) = kind else {
		return Err(Error::NoSuchSource { id });
	};
	// A stale row — a relative path no run can resolve — is no other source of the kind:
	// it can claim nothing ([`Source::is_stale`]).
	let others: Vec<String> = {
		let mut stmt = tx.prepare("SELECT dir FROM source WHERE kind = ?1 AND id <> ?2")?;
		stmt.query_map(params![kind, id], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?
	};
	let others_of_kind = others.iter().any(|dir| Path::new(dir).is_absolute());
	let gone = remove_source(&tx, id, kind == VCDS && !others_of_kind)?;
	tx.commit()?;
	Ok(gone)
}

/// The deletion behind [`drop_source`], inside the caller's transaction: every row of
/// every table that names the source ([`owning_tables`]), with what hangs off them.
/// `unowned` also takes the rows of [`VCDS_ONLY`] that name no source.
fn remove_source(conn: &Connection, id: i64, unowned: bool) -> rusqlite::Result<Rows> {
	let owned = |table: &str| match unowned && VCDS_ONLY.contains(&table) {
		true => "(source_id = ?1 OR source_id IS NULL)",
		false => "source_id = ?1",
	};
	// Counted before anything goes: a label file's measurements go with it, and would
	// leave nothing to count under their own column.
	let count = |table: &str| -> rusqlite::Result<u64> {
		let n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE {}", owned(table)), params![id], |row| {
			row.get(0)
		})?;
		Ok(n.max(0) as u64)
	};
	let gone = Rows {
		label_files: count("label_file")?,
		measurements: count("measurement")?,
		readings: count("reading")?,
		faults: count("fault")?,
	};
	for table in owning_tables() {
		delete_rows(conn, table, owned(table), id)?;
	}
	conn.execute("DELETE FROM source WHERE id = ?1", params![id])?;
	Ok(gone)
}

/// The VCDS installation `labels_dir` is is the project's one: every other VCDS source
/// goes, with everything under it. `setup`'s label step, and nothing else, says so — a
/// second installation replaces the first in a project, and the first one's row would
/// be left naming a folder and a language for nothing, which `faults` reads.
///
/// Not part of [`build_db`]: a build is also how a tool looks into some other folder's
/// label files, and that must never cost the project a source.
pub fn keep_only_vcds(db_path: &Path, labels_dir: &str) -> Result<(), Error> {
	let mut conn = open_existing(db_path)?;
	let tx = conn.transaction()?;
	let keep = source_id(&tx, VCDS, labels_dir)?;
	let others: Vec<i64> = {
		let mut stmt = tx.prepare("SELECT id FROM source WHERE kind = ?1 AND id <> ?2")?;
		stmt.query_map(params![VCDS, keep], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?
	};
	for other in others {
		remove_source(&tx, other, false)?;
	}
	tx.commit()?;
	Ok(())
}

/// Write what `setup` knows about a source on its row, stamped with the time now; the row
/// is made if this is the first thing written for the source. `meta.parsed_at` is ignored:
/// the stamp is this write's.
pub fn record_meta(db_path: &Path, kind: &str, dir: &str, meta: &Meta) -> Result<(), Error> {
	let mut conn = Connection::open(db_path)?;
	create_schema(&conn, db_path)?;
	let tx = conn.transaction()?;
	let source = source_id(&tx, kind, dir)?;
	tx.execute(
		"UPDATE source SET root = ?1, version = ?2, detail = ?3, parsed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?4",
		params![meta.root, meta.version, meta.detail, source],
	)?;
	tx.commit()?;
	Ok(())
}

/// Fill in what a source's row does not say yet from `meta` — what an older note about
/// the source recorded — leaving everything it does say as it is.
pub fn adopt_meta(db_path: &Path, id: i64, meta: &Meta) -> Result<(), Error> {
	let conn = open_existing(db_path)?;
	conn.execute(
		"UPDATE source SET root = COALESCE(root, ?1), version = COALESCE(version, ?2), \
		 detail = COALESCE(detail, ?3), parsed_at = COALESCE(parsed_at, ?4) WHERE id = ?5",
		params![meta.root, meta.version, meta.detail, meta.parsed_at, id],
	)?;
	Ok(())
}

/// What a write replaces before it inserts.
///
/// **Two answers, because there are two callers with two meanings.** `setup`
/// hands over a whole project, and what that project no longer has must go with
/// what it has: a variant whose table now refuses or comes back empty, or that
/// a re-downloaded project at the same folder simply does not carry, is not in
/// the batch at all, and deleting per variant in the batch left last run's rows
/// for it — rows the variant match still picked and the counts still counted. A
/// single-variant write is a statement about that variant alone.
#[derive(Clone, Copy)]
enum Replace {
	/// Every row this source wrote, whatever variant.
	Source,
	/// Every row any source of this kind wrote: a VCDS install's registry rows
	/// replace the last install's, as its label files do.
	Kind,
	/// Only the rows of each variant in the batch.
	EachVariant,
}

/// Replace one variant's channels from one ODIS source, and write these.
///
/// Replace rather than append: a second parse of the same project is a *reread*,
/// not a second opinion, and appending would double every channel. Scoped to
/// this source and this variant, so the source's other variants, another
/// project's rows and a VCDS parse of the same car are all untouched — design
/// §4.5's "an ODIS parse never deletes VCDS-derived rows or vice versa".
///
/// Returns how many channels landed.
pub fn put_readings(db_path: &Path, project_dir: &str, variant: &str, readings: &[vag_data_labels::odis::Reading]) -> Result<usize, Error> {
	write_readings(db_path, project_dir, ODIS, std::iter::once((variant, readings)), Replace::EachVariant)
}

/// Everything one ODIS source says about channels, in **one** transaction,
/// replacing everything that source said before.
///
/// **The batch is the source's whole contribution**: every channel row of this
/// source is deleted first — including variants not in the batch — and the rows
/// of every other source stay. On a first write that is the same rows, in the
/// same order, with the same ids as [`put_readings`] once per variant; that
/// equivalence was checked by diffing every row of a whole-project parse. What
/// differs is the cost: 669 transactions were 669 commits, each a journal write
/// and an fsync, and 2.4 s of a `vagcan setup` whose parse had come down to 10;
/// one transaction is a fraction of a second.
///
pub fn put_all_readings<'a>(
	db_path: &Path,
	project_dir: &str,
	variants: impl IntoIterator<Item = (&'a str, &'a [vag_data_labels::odis::Reading])>,
) -> Result<usize, Error> {
	write_readings(db_path, project_dir, ODIS, variants, Replace::Source)
}

/// Everything one VCDS install's measurement registry (`RM.rod`) says about
/// channels, in one transaction, replacing what **any** VCDS install said
/// before.
///
/// Any, not this one: two installs read into one project would otherwise both
/// answer for one file stem, and which of them a reader heard would be down to
/// row order. The last install read is the one, as it already is for the label
/// files. An ODIS project's rows are another kind of source and stay, and so do
/// the install's label files, which live in other tables. A variant here is the
/// stem of the unit's `.rod` file, which `F19E`/`F1A2` pick the same way they
/// pick an ODIS variant.
///
/// **Into a source that is already there, or nowhere.** `None`, with nothing written and
/// no source made, when the cache holds no VCDS source for `install_dir` — `setup`'s label
/// step makes it, and `vagcan setup --drop` removes it. A registry read is minutes long,
/// and one that was running when its source was dropped must not bring it back: the check
/// is inside the write's own transaction.
pub fn put_all_vcds_readings<'a>(
	db_path: &Path,
	install_dir: &str,
	variants: impl IntoIterator<Item = (&'a str, &'a [vag_data_labels::odis::Reading])>,
) -> Result<Option<usize>, Error> {
	if !db_path.is_file() {
		return Ok(None);
	}
	write_into(db_path, install_dir, VCDS, variants, Replace::Kind, false, READINGS)
}

/// Everything one ODIS source says about adaptation channels, in one transaction,
/// replacing everything that source said about them before — [`put_all_readings`] for
/// the variants' adaptation reads ([`vag_data_labels::odis::Project::adaptations`]),
/// into their own table. Returns how many landed.
pub fn put_all_adaptations<'a>(
	db_path: &Path,
	project_dir: &str,
	variants: impl IntoIterator<Item = (&'a str, &'a [vag_data_labels::odis::Reading])>,
) -> Result<usize, Error> {
	let landed = write_into(db_path, project_dir, ODIS, variants, Replace::Source, true, ADAPTATIONS)?.unwrap_or_default();
	// Kept even when nothing landed: a project that describes no adaptations is read, not
	// one read before they were kept ([`odis_sources_without_adaptations`]).
	let conn = Connection::open(db_path)?;
	if let Some(source) = held_source_id(&conn, ODIS, project_dir)? {
		conn.execute(
			"UPDATE source SET adaptations_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1",
			params![source],
		)?;
	}
	Ok(landed)
}

/// The ODIS sources whose adaptation channels were never read — read before they were
/// kept — by directory. A source whose read found none is not among them.
pub fn odis_sources_without_adaptations(db_path: &Path) -> Result<Vec<String>, Error> {
	let conn = open_existing(db_path)?;
	let mut stmt = conn.prepare("SELECT dir FROM source WHERE kind = ?1 AND adaptations_at IS NULL ORDER BY id")?;
	let rows = stmt.query_map(params![ODIS], |row| row.get(0))?;
	Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Where one kind of channel is kept: its rows, its text-table states, and the column
/// the states name their row by. Every name is the schema's own.
#[derive(Clone, Copy)]
struct Channels {
	rows: &'static str,
	levels: &'static str,
	link: &'static str,
}

/// A unit's measurement channels.
const READINGS: Channels = Channels {
	rows: "reading",
	levels: "reading_level",
	link: "reading_id",
};

/// A unit's adaptation channels, as an ODIS project describes them.
const ADAPTATIONS: Channels = Channels {
	rows: "adaptation_channel",
	levels: "adaptation_level",
	link: "adaptation_id",
};

/// The writer behind [`put_readings`] and [`put_all_readings`], which make their source.
fn write_readings<'a>(
	db_path: &Path,
	source_dir: &str,
	kind: &str,
	variants: impl IntoIterator<Item = (&'a str, &'a [vag_data_labels::odis::Reading])>,
	replace: Replace,
) -> Result<usize, Error> {
	Ok(write_into(db_path, source_dir, kind, variants, replace, true, READINGS)?.unwrap_or_default())
}

/// The one writer behind [`write_readings`] and [`put_all_vcds_readings`]: `make` says
/// whether a source that is not there is made, or the write is `None`.
fn write_into<'a>(
	db_path: &Path,
	source_dir: &str,
	kind: &str,
	variants: impl IntoIterator<Item = (&'a str, &'a [vag_data_labels::odis::Reading])>,
	replace: Replace,
	make: bool,
	table: Channels,
) -> Result<Option<usize>, Error> {
	let Channels { rows, levels, link } = table;
	let mut conn = Connection::open(db_path)?;
	create_schema(&conn, db_path)?;
	conn.pragma_update(None, "synchronous", "FULL")?;
	// Pages, negative meaning kibibytes: 64 MiB, so a whole project's rows
	// stay in memory instead of being flushed page by page mid-transaction.
	conn.pragma_update(None, "cache_size", -65536)?;
	// Immediate, so the check below and the write are one: a drop cannot commit between.
	let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
	let source = match make {
		true => source_id(&tx, kind, source_dir)?,
		false => match held_source_id(&tx, kind, source_dir)? {
			Some(source) => source,
			None => return Ok(None),
		},
	};

	// The table names are [`Channels`]' constants, never anything a caller supplied.
	match replace {
		Replace::Source => {
			tx.execute(
				&format!("DELETE FROM {levels} WHERE {link} IN (SELECT id FROM {rows} WHERE source_id = ?1)"),
				params![source],
			)?;
			tx.execute(&format!("DELETE FROM {rows} WHERE source_id = ?1"), params![source])?;
		}
		Replace::Kind => {
			let of_kind = format!("SELECT id FROM {rows} WHERE source_id IN (SELECT id FROM source WHERE kind = ?1)");
			tx.execute(&format!("DELETE FROM {levels} WHERE {link} IN ({of_kind})"), params![kind])?;
			tx.execute(&format!("DELETE FROM {rows} WHERE id IN ({of_kind})"), params![kind])?;
		}
		Replace::EachVariant => {}
	}

	let mut written = 0usize;
	{
		let mut delete_levels = tx.prepare(&format!(
			"DELETE FROM {levels} WHERE {link} IN (SELECT id FROM {rows} WHERE source_id = ?1 AND variant = ?2)"
		))?;
		let mut delete = tx.prepare(&format!("DELETE FROM {rows} WHERE source_id = ?1 AND variant = ?2"))?;
		let mut insert = tx.prepare(&format!(
			"INSERT INTO {rows} \
                (source_id, variant, did, name, unit, bit_offset, bit_length, signed, big_endian, text_id, \
                 scaling, factor, offset, anchor_raw, anchor_value) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"
		))?;
		let mut insert_level = tx.prepare(&format!("INSERT INTO {levels} ({link}, raw, upper, meaning) VALUES (?1, ?2, ?3, ?4)"))?;
		for (variant, readings) in variants {
			if let Replace::EachVariant = replace {
				delete_levels.execute(params![source, variant])?;
				delete.execute(params![source, variant])?;
			}
			for r in readings {
				let (scaling, factor, offset, anchor_raw, anchor_value) = match &r.scaling {
					Scaling::Linear(s) => ("linear", Some(s.factor), Some(s.offset), None, None),
					Scaling::Enum { .. } => ("enum", None, None, None, None),
					Scaling::Anchor { raw, value } => ("anchor", None, None, Some(*raw), Some(*value)),
				};
				insert.execute(params![
					source,
					variant,
					r.did,
					r.name,
					r.unit,
					r.bit_offset,
					r.bit_length,
					r.signed,
					r.big_endian,
					r.text_id,
					scaling,
					factor,
					offset,
					anchor_raw,
					anchor_value
				])?;
				let id = tx.last_insert_rowid();
				if let Scaling::Enum { levels } = &r.scaling {
					for level in levels {
						insert_level.execute(params![id, level.lower(), level.upper(), level.name()])?;
					}
				}
				written += 1;
			}
		}
	}
	tx.commit()?;
	Ok(Some(written))
}

/// Every row a VCDS install's other registries give the units read, in one
/// transaction, replacing what **any** VCDS install wrote there before — the rule
/// [`put_all_vcds_readings`] follows, for the same reason. `variant` is the unit's
/// `.rod` file stem. Returns how many rows landed — or `None`, writing nothing, when the
/// project holds no source for this installation: like the channels, these rows never
/// make a source again that `setup --drop` removed while the read ran.
pub fn put_all_vcds_registry_rows<'a>(
	db_path: &Path,
	install_dir: &str,
	variants: impl IntoIterator<Item = (&'a str, &'a [vag_data_labels::registries::Described])>,
) -> Result<Option<usize>, Error> {
	use vag_data_labels::registries::Case;
	if !db_path.is_file() {
		return Ok(None);
	}
	let mut conn = Connection::open(db_path)?;
	create_schema(&conn, db_path)?;
	conn.pragma_update(None, "synchronous", "FULL")?;
	conn.pragma_update(None, "cache_size", -65536)?;
	// Immediate, so the check below and the write are one: a drop cannot commit between.
	let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
	let Some(source) = held_source_id(&tx, VCDS, install_dir)? else {
		return Ok(None);
	};
	let of_kind = "SELECT id FROM registry_row WHERE source_id IN (SELECT id FROM source WHERE kind = ?1)";
	tx.execute(&format!("DELETE FROM registry_level WHERE registry_row_id IN ({of_kind})"), params![VCDS])?;
	tx.execute(&format!("DELETE FROM registry_row WHERE id IN ({of_kind})"), params![VCDS])?;
	let mut written = 0usize;
	{
		let mut insert = tx.prepare(
			"INSERT INTO registry_row \
                (source_id, variant, section, line, key, identifier, ordinal, lead, case_lower, case_upper, \
                 kind, signed, big_endian, bit_offset, bit_length, factor, offset, table_id, constant, unit, name, text_id, raw) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
		)?;
		let mut insert_level = tx.prepare("INSERT INTO registry_level (registry_row_id, raw, upper, meaning) VALUES (?1, ?2, ?3, ?4)")?;
		for (variant, rows) in variants {
			for d in rows {
				let (e, v) = (&d.entry, &d.entry.value);
				let (lower, upper) = match e.case {
					Some(Case::Range { lower, upper }) => (Some(lower), Some(upper)),
					_ => (None, None),
				};
				insert.execute(params![
					source,
					variant,
					d.section,
					e.row as i64,
					e.key,
					e.identifier,
					e.ordinal,
					e.lead,
					lower,
					upper,
					v.kind,
					v.signed,
					v.big_endian,
					v.bit_offset,
					v.bit_length,
					v.scale.map(|s| s.0),
					v.scale.map(|s| s.1),
					v.table,
					v.constant,
					d.unit,
					d.name,
					d.text_id,
					e.raw
				])?;
				let id = tx.last_insert_rowid();
				for level in &d.levels {
					insert_level.execute(params![id, level.lower(), level.upper(), level.name()])?;
				}
				written += 1;
			}
		}
	}
	tx.commit()?;
	Ok(Some(written))
}

/// How many registry rows one kind of source wrote in each section, by section tag, in tag
/// order: an ODIS project's freeze-frame rows share the table with a VCDS install's.
pub fn registry_row_counts(db_path: &Path, kind: &str) -> Result<Vec<(String, u64)>, Error> {
	let conn = open_existing(db_path)?;
	let mut stmt = conn.prepare(
		"SELECT section, COUNT(*) FROM registry_row WHERE source_id IN (SELECT id FROM source WHERE kind = ?1) \
         GROUP BY section ORDER BY section",
	)?;
	let rows = stmt.query_map([kind], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?.max(0) as u64)))?;
	Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// One registry row as the cache keeps it: the columns `RA.rod` and its siblings were
/// established to mean (`research/vcds-registry/README.md` §9), with its states.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryEntry {
	/// The DID (ADP), routine id (GES) or input/output DID (SOT).
	pub identifier: Option<u32>,
	/// The field's 1-based place among one identifier's fields; `None` for a one-field one.
	pub ordinal: Option<u32>,
	/// `RM.rod`'s value type, `flags & 63` ([`vag_data_labels::registries::Value::kind`]).
	pub kind: Option<u8>,
	pub signed: bool,
	pub big_endian: bool,
	pub bit_offset: Option<u32>,
	pub bit_length: Option<u32>,
	/// `(factor, offset)`, for the linear and identity types.
	pub scale: Option<(f64, f64)>,
	pub unit: Option<String>,
	pub name: Option<String>,
	pub text_id: Option<String>,
	/// A text table's states, in the registry's order.
	pub levels: Vec<vag_data_labels::Level>,
}

/// One variant's rows of one registry section (`ADP`, …), in the order they were stored.
pub fn registry_entries(db_path: &Path, variant: &str, section: &str) -> Result<Vec<RegistryEntry>, Error> {
	let conn = open_existing(db_path)?;
	let mut stmt = conn.prepare(
		"SELECT id, identifier, ordinal, kind, signed, big_endian, bit_offset, bit_length, factor, offset, unit, name, text_id \
         FROM registry_row WHERE variant = ?1 AND section = ?2 ORDER BY id",
	)?;
	type Row = (i64, RegistryEntry);
	let rows: Vec<Row> = stmt
		.query_map(params![variant, section], |row| {
			let (factor, offset): (Option<f64>, Option<f64>) = (row.get(8)?, row.get(9)?);
			Ok((
				row.get(0)?,
				RegistryEntry {
					identifier: row.get(1)?,
					ordinal: row.get(2)?,
					kind: row.get(3)?,
					signed: row.get(4)?,
					big_endian: row.get(5)?,
					bit_offset: row.get(6)?,
					bit_length: row.get(7)?,
					scale: factor.map(|f| (f, offset.unwrap_or(0.0))),
					unit: row.get(10)?,
					name: row.get(11)?,
					text_id: row.get(12)?,
					levels: Vec::new(),
				},
			))
		})?
		.collect::<rusqlite::Result<_>>()?;
	let mut levels = conn.prepare("SELECT raw, upper, meaning FROM registry_level WHERE registry_row_id = ?1 ORDER BY rowid")?;
	let mut out = Vec::with_capacity(rows.len());
	for (id, mut entry) in rows {
		entry.levels = levels
			.query_map(params![id], |row| {
				Ok(vag_data_labels::Level::range(row.get(0)?, row.get(1)?, row.get::<_, String>(2)?))
			})?
			.collect::<rusqlite::Result<_>>()?;
		out.push(entry);
	}
	Ok(out)
}

/// Every variant (a unit's `.rod` stem) holding rows of one registry section, in name order.
pub fn registry_variants(db_path: &Path, section: &str) -> Result<Vec<String>, Error> {
	let conn = open_existing(db_path)?;
	let mut stmt = conn.prepare("SELECT DISTINCT variant FROM registry_row WHERE section = ?1 ORDER BY variant")?;
	let rows = stmt.query_map(params![section], |row| row.get(0))?;
	Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The section tag VCDS's freeze-frame rows are stored under (`RF.rod [FFMUX]`).
pub const FREEZE_FRAME_SECTION: &str = "FFMUX";

/// One field of one of a unit's extended-data records, as a VCDS install describes it
/// (`RF.rod [FFMUX]`, `research/odis-dtc/freeze-frames.md`).
#[derive(Debug, Clone, PartialEq)]
pub struct FreezeField {
	/// The extended-data record number the field belongs to, or `999` for the unit's
	/// default record — the layout of any record number the unit lists no rows for.
	pub record: u32,
	/// The last record number of a case that covers a range (an ODIS case's upper limit);
	/// `None` for a case of one number.
	pub record_upper: Option<u32>,
	/// The row's first field as written: for a VCDS row, 100 plus the record's length in
	/// bytes in 897 of `RF.rod`'s 1,028 records; for an ODIS row, 100 plus the length the
	/// project proves ([`put_all_odis_freeze_frames`]), `None` where it proves none.
	pub lead: Option<u32>,
	/// `RM.rod`'s value type: 0 linear, 2 identity, 3 text table, 6 multiplexed, 7 raw
	/// bytes, 11 a reserved field VCDS does not show.
	pub kind: Option<u8>,
	pub signed: bool,
	pub big_endian: bool,
	pub bit_offset: Option<u32>,
	pub bit_length: Option<u32>,
	/// `(factor, offset)` for types 0 and 2.
	pub scale: Option<(f64, f64)>,
	pub unit: Option<String>,
	pub name: Option<String>,
	/// A text table's states that have words.
	pub levels: Vec<vag_data_labels::Level>,
}

/// The variants this cache holds freeze-frame rows for from one kind of source.
pub fn freeze_frame_variants(db_path: &Path, kind: &str) -> Result<Vec<String>, Error> {
	let conn = open_existing(db_path)?;
	let mut stmt = conn.prepare(
		"SELECT DISTINCT variant FROM registry_row WHERE section = ?1 \
           AND source_id IN (SELECT id FROM source WHERE kind = ?2) ORDER BY variant",
	)?;
	let rows = stmt.query_map(params![FREEZE_FRAME_SECTION, kind], |row| row.get::<_, String>(0))?;
	Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// One variant's freeze-frame fields from one kind of source, in the order its unit file
/// lists them. One kind only: the two sources are never read into one record.
pub fn freeze_frame_fields(db_path: &Path, kind: &str, variant: &str) -> Result<Vec<FreezeField>, Error> {
	let conn = open_existing(db_path)?;
	let mut stmt = conn.prepare(
		"SELECT id, key, lead, kind, signed, big_endian, bit_offset, bit_length, factor, offset, unit, name, case_upper \
         FROM registry_row WHERE variant = ?1 AND section = ?2 \
           AND source_id IN (SELECT id FROM source WHERE kind = ?3) ORDER BY id",
	)?;
	let mut levels = conn.prepare("SELECT raw, upper, meaning FROM registry_level WHERE registry_row_id = ?1 ORDER BY rowid")?;
	let rows: Vec<(i64, FreezeField)> = stmt
		.query_map(params![variant, FREEZE_FRAME_SECTION, kind], |row| {
			let factor: Option<f64> = row.get(8)?;
			let offset: Option<f64> = row.get(9)?;
			Ok((
				row.get(0)?,
				FreezeField {
					record: row.get(1)?,
					lead: row.get(2)?,
					kind: row.get(3)?,
					signed: row.get(4)?,
					big_endian: row.get(5)?,
					bit_offset: row.get(6)?,
					bit_length: row.get(7)?,
					scale: factor.zip(offset),
					unit: row.get(10)?,
					name: row.get(11)?,
					record_upper: row.get(12)?,
					levels: Vec::new(),
				},
			))
		})?
		.collect::<rusqlite::Result<_>>()?;
	let mut out = Vec::with_capacity(rows.len());
	for (id, mut field) in rows {
		field.levels = levels
			.query_map(params![id], |row| {
				let lower: i32 = row.get(0)?;
				let upper: i32 = row.get(1)?;
				Ok(vag_data_labels::Level::range(lower, upper, row.get::<_, String>(2)?))
			})?
			.collect::<rusqlite::Result<_>>()?;
		out.push(field);
	}
	Ok(out)
}

/// Every ODIS variant's extended-data records, as `vag_data_labels::odis::Project::extended_records`
/// reads them, in one transaction, replacing every freeze-frame row this ODIS source wrote
/// before — and only those: a VCDS install's rows are another source's, never touched, and
/// never read together with these ([`freeze_frame_fields`] takes one kind).
///
/// A row per field, in the `registry_row` shape VCDS's rows have: `key` the record number
/// (`999` for the default case), `case_lower`/`case_upper` an ODIS case's range, `lead` 100
/// plus the record's proven length, `kind` 0 linear, 3 text table, 7 raw bytes, 11 a coded
/// constant. A record with no fields is kept as one constant row, for its length. Returns
/// how many rows landed. `--drop` of the source removes them with the rest of its rows.
pub fn put_all_odis_freeze_frames<'a>(
	db_path: &Path,
	project_dir: &str,
	variants: impl IntoIterator<Item = (&'a str, &'a [vag_data_labels::odis::EnvironmentRecord])>,
) -> Result<usize, Error> {
	const DEFAULT: u32 = 999;
	let mut conn = Connection::open(db_path)?;
	create_schema(&conn, db_path)?;
	conn.pragma_update(None, "synchronous", "FULL")?;
	conn.pragma_update(None, "cache_size", -65536)?;
	let tx = conn.transaction()?;
	let source = source_id(&tx, ODIS, project_dir)?;
	let mine = "SELECT id FROM registry_row WHERE source_id = ?1 AND section = ?2";
	tx.execute(
		&format!("DELETE FROM registry_level WHERE registry_row_id IN ({mine})"),
		params![source, FREEZE_FRAME_SECTION],
	)?;
	tx.execute(
		&format!("DELETE FROM registry_row WHERE id IN ({mine})"),
		params![source, FREEZE_FRAME_SECTION],
	)?;
	let mut written = 0usize;
	{
		let mut insert = tx.prepare(
			"INSERT INTO registry_row \
                (source_id, variant, section, line, key, case_lower, case_upper, lead, kind, signed, big_endian, \
                 bit_offset, bit_length, factor, offset, unit, name, text_id, raw) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, '')",
		)?;
		let mut insert_level = tx.prepare("INSERT INTO registry_level (registry_row_id, raw, upper, meaning) VALUES (?1, ?2, ?3, ?4)")?;
		for (variant, records) in variants {
			let mut line = 0i64;
			for record in records {
				let (lower, upper) = match record.range {
					Some((lower, upper)) => (u32::try_from(lower).ok(), u32::try_from(upper).ok()),
					None => (None, None),
				};
				let key = match (record.range, lower) {
					(None, _) => DEFAULT,
					(Some(_), Some(lower)) => lower,
					// A negative record number is not one a byte carries.
					(Some(_), None) => continue,
				};
				let lead = record.length.map(|l| l + 100);
				let constant = vag_data_labels::odis::EnvironmentField {
					name: None,
					text_id: None,
					unit: None,
					bit_offset: None,
					bit_length: None,
					signed: false,
					big_endian: true,
					scaling: None,
					constant: true,
				};
				let fields = match record.fields.is_empty() {
					true => std::slice::from_ref(&constant),
					false => record.fields.as_slice(),
				};
				for f in fields {
					line += 1;
					let (kind, scale) = match (&f.scaling, f.constant) {
						(_, true) => (11u8, None),
						(Some(Scaling::Linear(l)), _) => (0, Some((l.factor, l.offset))),
						(Some(Scaling::Enum { .. }), _) => (3, None),
						_ => (7, None),
					};
					insert.execute(params![
						source,
						variant,
						FREEZE_FRAME_SECTION,
						line,
						key,
						lower.filter(|_| record.range.is_some_and(|(l, u)| l != u)),
						upper.filter(|_| record.range.is_some_and(|(l, u)| l != u)),
						lead,
						kind,
						f.signed,
						f.big_endian,
						f.bit_offset,
						f.bit_length,
						scale.map(|s| s.0),
						scale.map(|s| s.1),
						f.unit,
						f.name,
						f.text_id,
					])?;
					let id = tx.last_insert_rowid();
					if let Some(Scaling::Enum { levels }) = &f.scaling {
						for level in levels {
							insert_level.execute(params![id, level.lower(), level.upper(), level.name()])?;
						}
					}
					written += 1;
				}
			}
		}
	}
	tx.commit()?;
	Ok(written)
}

/// One variant's rows of one section, as `(identifier, ordinal, name, raw, levels)` — for tests.
#[cfg(test)]
#[allow(clippy::type_complexity)]
fn registry_rows_of(db_path: &Path, variant: &str, section: &str) -> Result<Vec<(Option<u32>, Option<u32>, Option<String>, String, usize)>, Error> {
	let conn = open_existing(db_path)?;
	let mut stmt = conn.prepare(
		"SELECT identifier, ordinal, name, r.raw, (SELECT COUNT(*) FROM registry_level l WHERE l.registry_row_id = r.id) \
         FROM registry_row r WHERE variant = ?1 AND section = ?2 ORDER BY r.id",
	)?;
	let rows = stmt.query_map(params![variant, section], |row| {
		Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get::<_, i64>(4)? as usize))
	})?;
	Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Record the language a source declares for itself, on its `source` row.
///
/// Makes the row if this is the first thing written for the source, so it can
/// be called before or after the rows it describes. `language` is whatever the
/// source said — `deu` off an ODIS project's `index.xml`, `eng`/`rus` from
/// which fault-text file a VCDS build ships — and is stored as given.
pub fn record_language(db_path: &Path, kind: &str, dir: &str, language: &str) -> Result<(), Error> {
	let mut conn = Connection::open(db_path)?;
	create_schema(&conn, db_path)?;
	let tx = conn.transaction()?;
	let source = source_id(&tx, kind, dir)?;
	tx.execute("UPDATE source SET language = ?1 WHERE id = ?2", params![language, source])?;
	tx.commit()?;
	Ok(())
}

/// Replace one variant's fault codes from one ODIS source, and write these.
///
/// Same rule as [`put_readings`]: a reread replaces, scoped to this source and
/// this variant, so a second project's rows for a variant of the same name
/// stand untouched beside them. Returns how many codes landed.
pub fn put_faults(db_path: &Path, project_dir: &str, variant: &str, faults: &[vag_data_labels::odis::Fault]) -> Result<usize, Error> {
	write_faults(db_path, project_dir, std::iter::once((variant, faults)), Replace::EachVariant)
}

/// Every fault code one ODIS source describes, in **one** transaction,
/// replacing everything that source described before — the same batch, and the
/// same whole-source rule, [`put_all_readings`] is for channels, with the same
/// per-connection `synchronous = FULL` and page cache, for the same reasons.
pub fn put_all_faults<'a>(
	db_path: &Path,
	project_dir: &str,
	variants: impl IntoIterator<Item = (&'a str, &'a [vag_data_labels::odis::Fault])>,
) -> Result<usize, Error> {
	write_faults(db_path, project_dir, variants, Replace::Source)
}

/// The one writer behind [`put_faults`] and [`put_all_faults`].
fn write_faults<'a>(
	db_path: &Path,
	project_dir: &str,
	variants: impl IntoIterator<Item = (&'a str, &'a [vag_data_labels::odis::Fault])>,
	replace: Replace,
) -> Result<usize, Error> {
	let mut conn = Connection::open(db_path)?;
	create_schema(&conn, db_path)?;
	conn.pragma_update(None, "synchronous", "FULL")?;
	conn.pragma_update(None, "cache_size", -65536)?;
	let tx = conn.transaction()?;
	let source = source_id(&tx, ODIS, project_dir)?;
	if let Replace::Source = replace {
		tx.execute("DELETE FROM fault WHERE source_id = ?1", params![source])?;
	}
	let mut written = 0usize;
	{
		let mut delete = tx.prepare("DELETE FROM fault WHERE source_id = ?1 AND variant = ?2")?;
		let mut insert = tx.prepare(
			"INSERT INTO fault (source_id, variant, dop, code, display, text, text_id, short_name, level, temporary) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
		)?;
		for (variant, faults) in variants {
			if let Replace::EachVariant = replace {
				delete.execute(params![source, variant])?;
			}
			for f in faults {
				insert.execute(params![
					source,
					variant,
					f.dop,
					f.code,
					f.display_code,
					f.text,
					f.text_id,
					f.short_name,
					f.level,
					f.temporary
				])?;
				written += 1;
			}
		}
	}
	tx.commit()?;
	Ok(written)
}

/// One fault row as the cache hands it back: the fault, and the language its
/// source declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedFault {
	pub fault: vag_data_labels::odis::Fault,
	/// The `language` of the row's `source`, `None` where the source did not
	/// say or was written before the column existed.
	pub language: Option<String>,
	/// The source's directory, so two sources in different languages can be
	/// told apart in a message.
	pub source_dir: String,
}

/// The fault codes this cache knows for one ECU variant, by number.
///
/// Every source's rows, each with its source's language: choosing between
/// sources is the caller's policy, and it needs the language to make it.
///
/// **In source order, then write order within a source.** A source's id never
/// changes, and its rows' ids do on every reread — so write order alone put a
/// reread source behind the others and turned "the first source wins" into
/// "the last source reread wins".
pub fn faults_of(db_path: &Path, variant: &str) -> Result<Vec<CachedFault>, Error> {
	let conn = open_existing(db_path)?;
	let mut stmt = conn.prepare(
		"SELECT f.dop, f.code, f.display, f.text, f.text_id, f.short_name, f.level, f.temporary, s.language, s.dir \
         FROM fault f JOIN source s ON s.id = f.source_id \
         WHERE f.variant = ?1 ORDER BY s.id, f.rowid",
	)?;
	let rows = stmt.query_map(params![variant], |row| {
		Ok(CachedFault {
			fault: vag_data_labels::odis::Fault {
				dop: row.get(0)?,
				code: row.get(1)?,
				display_code: row.get(2)?,
				text: row.get(3)?,
				text_id: row.get(4)?,
				short_name: row.get(5)?,
				level: row.get(6)?,
				temporary: row.get(7)?,
			},
			language: row.get(8)?,
			source_dir: row.get(9)?,
		})
	})?;
	Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Every ECU variant this cache holds fault codes for, in name order.
pub fn fault_variants(db_path: &Path) -> Result<Vec<String>, Error> {
	let conn = open_existing(db_path)?;
	let mut stmt = conn.prepare("SELECT DISTINCT variant FROM fault ORDER BY variant")?;
	let rows = stmt.query_map([], |row| row.get(0))?;
	Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// How much fault text an ODIS source described: `(variants, codes)`.
pub fn fault_counts(db_path: &Path) -> Result<(u64, u64), Error> {
	let conn = open_existing(db_path)?;
	let (variants, codes): (i64, i64) = conn.query_row("SELECT COUNT(DISTINCT variant), COUNT(*) FROM fault", [], |row| {
		Ok((row.get(0)?, row.get(1)?))
	})?;
	Ok((variants.max(0) as u64, codes.max(0) as u64))
}

/// One `reading` row as SQLite hands it back, before it becomes a
/// [`vag_data_labels::odis::Reading`].
///
/// Named rather than written inline because the column list is fourteen wide
/// and a positional tuple that long is unreadable at the point it is consumed —
/// the alias is what lets the `SELECT` above and the destructuring below be
/// checked against each other by eye.
type ReadingRow = (
	i64,
	u16,
	String,
	Option<String>,
	u32,
	u32,
	bool,
	bool,
	Option<String>,
	String,
	Option<f64>,
	Option<f64>,
	Option<i32>,
	Option<f64>,
);

/// The channels this cache knows for one ECU variant, by identifier, every
/// source's together and unranked — for tests. A reader that ranks the sources
/// asks each with [`readings_of_kind`].
#[cfg(test)]
fn readings_of(db_path: &Path, variant: &str) -> Result<Vec<vag_data_labels::odis::Reading>, Error> {
	read_variant(db_path, variant, None, READINGS)
}

/// The channels one kind of source ([`ODIS`], [`VCDS`]) knows for one variant.
///
/// Asked per kind because the two kinds can spell a variant alike —
/// `EV_Brake1UDSContiMK100ESP_036` is both a VCDS file stem and an ODIS
/// variant — and a reader that ranks one source above the other has to be able
/// to ask for each on its own.
pub fn readings_of_kind(db_path: &Path, kind: &str, variant: &str) -> Result<Vec<vag_data_labels::odis::Reading>, Error> {
	read_variant(db_path, variant, Some(kind), READINGS)
}

/// The adaptation channels an ODIS project describes for one variant, by DID and field
/// ([`put_all_adaptations`]).
pub fn odis_adaptations_of(db_path: &Path, variant: &str) -> Result<Vec<vag_data_labels::odis::Reading>, Error> {
	read_variant(db_path, variant, Some(ODIS), ADAPTATIONS)
}

/// Every ECU variant an ODIS project holds adaptation channels for, in name order.
pub fn adaptation_variants(db_path: &Path) -> Result<Vec<String>, Error> {
	let conn = open_existing(db_path)?;
	let mut stmt = conn.prepare("SELECT DISTINCT variant FROM adaptation_channel ORDER BY variant")?;
	let rows = stmt.query_map([], |row| row.get(0))?;
	Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The reader behind [`readings_of`], [`readings_of_kind`] and [`odis_adaptations_of`].
fn read_variant(db_path: &Path, variant: &str, kind: Option<&str>, table: Channels) -> Result<Vec<vag_data_labels::odis::Reading>, Error> {
	let Channels { rows, levels, link } = table;
	let conn = open_existing(db_path)?;
	// The table names are [`Channels`]' constants, never anything a caller supplied.
	let mut stmt = conn.prepare(&format!(
		"SELECT id, did, name, unit, bit_offset, bit_length, signed, big_endian, text_id, \
                scaling, factor, offset, anchor_raw, anchor_value \
         FROM {rows} WHERE variant = ?1 \
           AND (?2 IS NULL OR source_id IN (SELECT id FROM source WHERE kind = ?2)) \
         ORDER BY did, bit_offset"
	))?;
	let found: Vec<ReadingRow> = stmt
		.query_map(params![variant, kind], |row| {
			Ok((
				row.get(0)?,
				row.get(1)?,
				row.get(2)?,
				row.get(3)?,
				row.get(4)?,
				row.get(5)?,
				row.get(6)?,
				row.get(7)?,
				row.get(8)?,
				row.get(9)?,
				row.get(10)?,
				row.get(11)?,
				row.get(12)?,
				row.get(13)?,
			))
		})?
		.collect::<rusqlite::Result<_>>()?;

	let mut levels = conn.prepare(&format!("SELECT raw, upper, meaning FROM {levels} WHERE {link} = ?1 ORDER BY rowid"))?;
	let mut out = Vec::with_capacity(found.len());
	for (id, did, name, unit, bit_offset, bit_length, signed, big_endian, text_id, kind, factor, offset, anchor_raw, anchor_value) in found {
		// A row whose scaling columns disagree with its kind is skipped rather
		// than repaired: a channel reported with a scaling nobody wrote is worse
		// than a channel not reported at all.
		let scaling = match (kind.as_str(), factor, offset, anchor_raw, anchor_value) {
			("linear", Some(factor), Some(offset), _, _) => Scaling::Linear(vag_data_labels::LinearScale { factor, offset }),
			("anchor", _, _, Some(raw), Some(value)) => Scaling::Anchor { raw, value },
			("enum", ..) => Scaling::Enum {
				levels: levels
					.query_map(params![id], |row| {
						let lower: i32 = row.get(0)?;
						let upper: Option<i32> = row.get(1)?;
						Ok(vag_data_labels::Level::range(lower, upper.unwrap_or(lower), row.get::<_, String>(2)?))
					})?
					.collect::<rusqlite::Result<_>>()?,
			},
			_ => continue,
		};
		out.push(vag_data_labels::odis::Reading {
			did,
			name,
			unit,
			bit_offset,
			bit_length,
			signed,
			big_endian,
			scaling,
			text_id,
		});
	}
	Ok(out)
}

/// Every text id this cache knows, with one name it is used under.
///
/// The seed for the owner's own glossary (`~/.vagcan/names.csv`): a person
/// writing their own wording needs the id to key it by and a reminder of what
/// the channel is currently called, or they are translating a list of opaque
/// keys. `MIN()` picks the name rather than an arbitrary row so two runs on one
/// cache produce the same file.
///
/// An id an ODIS row carries takes ODIS's name: a VCDS row can carry the id of
/// another field — several fields can show one name — and the reader gives the
/// id to ODIS's field (`vag-cli-core`'s `extracted`), so the reminder names that
/// one.
pub fn text_ids(db_path: &Path) -> Result<Vec<(String, String)>, Error> {
	let conn = open_existing(db_path)?;
	let mut stmt = conn.prepare(
		"SELECT r.text_id, COALESCE(MIN(CASE WHEN s.kind = ?1 THEN r.name END), MIN(r.name)) \
         FROM reading r LEFT JOIN source s ON s.id = r.source_id \
         WHERE r.text_id IS NOT NULL AND r.text_id <> '' GROUP BY r.text_id ORDER BY r.text_id",
	)?;
	let rows = stmt.query_map(params![ODIS], |row| Ok((row.get(0)?, row.get(1)?)))?;
	Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// How much of a car the sources described: `(variants, channels)`, an ODIS
/// project's rows and a VCDS install's registry rows together.
///
/// One query rather than `reading_variants(…).len()`, because a caller that
/// wants the size of the answer should not have to materialise it — a project
/// is hundreds of variant names and hundreds of thousands of channels, and the
/// only place this is asked from is a status line printed before anything else
/// happens. Both halves ride `idx_reading_lookup`, so it stays in milliseconds
/// on the owner's 92 MB cache.
///
/// `(0, 0)` for a project whose VCDS installation was read with no car yet
/// connected: its label files live in another table, which is D1's split, and
/// the registry rows come only for the units of the cars recorded on this
/// machine. [`row_counts`] is the per-table dump for somebody who wants that.
pub fn channel_counts(db_path: &Path) -> Result<(u64, u64), Error> {
	let conn = open_existing(db_path)?;
	let (variants, channels): (i64, i64) = conn.query_row("SELECT COUNT(DISTINCT variant), COUNT(*) FROM reading", [], |row| {
		Ok((row.get(0)?, row.get(1)?))
	})?;
	Ok((variants.max(0) as u64, channels.max(0) as u64))
}

/// Whether any state level of one ECU variant was written before this cache kept
/// the upper end of each level's interval: a cache an older vagcan added the column
/// to in place, and nothing has read the project into since.
///
/// Such a level reads as its lower end alone, which is what it meant when it was
/// written — so nothing reads wrongly, but a switch that answers inside a band
/// rather than on its lower end is shown as bytes. Only `setup` re-run against
/// the project can fix that, and this is how a command knows to say so.
///
/// Per variant rather than per cache, because a cache is a whole platform's
/// half a million levels and a question asked before a screen opens must not
/// scan them; this rides the two indexes a channel lookup already uses. A cache
/// written by `setup` today has `upper` on every row, so there it is `false`.
pub fn levels_predate_bounds(db_path: &Path, variant: &str) -> Result<bool, Error> {
	let conn = open_existing(db_path)?;
	let sql = "SELECT EXISTS (SELECT 1 FROM reading r JOIN reading_level l ON l.reading_id = r.id WHERE r.variant = ?1 AND l.upper IS NULL)";
	Ok(conn.query_row(sql, params![variant], |row| row.get(0))?)
}

/// Every ECU variant this cache holds readings for, in name order, every
/// source's together — for tests; see [`reading_variants_of`].
#[cfg(test)]
fn reading_variants(db_path: &Path) -> Result<Vec<String>, Error> {
	let conn = open_existing(db_path)?;
	let mut stmt = conn.prepare("SELECT DISTINCT variant FROM reading ORDER BY variant")?;
	let rows = stmt.query_map([], |row| row.get(0))?;
	Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// How many channels one kind of source ([`ODIS`], [`VCDS`]) holds.
pub fn reading_count_of(db_path: &Path, kind: &str) -> Result<u64, Error> {
	let conn = open_existing(db_path)?;
	let n: i64 = conn.query_row(
		"SELECT COUNT(*) FROM reading WHERE source_id IN (SELECT id FROM source WHERE kind = ?1)",
		params![kind],
		|row| row.get(0),
	)?;
	Ok(n.max(0) as u64)
}

/// Every ECU variant one kind of source ([`ODIS`], [`VCDS`]) holds readings
/// for, in name order.
pub fn reading_variants_of(db_path: &Path, kind: &str) -> Result<Vec<String>, Error> {
	let conn = open_existing(db_path)?;
	let mut stmt = conn.prepare(
		"SELECT DISTINCT variant FROM reading \
         WHERE source_id IN (SELECT id FROM source WHERE kind = ?1) ORDER BY variant",
	)?;
	let rows = stmt.query_map(params![kind], |row| row.get(0))?;
	Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Load all label files back out of a SQLite DB into a `Vec<LabelFile>`
/// (reconstructing `Record::Measurement`/`Redirect`/`Adaptation`/`LongCoding`).
pub fn load_files(db_path: &Path) -> Result<Vec<LabelFile>, Error> {
	let conn = open_existing(db_path)?;
	read_files(&conn)
}

/// Convenience: load a DB straight into a ready-to-query `LabelDb`.
/// Row counts per table, in the order a person wants to read them.
///
/// Here rather than in the utility that prints them because the table names
/// are this crate's schema: a binary that spelled them out itself would keep
/// compiling after a rename and fail only when run.
pub fn row_counts(db_path: &Path) -> Result<Vec<(&'static str, i64)>, Error> {
	const TABLES: [&str; 7] = [
		"label_file",
		"measurement",
		"redirect",
		"adaptation",
		"long_coding",
		"registry_row",
		"registry_level",
	];
	let conn = open_existing(db_path)?;
	let mut out = Vec::with_capacity(TABLES.len());
	// The registry tables hold an ODIS project's freeze-frame rows too: these are a VCDS
	// install's counts, so only a VCDS source's rows are counted there.
	let vcds_rows = "SELECT id FROM registry_row WHERE source_id IN (SELECT id FROM source WHERE kind = 'vcds')";
	for table in TABLES {
		// The names are the constant above, never anything a caller supplied,
		// so the format! cannot carry an injection.
		let query = match table {
			"registry_row" => format!("SELECT COUNT(*) FROM registry_row WHERE id IN ({vcds_rows})"),
			"registry_level" => format!("SELECT COUNT(*) FROM registry_level WHERE registry_row_id IN ({vcds_rows})"),
			_ => format!("SELECT COUNT(*) FROM {table}"),
		};
		let n: i64 = conn.query_row(&query, [], |row| row.get(0))?;
		out.push((table, n));
	}
	Ok(out)
}

pub fn load_db(db_path: &Path) -> Result<LabelDb, Error> {
	Ok(LabelDb::new(load_files(db_path)?))
}

#[cfg(test)]
mod tests {
	use super::*;
	use vag_data_labels::Level;
	use vag_data_labels::load_label_files;

	/// Same synthetic 80-byte `.clb` fixture used in `vag_data_labels::clb`'s tests
	/// (TEA-CBC-encrypted with `KEY_CLB`, `w7 = 7`) — no proprietary data.
	const FIXTURE_HEX: &str = "002738e02cf98f11742ee0b6f41102c2e55c4890aa526e2753a9263c7947f8b656f3467dc8f892f6c03a000a00202d7dc10402a81d837c41c4b66f69b6b50479e421595f5f5c20f4d6edd2d07b99000a";

	fn hex_decode(s: &str) -> Vec<u8> {
		assert_eq!(s.len() % 2, 0);
		(0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
	}

	/// A unique-per-test-run temp dir under the system temp dir, cleaned up
	/// on drop. Holds both the input labels dir and the output sqlite path.
	struct TempWorkspace {
		labels_dir: std::path::PathBuf,
		db_path: std::path::PathBuf,
	}

	impl TempWorkspace {
		fn new(tag: &str) -> Self {
			let root = std::env::temp_dir().join(format!("vag-db-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
			let labels_dir = root.join("labels");
			std::fs::create_dir_all(&labels_dir).unwrap();
			let db_path = root.join("cache.sqlite");
			TempWorkspace { labels_dir, db_path }
		}

		fn root(&self) -> std::path::PathBuf {
			self.labels_dir.parent().unwrap().to_path_buf()
		}
	}

	impl Drop for TempWorkspace {
		fn drop(&mut self) {
			let _ = std::fs::remove_dir_all(self.root());
		}
	}

	/// Populate a temp labels dir with one `.lbl` (measurement + redirect +
	/// adaptation + long-coding) and the shared `.clb` fixture (two
	/// measurements).
	fn write_fixture_labels(ws: &TempWorkspace) {
		std::fs::write(
			ws.labels_dir.join("index.lbl"),
			b"001,1,Engine Speed,(G28),Range: 0...6500 RPM\n\
              REDIRECT,target.lbl,022-906-032-C  ; a comment that is not persisted\n\
              A091,1,Some Channel,,desc\n\
              LC,02,0~7,02,Manufacturer: Audi",
		)
		.unwrap();
		std::fs::write(ws.labels_dir.join("target.lbl"), b"002,2,Coolant,(G62),Range: -48...143 C").unwrap();
		let clb_bytes = hex_decode(FIXTURE_HEX);
		std::fs::write(ws.labels_dir.join("fixture.clb"), &clb_bytes).unwrap();
	}

	fn measurements_of(lf: &LabelFile) -> Vec<&Measurement> {
		lf.records
			.iter()
			.filter_map(|r| match r {
				Record::Measurement(m) => Some(m),
				_ => None,
			})
			.collect()
	}

	fn redirects_of(lf: &LabelFile) -> Vec<(Option<&str>, &str)> {
		lf.records
			.iter()
			.filter_map(|r| match r {
				Record::Redirect { target, selector, .. } => Some((selector.as_deref(), target.as_str())),
				_ => None,
			})
			.collect()
	}

	fn adaptations_of(lf: &LabelFile) -> Vec<&Record> {
		lf.records.iter().filter(|r| matches!(r, Record::Adaptation { .. })).collect()
	}

	fn long_codings_of(lf: &LabelFile) -> Vec<&Record> {
		lf.records.iter().filter(|r| matches!(r, Record::LongCoding { .. })).collect()
	}

	#[test]
	fn round_trip_reconstructs_same_measurements_and_redirects_as_load_label_files() {
		let ws = TempWorkspace::new("roundtrip");
		write_fixture_labels(&ws);

		let live = load_label_files(&ws.labels_dir).expect("load_label_files should succeed");
		assert_eq!(live.files.len(), 3, "sanity: 2 .lbl + 1 .clb parsed");

		let stats = build_db(&ws.labels_dir, &ws.db_path).expect("build_db should succeed");
		assert_eq!(stats.files, 3);
		assert_eq!(stats.measurements, 4); // 1 + 1 + 2 from the .clb fixture
		assert_eq!(stats.redirects, 1);
		assert_eq!(stats.adaptations, 1);
		assert_eq!(stats.long_codings, 1);

		let cached = load_files(&ws.db_path).expect("load_files should succeed");
		assert_eq!(cached.len(), live.files.len());

		for live_file in &live.files {
			let cached_file = cached
				.iter()
				.find(|f| f.source == live_file.source)
				.unwrap_or_else(|| panic!("{} missing from cached label files", live_file.source));

			let live_m = measurements_of(live_file);
			let cached_m = measurements_of(cached_file);
			assert_eq!(live_m, cached_m, "measurements mismatch for {}", live_file.source);

			let live_r = redirects_of(live_file);
			let cached_r = redirects_of(cached_file);
			assert_eq!(live_r, cached_r, "redirects mismatch for {}", live_file.source);

			let live_a = adaptations_of(live_file);
			let cached_a = adaptations_of(cached_file);
			assert_eq!(live_a, cached_a, "adaptations mismatch for {}", live_file.source);

			let live_lc = long_codings_of(live_file);
			let cached_lc = long_codings_of(cached_file);
			assert_eq!(live_lc, cached_lc, "long codings mismatch for {}", live_file.source);
		}

		// index.lbl carries the one adaptation + one long-coding record;
		// assert the reconstructed values round-trip exactly.
		let index_cached = cached.iter().find(|f| f.source == "index.lbl").expect("index.lbl present");
		match adaptations_of(index_cached).as_slice() {
			[
				Record::Adaptation {
					channel,
					index,
					name,
					location,
					description,
				},
			] => {
				assert_eq!(channel, "A091");
				assert_eq!(index, "1");
				assert_eq!(name, "Some Channel");
				assert_eq!(location, "");
				assert_eq!(description, "desc");
			}
			other => panic!("expected exactly one Adaptation, got {other:?}"),
		}
		match long_codings_of(index_cached).as_slice() {
			[Record::LongCoding { byte, bits, value, meaning }] => {
				assert_eq!(byte, "02");
				assert_eq!(bits, "0~7");
				assert_eq!(value, "02");
				assert_eq!(meaning, "Manufacturer: Audi");
			}
			other => panic!("expected exactly one LongCoding, got {other:?}"),
		}
	}

	#[test]
	fn build_db_is_idempotent_on_rebuild() {
		let ws = TempWorkspace::new("rebuild");
		write_fixture_labels(&ws);

		let first = build_db(&ws.labels_dir, &ws.db_path).expect("first build_db should succeed");
		let second = build_db(&ws.labels_dir, &ws.db_path).expect("second build_db should succeed");

		assert_eq!(first, second, "row counts must not double on rebuild");

		let conn = Connection::open(&ws.db_path).unwrap();
		let count = |table: &str| -> i64 { conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0)).unwrap() };
		assert_eq!(count("label_file"), first.files as i64);
		assert_eq!(count("measurement"), first.measurements as i64);
		assert_eq!(count("redirect"), first.redirects as i64);
		assert_eq!(count("adaptation"), first.adaptations as i64);
		assert_eq!(count("long_coding"), first.long_codings as i64);
	}

	#[test]
	fn schema_has_file_id_indices_for_fast_reload() {
		// `read_files` runs `WHERE file_id = ?` once per label file per child
		// table; without these indices that is a full table scan per file
		// (O(files x rows) on the ~2900-file label files). Assert they exist.
		let ws = TempWorkspace::new("indices");
		write_fixture_labels(&ws);
		build_db(&ws.labels_dir, &ws.db_path).expect("build_db should succeed");

		let conn = Connection::open(&ws.db_path).unwrap();
		let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'index'").unwrap();
		let indices: Vec<String> = stmt.query_map([], |row| row.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();

		for required in [
			"idx_measurement_lookup",
			"idx_redirect_file",
			"idx_adaptation_file",
			"idx_long_coding_file",
		] {
			assert!(indices.iter().any(|n| n == required), "missing index {required}; present: {indices:?}");
		}
	}

	#[test]
	fn load_db_resolve_matches_live_lookup() {
		let ws = TempWorkspace::new("lookup");
		write_fixture_labels(&ws);

		build_db(&ws.labels_dir, &ws.db_path).expect("build_db should succeed");

		let live_db = LabelDb::new(load_label_files(&ws.labels_dir).unwrap().files);
		let cached_db = load_db(&ws.db_path).expect("load_db should succeed");

		// The redirect in index.lbl sends "022-906-032-C" to target.lbl.
		let live_resolved = live_db.resolve("022-906-032-C").expect("live resolve");
		let cached_resolved = cached_db.resolve("022-906-032-C").expect("cached resolve");
		assert_eq!(live_resolved.source, cached_resolved.source);

		let live_m = live_db.measurement("022-906-032-C", 2, 2).expect("live measurement");
		let cached_m = cached_db.measurement("022-906-032-C", 2, 2).expect("cached measurement");
		assert_eq!(live_m.name, "Coolant");
		assert_eq!(cached_m.name, "Coolant");
		assert_eq!(live_m.unit, cached_m.unit);
		assert_eq!(live_m.range, cached_m.range);
	}

	/// One channel, in the shape `vag_data_labels::odis` hands them over.
	fn reading(did: u16, name: &str, scaling: Scaling) -> vag_data_labels::odis::Reading {
		vag_data_labels::odis::Reading {
			did,
			name: name.to_string(),
			unit: None,
			bit_offset: 0,
			bit_length: 16,
			signed: false,
			// Little-endian, because that is what the reference car's own proven
			// row is and what a round trip most needs to preserve: the value
			// that differs from the UDS convention is the one a dropped column
			// would silently get wrong.
			big_endian: false,
			scaling,
			text_id: Some("000116".to_string()),
		}
	}

	#[test]
	fn one_transaction_for_every_variant_writes_the_rows_one_per_variant_wrote() {
		// The whole-project write is a batch of the per-variant one, and what
		// makes it safe to switch `setup` over is that nothing about the rows
		// changes: same rows, same order, same ids, same levels — and a second
		// run of either still *replaces* rather than doubling.
		let levels = Scaling::Enum {
			levels: vec![Level::point(0, "P"), Level::point(1, "R")],
		};
		let identity = Scaling::Linear(vag_data_labels::measure::LinearScale { factor: 1.0, offset: 0.0 });
		let ecm = vec![reading(0x380A, "speed", identity.clone()), reading(0x2000, "rpm", levels.clone())];
		let gearbox = vec![reading(0x380A, "gear", levels)];
		let batch = [("EV_ECM", ecm.as_slice()), ("EV_Gearbox", gearbox.as_slice())];

		let dump = |path: &Path, with_id: bool| -> Vec<(i64, String, u16, String, String)> {
			let conn = open_existing(path).unwrap();
			let mut stmt = conn
				.prepare(
					"SELECT r.id, r.variant, r.did, r.scaling, COALESCE(GROUP_CONCAT(l.raw || '=' || l.meaning, ','), '') \
                     FROM reading r LEFT JOIN reading_level l ON l.reading_id = r.id \
                     GROUP BY r.id ORDER BY r.variant, r.did, r.id",
				)
				.unwrap();
			stmt
				.query_map([], |row| {
					Ok((if with_id { row.get(0)? } else { 0 }, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
				})
				.unwrap()
				.collect::<rusqlite::Result<_>>()
				.unwrap()
		};

		let one_by_one = TempWorkspace::new("onebyone");
		put_readings(&one_by_one.db_path, "/x/SK37X", "EV_ECM", &ecm).unwrap();
		put_readings(&one_by_one.db_path, "/x/SK37X", "EV_Gearbox", &gearbox).unwrap();
		let batched = TempWorkspace::new("batched");
		assert_eq!(put_all_readings(&batched.db_path, "/x/SK37X", batch).unwrap(), 3);
		let (a, b) = (dump(&one_by_one.db_path, true), dump(&batched.db_path, true));
		assert_eq!(a.len(), 3, "two ECM rows and one gearbox row: {a:?}");
		assert_eq!(a, b, "the batch must write exactly what the per-variant calls wrote");

		// The reruns that replace. The ids move on — a replaced row is a new row
		// — and nothing else does.
		put_readings(&one_by_one.db_path, "/x/SK37X", "EV_ECM", &ecm).unwrap();
		put_all_readings(&batched.db_path, "/x/SK37X", batch).unwrap();
		let (a, b) = (dump(&one_by_one.db_path, false), dump(&batched.db_path, false));
		assert_eq!(a.len(), 3, "the rerun replaced rather than added: {a:?}");
		assert_eq!(a, b);
	}

	#[test]
	fn an_odis_channel_survives_the_cache_with_every_scaling_shape_intact() {
		// The three shapes are not interchangeable: a gear selector forced into
		// a linear scaling reports confident nonsense, and an anchor whose slope
		// is unproven must not come back as a slope of one.
		let ws = TempWorkspace::new("readings");
		let rows = [
			reading(
				0x380A,
				"Getriebe-Eingangsdrehzahl",
				Scaling::Linear(vag_data_labels::LinearScale { factor: 1.0, offset: 0.0 }),
			),
			reading(
				0x2000,
				"Ganganzeige",
				Scaling::Enum {
					levels: vec![Level::point(1, "R"), Level::point(2, "N")],
				},
			),
			reading(0x2001, "Kalibrierpunkt", Scaling::Anchor { raw: 4096, value: 12.5 }),
		];
		assert_eq!(put_readings(&ws.db_path, "/x/SK37X", "EV_ECM", &rows).unwrap(), 3);

		let back = readings_of(&ws.db_path, "EV_ECM").unwrap();
		assert_eq!(back.len(), 3);
		// Ordered by identifier, so 0x2000 comes first whatever order they went in.
		assert_eq!(back[0].did, 0x2000);
		assert_eq!(
			back[0].scaling,
			Scaling::Enum {
				levels: vec![Level::point(1, "R"), Level::point(2, "N")]
			}
		);
		assert_eq!(back[1].scaling, Scaling::Anchor { raw: 4096, value: 12.5 });
		// The design's §1 cross-check, round-tripped: DID 0x380A raw.
		assert_eq!(back[2].did, 0x380A);
		assert_eq!(
			back[2].scaling,
			Scaling::Linear(vag_data_labels::LinearScale { factor: 1.0, offset: 0.0 })
		);
		assert_eq!(back[2].text_id.as_deref(), Some("000116"), "the join to names.json survives");
		assert_eq!(reading_variants(&ws.db_path).unwrap(), ["EV_ECM"]);
	}

	#[test]
	fn byte_order_survives_the_cache_because_the_proven_row_disagrees_with_the_convention() {
		// UDS payloads are big-endian by convention and the reference car's own
		// proven row is not: DID 0x380A is `u16` little-endian
		// (`.archive/research/labels/rod-labels.md:433`, established byte by byte against
		// a log), and the ODIS file agrees. A cache that dropped this column
		// would throw the parser's correctness away at the storage layer, and a
		// reader would report 690 /min as 45570.
		let ws = TempWorkspace::new("endian");
		let identity = Scaling::Linear(vag_data_labels::LinearScale { factor: 1.0, offset: 0.0 });
		let mut little = reading(0x380A, "Getriebe-Eingangsdrehzahl", identity.clone());
		little.big_endian = false;
		let mut big = reading(0x2000, "Motordrehzahl", identity);
		big.big_endian = true;
		put_readings(&ws.db_path, "/x/SK37X", "EV_ECM", &[little, big]).unwrap();

		let back = readings_of(&ws.db_path, "EV_ECM").unwrap();
		assert_eq!(back.len(), 2);
		assert_eq!(back[0].did, 0x2000);
		assert!(back[0].big_endian, "a big-endian channel came back little-endian");
		assert_eq!(back[1].did, 0x380A);
		assert!(!back[1].big_endian, "the one channel a drive proved came back the wrong way round");
	}

	#[test]
	fn the_size_of_a_project_is_answered_without_reading_it_all_back() {
		// What the overview screen prints before any other work happens. A
		// cache with no ODIS source answers (0, 0) rather than failing: that is
		// a VCDS-only project, which is an ordinary state and not an error.
		let ws = TempWorkspace::new("counts");
		let identity = Scaling::Linear(vag_data_labels::LinearScale { factor: 1.0, offset: 0.0 });
		write_fixture_labels(&ws);
		build_db(&ws.labels_dir, &ws.db_path).unwrap();
		assert_eq!(channel_counts(&ws.db_path).unwrap(), (0, 0));

		put_readings(
			&ws.db_path,
			"/x/SK37X",
			"EV_ECM",
			&[reading(0x2000, "a", identity.clone()), reading(0x2001, "b", identity.clone())],
		)
		.unwrap();
		put_readings(&ws.db_path, "/x/SK37X", "EV_Gearbox", &[reading(0x380A, "c", identity)]).unwrap();
		assert_eq!(channel_counts(&ws.db_path).unwrap(), (2, 3), "two variants, three channels between them");
	}

	#[test]
	fn rereading_one_project_replaces_its_channels_rather_than_doubling_them() {
		let ws = TempWorkspace::new("reread");
		let rows = [reading(
			0x380A,
			"a",
			Scaling::Linear(vag_data_labels::LinearScale { factor: 1.0, offset: 0.0 }),
		)];
		put_readings(&ws.db_path, "/x/SK37X", "EV_ECM", &rows).unwrap();
		put_readings(&ws.db_path, "/x/SK37X", "EV_ECM", &rows).unwrap();
		assert_eq!(readings_of(&ws.db_path, "EV_ECM").unwrap().len(), 1);
		// And one source row, not one per run.
		assert_eq!(sources_of(&ws.db_path).unwrap(), [("odis".to_string(), "/x/SK37X".to_string())]);
	}

	#[test]
	fn the_glossary_hint_for_an_id_odis_carries_is_odis_s_name() {
		// A VCDS row can carry an id ODIS gives another field — several fields
		// can show one name — and the reader gives the id to ODIS's field. The
		// glossary's reminder of what a channel is called follows the same rule.
		let ws = TempWorkspace::new("hint");
		let linear = Scaling::Linear(vag_data_labels::LinearScale { factor: 1.0, offset: 0.0 });
		let with_id = |did, name: &str, id: &str| vag_data_labels::odis::Reading {
			text_id: Some(id.to_string()),
			..reading(did, name, linear.clone())
		};
		put_readings(&ws.db_path, "/x/SK37X", "EV_Unit_001", &[with_id(0x2203, "the odis name", "IDE90001")]).unwrap();
		record_language(&ws.db_path, VCDS, "/x/vcds-en", "eng").unwrap();
		put_all_vcds_readings(
			&ws.db_path,
			"/x/vcds-en",
			[(
				"EV_Unit_VW37",
				&[with_id(0x2CD1, "a vcds name", "IDE90001"), with_id(0x2CD2, "only vcds", "IDE90002")][..],
			)],
		)
		.unwrap();
		assert_eq!(
			text_ids(&ws.db_path).unwrap(),
			[
				("IDE90001".to_string(), "the odis name".to_string()),
				("IDE90002".to_string(), "only vcds".to_string())
			]
		);
	}

	#[test]
	fn a_vcds_install_and_an_odis_project_keep_their_channels_apart_under_one_variant_name() {
		// `EV_Brake1UDSContiMK100ESP_036` is both a VCDS file stem and an ODIS
		// variant: a reader ranking one source over the other must be able to
		// ask each for its own rows, and rereading one must not touch the other.
		let ws = TempWorkspace::new("kinds");
		let linear = |factor| Scaling::Linear(vag_data_labels::LinearScale { factor, offset: 0.0 });
		let variant = "EV_Brake1UDSContiMK100ESP_036";
		put_readings(&ws.db_path, "/x/SK37X", variant, &[reading(0x1800, "odis", linear(0.1))]).unwrap();
		record_language(&ws.db_path, VCDS, "/x/vcds-en", "eng").unwrap();
		record_language(&ws.db_path, VCDS, "/x/vcds-ru", "rus").unwrap();
		put_all_vcds_readings(
			&ws.db_path,
			"/x/vcds-en",
			[(
				variant,
				&[reading(0x1800, "vcds", linear(0.01)), reading(0x1822, "vcds", linear(1.0))][..],
			)],
		)
		.unwrap();

		assert_eq!(readings_of(&ws.db_path, variant).unwrap().len(), 3, "the plain read sees both sources");
		let odis = readings_of_kind(&ws.db_path, ODIS, variant).unwrap();
		assert_eq!(odis.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["odis"]);
		assert_eq!(readings_of_kind(&ws.db_path, VCDS, variant).unwrap().len(), 2);
		assert_eq!(reading_variants_of(&ws.db_path, VCDS).unwrap(), [variant]);
		assert_eq!(
			(reading_count_of(&ws.db_path, VCDS).unwrap(), reading_count_of(&ws.db_path, ODIS).unwrap()),
			(2, 1)
		);

		// Rereading the install replaces its rows and leaves the project's.
		put_all_vcds_readings(&ws.db_path, "/x/vcds-en", [(variant, &[reading(0x1822, "vcds", linear(1.0))][..])]).unwrap();
		assert_eq!(readings_of_kind(&ws.db_path, VCDS, variant).unwrap().len(), 1);
		assert_eq!(readings_of_kind(&ws.db_path, ODIS, variant).unwrap().len(), 1);

		// And so does reading another install: two installs answering for one
		// stem would leave which of them a reader heard to row order.
		put_all_vcds_readings(&ws.db_path, "/x/vcds-ru", [(variant, &[reading(0x1800, "ru", linear(0.01))][..])]).unwrap();
		let vcds = readings_of_kind(&ws.db_path, VCDS, variant).unwrap();
		assert_eq!(vcds.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["ru"]);
		assert_eq!(readings_of_kind(&ws.db_path, ODIS, variant).unwrap().len(), 1);
	}

	#[test]
	fn one_directory_spelled_two_ways_is_one_source() {
		// The bug this fixes, on the owner's own cache: `~/Downloads/SK37X/`
		// and `~/Downloads/SK37X` went in as two sources, so the project was
		// stored twice — 621,468 rows for 310,734 channels, each offered twice
		// to anything that looked one up.
		let ws = TempWorkspace::new("spelling");
		let dir = ws.labels_dir.to_string_lossy().into_owned();
		std::fs::create_dir_all(&ws.labels_dir).unwrap();
		let rows = [reading(
			0x380A,
			"a",
			Scaling::Linear(vag_data_labels::LinearScale { factor: 1.0, offset: 0.0 }),
		)];

		put_readings(&ws.db_path, &dir, "EV_ECM", &rows).unwrap();
		put_readings(&ws.db_path, &format!("{dir}/"), "EV_ECM", &rows).unwrap();
		// And a third spelling that means the same directory without being a
		// trailing slash, because fixing one alias and waiting for the next is
		// how this comes back.
		put_readings(&ws.db_path, &format!("{dir}/./"), "EV_ECM", &rows).unwrap();

		assert_eq!(sources_of(&ws.db_path).unwrap().len(), 1, "one directory, one source");
		assert_eq!(readings_of(&ws.db_path, "EV_ECM").unwrap().len(), 1, "and one copy of its channels");
	}

	#[test]
	fn a_cache_that_already_holds_the_duplicate_is_merged_rather_than_left() {
		// Somebody's cache is already in the broken state, and nothing else
		// will ever repair it: the rows are legal, no constraint is violated,
		// and every lookup just returns two of everything.
		let ws = TempWorkspace::new("merge");
		std::fs::create_dir_all(&ws.labels_dir).unwrap();
		let dir = ws.labels_dir.to_string_lossy().into_owned();
		let rows = [reading(
			0x380A,
			"a",
			Scaling::Linear(vag_data_labels::LinearScale { factor: 1.0, offset: 0.0 }),
		)];
		put_readings(&ws.db_path, &dir, "EV_ECM", &rows).unwrap();

		// Forge the second row the old code would have written.
		let conn = Connection::open(&ws.db_path).unwrap();
		conn
			.execute("INSERT INTO source (kind, dir) VALUES ('odis', ?1)", params![format!("{dir}/")])
			.unwrap();
		let stale: i64 = conn
			.query_row("SELECT id FROM source WHERE dir = ?1", params![format!("{dir}/")], |r| r.get(0))
			.unwrap();
		conn
			.execute(
				"INSERT INTO reading (source_id, variant, did, name, bit_offset, bit_length, signed, big_endian, scaling, factor, offset) \
				 VALUES (?1, 'EV_ECM', 14346, 'a', 0, 16, 0, 1, 'linear', 1.0, 0.0)",
				params![stale],
			)
			.unwrap();
		drop(conn);
		assert_eq!(readings_of(&ws.db_path, "EV_ECM").unwrap().len(), 2, "the broken state");

		// The next import of that directory, under either spelling, repairs it.
		put_readings(&ws.db_path, &format!("{dir}/"), "EV_ECM", &rows).unwrap();
		assert_eq!(sources_of(&ws.db_path).unwrap().len(), 1);
		assert_eq!(readings_of(&ws.db_path, "EV_ECM").unwrap().len(), 1);
	}

	#[test]
	fn both_sources_live_in_one_cache_and_neither_erases_the_other() {
		// Design §4.5: an ODIS parse never deletes VCDS-derived rows or the
		// reverse. They are addressed differently — block/field against
		// identifier — which is why D1 keeps them in two tables.
		let ws = TempWorkspace::new("both");
		write_fixture_labels(&ws);
		build_db(&ws.labels_dir, &ws.db_path).unwrap();
		let rows = [reading(
			0x380A,
			"a",
			Scaling::Linear(vag_data_labels::LinearScale { factor: 1.0, offset: 0.0 }),
		)];
		put_readings(&ws.db_path, "/x/SK37X", "EV_ECM", &rows).unwrap();

		// The VCDS side is still queryable exactly as before.
		let db = load_db(&ws.db_path).unwrap();
		assert_eq!(db.measurement("022-906-032-C", 2, 2).unwrap().name, "Coolant");
		// And so is the ODIS side.
		assert_eq!(readings_of(&ws.db_path, "EV_ECM").unwrap().len(), 1);

		let sources = sources_of(&ws.db_path).unwrap();
		assert_eq!(sources.len(), 2, "{sources:?}");
		assert!(sources.iter().any(|(kind, _)| kind == VCDS));
		assert!(sources.iter().any(|(kind, _)| kind == ODIS));
		// A VCDS row still answers "which label directory was this built from",
		// which is what the freshness rule asks — in the canonical spelling,
		// because that is what makes two spellings of one directory one row.
		// On macOS a temporary directory is reached through the `/var` symlink
		// and stored as `/private/var`, which is the same place said properly.
		let canonical = std::fs::canonicalize(&ws.labels_dir).unwrap();
		assert_eq!(source_of(&ws.db_path).as_deref(), Some(canonical.to_string_lossy().as_ref()));

		// Rebuilding the VCDS side leaves the ODIS channels where they are.
		build_db(&ws.labels_dir, &ws.db_path).unwrap();
		assert_eq!(
			readings_of(&ws.db_path, "EV_ECM").unwrap().len(),
			1,
			"the rebuild took the ODIS rows with it"
		);
	}

	fn fault(code: u32, text: &str) -> vag_data_labels::odis::Fault {
		vag_data_labels::odis::Fault {
			dop: "DTCDOP_VAGUDS".into(),
			code,
			display_code: Some("B1168F2".into()),
			text: Some(text.into()),
			text_id: Some("B1168F2".into()),
			short_name: Some(format!("DTC_{code}")),
			level: 2,
			temporary: false,
		}
	}

	#[test]
	fn a_variants_fault_codes_survive_the_cache_with_their_sources_language() {
		let ws = TempWorkspace::new("faults");
		let rows = [fault(297, "Lenkwinkelsensor"), fault(291_104, "Temperatursensor Lenkradheizung")];
		assert_eq!(put_faults(&ws.db_path, "/x/SK37X", "EV_Brake", &rows).unwrap(), 2);
		// Before the source says what language it is, the rows come back with
		// none — a `NULL`, not a guess.
		let cached = faults_of(&ws.db_path, "EV_Brake").unwrap();
		assert_eq!(cached.len(), 2);
		assert_eq!(cached[0].fault, rows[0]);
		assert_eq!(cached[0].language, None);

		record_language(&ws.db_path, ODIS, "/x/SK37X", "deu").unwrap();
		let cached = faults_of(&ws.db_path, "EV_Brake").unwrap();
		assert_eq!(cached[1].fault, rows[1]);
		assert_eq!(cached[1].language.as_deref(), Some("deu"));
		assert_eq!(cached[1].source_dir, "/x/SK37X");
		assert_eq!(fault_variants(&ws.db_path).unwrap(), ["EV_Brake"]);
		assert_eq!(fault_counts(&ws.db_path).unwrap(), (1, 2));
		// A variant nobody wrote is an empty answer, not an error.
		assert!(faults_of(&ws.db_path, "EV_Nobody").unwrap().is_empty());
	}

	#[test]
	fn a_whole_project_reread_drops_the_variants_it_no_longer_has_and_no_other_sources() {
		// The stale-row bug: `setup` hands over only the variants that read this
		// time. A variant whose fault table now refuses, is empty, or is gone
		// from a re-downloaded project at the same folder kept last run's rows —
		// which `fault_variants` still offered to the variant match and
		// `fault_counts` still counted. A reread of a source is that source's
		// whole contribution, so everything else it wrote goes. Another
		// project's rows for the same variant names are not its to drop.
		let ws = TempWorkspace::new("stale-faults");
		let (a, b) = ([fault(297, "a")], [fault(298, "b")]);
		put_all_faults(&ws.db_path, "/x/SK37X", [("EV_A", &a[..]), ("EV_B", &b[..])]).unwrap();
		put_all_faults(&ws.db_path, "/y/SK37X-eng", [("EV_B", &b[..])]).unwrap();

		put_all_faults(&ws.db_path, "/x/SK37X", [("EV_A", &a[..])]).unwrap();

		let from = |variant: &str| -> Vec<String> { faults_of(&ws.db_path, variant).unwrap().into_iter().map(|row| row.source_dir).collect() };
		assert_eq!(from("EV_A"), ["/x/SK37X"]);
		assert_eq!(from("EV_B"), ["/y/SK37X-eng"], "the reread kept a variant it no longer has");
		assert_eq!(fault_counts(&ws.db_path).unwrap(), (2, 2));
	}

	#[test]
	fn a_whole_project_reread_drops_the_channels_of_variants_it_no_longer_has() {
		// The same rule for channels, and their levels with them: a
		// `reading_level` left pointing at a deleted `reading` is a row nothing
		// can reach and something still stores.
		let ws = TempWorkspace::new("stale-readings");
		let levels = Scaling::Enum {
			levels: vec![Level::point(0, "P")],
		};
		let a = [reading(0x2000, "a", levels.clone())];
		let b = [reading(0x2001, "b", levels)];
		put_all_readings(&ws.db_path, "/x/SK37X", [("EV_A", &a[..]), ("EV_B", &b[..])]).unwrap();
		put_all_readings(&ws.db_path, "/y/SK37X-eng", [("EV_B", &b[..])]).unwrap();

		put_all_readings(&ws.db_path, "/x/SK37X", [("EV_A", &a[..])]).unwrap();

		assert_eq!(readings_of(&ws.db_path, "EV_A").unwrap().len(), 1);
		assert_eq!(
			readings_of(&ws.db_path, "EV_B").unwrap().len(),
			1,
			"the reread kept a variant it no longer has, or dropped another source's"
		);
		assert_eq!(channel_counts(&ws.db_path).unwrap(), (2, 2));
		let conn = open_existing(&ws.db_path).unwrap();
		let orphans: i64 = conn
			.query_row(
				"SELECT COUNT(*) FROM reading_level WHERE reading_id NOT IN (SELECT id FROM reading)",
				[],
				|row| row.get(0),
			)
			.unwrap();
		assert_eq!(orphans, 0, "levels outlived their channels");
	}

	#[test]
	fn rereading_a_variants_faults_replaces_them_and_leaves_another_sources_alone() {
		let ws = TempWorkspace::new("faults-reread");
		put_faults(&ws.db_path, "/x/SK37X", "EV_Brake", &[fault(297, "a"), fault(298, "b")]).unwrap();
		put_faults(&ws.db_path, "/y/SK37X-eng", "EV_Brake", &[fault(297, "steering angle")]).unwrap();
		record_language(&ws.db_path, ODIS, "/y/SK37X-eng", "eng").unwrap();
		// A reread of the first project replaces its two rows with one, and
		// the second project's row for the same variant is untouched.
		put_faults(&ws.db_path, "/x/SK37X", "EV_Brake", &[fault(297, "a2")]).unwrap();
		let cached = faults_of(&ws.db_path, "EV_Brake").unwrap();
		let texts: Vec<(Option<&str>, Option<&str>)> = cached.iter().map(|c| (c.fault.text.as_deref(), c.language.as_deref())).collect();
		// And the first project still comes first. A reread writes new rows,
		// which land after the second project's; handing rows back in write
		// order made "the first source written wins" mean "the last one reread"
		// — two runs of `faults` either side of a `setup` disagreeing about which
		// text a code gets, and the note naming a source that no longer won.
		assert_eq!(texts, [(Some("a2"), None), (Some("steering angle"), Some("eng"))]);
	}

	/// A state channel whose levels are a band and a point — synthetic values.
	fn banded(did: u16) -> vag_data_labels::odis::Reading {
		reading(
			did,
			"Schalter",
			Scaling::Enum {
				levels: vec![Level::range(10, 49, "gedrückt"), Level::point(50, "losgelassen")],
			},
		)
	}

	#[test]
	fn adaptations_are_kept_apart_from_channels_and_replaced_per_source() {
		// Synthetic rows. An adaptation is never a channel, and a channel never an
		// adaptation, whichever DID the two share.
		let ws = TempWorkspace::new("adaptations");
		put_all_readings(&ws.db_path, "/x/SK37X", [("EV_Unit", &[banded(0x1000)][..])]).unwrap();
		let first = [banded(0x0550), banded(0x1000)];
		assert_eq!(put_all_adaptations(&ws.db_path, "/x/SK37X", [("EV_Unit", &first[..])]).unwrap(), 2);
		assert_eq!(
			odis_adaptations_of(&ws.db_path, "EV_Unit").unwrap(),
			first,
			"layout, scaling and states come back"
		);
		assert_eq!(readings_of(&ws.db_path, "EV_Unit").unwrap(), [banded(0x1000)]);
		assert_eq!(adaptation_variants(&ws.db_path).unwrap(), ["EV_Unit"]);
		// A second read of the project replaces what it said, every variant of it.
		put_all_adaptations(&ws.db_path, "/x/SK37X", [("EV_Other", &[banded(0x0551)][..])]).unwrap();
		assert_eq!(adaptation_variants(&ws.db_path).unwrap(), ["EV_Other"]);
		// Another project's rows are its own.
		put_all_adaptations(&ws.db_path, "/y/SK38X", [("EV_Unit", &[banded(0x0552)][..])]).unwrap();
		assert_eq!(adaptation_variants(&ws.db_path).unwrap(), ["EV_Other", "EV_Unit"]);
		assert_no_orphans(&ws.db_path, "adaptations replaced");
	}

	#[test]
	fn a_levels_interval_survives_the_cache() {
		let ws = TempWorkspace::new("level-bounds");
		put_readings(&ws.db_path, "/x/SK37X", "EV_Switch", &[banded(0x1000)]).unwrap();
		let back = readings_of(&ws.db_path, "EV_Switch").unwrap();
		assert_eq!(back[0].scaling, banded(0x1000).scaling);
		assert!(
			!levels_predate_bounds(&ws.db_path, "EV_Switch").unwrap(),
			"written today, so every level has both ends"
		);
	}

	/// The shape a cache an older vagcan added `upper` to in place: the column is there,
	/// and the rows written before it hold `NULL` — their lower ends alone.
	fn null_upper(db_path: &Path) {
		let conn = Connection::open(db_path).unwrap();
		conn.execute("UPDATE reading_level SET upper = NULL", []).unwrap();
	}

	#[test]
	fn levels_without_their_upper_ends_read_as_points_and_say_so() {
		let ws = TempWorkspace::new("level-old");
		put_readings(&ws.db_path, "/x/SK37X", "EV_Switch", &[banded(0x1000)]).unwrap();
		put_readings(
			&ws.db_path,
			"/x/SK37X",
			"EV_Other",
			&[reading(
				0x2000,
				"a",
				Scaling::Linear(vag_data_labels::LinearScale { factor: 1.0, offset: 0.0 }),
			)],
		)
		.unwrap();
		null_upper(&ws.db_path);

		let back = readings_of(&ws.db_path, "EV_Switch").expect("the cache has every column, so it reads");
		assert_eq!(
			back[0].scaling,
			Scaling::Enum {
				levels: vec![Level::point(10, "gedrückt"), Level::point(50, "losgelassen")]
			}
		);
		assert!(
			levels_predate_bounds(&ws.db_path, "EV_Switch").unwrap(),
			"its levels are lower bounds only"
		);
		assert!(
			!levels_predate_bounds(&ws.db_path, "EV_Other").unwrap(),
			"a variant with no levels is not short of any"
		);
		// Reading the project again is what brings the intervals back.
		put_all_readings(&ws.db_path, "/x/SK37X", [("EV_Switch", &[banded(0x1000)][..])]).unwrap();
		assert!(!levels_predate_bounds(&ws.db_path, "EV_Switch").unwrap());
		assert_eq!(readings_of(&ws.db_path, "EV_Switch").unwrap()[0].scaling, banded(0x1000).scaling);
	}

	/// Every shape an older vagcan left a cache in, each one table or column short of
	/// this schema, built from today's cache by taking that part away.
	fn older_shapes() -> Vec<(&'static str, &'static str)> {
		vec![
			// Level intervals (2026-09-26).
			// Rebuilt: SQLite's `DROP COLUMN` trips over the comment after the last one.
			(
				"reading_level.upper",
				"ALTER TABLE reading_level RENAME TO reading_level_today;\
				 CREATE TABLE reading_level (reading_id INTEGER NOT NULL REFERENCES reading(id), raw INTEGER NOT NULL, meaning TEXT NOT NULL);\
				 DROP TABLE reading_level_today;",
			),
			// Byte order (2026-08-08).
			("reading.big_endian", "ALTER TABLE reading DROP COLUMN big_endian"),
			// The language a source declares, with the fault table (2026-09-10).
			("source.language", "ALTER TABLE source DROP COLUMN language"),
			("`fault` table", "DROP TABLE fault"),
			// The source a measurement came from (2026-08-07).
			// Rebuilt, as SQLite will not drop a column a foreign key names.
			(
				"measurement.source_id",
				"ALTER TABLE measurement RENAME TO measurement_today;				 CREATE TABLE measurement (file_id INTEGER NOT NULL, block INTEGER NOT NULL, field INTEGER NOT NULL, 				    name TEXT NOT NULL, location TEXT NOT NULL, description TEXT NOT NULL, unit TEXT, range_min REAL, range_max REAL);				 DROP TABLE measurement_today;",
			),
		]
	}

	#[test]
	fn a_cache_an_older_vagcan_wrote_is_refused_with_what_to_run() {
		let rows = [reading(
			0x380A,
			"a",
			Scaling::Linear(vag_data_labels::LinearScale { factor: 1.0, offset: 0.0 }),
		)];
		for (missing, take_away) in older_shapes() {
			let ws = TempWorkspace::new("older");
			put_readings(&ws.db_path, "/x/SK37X", "EV_ECM", &rows).unwrap();
			assert!(!is_older(&ws.db_path).unwrap(), "{missing}: today's cache is not older");
			Connection::open(&ws.db_path).unwrap().execute_batch(take_away).unwrap();
			assert!(is_older(&ws.db_path).unwrap(), "{missing}");

			// Every reader and every writer says the same, and none of them guesses.
			let before = std::fs::read(&ws.db_path).unwrap();
			let read = readings_of(&ws.db_path, "EV_ECM").unwrap_err().to_string();
			let write = put_readings(&ws.db_path, "/x/SK37X", "EV_ECM", &rows).unwrap_err().to_string();
			let faults = fault_counts(&ws.db_path).unwrap_err().to_string();
			for why in [&read, &write, &faults] {
				assert!(why.contains("older vagcan"), "{missing}: {why}");
				assert!(why.contains("vagcan setup --refresh"), "{missing}: {why}");
				assert!(why.contains(missing), "{missing}: {why}");
			}
			assert_eq!(source_of(&ws.db_path), None, "{missing}");
			// And the refused reads and write changed nothing, to the byte.
			assert_eq!(std::fs::read(&ws.db_path).unwrap(), before, "{missing}: the refusal touched the file");
			assert_eq!(check(&ws.db_path).unwrap_err().to_string(), write, "{missing}");

			// `setup --refresh` removes it, says what it was read from, and the next write
			// builds today's.
			assert_eq!(
				remove_if_older(&ws.db_path).unwrap(),
				Some(vec![("odis".to_string(), "/x/SK37X".to_string())]),
				"{missing}"
			);
			assert!(!ws.db_path.exists(), "{missing}");
			put_readings(&ws.db_path, "/x/SK37X", "EV_ECM", &rows).unwrap();
			assert_eq!(readings_of(&ws.db_path, "EV_ECM").unwrap().len(), 1, "{missing}");
			assert_eq!(remove_if_older(&ws.db_path).unwrap(), None, "{missing}: today's cache stays");
			assert!(ws.db_path.exists(), "{missing}");
		}
	}

	#[test]
	fn the_first_shape_of_the_cache_names_its_one_source_as_vcds() {
		// `source` was one row under `CHECK (id = 0)` with no `kind`: a VCDS parse.
		let ws = TempWorkspace::new("first-shape");
		Connection::open(&ws.db_path)
			.unwrap()
			.execute_batch(
				"CREATE TABLE label_file (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, unit_address INTEGER, unit_name TEXT);\
				 CREATE TABLE source (id INTEGER PRIMARY KEY CHECK (id = 0), dir TEXT NOT NULL);\
				 INSERT INTO source (id, dir) VALUES (0, '/old/Labels');",
			)
			.unwrap();
		assert!(check(&ws.db_path).unwrap_err().to_string().contains("older vagcan"));
		assert_eq!(
			remove_if_older(&ws.db_path).unwrap(),
			Some(vec![("vcds".to_string(), "/old/Labels".to_string())])
		);
		assert!(!ws.db_path.exists());
	}

	#[test]
	fn a_file_that_is_not_a_database_is_refused_and_refresh_replaces_it() {
		let ws = TempWorkspace::new("not-a-db");
		std::fs::write(&ws.db_path, b"this is not an SQLite file, and it is longer than a header").unwrap();
		for why in [
			readings_of(&ws.db_path, "EV_ECM").unwrap_err().to_string(),
			put_readings(&ws.db_path, "/x/SK37X", "EV_ECM", &[]).unwrap_err().to_string(),
		] {
			assert!(why.contains("is not a database"), "{why}");
			assert!(why.contains("vagcan setup --refresh"), "{why}");
		}
		assert_eq!(remove_if_older(&ws.db_path).unwrap(), Some(Vec::new()));
		assert!(!ws.db_path.exists());
	}

	#[test]
	fn a_schema_that_fails_part_way_leaves_no_table_behind() {
		// The batch's last statement made to fail: an index named like a table that is
		// already there. Without the one transaction, every table before it would stay,
		// and the next open would find a cache short of an index.
		let ws = TempWorkspace::new("half-schema");
		Connection::open(&ws.db_path)
			.unwrap()
			.execute_batch("CREATE TABLE idx_reading_level (x INTEGER);")
			.unwrap();
		assert!(put_readings(&ws.db_path, "/x/SK37X", "EV_ECM", &[]).is_err());
		let conn = Connection::open(&ws.db_path).unwrap();
		assert!(columns(&conn, "source").unwrap().is_empty(), "the batch left `source` behind");
		assert!(columns(&conn, "reading").unwrap().is_empty(), "the batch left `reading` behind");
	}

	/// A registry row as `vag_data_labels::registries` hands it over. Invented.
	fn described(section: &'static str, identifier: u32, levels: usize) -> vag_data_labels::registries::Described {
		use vag_data_labels::registries::{Described, Entry, Value};
		Described {
			section,
			entry: Entry {
				row: 7,
				key: 930_001,
				identifier: Some(identifier),
				ordinal: Some(1),
				lead: None,
				case: None,
				value: Value {
					kind: Some(3),
					big_endian: true,
					bit_offset: Some(0),
					bit_length: Some(8),
					table: Some(51),
					..Default::default()
				},
				raw: format!("1,{identifier},51,3,,,,,0,0,8,,,40ABCD,0033"),
			},
			name: Some("invented".into()),
			text_id: Some("IDE90001".into()),
			unit: None,
			levels: (0..levels).map(|n| Level::point(n as i32, format!("state {n}"))).collect(),
		}
	}

	#[test]
	fn registry_rows_are_written_with_their_states_and_a_vcds_read_replaces_every_vcds_installs() {
		let ws = TempWorkspace::new("registry-rows");
		let a = [described("ADP", 0x1075, 2), described("GES", 0x0317, 0)];
		let b = [described("ADP", 0x1076, 0)];
		// No cache, then no source for the installation: nothing is written, nothing made.
		assert_eq!(
			put_all_vcds_registry_rows(&ws.db_path, "/x/vcds-en", [("EV_UnitA", &a[..])]).unwrap(),
			None
		);
		assert!(!ws.db_path.exists());
		record_language(&ws.db_path, VCDS, "/x/vcds-en", "eng").unwrap();
		record_language(&ws.db_path, VCDS, "/y/vcds-ru", "rus").unwrap();
		assert_eq!(put_all_vcds_registry_rows(&ws.db_path, "/z/gone", [("EV_UnitA", &a[..])]).unwrap(), None);
		assert_eq!(sources(&ws.db_path).unwrap().len(), 2, "the write made a source");
		assert_eq!(
			put_all_vcds_registry_rows(&ws.db_path, "/x/vcds-en", [("EV_UnitA", &a[..]), ("EV_UnitB", &b[..])]).unwrap(),
			Some(3)
		);
		assert_eq!(
			registry_rows_of(&ws.db_path, "EV_UnitA", "ADP").unwrap(),
			vec![(
				Some(0x1075),
				Some(1),
				Some("invented".to_string()),
				"1,4213,51,3,,,,,0,0,8,,,40ABCD,0033".to_string(),
				2
			)]
		);
		assert_eq!(
			registry_row_counts(&ws.db_path, VCDS).unwrap(),
			vec![("ADP".to_string(), 2), ("GES".to_string(), 1)]
		);
		// Read back as the columns were established to mean, states and all.
		assert_eq!(
			registry_entries(&ws.db_path, "EV_UnitA", "ADP").unwrap(),
			[RegistryEntry {
				identifier: Some(0x1075),
				ordinal: Some(1),
				kind: Some(3),
				signed: false,
				big_endian: true,
				bit_offset: Some(0),
				bit_length: Some(8),
				scale: None,
				unit: None,
				name: Some("invented".into()),
				text_id: Some("IDE90001".into()),
				levels: vec![Level::point(0, "state 0"), Level::point(1, "state 1")],
			}]
		);
		assert_eq!(registry_variants(&ws.db_path, "ADP").unwrap(), ["EV_UnitA", "EV_UnitB"]);
		assert_eq!(registry_variants(&ws.db_path, "GES").unwrap(), ["EV_UnitA"]);
		// An ODIS write beside it touches none of them.
		put_readings(&ws.db_path, "/x/SK37X", "EV_ECM", &[]).unwrap();
		assert_eq!(registry_row_counts(&ws.db_path, VCDS).unwrap().len(), 2);
		// Another install's read replaces every VCDS row, its states included.
		put_all_vcds_registry_rows(&ws.db_path, "/y/vcds-ru", [("EV_UnitB", &b[..])]).unwrap();
		assert_eq!(registry_row_counts(&ws.db_path, VCDS).unwrap(), vec![("ADP".to_string(), 1)]);
		assert!(registry_rows_of(&ws.db_path, "EV_UnitA", "ADP").unwrap().is_empty());
		let conn = Connection::open(&ws.db_path).unwrap();
		let levels: i64 = conn.query_row("SELECT COUNT(*) FROM registry_level", [], |r| r.get(0)).unwrap();
		assert_eq!(levels, 0, "the replaced rows' states went with them");
	}

	#[test]
	fn freeze_frame_fields_come_back_with_their_record_their_lead_and_their_states() {
		let ws = TempWorkspace::new("freeze-frames");
		let mut field = described(FREEZE_FRAME_SECTION, 0, 2);
		field.entry.key = 112;
		field.entry.identifier = None;
		field.entry.lead = Some(103);
		let mut volts = described(FREEZE_FRAME_SECTION, 0, 0);
		volts.entry.key = 112;
		volts.entry.identifier = None;
		volts.entry.lead = Some(103);
		volts.entry.value.kind = Some(0);
		volts.entry.value.bit_offset = Some(8);
		volts.entry.value.scale = Some((0.1, 0.0));
		volts.unit = Some("V".into());
		let other = described("ADP", 0x1075, 0);
		record_language(&ws.db_path, VCDS, "/x/vcds-en", "eng").unwrap();
		put_all_vcds_registry_rows(&ws.db_path, "/x/vcds-en", [("EV_UnitA", &[field, volts, other][..])]).unwrap();

		assert_eq!(freeze_frame_variants(&ws.db_path, VCDS).unwrap(), vec!["EV_UnitA".to_string()]);
		assert!(
			freeze_frame_variants(&ws.db_path, ODIS).unwrap().is_empty(),
			"one kind of source at a time"
		);
		assert!(freeze_frame_fields(&ws.db_path, ODIS, "EV_UnitA").unwrap().is_empty());
		let fields = freeze_frame_fields(&ws.db_path, VCDS, "EV_UnitA").unwrap();
		assert_eq!(fields.len(), 2, "the adaptation row is not a freeze-frame field");
		assert_eq!((fields[0].record, fields[0].lead, fields[0].kind), (112, Some(103), Some(3)));
		assert_eq!(fields[0].levels, vec![Level::point(0, "state 0"), Level::point(1, "state 1")]);
		assert_eq!(
			(fields[1].bit_offset, fields[1].scale, fields[1].unit.as_deref()),
			(Some(8), Some((0.1, 0.0)), Some("V"))
		);
		assert!(freeze_frame_fields(&ws.db_path, VCDS, "EV_UnitB").unwrap().is_empty());
	}

	#[test]
	fn odis_freeze_frames_are_their_own_sources_rows_and_go_when_it_is_dropped() {
		use vag_data_labels::odis::{EnvironmentField, EnvironmentRecord};
		let ws = TempWorkspace::new("odis-freeze-frames");
		let field = |name: &str, bit: u32, scaling: Option<Scaling>| EnvironmentField {
			name: Some(name.into()),
			text_id: None,
			unit: None,
			bit_offset: Some(bit),
			bit_length: Some(8),
			signed: false,
			big_endian: true,
			scaling,
			constant: false,
		};
		let records = [
			EnvironmentRecord {
				range: Some((112, 112)),
				name: Some("own".into()),
				length: Some(2),
				fields: vec![
					field(
						"volts",
						0,
						Some(Scaling::Linear(vag_data_labels::LinearScale { factor: 0.1, offset: 0.0 })),
					),
					field(
						"state",
						8,
						Some(Scaling::Enum {
							levels: vec![Level::point(0, "off"), Level::point(1, "on")],
						}),
					),
				],
			},
			EnvironmentRecord {
				range: Some((120, 125)),
				name: Some("a range".into()),
				length: None,
				fields: vec![field("raw", 0, None)],
			},
			EnvironmentRecord {
				range: None,
				name: Some("unknown freeze frame".into()),
				length: None,
				fields: Vec::new(),
			},
		];
		record_language(&ws.db_path, VCDS, "/x/vcds-en", "eng").unwrap();
		put_all_vcds_registry_rows(&ws.db_path, "/x/vcds-en", [("EV_UnitA", &[described(FREEZE_FRAME_SECTION, 0, 0)][..])]).unwrap();
		assert_eq!(
			put_all_odis_freeze_frames(&ws.db_path, "/x/SK37X", [("EV_UnitA_001", &records[..])]).unwrap(),
			4
		);

		assert_eq!(freeze_frame_variants(&ws.db_path, ODIS).unwrap(), vec!["EV_UnitA_001".to_string()]);
		let rows = freeze_frame_fields(&ws.db_path, ODIS, "EV_UnitA_001").unwrap();
		let shape = rows.iter().map(|r| (r.record, r.record_upper, r.lead, r.kind)).collect::<Vec<_>>();
		assert_eq!(
			shape,
			vec![
				(112, None, Some(102), Some(0)),
				(112, None, Some(102), Some(3)),
				(120, Some(125), None, Some(7)),
				(999, None, None, Some(11)),
			]
		);
		assert_eq!(rows[1].levels.len(), 2);
		// The VCDS rows are untouched and never read with them.
		assert_eq!(freeze_frame_fields(&ws.db_path, VCDS, "EV_UnitA").unwrap().len(), 1);
		assert!(freeze_frame_fields(&ws.db_path, VCDS, "EV_UnitA_001").unwrap().is_empty());

		// A second write replaces the first; dropping the source takes its rows.
		put_all_odis_freeze_frames(&ws.db_path, "/x/SK37X", [("EV_UnitA_001", &records[..1])]).unwrap();
		assert_eq!(freeze_frame_fields(&ws.db_path, ODIS, "EV_UnitA_001").unwrap().len(), 2);
		let odis = sources(&ws.db_path).unwrap().into_iter().find(|s| s.kind == ODIS).unwrap();
		drop_source(&ws.db_path, odis.id).unwrap();
		assert!(freeze_frame_variants(&ws.db_path, ODIS).unwrap().is_empty());
		assert_eq!(freeze_frame_fields(&ws.db_path, VCDS, "EV_UnitA").unwrap().len(), 1);
	}

	#[test]
	fn a_source_merged_into_another_hands_over_its_registry_rows_or_drops_a_second_copy() {
		let ws = TempWorkspace::new("registry-merge");
		let rows = [described("ADP", 0x1075, 1)];
		record_language(&ws.db_path, VCDS, "/x/vcds-en", "eng").unwrap();
		put_all_vcds_registry_rows(&ws.db_path, "/x/vcds-en", [("EV_UnitA", &rows[..])]).unwrap();
		let count = |conn: &Connection, source: i64| -> (i64, i64) {
			conn
				.query_row(
					"SELECT (SELECT COUNT(*) FROM registry_row WHERE source_id = ?1), \
				        (SELECT COUNT(*) FROM registry_level l JOIN registry_row r ON r.id = l.registry_row_id WHERE r.source_id = ?1)",
					params![source],
					|r| Ok((r.get(0)?, r.get(1)?)),
				)
				.unwrap()
		};
		let conn = Connection::open(&ws.db_path).unwrap();
		let first: i64 = conn.query_row("SELECT id FROM source", [], |r| r.get(0)).unwrap();
		// A second row for the same directory that holds nothing: the first's rows move to it.
		conn.execute("INSERT INTO source (kind, dir) VALUES ('vcds', '/x/vcds-en/')", []).unwrap();
		let keep = conn.last_insert_rowid();
		merge_into(&conn, keep, first).unwrap();
		assert_eq!(count(&conn, keep), (1, 1), "the only copy moved, its state with it");
		// A third that holds a copy of its own: that copy goes, with its state.
		conn
			.execute("INSERT INTO source (kind, dir) VALUES ('vcds', '/x/./vcds-en')", [])
			.unwrap();
		let other = conn.last_insert_rowid();
		conn
			.execute_batch(&format!(
				"INSERT INTO registry_row (source_id, variant, section, line, key, signed, big_endian, raw) VALUES ({other}, 'EV_UnitA', 'ADP', 7, 1, 0, 1, '');\
			 INSERT INTO registry_level (registry_row_id, raw, upper, meaning) VALUES (last_insert_rowid(), 0, 0, 'x');"
			))
			.unwrap();
		merge_into(&conn, keep, other).unwrap();
		assert_eq!(count(&conn, keep), (1, 1), "the second copy was dropped, not added");
		let total: (i64, i64) = conn
			.query_row(
				"SELECT (SELECT COUNT(*) FROM registry_row), (SELECT COUNT(*) FROM registry_level)",
				[],
				|r| Ok((r.get(0)?, r.get(1)?)),
			)
			.unwrap();
		assert_eq!(total, (1, 1), "nothing left behind");
		drop(conn);
		assert_no_orphans(&ws.db_path, "merged");
	}

	#[test]
	fn a_cache_from_before_the_adaptation_tables_is_upgraded_in_place_not_refused() {
		// Schema version 3: today's shape without `adaptation_channel` and `adaptation_level`.
		let ws = TempWorkspace::new("before-adaptations");
		put_readings(&ws.db_path, "/x/SK37X", "EV_BCM", &[banded(0x1000)]).unwrap();
		{
			let conn = Connection::open(&ws.db_path).unwrap();
			conn
				.execute_batch("DROP TABLE adaptation_level; DROP TABLE adaptation_channel; PRAGMA user_version = 3;")
				.unwrap();
		}
		assert!(check(&ws.db_path).is_ok(), "not refused");
		assert_eq!(readings_of(&ws.db_path, "EV_BCM").unwrap().len(), 1, "nothing lost");
		assert!(adaptation_variants(&ws.db_path).unwrap().is_empty(), "made, empty");
		let conn = Connection::open(&ws.db_path).unwrap();
		assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
		drop(conn);
		put_all_adaptations(&ws.db_path, "/x/SK37X", [("EV_BCM", &[banded(0x0550)][..])]).unwrap();
		assert_eq!(odis_adaptations_of(&ws.db_path, "EV_BCM").unwrap().len(), 1);
	}

	/// A project read before adaptations were kept is told from one whose read found none:
	/// the read is recorded even when no row lands.
	#[test]
	fn a_read_of_adaptations_is_recorded_even_when_none_land() {
		let ws = TempWorkspace::new("adaptations-read");
		put_readings(&ws.db_path, "/x/SK37X", "EV_BCM", &[banded(0x1000)]).unwrap();
		assert_eq!(odis_sources_without_adaptations(&ws.db_path).unwrap(), ["/x/SK37X"], "not read yet");
		put_all_adaptations(&ws.db_path, "/x/SK37X", std::iter::empty()).unwrap();
		assert!(odis_sources_without_adaptations(&ws.db_path).unwrap().is_empty(), "read, none found");
		assert!(adaptation_variants(&ws.db_path).unwrap().is_empty());
	}

	#[test]
	fn a_cache_from_before_the_registry_tables_is_upgraded_in_place_not_refused() {
		// Schema version 2: today's shape without `registry_row` and `registry_level`.
		let ws = TempWorkspace::new("before-registry-rows");
		let rows = [reading(
			0x380A,
			"a",
			Scaling::Linear(vag_data_labels::LinearScale { factor: 1.0, offset: 0.0 }),
		)];
		put_readings(&ws.db_path, "/x/SK37X", "EV_ECM", &rows).unwrap();
		{
			let conn = Connection::open(&ws.db_path).unwrap();
			conn
				.execute_batch("DROP TABLE registry_level; DROP TABLE registry_row; PRAGMA user_version = 2;")
				.unwrap();
			assert!(columns(&conn, "registry_row").unwrap().is_empty());
		}
		// Every reader opens it as it is, upgrading it on the way, and loses nothing.
		assert!(check(&ws.db_path).is_ok(), "not refused");
		assert_eq!(readings_of(&ws.db_path, "EV_ECM").unwrap().len(), 1);
		let conn = Connection::open(&ws.db_path).unwrap();
		assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
		assert!(!columns(&conn, "registry_row").unwrap().is_empty());
		assert!(!columns(&conn, "registry_level").unwrap().is_empty());
		drop(conn);
		assert_eq!(registry_row_counts(&ws.db_path, VCDS).unwrap(), vec![]);
		assert_eq!(remove_if_older(&ws.db_path).unwrap(), None, "and --refresh has nothing to remove");
		// The tables take rows.
		let a = [described("ADP", 0x1075, 1)];
		record_language(&ws.db_path, VCDS, "/x/vcds-en", "eng").unwrap();
		put_all_vcds_registry_rows(&ws.db_path, "/x/vcds-en", [("EV_UnitA", &a[..])]).unwrap();
		assert_eq!(registry_row_counts(&ws.db_path, VCDS).unwrap(), vec![("ADP".to_string(), 1)]);
	}

	#[test]
	fn a_cache_not_written_yet_is_not_an_older_one() {
		let ws = TempWorkspace::new("not-yet");
		assert!(!is_older(&ws.db_path).unwrap(), "no file");
		assert!(!ws.db_path.exists(), "asking made no file");
		std::fs::write(&ws.db_path, b"").unwrap();
		assert!(!is_older(&ws.db_path).unwrap(), "an empty file has none of the tables");
		put_readings(
			&ws.db_path,
			"/x/SK37X",
			"EV_ECM",
			&[reading(
				0x380A,
				"a",
				Scaling::Linear(vag_data_labels::LinearScale { factor: 1.0, offset: 0.0 }),
			)],
		)
		.expect("a write creates the tables in an empty file");
		assert_eq!(readings_of(&ws.db_path, "EV_ECM").unwrap().len(), 1);
	}

	#[test]
	fn the_cache_carries_the_label_files_unit_numbering() {
		// The numbering is what tells the tool that `44` is a power steering
		// unit on any VAG car. It has to survive the cache, or a second run
		// would silently fall back to the five built-in pairings.
		let ws = TempWorkspace::new("unitnumbers");
		std::fs::write(
			ws.labels_dir.join("6V-17.lbl"),
			b"; Component: J285 - Instrument Cluster (#17)\n001,1,Something,,",
		)
		.unwrap();
		std::fs::write(
			ws.labels_dir.join("5Q-44.lbl"),
			b"; Component: J500 - Power Steering (#44)\n001,1,Something,,",
		)
		.unwrap();

		build_db(&ws.labels_dir, &ws.db_path).expect("build_db should succeed");
		let cached = load_db(&ws.db_path).expect("load_db should succeed");
		let live = LabelDb::new(load_label_files(&ws.labels_dir).unwrap().files);

		assert_eq!(cached.unit_numbers(), live.unit_numbers());
		assert_eq!(cached.unit_name(0x44), Some("J500 - Power Steering"));
		assert_eq!(cached.unit_name(0x17), Some("J285 - Instrument Cluster"));
	}

	/// The environment variable that turns [`a_writer_that_dies_mid_transaction`]
	/// from a no-op into the dying writer. Holds the cache path.
	const DYING_WRITER: &str = "VAG_DATA_DB_DYING_WRITER";

	/// Not a test of its own: the child process
	/// [`a_setup_killed_mid_write_leaves_a_cache_every_reader_can_still_open`]
	/// re-executes this binary into. Without the variable it does nothing.
	///
	/// It is the writer `setup` is, killed where Ctrl-C kills it: inside one
	/// replacing transaction over the fault table, after the transaction has
	/// outgrown the page cache and started writing changed pages into the
	/// database file itself. On the reference project that happens inside
	/// [`put_all_faults`] and [`put_all_readings`] on their own — hundreds of
	/// thousands of rows — and here the cache is shrunk to ten pages so a few
	/// thousand rows do it. It matters: a journal whose transaction never touched
	/// the file is not hot under `synchronous = FULL`, and SQLite ignores it.
	/// `process::exit` runs no destructor, so the transaction is neither
	/// committed nor rolled back — the rollback journal is left on disk exactly
	/// as a SIGINT leaves it.
	#[test]
	fn a_writer_that_dies_mid_transaction() {
		let Some(db) = std::env::var_os(DYING_WRITER) else { return };
		let mut conn = Connection::open(std::path::PathBuf::from(db)).unwrap();
		conn.pragma_update(None, "synchronous", "FULL").unwrap();
		conn.pragma_update(None, "cache_size", 10).unwrap();
		let tx = conn.transaction().unwrap();
		tx.execute("DELETE FROM fault", []).unwrap();
		for code in 0..20_000 {
			tx.execute(
				"INSERT INTO fault (source_id, variant, dop, code, text, level, temporary) \
				 VALUES (1, 'EV_Brake', 'DTCDOP_VAGUDS', ?1, 'a replacement that never commits', 2, 0)",
				params![code],
			)
			.unwrap();
		}
		std::process::exit(42);
	}

	#[test]
	fn a_setup_killed_mid_write_leaves_a_cache_every_reader_can_still_open() {
		// The failure: `setup` interrupted inside a write transaction leaves
		// `cache.sqlite-journal` behind — a *hot* journal. The next connection has
		// to roll it back before it may read, and a read-only connection is not
		// allowed to, so every reader failed with "attempt to write a readonly
		// database" until the next setup. The callers turn that error into "no
		// ODIS fault names" and "no ODIS channels", silently.
		let ws = TempWorkspace::new("hotjournal");
		put_faults(&ws.db_path, "/x/SK37X", "EV_Brake", &[fault(297, "before")]).unwrap();
		record_language(&ws.db_path, ODIS, "/x/SK37X", "deu").unwrap();

		let status = std::process::Command::new(std::env::current_exe().unwrap())
			.args(["--exact", "tests::a_writer_that_dies_mid_transaction", "--test-threads=1", "--nocapture"])
			.env(DYING_WRITER, &ws.db_path)
			.status()
			.unwrap();
		assert_eq!(status.code(), Some(42), "the writer did not die where it was meant to");
		let journal = ws.db_path.with_file_name("cache.sqlite-journal");
		// The magic number SQLite writes at the head of a journal once that journal
		// is committed to — the mark that makes it hot, and an open roll it back.
		let head = std::fs::read(&journal).expect("sanity: a killed writer leaves its journal behind");
		assert_eq!(
			head.get(..8),
			Some(&[0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7][..]),
			"sanity: the journal is hot"
		);

		// Every reader answers, and answers with what was committed.
		let faults = faults_of(&ws.db_path, "EV_Brake").expect("faults_of on a cache with a hot journal");
		assert_eq!(faults.len(), 1, "the half-written transaction must have been rolled back");
		assert_eq!(faults[0].fault.text.as_deref(), Some("before"));
		assert_eq!(fault_variants(&ws.db_path).unwrap(), ["EV_Brake"]);
		assert_eq!(fault_counts(&ws.db_path).unwrap(), (1, 1));
		assert_eq!(source_languages(&ws.db_path).unwrap().len(), 1);
		assert!(readings_of(&ws.db_path, "EV_Brake").unwrap().is_empty());
		assert!(load_files(&ws.db_path).unwrap().is_empty());
		assert!(
			!journal.exists(),
			"the reader that opened it rolled the journal back: {:?} bytes",
			std::fs::metadata(&journal).map(|m| m.len())
		);
	}

	#[test]
	fn asking_a_cache_that_is_not_there_leaves_no_file_behind() {
		// A read that creates its own database writes a zero-byte `cache.sqlite`
		// into a project that has none, and that file then reads as "this
		// project has labels" to every caller that only tests whether it
		// exists — after which the next command dies on `no such table:
		// label_file` instead of working without names. Every read must fail
		// on the missing file and touch nothing.
		let ws = TempWorkspace::new("nocreate");
		assert!(!ws.db_path.is_file(), "sanity: the fixture wrote no cache");

		assert!(source_of(&ws.db_path).is_none());
		assert!(sources_of(&ws.db_path).is_err());
		assert!(readings_of(&ws.db_path, "EV_ECM").is_err());
		assert!(text_ids(&ws.db_path).is_err());
		assert!(channel_counts(&ws.db_path).is_err());
		assert!(reading_variants(&ws.db_path).is_err());
		assert!(load_files(&ws.db_path).is_err());
		assert!(row_counts(&ws.db_path).is_err());
		assert!(load_db(&ws.db_path).is_err());

		assert!(!ws.db_path.exists(), "a read created {}", ws.db_path.display());

		// And what is allowed to create one still does.
		write_fixture_labels(&ws);
		build_db(&ws.labels_dir, &ws.db_path).expect("build_db creates the cache it builds");
		assert!(ws.db_path.is_file());
		assert!(channel_counts(&ws.db_path).is_ok(), "the same reads work once it is there");
	}

	/// The cache's shape before [`SCHEMA_VERSION`] 1, as `master` wrote it on 2026-10-19:
	/// no source metadata, label files owned by nobody.
	const PREVIOUS_SCHEMA: &str = "
		CREATE TABLE label_file (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, unit_address INTEGER, unit_name TEXT);
		CREATE TABLE measurement (file_id INTEGER NOT NULL REFERENCES label_file(id), block INTEGER NOT NULL, field INTEGER NOT NULL,
		    name TEXT NOT NULL, location TEXT NOT NULL, description TEXT NOT NULL, unit TEXT, range_min REAL, range_max REAL,
		    source_id INTEGER REFERENCES source(id));
		CREATE TABLE redirect (file_id INTEGER NOT NULL REFERENCES label_file(id), selector TEXT, target TEXT NOT NULL);
		CREATE TABLE adaptation (file_id INTEGER NOT NULL REFERENCES label_file(id), channel TEXT NOT NULL, idx TEXT NOT NULL,
		    name TEXT NOT NULL, location TEXT NOT NULL, description TEXT NOT NULL);
		CREATE TABLE long_coding (file_id INTEGER NOT NULL REFERENCES label_file(id), byte TEXT NOT NULL, bits TEXT NOT NULL,
		    value TEXT NOT NULL, meaning TEXT NOT NULL);
		CREATE INDEX idx_measurement_lookup ON measurement(file_id, block, field);
		CREATE INDEX idx_redirect_selector  ON redirect(selector);
		CREATE INDEX idx_label_file_name    ON label_file(name);
		CREATE INDEX idx_redirect_file      ON redirect(file_id);
		CREATE INDEX idx_adaptation_file    ON adaptation(file_id);
		CREATE INDEX idx_long_coding_file   ON long_coding(file_id);
		CREATE TABLE source (id INTEGER PRIMARY KEY, kind TEXT NOT NULL, dir TEXT NOT NULL, language TEXT, UNIQUE (kind, dir));
		CREATE TABLE fault (source_id INTEGER NOT NULL REFERENCES source(id), variant TEXT NOT NULL, dop TEXT NOT NULL,
		    code INTEGER NOT NULL, display TEXT, text TEXT, text_id TEXT, short_name TEXT, level INTEGER NOT NULL, temporary INTEGER NOT NULL);
		CREATE INDEX idx_fault_lookup ON fault(variant, code);
		CREATE TABLE reading (id INTEGER PRIMARY KEY, source_id INTEGER NOT NULL REFERENCES source(id), variant TEXT NOT NULL,
		    did INTEGER NOT NULL, name TEXT NOT NULL, unit TEXT, bit_offset INTEGER NOT NULL, bit_length INTEGER NOT NULL,
		    signed INTEGER NOT NULL, big_endian INTEGER NOT NULL DEFAULT 1, text_id TEXT, scaling TEXT NOT NULL, factor REAL,
		    offset REAL, anchor_raw INTEGER, anchor_value REAL);
		CREATE TABLE reading_level (reading_id INTEGER NOT NULL REFERENCES reading(id), raw INTEGER NOT NULL, meaning TEXT NOT NULL, upper INTEGER);
		CREATE INDEX idx_reading_lookup ON reading(variant, did);
		CREATE INDEX idx_reading_level  ON reading_level(reading_id);
	";

	fn unit_scale(did: u16, name: &str) -> vag_data_labels::odis::Reading {
		reading(did, name, Scaling::Linear(vag_data_labels::LinearScale { factor: 1.0, offset: 0.0 }))
	}

	/// A cache in [`PREVIOUS_SCHEMA`]'s shape holding what the reference machine's held on
	/// 2026-10-20 (invented rows, the real shape): a VCDS source under a relative path left
	/// by a run in a checkout, the ODIS project, and the same VCDS installation again under
	/// its absolute path, which owns the label files. Plus what an older import left on
	/// some machines: the ODIS project a second time, spelled with a trailing slash, with
	/// a second copy of its rows.
	///
	/// Returns the ODIS project's and the installation's directories.
	fn previous_cache(ws: &TempWorkspace) -> (String, String) {
		let odis = ws.root().join("Downloads").join("SK37X");
		let labels = ws.root().join("vcds-en").join("Labels");
		std::fs::create_dir_all(&odis).unwrap();
		std::fs::create_dir_all(&labels).unwrap();
		let (odis, labels) = (normalise_dir(&odis.to_string_lossy()), normalise_dir(&labels.to_string_lossy()));
		let conn = Connection::open(&ws.db_path).unwrap();
		conn.execute_batch(PREVIOUS_SCHEMA).unwrap();
		conn
			.execute(
				"INSERT INTO source (id, kind, dir, language) VALUES (1, 'vcds', 'vendor/vcds-en/Labels', NULL)",
				[],
			)
			.unwrap();
		conn
			.execute(
				"INSERT INTO source (id, kind, dir, language) VALUES (2, 'odis', ?1, 'deu')",
				params![odis],
			)
			.unwrap();
		conn
			.execute(
				"INSERT INTO source (id, kind, dir, language) VALUES (3, 'vcds', ?1, 'eng')",
				params![labels],
			)
			.unwrap();
		conn
			.execute(
				"INSERT INTO source (id, kind, dir, language) VALUES (4, 'odis', ?1, NULL)",
				params![format!("{odis}/")],
			)
			.unwrap();
		for source in [2, 4] {
			conn
				.execute(
					"INSERT INTO fault (source_id, variant, dop, code, text, level, temporary) \
					 VALUES (?1, 'EV_Brake', 'DTCDOP_VAGUDS', 297, 'Fehler', 2, 0)",
					params![source],
				)
				.unwrap();
			conn
				.execute(
					"INSERT INTO reading (source_id, variant, did, name, bit_offset, bit_length, signed, big_endian, scaling) \
					 VALUES (?1, 'EV_Brake', 4096, 'Schalter', 0, 8, 0, 1, 'enum')",
					params![source],
				)
				.unwrap();
			let reading = conn.last_insert_rowid();
			conn
				.execute(
					"INSERT INTO reading_level (reading_id, raw, meaning, upper) VALUES (?1, 0, 'aus', 0)",
					params![reading],
				)
				.unwrap();
		}
		conn
			.execute_batch(
				"INSERT INTO reading (source_id, variant, did, name, bit_offset, bit_length, signed, big_endian, scaling, factor, offset) \
				     VALUES (3, 'EV_Brake1', 6178, 'vcds', 0, 16, 0, 1, 'linear', 1.0, 0.0);\
				 INSERT INTO label_file (id, name, unit_address, unit_name) VALUES (1, 'index.lbl', NULL, NULL);\
				 INSERT INTO measurement (file_id, block, field, name, location, description, source_id) \
				     VALUES (1, 1, 1, 'Engine Speed', '', '', 3);\
				 INSERT INTO redirect (file_id, selector, target) VALUES (1, '022-906-032-C', 'target.lbl');\
				 INSERT INTO adaptation (file_id, channel, idx, name, location, description) VALUES (1, 'A091', '1', 'x', '', '');\
				 INSERT INTO long_coding (file_id, byte, bits, value, meaning) VALUES (1, '02', '0~7', '02', 'x');",
			)
			.unwrap();
		(odis, labels)
	}

	/// Every row in the cache points at something that is there: a source, a label file, a
	/// reading. Asked of every foreign key [`SCHEMA`] declares, so a table added later is
	/// asked too.
	fn assert_no_orphans(db_path: &Path, context: &str) {
		let conn = Connection::open(db_path).unwrap();
		assert!(owning_tables().count() >= 4, "sanity: the schema's keys are read");
		for (table, column, parent) in schema_links() {
			let sql = format!("SELECT COUNT(*) FROM {table} WHERE {column} IS NOT NULL AND {column} NOT IN (SELECT id FROM {parent})");
			let orphans: i64 = conn.query_row(&sql, [], |row| row.get(0)).unwrap();
			assert_eq!(orphans, 0, "{context}: {table}.{column} names a {parent} that is gone");
		}
	}

	/// How many rows each table of [`SCHEMA`] holds.
	fn table_rows(db_path: &Path) -> Vec<(String, i64)> {
		let conn = Connection::open(db_path).unwrap();
		schema_shape()
			.iter()
			.map(|(table, _)| {
				let n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0)).unwrap();
				(table.clone(), n)
			})
			.collect()
	}

	fn label_rows(db_path: &Path) -> i64 {
		row_counts(db_path).unwrap().iter().map(|(_, n)| *n).sum()
	}

	#[test]
	fn a_cache_of_the_previous_shape_is_upgraded_in_place_and_keeps_every_row() {
		let ws = TempWorkspace::new("previous");
		let (odis, labels) = previous_cache(&ws);
		assert!(check(&ws.db_path).is_ok(), "the previous shape is upgraded, not refused");

		let sources = sources(&ws.db_path).unwrap();
		let shown: Vec<(i64, &str, &str, Option<&str>)> = sources
			.iter()
			.map(|s| (s.id, s.kind.as_str(), s.dir.as_str(), s.language.as_deref()))
			.collect();
		assert_eq!(
			shown,
			[(2, "odis", odis.as_str(), Some("deu")), (3, "vcds", labels.as_str(), Some("eng"))],
			"the trailing-slash ODIS row is folded into the first, the relative row that owned nothing is gone"
		);
		assert!(sources.iter().all(|s| !s.is_stale()));
		// One copy of the ODIS rows, levels included, and the VCDS rows untouched.
		assert_eq!(faults_of(&ws.db_path, "EV_Brake").unwrap().len(), 1);
		assert_eq!(readings_of_kind(&ws.db_path, ODIS, "EV_Brake").unwrap().len(), 1);
		assert_eq!(readings_of_kind(&ws.db_path, VCDS, "EV_Brake1").unwrap().len(), 1);
		assert_eq!(load_db(&ws.db_path).unwrap().len(), 1);
		// The label files belong to the source their measurements name.
		let (rows, unowned) = source_rows(&ws.db_path).unwrap();
		let expected = Rows {
			label_files: 1,
			measurements: 1,
			readings: 1,
			faults: 0,
		};
		assert_eq!(rows.get(&3).copied(), Some(expected));
		assert_eq!(rows.get(&2).map(|r| (r.faults, r.readings)), Some((1, 1)));
		assert_eq!(unowned, Rows::default());
		assert_eq!(
			source_of(&ws.db_path).as_deref(),
			Some(labels.as_str()),
			"the label files' source, not the first VCDS row"
		);
		assert_no_orphans(&ws.db_path, "after the upgrade");

		// Once: a second open finds today's shape and changes nothing.
		let before = std::fs::read(&ws.db_path).unwrap();
		assert_eq!(crate::sources(&ws.db_path).unwrap().len(), 2);
		assert_eq!(std::fs::read(&ws.db_path).unwrap(), before, "the upgrade ran again");
	}

	#[test]
	fn a_cache_upgraded_to_the_first_version_loses_the_rows_that_own_nothing_too() {
		// The reference machine's cache was upgraded to 1 before the empty rows were
		// removed (review of #37): its stale `vendor/vcds-en/Labels` row was still there.
		let ws = TempWorkspace::new("version-one");
		put_faults(&ws.db_path, "/x/SK37X", "EV_Brake", &[fault(297, "Fehler")]).unwrap();
		let conn = Connection::open(&ws.db_path).unwrap();
		conn
			.execute_batch("INSERT INTO source (kind, dir) VALUES ('vcds', 'vendor/vcds-en/Labels'); PRAGMA user_version = 1;")
			.unwrap();
		drop(conn);
		let held = sources(&ws.db_path).unwrap();
		assert_eq!(held.iter().map(|s| s.dir.as_str()).collect::<Vec<_>>(), ["/x/SK37X"]);
		let conn = Connection::open(&ws.db_path).unwrap();
		assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
	}

	#[test]
	fn a_relative_row_that_holds_rows_is_kept_as_stale_and_claims_nothing() {
		// No path is guessed to delete it: the rows under it are kept, and it is reported.
		// But it is no other source of its kind — dropping the real VCDS source is dropping
		// the last one, and takes the label rows nobody owns.
		let ws = TempWorkspace::new("stale-holds");
		let conn = Connection::open(&ws.db_path).unwrap();
		conn.execute_batch(PREVIOUS_SCHEMA).unwrap();
		conn
			.execute_batch(
				"INSERT INTO source (id, kind, dir) VALUES (1, 'vcds', 'vendor/vcds-en/Labels'), (2, 'vcds', '/x/vcds-en/Labels');\
				 INSERT INTO reading (source_id, variant, did, name, bit_offset, bit_length, signed, big_endian, scaling, factor, offset) \
				     VALUES (1, 'EV_A', 1, 'a', 0, 8, 0, 1, 'linear', 1.0, 0.0), (2, 'EV_A', 2, 'b', 0, 8, 0, 1, 'linear', 1.0, 0.0);",
			)
			.unwrap();
		drop(conn);
		let held = sources(&ws.db_path).unwrap();
		assert_eq!(held.len(), 2, "a relative row with rows under it is kept");
		assert!(held[0].is_stale());
		let conn = Connection::open(&ws.db_path).unwrap();
		conn.execute("INSERT INTO label_file (name) VALUES ('orphan.lbl')", []).unwrap();
		drop(conn);
		assert_eq!(
			drop_source(&ws.db_path, 2).unwrap().label_files,
			1,
			"the stale row counted as another VCDS source"
		);
	}

	#[test]
	fn a_relative_source_is_never_resolved_against_the_working_directory() {
		// `src` resolves from where the tests run, which is exactly the guess not to make:
		// the row was written by a run somewhere else.
		let ws = TempWorkspace::new("relative");
		let here = normalise_dir("src");
		assert!(Path::new(&here).is_absolute(), "sanity: `src` resolves from the test's directory");
		let conn = Connection::open(&ws.db_path).unwrap();
		conn.execute_batch(PREVIOUS_SCHEMA).unwrap();
		conn.execute("INSERT INTO source (id, kind, dir) VALUES (1, 'odis', 'src')", []).unwrap();
		conn
			.execute("INSERT INTO source (id, kind, dir) VALUES (2, 'odis', ?1)", params![here])
			.unwrap();
		// Each holds a row, or the upgrade would drop it as a note of nothing.
		conn
			.execute_batch(
				"INSERT INTO fault (source_id, variant, dop, code, level, temporary) VALUES (1, 'EV_A', 'D', 1, 0, 0), (2, 'EV_A', 'D', 1, 0, 0);",
			)
			.unwrap();
		drop(conn);
		assert_eq!(sources(&ws.db_path).unwrap().len(), 2, "the upgrade merged a relative row by guessing");
		// Nor does a write of the absolute directory claim the relative row.
		put_faults(&ws.db_path, &here, "EV_Brake", &[fault(297, "x")]).unwrap();
		let held = sources(&ws.db_path).unwrap();
		assert_eq!(held.len(), 2);
		assert!(held[0].is_stale() && held[0].dir == "src");
		// A path being imported is resolved where it was typed: `src` typed here is the
		// absolute row.
		let conn = Connection::open(&ws.db_path).unwrap();
		assert_eq!(source_id(&conn, ODIS, "src").unwrap(), held[1].id);
		// One that resolves nowhere is kept as typed, and that spelling is still its row.
		let first = source_id(&conn, VCDS, "vendor/vcds-en/Labels").unwrap();
		assert_eq!(source_id(&conn, VCDS, "vendor/vcds-en/Labels").unwrap(), first);
	}

	#[test]
	fn dropping_a_source_takes_every_row_it_wrote_and_nothing_else() {
		// One cache holding every kind of row: the VCDS label files, a VCDS registry's
		// channels, an ODIS project's channels with their levels, and its fault text.
		let ws = TempWorkspace::new("drop");
		write_fixture_labels(&ws);
		build_db(&ws.labels_dir, &ws.db_path).unwrap();
		let labels = sources(&ws.db_path).unwrap()[0].dir.clone();
		put_all_vcds_readings(&ws.db_path, &labels, [("EV_Brake1", &[unit_scale(0x1822, "vcds")][..])]).unwrap();
		put_all_vcds_registry_rows(&ws.db_path, &labels, [("EV_Brake1", &[described("ADP", 0x1075, 1)][..])]).unwrap();
		put_all_readings(&ws.db_path, "/x/SK37X", [("EV_Brake1", &[banded(0x1000)][..])]).unwrap();
		put_all_faults(&ws.db_path, "/x/SK37X", [("EV_Brake1", &[fault(297, "Fehler")][..])]).unwrap();
		put_all_adaptations(&ws.db_path, "/x/SK37X", [("EV_Brake1", &[banded(0x0550)][..])]).unwrap();
		// Its freeze-frame rows too, one a text table with its states in `registry_level`.
		let state = vag_data_labels::odis::EnvironmentField {
			name: Some("Sim state".into()),
			text_id: None,
			unit: None,
			bit_offset: Some(0),
			bit_length: Some(8),
			signed: false,
			big_endian: true,
			scaling: Some(Scaling::Enum {
				levels: vec![Level::point(0, "off"), Level::point(1, "on")],
			}),
			constant: false,
		};
		let frame = vag_data_labels::odis::EnvironmentRecord {
			range: Some((112, 112)),
			name: None,
			length: Some(1),
			fields: vec![state],
		};
		put_all_odis_freeze_frames(&ws.db_path, "/x/SK37X", [("EV_Brake1", std::slice::from_ref(&frame))]).unwrap();
		let all = sources(&ws.db_path).unwrap();
		let (vcds, odis) = (all[0].id, all[1].id);
		let pristine = std::fs::read(&ws.db_path).unwrap();
		let (rows, _) = source_rows(&ws.db_path).unwrap();
		// The fixture fills every table the schema has: a table added later that it does
		// not fill would pass the orphan check below while proving nothing. Fill it here.
		for (table, n) in table_rows(&ws.db_path) {
			assert!(n > 0, "the fixture leaves `{table}` empty: give it a row, so a drop is tested on it");
		}

		// The ODIS project: its channels, levels and fault text go; the VCDS rows stay
		// and are what every reader now answers with.
		let gone = drop_source(&ws.db_path, odis).unwrap();
		assert_eq!(gone, rows[&odis]);
		assert_eq!((gone.readings, gone.faults), (1, 1));
		assert_eq!(sources(&ws.db_path).unwrap().iter().map(|s| s.id).collect::<Vec<_>>(), [vcds]);
		assert!(fault_variants(&ws.db_path).unwrap().is_empty());
		assert!(readings_of_kind(&ws.db_path, ODIS, "EV_Brake1").unwrap().is_empty());
		assert!(
			odis_adaptations_of(&ws.db_path, "EV_Brake1").unwrap().is_empty(),
			"its adaptations went with it"
		);
		assert_eq!(readings_of_kind(&ws.db_path, VCDS, "EV_Brake1").unwrap().len(), 1);
		assert_eq!(load_db(&ws.db_path).unwrap().measurement("022-906-032-C", 2, 2).unwrap().name, "Coolant");
		let levels: i64 = Connection::open(&ws.db_path)
			.unwrap()
			.query_row("SELECT COUNT(*) FROM reading_level", [], |row| row.get(0))
			.unwrap();
		assert_eq!(levels, 0, "the dropped channel's levels stayed");
		assert!(
			freeze_frame_variants(&ws.db_path, ODIS).unwrap().is_empty(),
			"its freeze frames went with it"
		);
		assert_no_orphans(&ws.db_path, "ODIS dropped");
		assert_eq!(
			registry_row_counts(&ws.db_path, VCDS).unwrap(),
			vec![("ADP".to_string(), 1)],
			"the VCDS registry rows stay with their source"
		);

		// The VCDS installation, from the same starting point: every label table empties,
		// and the ODIS rows stay.
		std::fs::write(&ws.db_path, &pristine).unwrap();
		let gone = drop_source(&ws.db_path, vcds).unwrap();
		assert_eq!(gone, rows[&vcds]);
		assert!(gone.label_files > 0 && gone.measurements > 0 && gone.readings == 1, "{gone:?}");
		assert_eq!(label_rows(&ws.db_path), 0, "a label table kept rows");
		assert!(source_of(&ws.db_path).is_none());
		assert_eq!(faults_of(&ws.db_path, "EV_Brake1").unwrap().len(), 1);
		assert_eq!(readings_of_kind(&ws.db_path, ODIS, "EV_Brake1").unwrap().len(), 1);
		assert_eq!(odis_adaptations_of(&ws.db_path, "EV_Brake1").unwrap(), [banded(0x0550)]);
		assert!(
			registry_row_counts(&ws.db_path, VCDS).unwrap().is_empty(),
			"the registry rows went with it"
		);
		let frames = freeze_frame_fields(&ws.db_path, ODIS, "EV_Brake1").unwrap();
		assert_eq!(frames.len(), 1, "the ODIS freeze frames stay with their source");
		assert_eq!(frames[0].levels.len(), 2, "and their states with them");
		assert_no_orphans(&ws.db_path, "VCDS dropped");

		// An id that is not there is said as one.
		assert!(matches!(drop_source(&ws.db_path, vcds), Err(Error::NoSuchSource { .. })));
	}

	#[test]
	fn dropping_the_last_vcds_source_takes_label_files_nobody_owns() {
		// A cache upgraded with two VCDS rows and no measurement naming either leaves its
		// label files owned by nobody. They are VCDS's, and the last VCDS source takes them.
		let ws = TempWorkspace::new("unowned");
		let conn = Connection::open(&ws.db_path).unwrap();
		conn.execute_batch(PREVIOUS_SCHEMA).unwrap();
		conn
			.execute_batch(
				"INSERT INTO source (id, kind, dir) VALUES (1, 'vcds', '/a/Labels'), (2, 'vcds', '/b/Labels'), (3, 'odis', '/x/SK37X');\
				 INSERT INTO label_file (id, name) VALUES (1, 'index.lbl');\
				 INSERT INTO redirect (file_id, selector, target) VALUES (1, NULL, 'x.lbl');",
			)
			.unwrap();
		drop(conn);
		assert_eq!(source_rows(&ws.db_path).unwrap().1.label_files, 1, "owned by nobody");
		drop_source(&ws.db_path, 1).unwrap();
		assert_eq!(
			source_rows(&ws.db_path).unwrap().1.label_files,
			1,
			"another VCDS source may still claim them"
		);
		assert_eq!(drop_source(&ws.db_path, 2).unwrap().label_files, 1);
		assert_eq!(label_rows(&ws.db_path), 0);
		assert_no_orphans(&ws.db_path, "the last VCDS source dropped");
	}

	#[test]
	fn a_build_alone_never_costs_the_cache_a_source() {
		// The defect (review of #37): `build_db` removed every other VCDS source, and
		// `dev vcds labels <dir> --part` built through it — one lookup against a one-file
		// folder took the project's installation, its registry channels and its metadata.
		let ws = TempWorkspace::new("build-keeps");
		write_fixture_labels(&ws);
		build_db(&ws.labels_dir, &ws.db_path).unwrap();
		let labels = ws.labels_dir.to_string_lossy().into_owned();
		put_all_vcds_readings(&ws.db_path, &labels, [("EV_Brake1", &[unit_scale(0x1822, "vcds")][..])]).unwrap();
		let other = ws.root().join("other-labels");
		std::fs::create_dir_all(&other).unwrap();
		std::fs::copy(ws.labels_dir.join("target.lbl"), other.join("target.lbl")).unwrap();
		build_db(&other, &ws.db_path).unwrap();
		let held = sources(&ws.db_path).unwrap();
		assert_eq!(held.len(), 2, "{held:?}");
		assert_eq!(
			readings_of_kind(&ws.db_path, VCDS, "EV_Brake1").unwrap().len(),
			1,
			"the registry rows went"
		);
	}

	#[test]
	fn a_second_vcds_installation_replaces_the_first_source_row_and_all() {
		// Its label files and registry rows were already replaced; the first one's row
		// was left naming a folder and a language for nothing — setup's label step says
		// which installation is the project's.
		let ws = TempWorkspace::new("second-install");
		write_fixture_labels(&ws);
		build_db(&ws.labels_dir, &ws.db_path).unwrap();
		record_language(&ws.db_path, VCDS, &ws.labels_dir.to_string_lossy(), "rus").unwrap();
		let other = ws.root().join("other-labels");
		std::fs::create_dir_all(&other).unwrap();
		std::fs::copy(ws.labels_dir.join("target.lbl"), other.join("target.lbl")).unwrap();
		build_db(&other, &ws.db_path).unwrap();
		keep_only_vcds(&ws.db_path, &other.to_string_lossy()).unwrap();
		let held = sources(&ws.db_path).unwrap();
		assert_eq!(held.len(), 1, "{held:?}");
		assert_eq!(held[0].dir, normalise_dir(&other.to_string_lossy()));
		assert_eq!(held[0].language, None, "the first build's language went with it");
		assert_no_orphans(&ws.db_path, "a second installation");
	}

	#[test]
	fn reading_the_same_folder_again_keeps_one_source_row() {
		let ws = TempWorkspace::new("reread");
		write_fixture_labels(&ws);
		let dir = ws.labels_dir.to_string_lossy().into_owned();
		build_db(&ws.labels_dir, &ws.db_path).unwrap();
		build_db(Path::new(&format!("{dir}/")), &ws.db_path).unwrap();
		record_meta(&ws.db_path, VCDS, &format!("{dir}/./"), &Meta::default()).unwrap();
		put_all_vcds_readings(&ws.db_path, &dir, [("EV_Brake1", &[unit_scale(0x1822, "vcds")][..])]).unwrap();
		assert_eq!(sources(&ws.db_path).unwrap().len(), 1);
	}

	#[test]
	fn what_setup_records_about_a_source_is_kept_on_its_row() {
		let ws = TempWorkspace::new("meta");
		let meta = Meta {
			root: Some("/x/vcds-en".into()),
			version: Some("Codes.dat".into()),
			detail: None,
			parsed_at: Some("ignored".into()),
		};
		record_meta(&ws.db_path, VCDS, "/x/vcds-en/Labels", &meta).unwrap();
		let first = sources(&ws.db_path).unwrap().remove(0);
		assert_eq!(first.path(), "/x/vcds-en", "a person is shown the folder setup was pointed at");
		assert_eq!(first.meta.version.as_deref(), Some("Codes.dat"));
		let stamp = first.meta.parsed_at.clone().unwrap();
		assert!(stamp.contains('T') && stamp.ends_with('Z'), "{stamp}");
		// A reread is the same row, restamped.
		std::thread::sleep(std::time::Duration::from_millis(5));
		record_meta(&ws.db_path, VCDS, "/x/vcds-en/Labels/", &meta).unwrap();
		let again = sources(&ws.db_path).unwrap();
		assert_eq!(again.len(), 1);
		assert!(again[0].meta.parsed_at.as_deref().unwrap() > stamp.as_str());
		// An older note fills only what the row does not say.
		let older = Meta {
			root: Some("/elsewhere".into()),
			version: None,
			detail: Some("from an older note".into()),
			parsed_at: Some("2026-01-01T00:00:00Z".into()),
		};
		adopt_meta(&ws.db_path, again[0].id, &older).unwrap();
		let adopted = sources(&ws.db_path).unwrap().remove(0);
		assert_eq!(adopted.meta.root.as_deref(), Some("/x/vcds-en"));
		assert_eq!(adopted.meta.detail.as_deref(), Some("from an older note"));
		assert_eq!(adopted.meta.parsed_at, again[0].meta.parsed_at);
	}

	#[test]
	fn a_registry_write_into_a_dropped_source_writes_nothing_and_makes_no_source() {
		// A `watch` reading the registry for minutes while `setup --drop vcds` commits:
		// its write must not bring the source back.
		let ws = TempWorkspace::new("registry-race");
		let rows = [unit_scale(0x1822, "vcds")];
		assert_eq!(
			put_all_vcds_readings(&ws.db_path, "/x/vcds-en/Labels", [("EV_A", &rows[..])]).unwrap(),
			None
		);
		assert!(!ws.db_path.exists(), "a write with no cache made one");
		record_language(&ws.db_path, VCDS, "/x/vcds-en/Labels", "eng").unwrap();
		assert_eq!(
			put_all_vcds_readings(&ws.db_path, "/x/vcds-en/Labels", [("EV_A", &rows[..])]).unwrap(),
			Some(1)
		);
		let id = sources(&ws.db_path).unwrap()[0].id;
		drop_source(&ws.db_path, id).unwrap();
		assert_eq!(
			put_all_vcds_readings(&ws.db_path, "/x/vcds-en/Labels", [("EV_A", &rows[..])]).unwrap(),
			None
		);
		assert!(sources(&ws.db_path).unwrap().is_empty(), "the write made the dropped source again");
		assert_eq!(reading_count_of(&ws.db_path, VCDS).unwrap(), 0);
	}

	#[test]
	fn processes_opening_one_cache_to_upgrade_it_at_once_all_succeed() {
		// The upgrade takes the write lock first: under a shared lock, two openers both
		// read and then both fail to write, and one of them got `SQLITE_BUSY`.
		let ws = TempWorkspace::new("concurrent-upgrade");
		previous_cache(&ws);
		let db = ws.db_path.clone();
		let openers: Vec<_> = (0..6)
			.map(|_| {
				let db = db.clone();
				std::thread::spawn(move || sources(&db).map(|s| s.len()))
			})
			.collect();
		for opener in openers {
			assert_eq!(opener.join().unwrap().unwrap(), 2);
		}
		assert_no_orphans(&db, "upgraded by six at once");
	}
}
