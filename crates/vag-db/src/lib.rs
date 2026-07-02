//! SQLite cache for the VCDS label corpus.
//!
//! Parsing every `.lbl` and decrypting+parsing every `.clb` file (see
//! `vag_data::load_corpus`) is the expensive part of loading the corpus; this
//! crate persists the *parsed* result to SQLite so later runs can skip that
//! work entirely. This is a fast-load cache only: `REDIRECT` chain resolution
//! stays in the existing, reviewed [`vag_data::LabelDb`] — this crate just
//! reconstructs the same `Vec<LabelFile>` that `load_corpus` would produce and
//! hands it to `LabelDb::new`.
//!
//! `vag-data` stays pure-Rust; this crate is the only place in the workspace
//! that depends on `rusqlite`.

use std::path::Path;

use rusqlite::{params, Connection};

use vag_data::label::{LabelFile, Measurement, Record};
use vag_data::{load_corpus, LabelDb};

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
    id      INTEGER PRIMARY KEY,
    name    TEXT NOT NULL UNIQUE
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
    range_max   REAL
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
"#;

fn create_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

/// Insert every file + its records into `conn`, in one transaction. The
/// `Record::Other` variant is not persisted — it's non-semantic (comments /
/// unrecognized record kinds).
fn insert_files(conn: &mut Connection, files: &[LabelFile]) -> Result<BuildStats, Error> {
    let mut stats = BuildStats::default();
    let tx = conn.transaction()?;
    {
        let mut insert_file = tx.prepare("INSERT INTO label_file (name) VALUES (?1)")?;
        let mut insert_measurement = tx.prepare(
            "INSERT INTO measurement \
                (file_id, block, field, name, location, description, unit, range_min, range_max) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        let mut insert_redirect = tx
            .prepare("INSERT INTO redirect (file_id, selector, target) VALUES (?1, ?2, ?3)")?;
        let mut insert_adaptation = tx.prepare(
            "INSERT INTO adaptation (file_id, channel, idx, name, location, description) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        let mut insert_long_coding = tx.prepare(
            "INSERT INTO long_coding (file_id, byte, bits, value, meaning) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;

        for lf in files {
            insert_file.execute(params![lf.source])?;
            let file_id = tx.last_insert_rowid();
            stats.files += 1;

            for r in &lf.records {
                match r {
                    Record::Measurement(m) => {
                        let range_min = m.range.map(|r| r[0]);
                        let range_max = m.range.map(|r| r[1]);
                        insert_measurement.execute(params![
                            file_id, m.block, m.field, m.name, m.location, m.description,
                            m.unit, range_min, range_max
                        ])?;
                        stats.measurements += 1;
                    }
                    Record::Redirect { target, selector, .. } => {
                        insert_redirect.execute(params![file_id, selector, target])?;
                        stats.redirects += 1;
                    }
                    Record::Adaptation { channel, index, name, location, description } => {
                        insert_adaptation
                            .execute(params![file_id, channel, index, name, location, description])?;
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
    let mut file_stmt = conn.prepare("SELECT id, name FROM label_file ORDER BY id")?;
    let file_rows: Vec<(i64, String)> = file_stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(file_stmt);

    let mut meas_stmt = conn.prepare(
        "SELECT block, field, name, location, description, unit, range_min, range_max \
         FROM measurement WHERE file_id = ?1 ORDER BY rowid",
    )?;
    let mut redirect_stmt =
        conn.prepare("SELECT selector, target FROM redirect WHERE file_id = ?1 ORDER BY rowid")?;
    let mut adapt_stmt = conn.prepare(
        "SELECT channel, idx, name, location, description \
         FROM adaptation WHERE file_id = ?1 ORDER BY rowid",
    )?;
    let mut lc_stmt = conn.prepare(
        "SELECT byte, bits, value, meaning FROM long_coding WHERE file_id = ?1 ORDER BY rowid",
    )?;

    let mut files = Vec::with_capacity(file_rows.len());
    for (file_id, name) in file_rows {
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

        files.push(LabelFile { source: name, records });
    }

    Ok(files)
}

/// Build (or overwrite) a SQLite DB at `db_path` from a labels directory.
/// Returns row counts. Wraps all inserts in one transaction for speed.
pub fn build_db(labels_dir: &Path, db_path: &Path) -> Result<BuildStats, Error> {
    let load = load_corpus(labels_dir)?;
    let mut conn = Connection::open(db_path)?;
    create_schema(&conn)?;
    insert_files(&mut conn, &load.files)
}

/// Load all label files back out of a SQLite DB into a `Vec<LabelFile>`
/// (reconstructing `Record::Measurement`/`Redirect`/`Adaptation`/`LongCoding`).
pub fn load_files(db_path: &Path) -> Result<Vec<LabelFile>, Error> {
    let conn = Connection::open(db_path)?;
    read_files(&conn)
}

/// Convenience: load a DB straight into a ready-to-query `LabelDb`.
pub fn load_db(db_path: &Path) -> Result<LabelDb, Error> {
    Ok(LabelDb::new(load_files(db_path)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vag_data::load_corpus;

    /// Same synthetic 80-byte `.clb` fixture used in `vag_data::clb`'s tests
    /// (TEA-CBC-encrypted with `KEY_CLB`, `w7 = 7`) — no proprietary data.
    const FIXTURE_HEX: &str = "002738e02cf98f11742ee0b6f41102c2e55c4890aa526e2753a9263c7947f8b656f3467dc8f892f6c03a000a00202d7dc10402a81d837c41c4b66f69b6b50479e421595f5f5c20f4d6edd2d07b99000a";

    fn hex_decode(s: &str) -> Vec<u8> {
        assert_eq!(s.len() % 2, 0);
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// A unique-per-test-run temp dir under the system temp dir, cleaned up
    /// on drop. Holds both the input labels dir and the output sqlite path.
    struct TempWorkspace {
        labels_dir: std::path::PathBuf,
        db_path: std::path::PathBuf,
    }

    impl TempWorkspace {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "vag-db-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
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

    /// Populate a temp labels dir with one `.lbl` (measurement + redirect)
    /// and the shared `.clb` fixture (two measurements).
    fn write_fixture_labels(ws: &TempWorkspace) {
        std::fs::write(
            ws.labels_dir.join("index.lbl"),
            b"001,1,Engine Speed,(G28),Range: 0...6500 RPM\n\
              REDIRECT,target.lbl,022-906-032-C  ; a comment that is not persisted",
        )
        .unwrap();
        std::fs::write(
            ws.labels_dir.join("target.lbl"),
            b"002,2,Coolant,(G62),Range: -48...143 C",
        )
        .unwrap();
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
                Record::Redirect { target, selector, .. } => {
                    Some((selector.as_deref(), target.as_str()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn round_trip_reconstructs_same_measurements_and_redirects_as_load_corpus() {
        let ws = TempWorkspace::new("roundtrip");
        write_fixture_labels(&ws);

        let live = load_corpus(&ws.labels_dir).expect("load_corpus should succeed");
        assert_eq!(live.files.len(), 3, "sanity: 2 .lbl + 1 .clb parsed");

        let stats = build_db(&ws.labels_dir, &ws.db_path).expect("build_db should succeed");
        assert_eq!(stats.files, 3);
        assert_eq!(stats.measurements, 4); // 1 + 1 + 2 from the .clb fixture
        assert_eq!(stats.redirects, 1);

        let cached = load_files(&ws.db_path).expect("load_files should succeed");
        assert_eq!(cached.len(), live.files.len());

        for live_file in &live.files {
            let cached_file = cached
                .iter()
                .find(|f| f.source == live_file.source)
                .unwrap_or_else(|| panic!("{} missing from cached corpus", live_file.source));

            let live_m = measurements_of(live_file);
            let cached_m = measurements_of(cached_file);
            assert_eq!(
                live_m, cached_m,
                "measurements mismatch for {}",
                live_file.source
            );

            let live_r = redirects_of(live_file);
            let cached_r = redirects_of(cached_file);
            assert_eq!(
                live_r, cached_r,
                "redirects mismatch for {}",
                live_file.source
            );
        }
    }

    #[test]
    fn load_db_resolve_matches_live_lookup() {
        let ws = TempWorkspace::new("lookup");
        write_fixture_labels(&ws);

        build_db(&ws.labels_dir, &ws.db_path).expect("build_db should succeed");

        let live_db = LabelDb::new(load_corpus(&ws.labels_dir).unwrap().files);
        let cached_db = load_db(&ws.db_path).expect("load_db should succeed");

        // The redirect in index.lbl sends "022-906-032-C" to target.lbl.
        let live_resolved = live_db.resolve("022-906-032-C").expect("live resolve");
        let cached_resolved = cached_db.resolve("022-906-032-C").expect("cached resolve");
        assert_eq!(live_resolved.source, cached_resolved.source);

        let live_m = live_db.measurement("022-906-032-C", 2, 2).expect("live measurement");
        let cached_m = cached_db
            .measurement("022-906-032-C", 2, 2)
            .expect("cached measurement");
        assert_eq!(live_m.name, "Coolant");
        assert_eq!(cached_m.name, "Coolant");
        assert_eq!(live_m.unit, cached_m.unit);
        assert_eq!(live_m.range, cached_m.range);
    }
}
