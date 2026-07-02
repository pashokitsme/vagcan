//! Label lookup layer: turns a parsed label corpus into a queryable
//! [`LabelDb`] that resolves `REDIRECT` chains from ECU part numbers to
//! the terminal [`LabelFile`] and its [`Measurement`]s.
//!
//! See `.superpowers/sdd/lookup-spec.md` for the full algorithm and
//! matching rules this module implements.

use std::collections::HashMap;

use crate::label::{LabelFile, Measurement, Record};

/// Maximum number of redirect hops followed before giving up (cycle guard).
const MAX_DEPTH: usize = 16;

/// A queryable index over a corpus of parsed label files.
///
/// Owns the [`LabelFile`]s; all accessors return references into them.
pub struct LabelDb {
    files: Vec<LabelFile>,
    /// Normalized (uppercased) file name -> index into `files`. Populated
    /// with both the full source name and the name without its extension, so
    /// `target` refs (which include an extension) and part-number-named
    /// files (which usually don't) both resolve. First file to claim a key
    /// wins ties.
    file_index: HashMap<String, usize>,
    /// Every `Record::Redirect` with a selector, flattened across the whole
    /// corpus in encounter order, for the corpus-wide initial match (spec
    /// step 2). `order` preserves that encounter order for the "first
    /// encountered" specificity tiebreak.
    redirects: Vec<RedirectEntry>,
}

/// One `REDIRECT` row collected across the whole corpus, with enough
/// context to match a selector and pick the most specific hit.
struct RedirectEntry {
    order: usize,
    target: String,
    selector: String,
}

/// One candidate redirect target considered while resolving a part number.
struct Candidate<'a> {
    order: usize,
    target: &'a str,
    selector: &'a str,
}

impl LabelDb {
    /// Build from all parsed label files (order irrelevant).
    pub fn new(files: Vec<LabelFile>) -> Self {
        let mut file_index = HashMap::new();
        let mut redirects = Vec::new();
        let mut order = 0usize;
        for (i, f) in files.iter().enumerate() {
            let full = normalize(&f.source);
            let bare = strip_ext(&full).to_string();
            file_index.entry(full.clone()).or_insert(i);
            if bare != full {
                file_index.entry(bare).or_insert(i);
            }
            for r in &f.records {
                if let Record::Redirect {
                    target,
                    selector: Some(sel),
                    ..
                } = r
                {
                    redirects.push(RedirectEntry {
                        order,
                        target: target.clone(),
                        selector: sel.clone(),
                    });
                    order += 1;
                }
            }
        }
        LabelDb {
            files,
            file_index,
            redirects,
        }
    }

    /// Number of files indexed.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The label file whose name matches `name` (case-insensitive; with or
    /// without extension - try exact, then basename without extension).
    pub fn file(&self, name: &str) -> Option<&LabelFile> {
        let key = normalize(name);
        self.file_index
            .get(&key)
            .copied()
            .map(|i| &self.files[i])
    }

    /// Resolve an ECU part number to the terminal LabelFile that applies,
    /// following REDIRECT chains. Returns None if no selector matches and no
    /// file is named after the part number.
    pub fn resolve(&self, part_no: &str) -> Option<&LabelFile> {
        let pn = normalize(part_no);

        // Step 2: pick the initial `current` name.
        let mut current: String = match self.best_redirect_target(&pn) {
            Some(target) => target.to_string(),
            None => {
                // Fall back to a part-number-named file.
                self.file(&pn)?;
                pn.clone()
            }
        };

        // Step 3: follow the chain.
        let mut depth = 0;
        loop {
            let file = self.file(&current)?;
            // Only redirects within *this* file count for chain-following.
            let next = Self::best_redirect_in(file, &pn);
            match next {
                Some(target) => {
                    if depth >= MAX_DEPTH {
                        return Some(file);
                    }
                    depth += 1;
                    current = target.to_string();
                }
                None => return Some(file),
            }
        }
    }

    /// All measurements for a part number (from its resolved file). Empty if
    /// unresolved.
    pub fn measurements(&self, part_no: &str) -> Vec<&Measurement> {
        match self.resolve(part_no) {
            Some(file) => file
                .records
                .iter()
                .filter_map(|r| match r {
                    Record::Measurement(m) => Some(m),
                    _ => None,
                })
                .collect(),
            None => Vec::new(),
        }
    }

    /// A single measurement by (part_no, block, field).
    pub fn measurement(&self, part_no: &str, block: u16, field: u8) -> Option<&Measurement> {
        self.measurements(part_no)
            .into_iter()
            .find(|m| m.block == block && m.field == field)
    }

    /// Find the most specific redirect target across the WHOLE corpus whose
    /// selector matches `pn`, using the flattened index built in [`Self::new`].
    /// Used only for the initial lookup (step 2).
    fn best_redirect_target(&self, pn: &str) -> Option<&str> {
        let candidates = self
            .redirects
            .iter()
            .filter(|r| selector_matches(&r.selector, pn))
            .map(|r| Candidate {
                order: r.order,
                target: &r.target,
                selector: &r.selector,
            })
            .collect();
        Self::pick_most_specific(candidates).map(|c| c.target)
    }

    /// Find the most specific redirect target within a single file whose
    /// selector matches `pn`. Used while following the chain (step 3).
    fn best_redirect_in<'a>(file: &'a LabelFile, pn: &str) -> Option<&'a str> {
        let mut order = 0usize;
        let mut candidates: Vec<Candidate> = Vec::new();
        for r in &file.records {
            if let Record::Redirect {
                target,
                selector: Some(sel),
                ..
            } = r
            {
                if selector_matches(sel, pn) {
                    candidates.push(Candidate {
                        order,
                        target,
                        selector: sel,
                    });
                    order += 1;
                }
            }
        }
        Self::pick_most_specific(candidates).map(|c| c.target)
    }

    fn pick_most_specific<'a>(candidates: Vec<Candidate<'a>>) -> Option<Candidate<'a>> {
        candidates.into_iter().min_by(|a, b| {
            let wa = wildcard_count(a.selector);
            let wb = wildcard_count(b.selector);
            wa.cmp(&wb)
                .then_with(|| {
                    let pa = literal_prefix_len(a.selector);
                    let pb = literal_prefix_len(b.selector);
                    pb.cmp(&pa) // longer prefix wins -> smaller ordering value
                })
                .then_with(|| a.order.cmp(&b.order))
        })
    }
}

/// Uppercase + trim a name/selector/part-number for case-insensitive matching.
/// Separators (`-`) are significant and left untouched.
fn normalize(s: &str) -> String {
    s.trim().to_ascii_uppercase()
}

/// Strip a trailing `.ext` (last dot onward), if any. Input is expected
/// already normalized (uppercased).
fn strip_ext(s: &str) -> &str {
    match s.rfind('.') {
        Some(i) => &s[..i],
        None => s,
    }
}

/// Number of `?` wildcard characters in a selector.
fn wildcard_count(selector: &str) -> usize {
    selector.chars().filter(|&c| c == '?').count()
}

/// Length of the literal (non-`?`) prefix before the first `?`.
fn literal_prefix_len(selector: &str) -> usize {
    selector.chars().take_while(|&c| c != '?').count()
}

/// Whether `selector` (already the raw, un-normalized text from the record)
/// matches `pn` (already normalized: uppercased + trimmed) per the spec:
/// same length after uppercasing, and every char equal or the selector char
/// is `?`.
fn selector_matches(selector: &str, pn: &str) -> bool {
    let sel = normalize(selector);
    if sel.len() != pn.len() {
        return false;
    }
    sel.chars()
        .zip(pn.chars())
        .all(|(s, p)| s == '?' || s == p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::label::parse_label;

    #[test]
    fn direct_file_named_after_part_number_resolves() {
        let file = parse_label(
            "022-906-032-C.LBL",
            b"001,1,Engine Speed,,Range: 0...6500 RPM",
        );
        let db = LabelDb::new(vec![file]);

        let resolved = db.resolve("022-906-032-C").expect("should resolve");
        assert_eq!(resolved.source, "022-906-032-C.LBL");

        let m = db
            .measurement("022-906-032-C", 1, 1)
            .expect("measurement should be found");
        assert_eq!(m.name, "Engine Speed");
        assert_eq!(m.unit.as_deref(), Some("RPM"));
    }

    #[test]
    fn one_hop_redirect_resolves_to_target() {
        let index = parse_label(
            "INDEX.LBL",
            b"REDIRECT,TARGET.LBL,022-906-032-C",
        );
        let target = parse_label(
            "TARGET.LBL",
            b"001,1,Engine Speed,,Range: 0...6500 RPM",
        );
        let db = LabelDb::new(vec![index, target]);

        let resolved = db.resolve("022-906-032-C").expect("should resolve");
        assert_eq!(resolved.source, "TARGET.LBL");

        let ms = db.measurements("022-906-032-C");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].name, "Engine Speed");
    }

    #[test]
    fn most_specific_selector_wins() {
        let index = parse_label(
            "INDEX.LBL",
            b"REDIRECT,BASE.LBL,022-906-032\nREDIRECT,EXACT.LBL,022-906-032-C",
        );
        let base = parse_label("BASE.LBL", b"001,1,Base Measurement,,");
        let exact = parse_label("EXACT.LBL", b"002,2,Exact Measurement,,");
        let db = LabelDb::new(vec![index, base, exact]);

        let resolved = db.resolve("022-906-032-C").expect("should resolve");
        assert_eq!(resolved.source, "EXACT.LBL");
    }

    #[test]
    fn wildcard_selector_matches_same_length_part_number() {
        let index = parse_label(
            "INDEX.LBL",
            b"REDIRECT,TARGET.LBL,----???-???-???",
        );
        let target = parse_label("TARGET.LBL", b"001,1,Wildcard Hit,,");
        let db = LabelDb::new(vec![index, target]);

        // Selector "----???-???-???" is 15 chars: literal '-' at indices
        // 0,1,2,3,7,11 and '?' everywhere else. A same-length candidate with
        // matching literal dashes should hit via wildcard.
        let selector_len = "----???-???-???".len();
        assert_eq!(selector_len, 15);
        let matching_pn = "----111-222-333"; // 15 chars, dashes line up with the selector
        assert_eq!(matching_pn.len(), 15);

        let resolved = db.resolve(matching_pn).expect("should resolve via wildcard");
        assert_eq!(resolved.source, "TARGET.LBL");

        // Non-matching length must not resolve via this redirect (and no
        // file is named after it either).
        assert!(db.resolve("022-906-032-C").is_none());
    }

    #[test]
    fn chain_of_two_redirects_resolves_and_cycle_terminates() {
        let a = parse_label("A.LBL", b"REDIRECT,B.LBL,022-906-032-C");
        let b = parse_label("B.LBL", b"REDIRECT,C.LBL,022-906-032-C");
        let c = parse_label("C.LBL", b"001,1,Terminal Measurement,,");
        let db = LabelDb::new(vec![a, b, c]);

        let resolved = db.resolve("022-906-032-C").expect("should resolve chain");
        assert_eq!(resolved.source, "C.LBL");

        // Self-referential cycle must terminate rather than loop forever.
        let cyclic = parse_label("CYCLE.LBL", b"REDIRECT,CYCLE.LBL,111-111-111-A");
        let db2 = LabelDb::new(vec![cyclic]);
        let resolved2 = db2.resolve("111-111-111-A");
        // It terminates at the (still-redirecting) file after MAX_DEPTH hops.
        assert_eq!(resolved2.map(|f| f.source.as_str()), Some("CYCLE.LBL"));
    }

    #[test]
    fn unresolved_part_number_returns_none_and_empty() {
        let file = parse_label("SOMETHING.LBL", b"001,1,Irrelevant,,");
        let db = LabelDb::new(vec![file]);

        assert!(db.resolve("999-999-999-Z").is_none());
        assert!(db.measurements("999-999-999-Z").is_empty());
        assert!(db.measurement("999-999-999-Z", 1, 1).is_none());
    }

    #[test]
    fn file_lookup_is_case_insensitive_and_extension_optional() {
        let file = parse_label("066-906-032-AQN.clb", b"001,1,Test,,");
        let db = LabelDb::new(vec![file]);

        assert!(db.file("066-906-032-aqn.CLB").is_some());
        assert!(db.file("066-906-032-AQN").is_some());
        assert_eq!(db.len(), 1);
        assert!(!db.is_empty());
    }
}
