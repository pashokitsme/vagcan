//! SQLite cache for the VCDS label files.
//!
//! Parsing every `.lbl` and decrypting+parsing every `.clb` file (see
//! `vag_data::load_label_files`) is the expensive part of loading the label files; this
//! crate persists the *parsed* result to SQLite so later runs can skip that
//! work entirely. This is a fast-load cache only: `REDIRECT` chain resolution
//! stays in the existing, reviewed [`vag_data::LabelDb`] — this crate just
//! reconstructs the same `Vec<LabelFile>` that `load_label_files` would produce and
//! hands it to `LabelDb::new`.
//!
//! `vag-data` stays pure-Rust; this crate is the only place in the workspace
//! that depends on `rusqlite`.

use std::path::Path;

use rusqlite::{Connection, params};

use vag_data::label::{LabelFile, Measurement, Record};
use vag_data::{LabelDb, Scaling, load_label_files};

/// Errors from building or loading a vag-db SQLite cache.
#[derive(Debug)]
pub enum Error {
	Sqlite(rusqlite::Error),
	Io(std::io::Error),
}

impl std::fmt::Display for Error {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Error::Sqlite(e) => write!(f, "sqlite error: {e}"),
			Error::Io(e) => write!(f, "io error: {e}"),
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
    unit_name    TEXT
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
CREATE TABLE IF NOT EXISTS source (
    id   INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    dir  TEXT NOT NULL,
    UNIQUE (kind, dir)
);
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
    -- (`research/labels/rod-labels.md:433`, established byte by byte against a
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
-- The levels of a `TEXTTABLE` scaling: each raw value means one thing, and
-- there is no scale between them. A child table because a gear selector has as
-- many rows as it has positions and a linear channel has none.
CREATE TABLE IF NOT EXISTS reading_level (
    reading_id INTEGER NOT NULL REFERENCES reading(id),
    raw        INTEGER NOT NULL,
    meaning    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_reading_lookup ON reading(variant, did);
CREATE INDEX IF NOT EXISTS idx_reading_level  ON reading_level(reading_id);
"#;

fn create_schema(conn: &Connection) -> rusqlite::Result<()> {
	// Migrating *before* the batch, not after, and the order is load-bearing:
	// the batch creates `reading`, which carries a foreign key to `source`, and
	// SQLite rewrites such a reference when the table it points at is renamed —
	// so a `reading` created first would come out pointing at
	// `source_before_kind`, which the migration then drops. The first insert
	// after that fails with "no such table: main.source_before_kind", a very
	// long way from what actually happened. Migrating first leaves nothing
	// pointing at `source` when it is rebuilt.
	migrate(conn)?;
	conn.execute_batch(SCHEMA)
}

/// Bring a cache written by an older build up to the current schema.
///
/// Two changes, both un-doable by `CREATE TABLE IF NOT EXISTS` because the
/// table is already there under the old shape:
///
/// - `source` was one row with a `CHECK (id = 0)` and no `kind`. SQLite cannot
///   drop a check constraint, so the table is rebuilt and its one row carried
///   over as `kind = 'vcds'` — which is what it was, since a VCDS installation
///   was the only source that existed when it was written.
/// - `measurement` gained `source_id`. Existing rows keep `NULL`: they came from
///   whatever the single `source` row named, and back-filling a foreign key to
///   make that explicit would be rewriting history that the rebuilt `source`
///   row already records.
///
/// Idempotent, because it runs on every open and most opens have nothing to do.
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
	if columns(conn, "source")?.is_empty() {
		// A cache that does not exist yet. The batch is about to create it in
		// the current shape, and there is nothing to carry over.
		return Ok(());
	}
	if !has_column(conn, "source", "kind")? {
		conn.execute_batch(
			"ALTER TABLE source RENAME TO source_before_kind;\
             CREATE TABLE source (\
                 id   INTEGER PRIMARY KEY,\
                 kind TEXT NOT NULL,\
                 dir  TEXT NOT NULL,\
                 UNIQUE (kind, dir)\
             );\
             INSERT INTO source (id, kind, dir) SELECT id, 'vcds', dir FROM source_before_kind;\
             DROP TABLE source_before_kind;",
		)?;
	}
	let measurement = columns(conn, "measurement")?;
	if !measurement.is_empty() && !measurement.iter().any(|name| name == "source_id") {
		conn.execute("ALTER TABLE measurement ADD COLUMN source_id INTEGER REFERENCES source(id)", [])?;
	}
	// `reading` gained `big_endian` after the first ODIS parses were written.
	// The default is `1` because that is UDS's convention and so the only
	// defensible guess for a row nobody recorded it for — but a guess is what it
	// is, and `setup` re-run against the project replaces the row with the
	// answer the file actually gives.
	let reading = columns(conn, "reading")?;
	if !reading.is_empty() && !reading.iter().any(|name| name == "big_endian") {
		conn.execute("ALTER TABLE reading ADD COLUMN big_endian INTEGER NOT NULL DEFAULT 1", [])?;
	}
	Ok(())
}

/// Whether a table already has a column, asked of the database rather than of a
/// version number nobody remembers to bump.
fn has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
	Ok(columns(conn, table)?.iter().any(|name| name == column))
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
fn normalise_dir(dir: &str) -> String {
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

/// Find or make the `source` row for one directory, and return its id.
///
/// Rows already written under another spelling of the same directory are
/// **dropped**, not repointed: a cache that already holds the duplicate has no
/// other way back, and repointing would leave the same channel twice under one
/// source, which is the very thing being undone. Dropping is safe because a
/// duplicate source is by construction a second reading of the *same directory*,
/// and every caller of this function is an import that is about to write that
/// directory's contents again.
///
/// Done here rather than in a schema migration because it is the spelling rule
/// that decides which rows are the same, and the rule lives here.
fn source_id(conn: &Connection, kind: &str, dir: &str) -> rusqlite::Result<i64> {
	let wanted = normalise_dir(dir);
	let mut same: Vec<i64> = {
		let mut stmt = conn.prepare("SELECT id, dir FROM source WHERE kind = ?1 ORDER BY id")?;
		let rows = stmt.query_map(params![kind], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?;
		rows
			.collect::<rusqlite::Result<Vec<_>>>()?
			.into_iter()
			.filter(|(_, stored)| normalise_dir(stored) == wanted)
			.map(|(id, _)| id)
			.collect()
	};
	let Some(keep) = same.first().copied() else {
		conn.execute("INSERT INTO source (kind, dir) VALUES (?1, ?2)", params![kind, wanted])?;
		return conn.query_row("SELECT id FROM source WHERE kind = ?1 AND dir = ?2", params![kind, wanted], |row| {
			row.get(0)
		});
	};
	same.remove(0);
	for other in same {
		conn.execute("DELETE FROM reading WHERE source_id = ?1", params![other])?;
		// A `measurement` row predating the `source_id` column carries NULL and
		// belongs to nobody, so it is matched by id and never swept along.
		conn.execute("DELETE FROM measurement WHERE source_id = ?1", params![other])?;
		conn.execute("DELETE FROM source WHERE id = ?1", params![other])?;
	}
	// The survivor is rewritten in the spelling this function will look for next
	// time, so the merge happens once rather than on every run.
	conn.execute("UPDATE source SET dir = ?1 WHERE id = ?2", params![wanted, keep])?;
	Ok(keep)
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

		let mut insert_file = tx.prepare("INSERT INTO label_file (name, unit_address, unit_name) VALUES (?1, ?2, ?3)")?;
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

		let unit = address.zip(unit_name).map(|(address, name)| vag_data::label::UnitLabel { address, name });
		files.push(LabelFile { source: name, records, unit });
	}

	Ok(files)
}

/// Build (or overwrite) a SQLite DB at `db_path` from a labels directory.
/// Returns row counts. Wraps all inserts in one transaction for speed.
pub fn build_db(labels_dir: &Path, db_path: &Path) -> Result<BuildStats, Error> {
	let load = load_label_files(labels_dir)?;
	let mut conn = Connection::open(db_path)?;
	create_schema(&conn)?;
	insert_files(&mut conn, &load.files, &labels_dir.to_string_lossy())
}

/// The label-file directory `db_path` was built from, if it says.
///
/// `None` for a cache written before this was recorded, one that cannot be
/// opened, or one holding no VCDS source at all — a pure-ODIS project's cache is
/// the third case, and it is not stale, it is simply about nothing this question
/// concerns. The caller decides what to do with that (see the freshness rule in
/// `vagcan::labels`, which trusts a cache whose source directory is gone).
pub fn source_of(db_path: &Path) -> Option<String> {
	let conn = Connection::open(db_path).ok()?;
	conn
		.query_row("SELECT dir FROM source WHERE kind = ?1 ORDER BY id LIMIT 1", [VCDS], |row| {
			row.get::<_, String>(0)
		})
		.ok()
}

/// Every source that has ever written into this cache, oldest first.
pub fn sources_of(db_path: &Path) -> Result<Vec<(String, String)>, Error> {
	let conn = Connection::open(db_path)?;
	let mut stmt = conn.prepare("SELECT kind, dir FROM source ORDER BY id")?;
	let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
	Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Replace everything one ODIS source has contributed, and write these readings.
///
/// Replace rather than append: a second parse of the same project is a *reread*,
/// not a second opinion, and appending would double every channel. Scoped to
/// this source's own rows, so a VCDS parse of the same car is untouched —
/// design §4.5's "an ODIS parse never deletes VCDS-derived rows or vice versa".
///
/// Returns how many channels landed.
pub fn put_readings(db_path: &Path, project_dir: &str, variant: &str, readings: &[vag_data::odis::Reading]) -> Result<usize, Error> {
	let mut conn = Connection::open(db_path)?;
	create_schema(&conn)?;
	let tx = conn.transaction()?;
	let source = source_id(&tx, ODIS, project_dir)?;
	tx.execute(
		"DELETE FROM reading_level WHERE reading_id IN \
         (SELECT id FROM reading WHERE source_id = ?1 AND variant = ?2)",
		params![source, variant],
	)?;
	tx.execute("DELETE FROM reading WHERE source_id = ?1 AND variant = ?2", params![source, variant])?;

	let mut written = 0usize;
	{
		let mut insert = tx.prepare(
			"INSERT INTO reading \
                (source_id, variant, did, name, unit, bit_offset, bit_length, signed, big_endian, text_id, \
                 scaling, factor, offset, anchor_raw, anchor_value) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
		)?;
		let mut insert_level = tx.prepare("INSERT INTO reading_level (reading_id, raw, meaning) VALUES (?1, ?2, ?3)")?;
		for r in readings {
			let (kind, factor, offset, anchor_raw, anchor_value) = match &r.scaling {
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
				kind,
				factor,
				offset,
				anchor_raw,
				anchor_value
			])?;
			let id = tx.last_insert_rowid();
			if let Scaling::Enum { levels } = &r.scaling {
				for (raw, meaning) in levels {
					insert_level.execute(params![id, raw, meaning])?;
				}
			}
			written += 1;
		}
	}
	tx.commit()?;
	Ok(written)
}

/// One `reading` row as SQLite hands it back, before it becomes a
/// [`vag_data::odis::Reading`].
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

/// The channels this cache knows for one ECU variant, by identifier.
pub fn readings_of(db_path: &Path, variant: &str) -> Result<Vec<vag_data::odis::Reading>, Error> {
	let conn = Connection::open(db_path)?;
	let mut stmt = conn.prepare(
		"SELECT id, did, name, unit, bit_offset, bit_length, signed, big_endian, text_id, \
                scaling, factor, offset, anchor_raw, anchor_value \
         FROM reading WHERE variant = ?1 ORDER BY did, bit_offset",
	)?;
	let rows: Vec<ReadingRow> = stmt
		.query_map(params![variant], |row| {
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

	let mut levels = conn.prepare("SELECT raw, meaning FROM reading_level WHERE reading_id = ?1 ORDER BY rowid")?;
	let mut out = Vec::with_capacity(rows.len());
	for (id, did, name, unit, bit_offset, bit_length, signed, big_endian, text_id, kind, factor, offset, anchor_raw, anchor_value) in rows {
		// A row whose scaling columns disagree with its kind is skipped rather
		// than repaired: a channel reported with a scaling nobody wrote is worse
		// than a channel not reported at all.
		let scaling = match (kind.as_str(), factor, offset, anchor_raw, anchor_value) {
			("linear", Some(factor), Some(offset), _, _) => Scaling::Linear(vag_data::LinearScale { factor, offset }),
			("anchor", _, _, Some(raw), Some(value)) => Scaling::Anchor { raw, value },
			("enum", ..) => Scaling::Enum {
				levels: levels
					.query_map(params![id], |row| Ok((row.get(0)?, row.get(1)?)))?
					.collect::<rusqlite::Result<_>>()?,
			},
			_ => continue,
		};
		out.push(vag_data::odis::Reading {
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
pub fn text_ids(db_path: &Path) -> Result<Vec<(String, String)>, Error> {
	let conn = Connection::open(db_path)?;
	let mut stmt = conn.prepare(
		"SELECT text_id, MIN(name) FROM reading \
         WHERE text_id IS NOT NULL AND text_id <> '' GROUP BY text_id ORDER BY text_id",
	)?;
	let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
	Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Every ECU variant this cache holds readings for, in name order.
pub fn reading_variants(db_path: &Path) -> Result<Vec<String>, Error> {
	let conn = Connection::open(db_path)?;
	let mut stmt = conn.prepare("SELECT DISTINCT variant FROM reading ORDER BY variant")?;
	let rows = stmt.query_map([], |row| row.get(0))?;
	Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Load all label files back out of a SQLite DB into a `Vec<LabelFile>`
/// (reconstructing `Record::Measurement`/`Redirect`/`Adaptation`/`LongCoding`).
pub fn load_files(db_path: &Path) -> Result<Vec<LabelFile>, Error> {
	let conn = Connection::open(db_path)?;
	read_files(&conn)
}

/// Convenience: load a DB straight into a ready-to-query `LabelDb`.
/// Row counts per table, in the order a person wants to read them.
///
/// Here rather than in the utility that prints them because the table names
/// are this crate's schema: a binary that spelled them out itself would keep
/// compiling after a rename and fail only when run.
pub fn row_counts(db_path: &Path) -> Result<Vec<(&'static str, i64)>, Error> {
	const TABLES: [&str; 5] = ["label_file", "measurement", "redirect", "adaptation", "long_coding"];
	let conn = rusqlite::Connection::open(db_path)?;
	let mut out = Vec::with_capacity(TABLES.len());
	for table in TABLES {
		// The names are the constant above, never anything a caller supplied,
		// so the format! cannot carry an injection.
		let n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))?;
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
	use vag_data::load_label_files;

	/// Same synthetic 80-byte `.clb` fixture used in `vag_data::clb`'s tests
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

	/// One channel, in the shape `vag_data::odis` hands them over.
	fn reading(did: u16, name: &str, scaling: Scaling) -> vag_data::odis::Reading {
		vag_data::odis::Reading {
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
	fn an_odis_channel_survives_the_cache_with_every_scaling_shape_intact() {
		// The three shapes are not interchangeable: a gear selector forced into
		// a linear scaling reports confident nonsense, and an anchor whose slope
		// is unproven must not come back as a slope of one.
		let ws = TempWorkspace::new("readings");
		let rows = [
			reading(
				0x380A,
				"Getriebe-Eingangsdrehzahl",
				Scaling::Linear(vag_data::LinearScale { factor: 1.0, offset: 0.0 }),
			),
			reading(
				0x2000,
				"Ganganzeige",
				Scaling::Enum {
					levels: vec![(1, "R".to_string()), (2, "N".to_string())],
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
				levels: vec![(1, "R".to_string()), (2, "N".to_string())]
			}
		);
		assert_eq!(back[1].scaling, Scaling::Anchor { raw: 4096, value: 12.5 });
		// The design's §1 cross-check, round-tripped: DID 0x380A raw.
		assert_eq!(back[2].did, 0x380A);
		assert_eq!(back[2].scaling, Scaling::Linear(vag_data::LinearScale { factor: 1.0, offset: 0.0 }));
		assert_eq!(back[2].text_id.as_deref(), Some("000116"), "the join to names.json survives");
		assert_eq!(reading_variants(&ws.db_path).unwrap(), ["EV_ECM"]);
	}

	#[test]
	fn byte_order_survives_the_cache_because_the_proven_row_disagrees_with_the_convention() {
		// UDS payloads are big-endian by convention and the reference car's own
		// proven row is not: DID 0x380A is `u16` little-endian
		// (`research/labels/rod-labels.md:433`, established byte by byte against
		// a log), and the ODIS file agrees. A cache that dropped this column
		// would throw the parser's correctness away at the storage layer, and a
		// reader would report 690 /min as 45570.
		let ws = TempWorkspace::new("endian");
		let identity = Scaling::Linear(vag_data::LinearScale { factor: 1.0, offset: 0.0 });
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
	fn a_reading_written_before_the_column_existed_migrates_to_the_uds_convention() {
		// Not a fact, a guess — the only defensible one, since big-endian is
		// what UDS says. `setup` re-run against the project replaces the row
		// with the answer the file actually gives.
		let ws = TempWorkspace::new("endian-old");
		{
			let conn = Connection::open(&ws.db_path).unwrap();
			create_schema(&conn).unwrap();
			conn.execute("ALTER TABLE reading DROP COLUMN big_endian", []).unwrap();
			conn
				.execute_batch(
					"INSERT INTO source (id, kind, dir) VALUES (1, 'odis', '/x/SK37X');\
                 INSERT INTO reading (source_id, variant, did, name, bit_offset, bit_length, signed, scaling, factor, offset) \
                 VALUES (1, 'EV_ECM', 14346, 'a', 0, 16, 0, 'linear', 1.0, 0.0);",
				)
				.unwrap();
		}
		let conn = Connection::open(&ws.db_path).unwrap();
		create_schema(&conn).unwrap();
		drop(conn);
		let back = readings_of(&ws.db_path, "EV_ECM").unwrap();
		assert_eq!(back.len(), 1);
		assert!(back[0].big_endian, "a row with no recorded byte order must take UDS's convention");
	}

	#[test]
	fn rereading_one_project_replaces_its_channels_rather_than_doubling_them() {
		let ws = TempWorkspace::new("reread");
		let rows = [reading(0x380A, "a", Scaling::Linear(vag_data::LinearScale { factor: 1.0, offset: 0.0 }))];
		put_readings(&ws.db_path, "/x/SK37X", "EV_ECM", &rows).unwrap();
		put_readings(&ws.db_path, "/x/SK37X", "EV_ECM", &rows).unwrap();
		assert_eq!(readings_of(&ws.db_path, "EV_ECM").unwrap().len(), 1);
		// And one source row, not one per run.
		assert_eq!(sources_of(&ws.db_path).unwrap(), [("odis".to_string(), "/x/SK37X".to_string())]);
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
		let rows = [reading(0x380A, "a", Scaling::Linear(vag_data::LinearScale { factor: 1.0, offset: 0.0 }))];

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
		let rows = [reading(0x380A, "a", Scaling::Linear(vag_data::LinearScale { factor: 1.0, offset: 0.0 }))];
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
		let rows = [reading(0x380A, "a", Scaling::Linear(vag_data::LinearScale { factor: 1.0, offset: 0.0 }))];
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

	#[test]
	fn a_cache_from_before_the_schema_split_opens_and_migrates() {
		// The shape every existing `~/.vagcan` holds: one `source` row under a
		// `CHECK (id = 0)` SQLite cannot drop, and a `measurement` with no
		// `source_id`. It has to open, not be rebuilt from an installation that
		// may well have been deleted (D5).
		let ws = TempWorkspace::new("oldcache");
		{
			let conn = Connection::open(&ws.db_path).unwrap();
			conn
				.execute_batch(
					"CREATE TABLE label_file (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, unit_address INTEGER, unit_name TEXT);\
                     CREATE TABLE measurement (file_id INTEGER NOT NULL, block INTEGER NOT NULL, field INTEGER NOT NULL, \
                        name TEXT NOT NULL, location TEXT NOT NULL, description TEXT NOT NULL, unit TEXT, range_min REAL, range_max REAL);\
                     CREATE TABLE source (id INTEGER PRIMARY KEY CHECK (id = 0), dir TEXT NOT NULL);\
                     INSERT INTO label_file (name) VALUES ('index.lbl');\
                     INSERT INTO measurement VALUES (1, 1, 1, 'Engine Speed', '(G28)', '', 'RPM', NULL, NULL);\
                     INSERT INTO source (id, dir) VALUES (0, '/old/Labels');",
				)
				.unwrap();
		}
		{
			let conn = Connection::open(&ws.db_path).unwrap();
			create_schema(&conn).unwrap();
		}
		// The one row it had is carried over as what it was: a VCDS parse was
		// the only source that existed when it was written.
		assert_eq!(sources_of(&ws.db_path).unwrap(), [("vcds".to_string(), "/old/Labels".to_string())]);
		assert_eq!(source_of(&ws.db_path).as_deref(), Some("/old/Labels"));
		// Nothing it already held was lost to the migration.
		let files = load_files(&ws.db_path).unwrap();
		assert_eq!(files.len(), 1);
		assert_eq!(measurements_of(&files[0])[0].name, "Engine Speed");
		// And ODIS channels can now go in beside them.
		let rows = [reading(0x380A, "a", Scaling::Linear(vag_data::LinearScale { factor: 1.0, offset: 0.0 }))];
		put_readings(&ws.db_path, "/x/SK37X", "EV_ECM", &rows).unwrap();
		assert_eq!(sources_of(&ws.db_path).unwrap().len(), 2);
		// Running it again is a no-op, because it runs on every open.
		let conn = Connection::open(&ws.db_path).unwrap();
		create_schema(&conn).unwrap();
		assert_eq!(sources_of(&ws.db_path).unwrap().len(), 2);
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
}
