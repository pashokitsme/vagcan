//! Corpus loading: walk a VCDS `Labels/` directory, parse every plaintext
//! `.lbl` file and decrypt+parse every encrypted `.clb` file into a `Vec<LabelFile>`.
//!
//! Shared by the `vag-labels` binary (JSON/summary/lookup CLI) and the
//! `vag-db` crate (SQLite cache builder), so both stay in sync on how the
//! corpus is walked and parsed.

use std::io;
use std::path::Path;

use crate::clb::decrypt_clb;
use crate::label::{parse_label, LabelFile};

/// Outcome of walking a labels directory.
pub struct CorpusLoad {
    /// Parsed `.lbl` files and decrypted-then-parsed `.clb` files.
    pub files: Vec<LabelFile>,
    pub lbl_count: usize,
    pub clb_count: usize,
    pub other_count: usize,
    /// Files that matched a known extension but could not be read (skipped,
    /// not fatal).
    pub read_errors: usize,
}

fn has_ext(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case(ext))
        .unwrap_or(false)
}

fn file_name_or(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(fallback)
        .to_string()
}

/// Walk `dir`, parse every `.lbl`, decrypt+parse every `.clb`, into a corpus.
/// Non-file entries and other extensions are counted, not parsed. Read errors
/// are counted (and the file skipped), never fatal.
pub fn load_corpus(dir: &Path) -> io::Result<CorpusLoad> {
    let entries = std::fs::read_dir(dir)?;

    let mut files = Vec::new();
    let mut lbl_count = 0;
    let mut clb_count = 0;
    let mut other_count = 0;
    let mut read_errors = 0;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if has_ext(&path, "lbl") {
            lbl_count += 1;
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let name = file_name_or(&path, "<?>");
                    files.push(parse_label(name, &bytes));
                }
                Err(_) => read_errors += 1,
            }
        } else if has_ext(&path, "clb") {
            clb_count += 1;
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let name = file_name_or(&path, "<?>");
                    let decoded = decrypt_clb(&bytes);
                    files.push(parse_label(name, &decoded));
                }
                Err(_) => read_errors += 1,
            }
        } else {
            other_count += 1;
        }
    }

    Ok(CorpusLoad {
        files,
        lbl_count,
        clb_count,
        other_count,
        read_errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same synthetic 80-byte `.clb` fixture used in `clb.rs`'s tests
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
    /// on drop.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "vag-data-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn load_corpus_parses_lbl_and_clb_and_counts_other() {
        let dir = TempDir::new("corpus-test");

        std::fs::write(
            dir.0.join("plain.lbl"),
            b"001,1,Engine Speed,,Range: 0...6500 RPM",
        )
        .unwrap();

        let clb_bytes = hex_decode(FIXTURE_HEX);
        std::fs::write(dir.0.join("fixture.clb"), &clb_bytes).unwrap();

        std::fs::write(dir.0.join("readme.txt"), b"not a label file").unwrap();

        let load = load_corpus(&dir.0).expect("load_corpus should succeed");

        assert_eq!(load.lbl_count, 1);
        assert_eq!(load.clb_count, 1);
        assert_eq!(load.other_count, 1);
        assert_eq!(load.read_errors, 0);
        assert_eq!(load.files.len(), 2);

        let plain = load
            .files
            .iter()
            .find(|f| f.source == "plain.lbl")
            .expect("plain.lbl present");
        assert_eq!(plain.records.len(), 1);

        let fixture = load
            .files
            .iter()
            .find(|f| f.source == "fixture.clb")
            .expect("fixture.clb present");
        assert_eq!(fixture.records.len(), 2);
    }
}
