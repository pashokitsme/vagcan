//! Label lookup layer: turns a parsed label corpus into a queryable
//! [`LabelDb`] that resolves `REDIRECT` chains from ECU part numbers to
//! the terminal [`LabelFile`] and its [`Measurement`]s.
//!
//! See `.superpowers/sdd/lookup-spec.md` for the full algorithm and
//! matching rules this module implements.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use crate::label::{LabelFile, Measurement, Record};

/// Maximum number of redirect hops followed before giving up (cycle guard).
const MAX_DEPTH: usize = 16;

/// A queryable index over a corpus of parsed label files.
///
/// Owns the [`LabelFile`]s; all accessors return references into them.
///
/// Lookups are the hot path of `vagcan info` (part number -> label file ->
/// measuring-block names), so everything is indexed at build time:
///
/// - exact (wildcard-free) corpus-wide redirect selectors live in a
///   `HashMap` (O(1) hit; exact selectors always beat wildcards on the
///   wildcard-count tiebreak, so a hit short-circuits),
/// - wildcard selectors are pre-normalized with their specificity metrics
///   precomputed, and grouped by selector byte length (a selector only ever
///   matches a same-length part number), so a lookup scans just its own
///   length bucket with zero allocations,
/// - each file's redirects are pre-normalized per file for chain-following,
/// - each file gets a `(block, field) -> record index` map so
///   [`Self::measurement`] is O(1) after resolution,
/// - resolved part numbers are memoized (`resolve_cache`), so repeated
///   lookups against the same ECU skip redirect matching entirely.
pub struct LabelDb {
    files: Vec<LabelFile>,
    /// Normalized (uppercased) file name -> index into `files`. Populated
    /// with both the full source name and the name without its extension, so
    /// `target` refs (which include an extension) and part-number-named
    /// files (which usually don't) both resolve. First file to claim a key
    /// wins ties.
    file_index: HashMap<String, usize>,
    /// Corpus-wide exact (no `?`) redirect selectors, normalized. First
    /// encountered entry wins duplicate selectors, matching the old
    /// flattened-scan order tiebreak.
    exact_redirects: HashMap<String, String>,
    /// Corpus-wide wildcard redirect selectors, grouped by selector byte
    /// length. Encounter order within each bucket is corpus order, so the
    /// specificity tiebreak can use the entry's `order` field.
    wildcard_redirects: HashMap<usize, Vec<PreparedRedirect>>,
    /// Per-file prepared redirects (parallel to `files`), for chain-following
    /// without re-scanning `records` or re-normalizing selectors.
    file_redirects: Vec<Vec<PreparedRedirect>>,
    /// Per-file `(block, field)` -> index into that file's `records` of the
    /// first non-empty-name measurement (parallel to `files`).
    measurement_index: Vec<HashMap<(u16, u8), usize>>,
    /// Memoized [`Self::resolve`] results: normalized part number -> resolved
    /// file index (`None` = known miss). Interior mutability keeps the
    /// lookup API `&self`; `Mutex` keeps `LabelDb: Sync`.
    resolve_cache: Mutex<HashMap<String, Option<usize>>>,
    /// The corpus's own unit numbering: diagnostic address -> the name the
    /// corpus gives it, extracted once here rather than re-derived per lookup
    /// (about a thousand of the three thousand files carry a `Component:`
    /// header). Sorted by address. See [`Self::unit_numbers`].
    unit_numbers: Vec<(u8, String)>,
}

/// One `REDIRECT` row with its selector pre-normalized and its specificity
/// metrics precomputed, so matching a part number allocates nothing.
#[derive(Clone)]
struct PreparedRedirect {
    /// Encounter order (corpus-wide for `wildcard_redirects`, per-file for
    /// `file_redirects`) — the final "first encountered wins" tiebreak.
    order: usize,
    /// Normalized redirect target (file name, usually with extension).
    target: String,
    /// Normalized selector (uppercased, trimmed).
    selector: String,
    /// Number of `?` wildcards in the selector (fewer = more specific).
    wildcards: usize,
    /// Length of the literal prefix before the first `?` (longer = more
    /// specific).
    literal_prefix: usize,
}

impl PreparedRedirect {
    fn new(order: usize, target: &str, selector: &str) -> Self {
        let selector = normalize(selector);
        PreparedRedirect {
            order,
            target: normalize(target),
            wildcards: wildcard_count(&selector),
            literal_prefix: literal_prefix_len(&selector),
            selector,
        }
    }
}

impl LabelDb {
    /// Build from all parsed label files (order irrelevant).
    pub fn new(files: Vec<LabelFile>) -> Self {
        let mut file_index = HashMap::new();
        let mut exact_redirects: HashMap<String, String> = HashMap::new();
        let mut wildcard_redirects: HashMap<usize, Vec<PreparedRedirect>> = HashMap::new();
        let mut file_redirects = Vec::with_capacity(files.len());
        let mut measurement_index = Vec::with_capacity(files.len());
        let mut order = 0usize;
        for (i, f) in files.iter().enumerate() {
            let full = normalize(&f.source);
            let bare = strip_ext(&full).to_string();
            file_index.entry(full.clone()).or_insert(i);
            if bare != full {
                file_index.entry(bare).or_insert(i);
            }
            let mut prepared: Vec<PreparedRedirect> = Vec::new();
            let mut m_index: HashMap<(u16, u8), usize> = HashMap::new();
            for (ri, r) in f.records.iter().enumerate() {
                match r {
                    Record::Redirect {
                        target,
                        selector: Some(sel),
                        ..
                    } => {
                        let mut entry = PreparedRedirect::new(order, target, sel);
                        if entry.wildcards == 0 {
                            exact_redirects
                                .entry(entry.selector.clone())
                                .or_insert_with(|| entry.target.clone());
                        } else {
                            wildcard_redirects
                                .entry(entry.selector.len())
                                .or_default()
                                .push(entry.clone());
                        }
                        // Per-file list uses per-file encounter order.
                        entry.order = ri;
                        prepared.push(entry);
                        order += 1;
                    }
                    Record::Measurement(m) if !m.name.trim().is_empty() => {
                        m_index.entry((m.block, m.field)).or_insert(ri);
                    }
                    _ => {}
                }
            }
            file_redirects.push(prepared);
            measurement_index.push(m_index);
        }
        let unit_numbers = collect_unit_numbers(&files);
        LabelDb {
            files,
            file_index,
            exact_redirects,
            wildcard_redirects,
            file_redirects,
            measurement_index,
            resolve_cache: Mutex::new(HashMap::new()),
            unit_numbers,
        }
    }

    /// The unit numbering the corpus itself states: every diagnostic address
    /// that appears in a `; Component: … (#17)` header, with the name the
    /// corpus gives it, sorted by address.
    ///
    /// This is the hundred-odd-row table that would otherwise be written into
    /// the code — `17` is the instrument cluster on every VAG car, not on this
    /// Škoda. It is **numbers and names only**: no label file anywhere states
    /// which CAN id a number is answered on, so pairing a number with a request
    /// id still takes the car (`vagcan units --identify --labels`) or a user's
    /// own note.
    pub fn unit_numbers(&self) -> &[(u8, String)] {
        &self.unit_numbers
    }

    /// What the corpus calls one diagnostic address.
    pub fn unit_name(&self, address: u8) -> Option<&str> {
        self.unit_numbers
            .binary_search_by_key(&address, |(a, _)| *a)
            .ok()
            .map(|i| self.unit_numbers[i].1.as_str())
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
        self.file_idx(&normalize(name)).map(|i| &self.files[i])
    }

    /// Index of the file whose normalized name is `key`.
    fn file_idx(&self, key: &str) -> Option<usize> {
        self.file_index.get(key).copied()
    }

    /// Resolve an ECU part number to the terminal LabelFile that applies,
    /// following REDIRECT chains. Returns None if no selector matches and no
    /// file is named after the part number.
    ///
    /// O(1) for exact-selector and repeated (memoized) lookups; wildcard
    /// matching scans only the same-length selector bucket.
    pub fn resolve(&self, part_no: &str) -> Option<&LabelFile> {
        self.resolve_idx(part_no).map(|i| &self.files[i])
    }

    /// Which control unit a part number belongs to, as the corpus describes it
    /// — its diagnostic address and the corpus's name for it.
    ///
    /// This is how the tool learns that `0CW300041G` is unit `02` and a
    /// transmission controller without a table of one car's units in the code.
    /// The answer comes from the label file the part number resolves to; if
    /// that file has no `Component:` header, the redirect that led to it
    /// usually does, so the chain is walked.
    pub fn unit_for_part(&self, part_no: &str) -> Option<&crate::label::UnitLabel> {
        if let Some(unit) = self.resolve(part_no).and_then(|f| f.unit.as_ref()) {
            return Some(unit);
        }
        // The terminal file said nothing; ask whatever pointed at it.
        let target = self.resolve(part_no)?.source.to_ascii_uppercase();
        self.files
            .iter()
            .find(|f| {
                f.unit.is_some()
                    && f.records.iter().any(|r| match r {
                        crate::label::Record::Redirect { target: t, .. } => {
                            t.to_ascii_uppercase() == target
                        }
                        _ => false,
                    })
            })
            .and_then(|f| f.unit.as_ref())
    }

    /// Memoizing front-end for [`Self::resolve_uncached`].
    fn resolve_idx(&self, part_no: &str) -> Option<usize> {
        // The cache is checked first, under the caller's own key, because that
        // is the whole point of it. It used to be keyed by the *resolved*
        // spelling, which cannot be known without running the resolution — so
        // every call paid for `resolve_one` across all candidate spellings
        // before it ever looked at the cache, and the memo saved nothing. A
        // unit asks for its own part number over and over, so the input string
        // is the key that actually repeats.
        {
            let cache = self
                .resolve_cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(&hit) = cache.get(part_no) {
                return hit;
            }
        }
        // Miss: try each spelling the corpus might use, first that resolves
        // wins. `normalize(part_no)` is always the first candidate, so the old
        // fallback to it added nothing — a run that resolves nothing is `None`.
        let result = part_number_candidates(part_no)
            .iter()
            .find_map(|c| self.resolve_one(c));
        self.resolve_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(part_no.to_string(), result);
        result
    }

    /// The actual resolution: corpus-wide initial match (spec step 2), then
    /// follow in-file redirect chains (step 3). `pn` is already normalized.
    /// Resolve one exact spelling (the caller has already normalised it).
    fn resolve_one(&self, pn: &str) -> Option<usize> {
        // Step 2: pick the initial file.
        let initial = match self.best_redirect_target(pn) {
            Some(target) => target,
            None => pn, // fall back to a part-number-named file
        };
        let mut current = self.file_idx(initial)?;

        // Step 3: follow the chain. Only redirects within the current file
        // count for chain-following.
        let mut depth = 0;
        while let Some(target) = Self::best_match(&self.file_redirects[current], pn) {
            if depth >= MAX_DEPTH {
                return Some(current);
            }
            depth += 1;
            current = self.file_idx(target)?;
        }
        Some(current)
    }

    /// All measurements for a part number (from its resolved file). Empty if
    /// unresolved. Empty-name placeholder records (`name.trim().is_empty()`)
    /// are skipped — they're pure filler, not real measurements.
    pub fn measurements(&self, part_no: &str) -> Vec<&Measurement> {
        match self.resolve_idx(part_no) {
            Some(i) => self.files[i]
                .records
                .iter()
                .filter_map(|r| match r {
                    Record::Measurement(m) if !m.name.trim().is_empty() => Some(m),
                    _ => None,
                })
                .collect(),
            None => Vec::new(),
        }
    }

    /// Every measurement across the WHOLE corpus whose block id is `block`
    /// (and, if `field` is `Some`, whose field matches too). Returns each hit
    /// as `(file source name, measurement)`; empty-name placeholder records are
    /// skipped. Results are sorted by `(source, field)` for deterministic output.
    ///
    /// This is the cross-corpus counterpart to [`Self::measurement`], which is
    /// scoped to a single resolved part number. Used by `vagcan vcds labels --block`
    /// to answer "which label files define measuring block N, and what is it?".
    pub fn measurements_by_block(
        &self,
        block: u16,
        field: Option<u8>,
    ) -> Vec<(&str, &Measurement)> {
        let mut hits: Vec<(&str, &Measurement)> = self
            .files
            .iter()
            .flat_map(|f| {
                f.records.iter().filter_map(move |r| match r {
                    Record::Measurement(m)
                        if m.block == block
                            && field.is_none_or(|wanted| m.field == wanted)
                            && !m.name.trim().is_empty() =>
                    {
                        Some((f.source.as_str(), m))
                    }
                    _ => None,
                })
            })
            .collect();
        hits.sort_by(|a, b| a.0.cmp(b.0).then_with(|| a.1.field.cmp(&b.1.field)));
        hits
    }

    /// A single measurement by (part_no, block, field). Empty-name placeholder
    /// records are skipped (returns `None`), since an empty slot isn't a real
    /// measurement for lookup purposes.
    ///
    /// O(1) after part-number resolution (per-file `(block, field)` index).
    pub fn measurement(&self, part_no: &str, block: u16, field: u8) -> Option<&Measurement> {
        let i = self.resolve_idx(part_no)?;
        let ri = *self.measurement_index[i].get(&(block, field))?;
        match &self.files[i].records[ri] {
            Record::Measurement(m) => Some(m),
            _ => None, // unreachable: the index only stores Measurement rows
        }
    }

    /// Find the most specific redirect target across the WHOLE corpus whose
    /// selector matches `pn` (already normalized). Used only for the initial
    /// lookup (step 2). An exact-selector hit wins outright: it has zero
    /// wildcards, and the wildcard count is the primary specificity key.
    fn best_redirect_target(&self, pn: &str) -> Option<&str> {
        if let Some(target) = self.exact_redirects.get(pn) {
            return Some(target);
        }
        // Only same-length wildcard selectors can match.
        Self::best_match(self.wildcard_redirects.get(&pn.len())?, pn)
    }

    /// The most specific matching redirect target among `redirects` for `pn`
    /// (already normalized): fewest wildcards, then longest literal prefix,
    /// then first encountered.
    fn best_match<'a>(redirects: &'a [PreparedRedirect], pn: &str) -> Option<&'a str> {
        redirects
            .iter()
            .filter(|r| selector_matches_normalized(&r.selector, pn))
            .min_by(|a, b| {
                a.wildcards
                    .cmp(&b.wildcards)
                    .then_with(|| b.literal_prefix.cmp(&a.literal_prefix)) // longer prefix wins
                    .then_with(|| a.order.cmp(&b.order))
            })
            .map(|r| r.target.as_str())
    }
}

/// Pull the corpus's unit numbering out of the `Component:` headers.
///
/// A number is named many times over — 108 files call `01` an engine — and not
/// always with the same words, since the name belongs to a car's own ECU and
/// the corpus spans two decades of them. The **most frequent** spelling wins,
/// ties broken alphabetically, so the answer depends on the corpus and not on
/// the order its files happened to be read in.
fn collect_unit_numbers(files: &[LabelFile]) -> Vec<(u8, String)> {
    let mut counts: HashMap<u8, HashMap<&str, usize>> = HashMap::new();
    for unit in files.iter().filter_map(|f| f.unit.as_ref()) {
        *counts
            .entry(unit.address)
            .or_default()
            .entry(unit.name.as_str())
            .or_default() += 1;
    }
    let mut out: Vec<(u8, String)> = counts
        .into_iter()
        .filter_map(|(address, names)| {
            names
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
                .map(|(name, _)| (address, name.to_string()))
        })
        .collect();
    out.sort_by_key(|(address, _)| *address);
    out
}

/// Uppercase + trim a name/selector/part-number for case-insensitive matching.
/// Separators (`-`) are significant and left untouched.
fn normalize(s: &str) -> String {
    s.trim().to_ascii_uppercase()
}

/// The spellings a VAG part number can appear under in a label corpus.
///
/// A control unit reports its number packed — `0AM927769E`, `06K907425B` —
/// while the corpus names files in the printed form, grouped and hyphenated,
/// and usually without the index letter: `0AM-927-769.clb`,
/// `06K-907-425-V1.clb`. Looking up only what the car said therefore finds
/// nothing, which is exactly what happened on the reference gearbox.
///
/// Returns the forms to try, most specific first: as given, hyphenated with
/// the index letter, and hyphenated without it. Anything that is not
/// `AAA NNN NNN` shaped is returned unchanged rather than forced into groups.
pub fn part_number_candidates(part_no: &str) -> Vec<String> {
    let pn = normalize(part_no);
    let mut out = vec![pn.clone()];

    let core: String = pn.chars().filter(|c| *c != '-' && *c != ' ').collect();
    // The printed form is three characters, then two groups of three digits,
    // then an optional index.
    if core.len() < 9 || !core[3..9].chars().all(|c| c.is_ascii_digit()) {
        return out;
    }
    let grouped = format!("{}-{}-{}", &core[..3], &core[3..6], &core[6..9]);
    let index = &core[9..];
    if !index.is_empty() {
        let with_index = format!("{grouped}-{index}");
        if !out.contains(&with_index) {
            out.push(with_index);
        }
    }
    if !out.contains(&grouped) {
        out.push(grouped);
    }
    out
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

/// Whether `selector` matches `pn` per the spec: same length and every char
/// equal or the selector char is `?`. Both sides must already be normalized
/// (uppercased + trimmed), so no allocation happens per comparison.
fn selector_matches_normalized(selector: &str, pn: &str) -> bool {
    if selector.len() != pn.len() {
        return false;
    }
    selector
        .chars()
        .zip(pn.chars())
        .all(|(s, p)| s == '?' || s == p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::label::parse_label;

    #[test]
    fn a_part_number_is_tried_in_the_spellings_a_corpus_uses() {
        // What the car reports versus how the corpus names its files. The
        // reference gearbox says 0AM927769E; the file is 0AM-927-769.clb.
        assert_eq!(
            part_number_candidates("0AM927769E"),
            vec!["0AM927769E", "0AM-927-769-E", "0AM-927-769"]
        );
        // The engine's hardware number, whose corpus files carry a variant
        // suffix instead of the index letter.
        assert_eq!(
            part_number_candidates("06K907425B"),
            vec!["06K907425B", "06K-907-425-B", "06K-907-425"]
        );
        // Already printed: no duplicate spellings.
        assert_eq!(
            part_number_candidates("8V0 906 264 H"),
            vec!["8V0 906 264 H", "8V0-906-264-H", "8V0-906-264"]
        );
        // Not part-number shaped: left alone rather than forced into groups.
        assert_eq!(part_number_candidates("EV_ECM18TFS"), vec!["EV_ECM18TFS"]);
        assert_eq!(part_number_candidates("1K0"), vec!["1K0"]);
    }

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
    fn same_length_selectors_tiebreak_by_wildcard_count() {
        // Both selectors are 13 chars and both match "022-906-032-C", so
        // `pick_most_specific`'s wildcard-count tiebreak (not length
        // filtering) is what has to pick EXACT over WILD here.
        let index = parse_label(
            "INDEX.LBL",
            b"REDIRECT,EXACT.LBL,022-906-032-C\nREDIRECT,WILD.LBL,022-906-0??-C",
        );
        let exact = parse_label("EXACT.LBL", b"001,1,FromExact,,");
        let wild = parse_label("WILD.LBL", b"001,1,FromWild,,");
        let db = LabelDb::new(vec![index, exact, wild]);

        assert_eq!("022-906-032-C".len(), "022-906-0??-C".len());

        let resolved = db.resolve("022-906-032-C").expect("should resolve");
        assert_eq!(resolved.source, "EXACT.LBL");

        let ms = db.measurements("022-906-032-C");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].name, "FromExact");
    }

    #[test]
    fn same_length_same_wildcard_count_tiebreak_by_literal_prefix_length() {
        // Both selectors are 13 chars, both have exactly one `?`, and both
        // match "022-906-032-C" -- so the only thing that can distinguish
        // them is the literal-prefix-length tiebreak (longer prefix wins).
        let index = parse_label(
            "INDEX.LBL",
            b"REDIRECT,LONGPREFIX.LBL,022-906-0?2-C\nREDIRECT,SHORTPREFIX.LBL,02?-906-032-C",
        );
        let long_prefix = parse_label("LONGPREFIX.LBL", b"001,1,FromLongPrefix,,");
        let short_prefix = parse_label("SHORTPREFIX.LBL", b"001,1,FromShortPrefix,,");
        let db = LabelDb::new(vec![index, long_prefix, short_prefix]);

        assert_eq!(wildcard_count("022-906-0?2-C"), 1);
        assert_eq!(wildcard_count("02?-906-032-C"), 1);
        assert!(literal_prefix_len("022-906-0?2-C") > literal_prefix_len("02?-906-032-C"));

        let resolved = db.resolve("022-906-032-C").expect("should resolve");
        assert_eq!(resolved.source, "LONGPREFIX.LBL");

        let ms = db.measurements("022-906-032-C");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].name, "FromLongPrefix");
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
    fn empty_name_placeholder_measurements_are_filtered_from_lookup() {
        let file = parse_label(
            "022-906-032-C.LBL",
            b"003,1,Vehicle Speed,,Range: 0...300 km/h\n012,1,,,",
        );
        let db = LabelDb::new(vec![file]);

        let ms = db.measurements("022-906-032-C");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].name, "Vehicle Speed");

        assert!(db.measurement("022-906-032-C", 12, 1).is_none());
        let real = db
            .measurement("022-906-032-C", 3, 1)
            .expect("real measurement should be found");
        assert_eq!(real.name, "Vehicle Speed");
    }

    /// Build a synthetic-but-realistic corpus: `n` target files, an index
    /// file with one exact REDIRECT per target plus a handful of wildcard
    /// redirects, and a two-hop chain. Part numbers follow the real
    /// `XXX-XXX-XXX-XX` shape so wildcard matching is exercised for real.
    fn generated_corpus(n: usize) -> Vec<LabelFile> {
        let mut files = Vec::with_capacity(n + 3);
        let mut index_src = String::new();
        for i in 0..n {
            // Exact selector for part number i -> its target file.
            index_src.push_str(&format!("REDIRECT,T{i:04}.LBL,{:03}-906-{:03}-AB\n", i / 500, i % 500));
            let body = format!(
                "001,1,Engine Speed {i},,Range: 0...6500 RPM\n007,2,Coolant Temp {i},,Range: -48...143 C\n012,1,,,",
            );
            files.push(parse_label(format!("T{i:04}.LBL"), body.as_bytes()));
        }
        // Wildcard redirect: any 899-906-xxx-AB part number -> chain head.
        index_src.push_str("REDIRECT,CHAIN.LBL,899-906-???-AB\n");
        files.push(parse_label("INDEX.LBL", index_src.as_bytes()));
        // Two-hop chain: CHAIN.LBL redirects (same selector shape) to FINAL.LBL.
        files.push(parse_label(
            "CHAIN.LBL",
            b"REDIRECT,FINAL.LBL,899-906-???-AB\n",
        ));
        files.push(parse_label(
            "FINAL.LBL",
            b"003,1,Chain Terminal,,Range: 0...100 %\n",
        ));
        files
    }

    #[test]
    fn bulk_lookups_over_generated_corpus_resolve_correctly() {
        let n = 300;
        let db = LabelDb::new(generated_corpus(n));

        // Two passes: the second exercises any memoized path with identical results.
        for _pass in 0..2 {
            for i in 0..n {
                let pn = format!("{:03}-906-{:03}-AB", i / 500, i % 500);
                // REDIRECT case: part number resolves through INDEX.LBL.
                let resolved = db.resolve(&pn).unwrap_or_else(|| panic!("{pn} must resolve"));
                assert_eq!(resolved.source, format!("T{i:04}.LBL"));

                // Measuring-block id -> human name.
                let m = db
                    .measurement(&pn, 7, 2)
                    .unwrap_or_else(|| panic!("{pn} block 7 field 2 must exist"));
                assert_eq!(m.name, format!("Coolant Temp {i}"));
                assert_eq!(m.unit.as_deref(), Some("C"));

                // Empty-name placeholder is never returned.
                assert!(db.measurement(&pn, 12, 1).is_none());
            }

            // Wildcard REDIRECT + two-hop chain.
            let resolved = db.resolve("899-906-123-AB").expect("wildcard chain resolves");
            assert_eq!(resolved.source, "FINAL.LBL");
            let m = db.measurement("899-906-456-AB", 3, 1).expect("chain measurement");
            assert_eq!(m.name, "Chain Terminal");

            // Misses stay misses (also on the repeat pass).
            assert!(db.resolve("999-999-999-ZZ").is_none());
            assert!(db.measurement("999-999-999-ZZ", 1, 1).is_none());
        }
    }

    #[test]
    fn exact_selector_beats_wildcard_in_generated_corpus() {
        // Part number 000-906-042-AB has BOTH an exact redirect (to T0042)
        // and matches the wildcard 0??-906-???-AB; exact must win.
        let mut files = generated_corpus(50);
        let index_extra = parse_label(
            "EXTRA.LBL",
            b"REDIRECT,WRONG.LBL,0??-906-???-AB\n",
        );
        let wrong = parse_label("WRONG.LBL", b"001,1,Wrong Target,,");
        files.push(index_extra);
        files.push(wrong);
        let db = LabelDb::new(files);

        let resolved = db.resolve("000-906-042-AB").expect("must resolve");
        assert_eq!(resolved.source, "T0042.LBL");
    }

    #[test]
    fn measurements_by_block_scans_the_whole_corpus() {
        // Block 2 appears in two files (fields 1 and 2 in one, field 1 in the
        // other); block 7 in one. An empty-name block-2 row must be skipped.
        let a = parse_label(
            "AAA.LBL",
            b"002,1,Engine Speed,,Range: 0...6500 RPM\n002,2,Coolant,,Range: -48...143 C\n007,1,Boost,,Range: 0...2500 mbar",
        );
        let b = parse_label(
            "BBB.LBL",
            b"002,1,Vehicle Speed,,Range: 0...300 km/h\n002,3,,,",
        );
        let db = LabelDb::new(vec![a, b]);

        // All fields of block 2 across the corpus (empty-name field 3 skipped).
        let all = db.measurements_by_block(2, None);
        assert_eq!(all.len(), 3);
        // Sorted by (source, field): AAA(1), AAA(2), BBB(1).
        assert_eq!((all[0].0, all[0].1.name.as_str()), ("AAA.LBL", "Engine Speed"));
        assert_eq!((all[1].0, all[1].1.name.as_str()), ("AAA.LBL", "Coolant"));
        assert_eq!((all[2].0, all[2].1.name.as_str()), ("BBB.LBL", "Vehicle Speed"));

        // Field filter narrows to a single (block, field) across files.
        let f1 = db.measurements_by_block(2, Some(1));
        assert_eq!(f1.len(), 2);
        assert!(f1.iter().all(|(_, m)| m.field == 1));

        // A block present in only one file.
        let b7 = db.measurements_by_block(7, None);
        assert_eq!(b7.len(), 1);
        assert_eq!(b7[0].1.name, "Boost");

        // A block nobody defines → empty, not a panic.
        assert!(db.measurements_by_block(99, None).is_empty());
    }

    #[test]
    fn the_corpus_states_its_own_unit_numbering() {
        // Three files naming unit 17 — two agreeing, one an older spelling —
        // and one naming 44, a number no code in this project knows.
        let header = |address: &str, name: &str| {
            format!("; Component: {name} (#{address})\n001,1,Something,,")
        };
        let db = LabelDb::new(vec![
            parse_label("A.LBL", header("17", "J285 - Instrument Cluster").as_bytes()),
            parse_label("B.LBL", header("17", "J285 - Instrument Cluster").as_bytes()),
            parse_label("C.LBL", header("17", "Instruments").as_bytes()),
            parse_label("D.LBL", header("44", "J500 - Power Steering").as_bytes()),
            parse_label("E.LBL", b"001,1,No header at all,,"),
        ]);

        // Sorted by address, one row per number, the majority spelling.
        assert_eq!(
            db.unit_numbers(),
            [
                (0x17, "J285 - Instrument Cluster".to_string()),
                (0x44, "J500 - Power Steering".to_string()),
            ]
        );
        assert_eq!(db.unit_name(0x44), Some("J500 - Power Steering"));
        assert_eq!(db.unit_name(0x03), None);

        // The number is hex, as the corpus writes it: `(#17)` is 0x17, not 17.
        assert_eq!(db.unit_numbers()[0].0, 23);
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
