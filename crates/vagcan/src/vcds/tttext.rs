//! `vagcan vcds tttext` — recover names from the label files' global text table.
//!
//! Was the `vag-tttext` binary. Every record of `TTTEXT.ROD`'s `[TXT]` section
//! is enciphered under its own substitution, so there is no single key to find:
//! the attack is dictionary-driven and bootstraps, with words read off records
//! it solves becoming vocabulary for the next pass.
//!
//! Only records read with **no unresolved letter** are output. A partial reading
//! is not a weaker result — it is an unmarked guess, and a name with a guessed
//! letter reads exactly like a name without one.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;

use anyhow::{Context, Result};

use vag_data::tttext::{self, Dictionary, Limits, Record};

/// How far the best reading of a token must outweigh the runner-up before the
/// completion step will pin its letters. A likelihood ratio, not a proof: at
/// 20x a token with a real second reading is left unpinned and the record is
/// dropped, which is the direction to err in.
const MARGIN: f32 = 20.0;

/// How many letters a reading must have before it is worth keeping.
///
/// `research/labels/tttext-codec.md` §7. Under a dozen there is too little
/// evidence for the vocabulary check to mean anything, and the acronyms that
/// dominate below it are not names.
const MIN_LETTERS: usize = 12;

/// Where a label word's occurrence count stops being usage and starts being
/// enum-literal noise.
///
/// The in-domain prior wants a word's label-file frequency — it is what tells
/// `voltage` from the rarities that share its shape. But a `.lbl` file lists a
/// few status literals thousands of times (`OK` 5 403, `ON` 7 679, `LC` 5 287
/// on the reference label files), enough to outrank `of` (3 947) and drive the
/// search to read `Status ok` for `Status of`. Saturating the count here ties
/// those giants to one another and to `of` — the tie then breaks on the
/// decoded-label files frequency, measured off cleaner text — while every genuine
/// content word, whose count is far below this, keeps its full weight. Not a
/// property of any car: a property of how label files spell enumerations.
const SEED_FREQUENCY_CAP: u32 = 1_000;

pub struct Options<'a> {
	pub file: &'a str,
	pub words: &'a [String],
	pub names: Option<&'a str>,
	pub out: Option<&'a str>,
	pub catalog: Option<&'a str>,
	pub partial: Option<&'a str>,
	pub passes: usize,
	pub steps: Option<u32>,
	pub check: usize,
	pub gated: bool,
}

/// How much of the text table a run actually read.
///
/// The attack is expected to fall short — a record with one unresolved letter
/// is withheld rather than guessed. So "it finished" and "it got everything"
/// are different facts, and a caller that reports success has to be able to
/// tell them apart.
pub struct Coverage {
	pub read: usize,
	pub total: usize,
	/// Records long enough to be a name at all — the honest denominator.
	///
	/// The gate drops any reading under [`MIN_LETTERS`] letters, because below a
	/// dozen there is too little evidence for the vocabulary check to mean
	/// anything and what dominates there is acronyms and status codes, not
	/// names. That is knowable *before* solving anything: the cipher substitutes
	/// letters for letters and digits for digits, so a record's letter count
	/// survives encipherment. Reporting against every record instead made a run
	/// that read four candidates in five look like one that managed a little
	/// over half.
	pub candidates: usize,
}

/// Letters in a record's payload — the same count before and after the cipher.
fn letters_of(cipher: &str) -> usize {
	let body = cipher.split_once(',').map_or(cipher, |(_, rest)| rest);
	body.chars().filter(|c| c.is_ascii_alphabetic()).count()
}

pub fn run(opts: Options<'_>) -> Result<Coverage> {
	let mut limits = Limits::default();
	if let Some(steps) = opts.steps {
		limits.steps = steps;
	}

	let text = std::fs::read(opts.file).with_context(|| format!("reading {:?}", opts.file))?;
	let records = parse(&text);
	eprintln!("{} records", records.len());

	// The vocabulary. In-domain words outweigh a general list heavily: the
	// label files' own language is the strongest prior available, and a general
	// dictionary is there to catch the rest.
	let mut dict = Dictionary::default();
	let mut known: HashSet<String> = HashSet::new();
	if let Some(path) = opts.names {
		for word in words_of_json(path) {
			known.insert(word.clone());
			dict.insert(&word, 400.0);
		}
		eprintln!("{} words from names already recovered", known.len());
	}
	for spec in opts.words {
		// `FILE` or `FILE:WEIGHT`. The weight is the prior: the label files' own
		// label files are in-domain and must outrank a general word list, or
		// the search prefers an English rarity to the term the label files use.
		let (path, weight) = match spec.rsplit_once(':').and_then(|(p, w)| Some((p, w.parse().ok()?))) {
			Some((path, weight)) => (path, weight),
			None => (spec.as_str(), 8.0f32),
		};
		let files = word_files(std::path::Path::new(path));
		if files.is_empty() {
			eprintln!("skipping {path}: nothing readable there");
			continue;
		}
		let before = known.len();
		// Count occurrences, but spend them carefully (below). The label files
		// says `voltage` where it never says `boltage`, and that is the prior
		// that keeps the search off English rarities — but its raw counts are
		// dominated by *enum literals*, the `OK`/`ON`/`LC` status values that
		// appear thousands of times, and letting those decide would make the
		// search read `Status ok` for `Status of`.
		let mut local: HashMap<String, u32> = HashMap::new();
		for file in &files {
			// Read as bytes, not as a string. A VCDS `Labels/` directory is
			// Latin-1 and holds umlauts, so `read_to_string` refuses the whole
			// file — which silently cost the run its entire in-domain
			// vocabulary and left an English word list deciding what VW calls
			// things. Only the ASCII letters are wanted anyway.
			let Ok(bytes) = std::fs::read(file) else { continue };
			for word in bytes.split(|b| !b.is_ascii_alphabetic()) {
				if word.len() < 2 {
					continue;
				}
				let word = String::from_utf8_lossy(word).to_ascii_lowercase();
				*local.entry(word).or_insert(0) += 1;
			}
		}
		for (word, count) in local {
			known.insert(word.clone());
			// Frequency, saturated. A word's count separates `voltage` from the
			// rarities that share its shape, which is the whole in-domain prior;
			// but past [`SEED_FREQUENCY_CAP`] the count stops being usage and
			// becomes enum-literal noise — the `OK`/`ON`/`LC` status values a
			// label file lists thousands of times. Saturating there ties those
			// giants to one another and to `of`, so the search is not driven to
			// read `Status ok` for `Status of`, while every genuine content word
			// (whose count is far below the cap) keeps its full weight.
			let scale = count.min(SEED_FREQUENCY_CAP) as f32;
			dict.insert(&word, weight * scale);
		}
		eprintln!(
			"{} words from {path} ({} file(s)) at weight {weight}×frequency (capped)",
			known.len() - before,
			files.len()
		);
	}
	dict.finish();
	anyhow::ensure!(!dict.is_empty(), "no vocabulary: pass --names and/or --words");

	// The vocabulary as it stands *before* the bootstrap, kept for the catalog
	// gate's membership check. This is not a micro-optimisation, it is the
	// difference between a check and a tautology: the passes below feed words
	// read off solved records back into the dictionary, so asking the grown
	// vocabulary whether a word is a word teaches the gate its own misreadings
	// and then passes them. Measured on the reference label files, gating membership
	// against the grown dictionary let `Ejzc xjpx dyjje agrope acpcj cgijfbc`
	// through as a name; against the seed it is six words nothing has ever
	// attested and the record is dropped. (The gate's *ambiguity margin* is a
	// separate question, answered by the frequency prior below — a statistic a
	// lone misreading cannot fake, so it may be measured on the decode.)
	let seed_words = known.clone();

	// Records sharing the repetition pattern of their *letter runs* hold the
	// same words under different keys, so one solve serves the cluster. The
	// pattern of the whole payload would not do: the trailing numeric fields
	// differ between records that carry identical text, which splits the
	// cluster and throws away the free members.
	let mut clusters: BTreeMap<Vec<u8>, Vec<usize>> = BTreeMap::new();
	for (index, record) in records.iter().enumerate() {
		let tokens = tttext::tokens(&record.cipher);
		if tokens.len() >= 2 {
			clusters.entry(tttext::pattern(&tokens.join("|"))).or_default().push(index);
		}
	}
	eprintln!("{} clusters with something to solve", clusters.len());

	let mut solved: HashMap<usize, String> = HashMap::new();
	let mut partials: Vec<(usize, String)> = Vec::new();
	for pass in 1..=opts.passes {
		let mut learned = 0usize;
		let mut fresh = 0usize;
		let mut partial = 0usize;
		partials.clear();

		// Each pass walks every cluster and says nothing until it ends, which on
		// the real table is minutes of blank screen. Reported per cluster rather
		// than by a bare spinner: how far through the pass is the useful number,
		// and it is the one the loop already has.
		let total = clusters.len();
		let mut progress = crate::progress::Line::new();
		for (at, members) in clusters.values().enumerate() {
			progress.update(&format!(
				"pass {pass} of {} — cluster {} of {total}, {} read so far",
				opts.passes,
				at + 1,
				solved.len()
			));
			// A cluster already read needs no second look.
			if members.iter().any(|m| solved.contains_key(m)) {
				continue;
			}
			let leader = members[0];
			let cipher = &records[leader].cipher;
			let Some(solution) = tttext::solve(cipher, &dict, limits) else {
				continue;
			};
			// The search stops at its best-scoring reading, which routinely
			// leaves a token unexplained; the letters the rest of the record
			// pins are evidence it did not have at the time, so re-filter that
			// token against them before giving the record up.
			let key = match solution.key.covers(cipher) {
				true => solution.key,
				false => tttext::complete(cipher, &solution.key, &dict, MARGIN),
			};
			if !key.covers(cipher) {
				partial += 1;
				partials.push((leader, key.decode(cipher)));
				continue;
			}
			let plain = key.decode(cipher);
			fresh += 1;
			solved.insert(leader, plain.clone());

			// Every other member is free — and checked: a member whose tokens
			// do not line up letter for letter is not the same text and is
			// dropped. Only the letter runs are compared, because that is what
			// the cluster claims is shared.
			let words: Vec<&str> = tttext::tokens(&plain);
			for member in &members[1..] {
				let cipher = &records[*member].cipher;
				let Some(key) = transfer_tokens(cipher, &words) else { continue };
				if key.covers(cipher) {
					solved.insert(*member, key.decode(cipher));
				}
			}

			// Words this reading contributes, for the next pass.
			for word in plain.split(|c: char| !c.is_ascii_alphabetic()) {
				if word.len() >= 3 && known.insert(word.to_ascii_lowercase()) {
					dict.insert(word, 300.0);
					learned += 1;
				}
			}
		}
		dict.finish();
		progress.finish();
		eprintln!(
			"pass {pass}: {fresh} clusters read ({} records), {learned} new words, \
             {partial} rejected for unresolved letters",
			solved.len()
		);
		if learned == 0 {
			break;
		}
	}

	// The prior. Everything above weighed a word by where it came from — every
	// in-domain word the same — so the search breaks a tie between two real
	// words (`of`/`ob`, `by`/`bf`, the writeup's `oil`/`bil`) by nothing better
	// than alphabetical order, and a cluster leader that guesses wrong pins that
	// letter for every member it feeds. The word's frequency *in the decoded
	// label files themselves* is the signal that settles those ties on evidence: `of`
	// outnumbers `ob` thousands to one (`research/labels/tttext-codec.md` §7).
	//
	// It is measured here from the decode, never a table of words baked into the
	// binary — that would be exactly the car-specific data CLAUDE.md forbids.
	// The first solve is right for the overwhelming majority of records, so its
	// word counts are a sound prior; re-solving every cluster leader under them
	// corrects the ones a flat weight left to chance, and the members it feeds
	// follow.
	let prior = word_frequency(&solved);
	eprintln!(
		"prior: {} word types over {} tokens of the decoded label files",
		prior.len(),
		prior.values().sum::<u32>()
	);
	let prior_dict = frequency_dictionary(&known, &prior);
	let solved = resolve_pass(&records, &clusters, &prior_dict, limits);
	eprintln!("re-solved under the frequency prior: {} records", solved.len());

	// The gate's ambiguity margin is a ratio of weights, so it too needs the
	// frequency prior — measured now on the *re-solved* label files — rather than the
	// flat seed weights, under which every real-word pair looks 1:1 and the 20×
	// test drops the record. Membership stays the seed vocabulary (below): a
	// statistic a lone misreading cannot fake is a fair judge of ambiguity, but
	// whether a word is a word is not a thing the label files get to vote on.
	let gate_freq = word_frequency(&solved);
	let gate_dict = frequency_dictionary(&known, &gate_freq);

	if opts.check > 0 {
		recheck(&records, &clusters, &solved, &gate_dict, limits, opts.check, opts.gated, &known);
	}

	if let Some(path) = opts.partial {
		let mut sink = std::io::BufWriter::new(std::fs::File::create(path).with_context(|| format!("creating the partial output file {path:?}"))?);
		for (index, plain) in &partials {
			writeln!(sink, "{:06}\t{plain}", records[*index].id)?;
		}
		eprintln!("{} partial readings written to {path}", partials.len());
	}

	if let Some(path) = opts.catalog {
		write_catalog(path, &records, &solved, &gate_dict, &seed_words)?;
	}

	// Readings go to stdout when nobody said where to put them — unless a
	// catalog was asked for, in which case the gated names *are* the answer and
	// a hundred thousand ungated lines scrolling past them is not a default
	// anyone wants.
	let mut sink: Option<Box<dyn Write>> = match (opts.out, opts.catalog) {
		(Some(path), _) => Some(Box::new(std::io::BufWriter::new(
			std::fs::File::create(path).with_context(|| format!("creating the output file {path:?}"))?,
		))),
		(None, Some(_)) => None,
		(None, None) => Some(Box::new(std::io::stdout().lock())),
	};
	let mut ordered: Vec<(u32, &String)> = solved.iter().map(|(index, plain)| (records[*index].id, plain)).collect();
	ordered.sort_unstable();
	if let Some(sink) = sink.as_mut() {
		for (id, plain) in &ordered {
			writeln!(sink, "{id:06}\t{plain}")?;
		}
	}
	eprintln!(
		"{} of {} records read ({:.1} %)",
		ordered.len(),
		records.len(),
		100.0 * ordered.len() as f32 / records.len() as f32
	);
	Ok(Coverage {
		read: ordered.len(),
		total: records.len(),
		candidates: records.iter().filter(|r| letters_of(&r.cipher) >= MIN_LETTERS).count(),
	})
}

/// Word frequency over a set of decoded records — the prior.
///
/// This is the statistic `research/labels/tttext-codec.md` §7 calls "a
/// word-frequency prior measured on the decoded label files themselves": every word of
/// every reading, counted. It is what tells `of` from `ob` and `oil` from
/// `bil` — real words the vocabulary holds at equal footing until their counts
/// separate them by three orders of magnitude. Two-letter words are counted
/// too, because those are exactly the function words (`of`, `by`, `in`) a
/// leader misreads and pins wrong for a whole cluster.
fn word_frequency(solved: &HashMap<usize, String>) -> HashMap<String, u32> {
	let mut freq: HashMap<String, u32> = HashMap::new();
	for plain in solved.values() {
		for word in plain.split(|c: char| !c.is_ascii_alphabetic()) {
			if word.len() >= 2 {
				*freq.entry(word.to_ascii_lowercase()).or_insert(0) += 1;
			}
		}
	}
	freq
}

/// A dictionary whose weight *is* each word's decoded-label files frequency.
///
/// The whole vocabulary stays a candidate — a rare-but-real term must not be
/// lost because it happened not to be decoded — so every known word is present,
/// at a floor of one. A word the vocabulary never listed but the label files did
/// (the two-letter function words the bootstrap's length cut kept out of
/// `known`) joins at its frequency, which for those words is the entire point.
fn frequency_dictionary(known: &HashSet<String>, freq: &HashMap<String, u32>) -> Dictionary {
	let mut dict = Dictionary::default();
	for word in known {
		dict.insert(word, *freq.get(word).unwrap_or(&0) as f32 + 1.0);
	}
	for (word, count) in freq {
		// `insert` keeps the larger weight, so this only ever adds words `known`
		// lacked; the counts already agree for the ones it holds.
		dict.insert(word, *count as f32 + 1.0);
	}
	dict.finish();
	dict
}

/// Solve every cluster once under a fixed dictionary — the re-solve.
///
/// Unlike the bootstrap loop this learns nothing and runs once: the dictionary
/// it is handed already carries the frequency prior, so the point is only to
/// let each cluster leader reconsider its tie-broken tokens with that prior in
/// hand and to carry the corrected reading to the members it feeds. The leader,
/// the completion and the transfer are the same three steps the bootstrap runs;
/// only the weight behind them has changed.
fn resolve_pass(records: &[Record], clusters: &BTreeMap<Vec<u8>, Vec<usize>>, dict: &Dictionary, limits: Limits) -> HashMap<usize, String> {
	let mut solved: HashMap<usize, String> = HashMap::new();
	for members in clusters.values() {
		let leader = members[0];
		let cipher = &records[leader].cipher;
		let Some(solution) = tttext::solve(cipher, dict, limits) else { continue };
		let key = match solution.key.covers(cipher) {
			true => solution.key,
			false => tttext::complete(cipher, &solution.key, dict, MARGIN),
		};
		if !key.covers(cipher) {
			continue;
		}
		let plain = key.decode(cipher);
		solved.insert(leader, plain.clone());
		let words: Vec<&str> = tttext::tokens(&plain);
		for member in &members[1..] {
			let cipher = &records[*member].cipher;
			let Some(key) = transfer_tokens(cipher, &words) else { continue };
			if key.covers(cipher) {
				solved.insert(*member, key.decode(cipher));
			}
		}
	}
	solved
}

/// Validation, and it is not optional.
///
/// Within a cluster the member's cipher pattern equals the leader's *plaintext*
/// pattern by construction, so [`transfer_tokens`] can never fail injectivity —
/// it is free, but it is not a check. The check is to solve a member from
/// scratch, under its own key and its own search path, and see whether it says
/// the same thing. Two plaintexts can share a repetition pattern; this is what
/// would catch it.
#[allow(clippy::too_many_arguments)]
fn recheck(
	records: &[Record],
	clusters: &BTreeMap<Vec<u8>, Vec<usize>>,
	solved: &HashMap<usize, String>,
	dict: &Dictionary,
	limits: Limits,
	check: usize,
	gated: bool,
	known: &HashSet<String>,
) {
	let mut members: Vec<(usize, &String)> = Vec::new();
	for group in clusters.values() {
		for member in &group[1..] {
			match solved.get(member) {
				// Only readings a catalog would consider. Sampling every
				// transferred member measures the transfer over acronym soup
				// that no gate would ship, which is not the number that matters
				// and is not what the published 599/600 was measured on.
				Some(plain) if !gated || shippable(plain, known) => members.push((*member, plain)),
				_ => {}
			}
		}
	}
	// A fixed stride over the sorted list rather than a random sample:
	// reproducible, and it cannot be accused of picking the easy ones.
	let stride = (members.len() / check.max(1)).max(1);
	let (mut agree, mut disagree, mut unusable) = (0usize, 0usize, 0usize);
	let mut examples: Vec<(u32, String, String)> = Vec::new();
	for (index, transferred) in members.iter().step_by(stride).take(check) {
		let cipher = &records[*index].cipher;
		let Some(solution) = tttext::solve(cipher, dict, limits) else {
			unusable += 1;
			continue;
		};
		let key = tttext::complete(cipher, &solution.key, dict, MARGIN);
		if !key.covers(cipher) {
			unusable += 1;
			continue;
		}
		let fresh = key.decode(cipher);
		if fresh.eq_ignore_ascii_case(transferred) {
			agree += 1;
		} else {
			disagree += 1;
			if examples.len() < 20 {
				examples.push((records[*index].id, (*transferred).clone(), fresh));
			}
		}
	}
	eprintln!(
		"independent re-solve of transferred members: agree {agree} disagree {disagree} \
         unusable {unusable}"
	);
	for (id, transferred, fresh) in &examples {
		eprintln!("  {id:06} transferred {transferred:?} != re-solved {fresh:?}");
	}
}

/// Every file a `--words` argument names: the file itself, or a directory's
/// worth.
///
/// A directory is the ordinary case now that `vagcan setup` passes the VCDS
/// `Labels/` tree straight in. The recursion is what makes that work on an
/// install root as well as on `Labels/` itself.
fn word_files(at: &std::path::Path) -> Vec<std::path::PathBuf> {
	if at.is_file() {
		return vec![at.to_path_buf()];
	}
	let Ok(entries) = std::fs::read_dir(at) else { return Vec::new() };
	let mut out: Vec<std::path::PathBuf> = Vec::new();
	let mut dirs: Vec<std::path::PathBuf> = Vec::new();
	for entry in entries.flatten() {
		let path = entry.path();
		if path.is_dir() {
			dirs.push(path);
		} else {
			out.push(path);
		}
	}
	// Sorted, so two runs over one set of label files build the same dictionary and read
	// the same names out of it.
	out.sort();
	dirs.sort();
	for dir in dirs {
		out.extend(word_files(&dir));
	}
	out
}

/// Write the readings that clear §7's gate as a `{"<text id>": "<name>"}`
/// catalog.
///
/// **The gate is the product, not the decode.** 61 % of the section decodes;
/// what a name catalog may contain is far less than that, because a fluent
/// wrong reading is indistinguishable from a right one at the point of use.
/// `research/labels/tttext-codec.md` §7 states the filters and this applies
/// them: the framing rule, no unresolved letter, no digit, at least
/// [`MIN_LETTERS`] letters, every word of length ≥ 3 a word the vocabulary
/// knows, the [`MARGIN`] ambiguity check per token, and no name twice.
fn write_catalog(path: &str, records: &[Record], solved: &HashMap<usize, String>, dict: &Dictionary, known: &HashSet<String>) -> Result<()> {
	// By name first: two records reading the same way is the signature of a
	// truncated enumeration (`… of cylinder 4` losing its digit), so both go.
	let mut by_name: BTreeMap<String, Vec<u32>> = BTreeMap::new();
	for (index, plain) in solved {
		let Some(name) = gated_name(&records[*index].cipher, plain, dict, known) else {
			continue;
		};
		by_name.entry(name).or_default().push(records[*index].id);
	}

	let mut catalog: BTreeMap<String, String> = BTreeMap::new();
	let mut duplicated = 0usize;
	for (name, ids) in by_name {
		if ids.len() > 1 {
			duplicated += ids.len();
			continue;
		}
		catalog.insert(format!("{:06}", ids[0]), name);
	}

	if let Some(parent) = std::path::Path::new(path).parent().filter(|p| !p.as_os_str().is_empty()) {
		std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
	}
	std::fs::write(path, serde_json::to_string_pretty(&catalog)?).with_context(|| format!("writing the name catalog {path:?}"))?;
	eprintln!(
		"{} names written to {path} ({} dropped for reading the same as another record)",
		catalog.len(),
		duplicated
	);
	Ok(())
}

/// The name a record contributes to the catalog, or nothing.
///
/// Nothing is the ordinary answer. A partial reading is not a weaker result but
/// an unmarked guess, and the same is true of an ambiguous one — see
/// [`unambiguous`].
fn gated_name(cipher: &str, plain: &str, dict: &Dictionary, known: &HashSet<String>) -> Option<String> {
	if plain.contains('?') {
		return None;
	}
	let name = framed(plain)?;
	// Digits are not recovered at all (§6): the glyph class that carries them
	// is unbroken, so a name containing one contains a guess.
	if name.chars().any(|c| c.is_ascii_digit()) {
		return None;
	}
	if name.chars().filter(|c| c.is_ascii_alphabetic()).count() < MIN_LETTERS {
		return None;
	}
	if !name
		.split(|c: char| !c.is_ascii_alphabetic())
		.filter(|w| w.len() >= 3)
		.all(|w| known.contains(&w.to_ascii_lowercase()))
	{
		return None;
	}
	unambiguous(cipher, plain, dict, MARGIN).then_some(name)
}

/// The name, with the record's trailing field run removed.
///
/// A record is `<name><sep><digit><sep><number>` and the tail is inside the
/// glyph class, so it survives the decode as noise. Stripping the trailing run
/// of non-letters is right — unless the *name itself* ended in a digit, in
/// which case the digit is inside the run and gets eaten, silently turning
/// `… of cylinder 4` into `… of cylinder`. The guard is that the run's first
/// character must recur later in it: that is what makes it a separator rather
/// than the end of the name.
fn framed(plain: &str) -> Option<String> {
	let cut = plain.trim_end().len()
		- plain
			.trim_end()
			.chars()
			.rev()
			.take_while(|c| !c.is_ascii_alphabetic() && *c != ' ')
			.map(char::len_utf8)
			.sum::<usize>();
	let (name, run) = plain.trim_end().split_at(cut);
	let mut run = run.chars();
	let first = run.next()?;
	run.any(|c| c == first).then(|| name.trim_end().to_string())
}

/// Whether every word of the reading beats its best alternative by `margin`.
///
/// This is §7's ambiguity filter and it is the one that matters. `Hill bytes to
/// maintain backward compatibility` is fluent, dictionary-clean and stable
/// across keys, and the word is `Fill`: a letter occurring once in a record is
/// pinned by nothing but the dictionary. So each token is re-read against every
/// word of its pattern class that the *rest of the record* allows, and a token
/// whose winner does not outweigh the runner-up by `margin` costs the record.
///
/// It is deliberately re-derived here rather than trusted from the solve. The
/// search stops at its best-scoring reading and has no opinion about how close
/// second place was.
fn unambiguous(cipher: &str, plain: &str, dict: &Dictionary, margin: f32) -> bool {
	let ciphered = tttext::tokens(cipher);
	let read = tttext::tokens(plain);
	if ciphered.len() != read.len() {
		return false;
	}
	for (at, token) in ciphered.iter().enumerate() {
		// Short tokens are not dictionary-checked anywhere in §7 — there is no
		// vocabulary at two letters, only noise.
		if token.len() < 3 {
			continue;
		}
		let Some(rest) = pinned_by_others(&ciphered, &read, at) else {
			return false;
		};
		let (mut chosen, mut runner_up) = (0.0f32, 0.0f32);
		for (word, weight) in dict.candidates(token) {
			if !fits(token, word, rest) {
				continue;
			}
			if word.eq_ignore_ascii_case(read[at]) {
				chosen = chosen.max(*weight);
			} else {
				runner_up = runner_up.max(*weight);
			}
		}
		// A reading the dictionary does not contain at all cannot be defended;
		// one the dictionary ranks below an alternative is a coin toss.
		if chosen == 0.0 || (runner_up > 0.0 && chosen < margin * runner_up) {
			return false;
		}
	}
	true
}

/// The letter mapping every token *except* `at` forces.
fn pinned_by_others(ciphered: &[&str], read: &[&str], at: usize) -> Option<tttext::Key> {
	let mut key = tttext::Key::default();
	for (index, token) in ciphered.iter().enumerate() {
		if index == at {
			continue;
		}
		for (c, p) in token.bytes().zip(read[index].bytes()) {
			let (c, p) = (c.to_ascii_lowercase(), p.to_ascii_lowercase());
			if !c.is_ascii_lowercase() || !p.is_ascii_lowercase() || !key.insert(c - b'a', p - b'a') {
				return None;
			}
		}
	}
	Some(key)
}

/// Whether `word` can be this token's reading given the letters already pinned.
fn fits(token: &str, word: &str, mut pinned: tttext::Key) -> bool {
	token.len() == word.len()
		&& token.bytes().zip(word.bytes()).all(|(c, p)| {
			let (c, p) = (c.to_ascii_lowercase(), p.to_ascii_lowercase());
			c.is_ascii_lowercase() && p.is_ascii_lowercase() && pinned.insert(c - b'a', p - b'a')
		})
}

/// Whether a reading is the kind a catalog would consider at all.
///
/// Three of the five filters of `research/labels/tttext-codec.md` §7: enough letters
/// to be sure of, no unresolved letter, and every word of length >= 3 a word
/// the vocabulary knows. The other two — the ambiguity margin and the framing
/// rule — are not reimplemented here; this is a sampling filter, not the gate.
fn shippable(plain: &str, known: &HashSet<String>) -> bool {
	if plain.contains('?') {
		return false;
	}
	if plain.chars().filter(|c| c.is_ascii_alphabetic()).count() < 12 {
		return false;
	}
	plain
		.split(|c: char| !c.is_ascii_alphabetic())
		.filter(|w| w.len() >= 3)
		.all(|w| known.contains(&w.to_ascii_lowercase()))
}

/// Read a cluster member's key off the leader's plaintext words.
///
/// The members share only their letter runs — the trailing numeric fields
/// differ — so the alignment is token by token. A member whose tokens do not
/// line up injectively is not the same text and gets no key at all, which is
/// the check that makes the transfer safe rather than merely free.
fn transfer_tokens(cipher: &str, words: &[&str]) -> Option<tttext::Key> {
	let tokens = tttext::tokens(cipher);
	if tokens.len() != words.len() {
		return None;
	}
	let mut key = tttext::Key::default();
	for (token, word) in tokens.iter().zip(words) {
		if token.len() != word.len() {
			return None;
		}
		for (c, p) in token.bytes().zip(word.bytes()) {
			let (c, p) = (c.to_ascii_lowercase(), p.to_ascii_lowercase());
			if !c.is_ascii_lowercase() || !p.is_ascii_lowercase() {
				return None;
			}
			if !key.insert(c - b'a', p - b'a') {
				return None;
			}
		}
	}
	Some(key)
}

/// Split the section into records. The id is plaintext; everything after the
/// first comma is the enciphered payload.
fn parse(bytes: &[u8]) -> Vec<Record> {
	// Latin-1, not UTF-8. The section carries 19 distinct high bytes — the
	// umlauts, `°`, `µ` — and they are *pass-through*: outside every enciphered
	// class, so they are plaintext evidence. `from_utf8_lossy` would fold all
	// of them onto one replacement character, which merges records that differ
	// only in which umlaut they use and silently deletes the `°C` crib.
	let text: String = bytes.iter().map(|b| char::from(*b)).collect();
	text
		.lines()
		.filter_map(|line| {
			let (id, cipher) = line.split_once(',')?;
			Some(Record {
				id: id.trim().parse().ok()?,
				cipher: cipher.to_string(),
			})
		})
		.collect()
}

/// Words from a `{"id": "name"}` catalog.
fn words_of_json(path: &str) -> Vec<String> {
	let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
	let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
		return Vec::new();
	};
	let Some(map) = value.as_object() else { return Vec::new() };
	let mut out = Vec::new();
	for name in map.values().filter_map(|v| v.as_str()) {
		for word in name.split(|c: char| !c.is_ascii_alphabetic()) {
			if word.len() >= 2 {
				out.push(word.to_ascii_lowercase());
			}
		}
	}
	out.sort_unstable();
	out.dedup();
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_record_keeps_its_high_bytes() {
		// `°` is 0xB0 and is pass-through, so it survives the cipher and is a
		// crib. Reading the section as UTF-8 would destroy it.
		let records = parse(b"000123,abc \xb0C\n");
		assert_eq!(records.len(), 1);
		assert_eq!(records[0].id, 123);
		assert!(records[0].cipher.contains('\u{b0}'), "{:?}", records[0].cipher);
	}

	#[test]
	fn a_line_without_an_id_is_dropped_rather_than_guessed() {
		assert!(parse(b"not a record\n").is_empty());
	}

	#[test]
	fn the_trailing_field_run_is_cut_only_where_it_is_really_a_separator() {
		// `<name><sep><digit><sep><number>`: the separator recurs, so the run
		// is the record's tail and the name is what precedes it.
		assert_eq!(framed("Intake air temperature-4-23513").as_deref(), Some("Intake air temperature"));
		assert_eq!(framed("Engine speed.5.245,1").as_deref(), Some("Engine speed"));
		// A name that genuinely ends in a digit is the failure this guards:
		// the digit is inside the run, and cutting it would ship
		// `… of cylinder` six times over. One non-recurring character is not a
		// separator, so the record is dropped instead.
		assert_eq!(framed("Pressure of cylinder 4"), None);
		assert_eq!(framed("Nothing after the letters"), None);
	}

	#[test]
	fn a_word_pinned_by_nothing_but_the_dictionary_costs_the_record() {
		// The documented case: `Hill` and `Fill` differ in one letter that the
		// record uses once, so the rest of the record has no opinion and the
		// reading is a coin toss. Both are ordinary words, so the vocabulary
		// check passes it and only the margin catches it.
		let mut dict = Dictionary::default();
		for word in ["hill", "fill", "bytes", "backward"] {
			dict.insert(word, 100.0);
		}
		dict.finish();
		// A cipher whose first token maps to `hill` under a key that the other
		// tokens fix; `q`→`h`/`f` is the letter nothing else pins.
		assert!(!unambiguous("qill bytes", "hill bytes", &dict, MARGIN));
		// The same record with one reading far ahead of the other survives.
		let mut skewed = Dictionary::default();
		skewed.insert("hill", 1000.0);
		skewed.insert("fill", 1.0);
		skewed.insert("bytes", 100.0);
		skewed.finish();
		assert!(unambiguous("qill bytes", "hill bytes", &skewed, MARGIN));
	}

	#[test]
	fn the_gate_keeps_a_clean_reading_and_drops_the_four_kinds_of_unclean_one() {
		let mut dict = Dictionary::default();
		let mut known = HashSet::new();
		for word in ["intake", "air", "temperature", "bank"] {
			dict.insert(word, 100.0);
			known.insert(word.to_string());
		}
		dict.finish();
		let keep = |cipher: &str, plain: &str| gated_name(cipher, plain, &dict, &known);

		// Nothing in this reading is guessed: every letter resolved, every word
		// known, one candidate per pattern, and a real trailing field run.
		assert_eq!(
			keep("intake air temperature-4-2", "intake air temperature-4-2").as_deref(),
			Some("intake air temperature")
		);
		// An unresolved letter.
		assert_eq!(keep("intake air temper?ture-4-2", "intake air temper?ture-4-2"), None);
		// A word the vocabulary does not have.
		assert_eq!(keep("intake air tempxrature-4-2", "intake air tempxrature-4-2"), None);
		// Too few letters to be sure of.
		assert_eq!(keep("air bank-4-2", "air bank-4-2"), None);
		// No trailing run at all, so the framing cannot be shown to be clean.
		assert_eq!(keep("intake air temperature", "intake air temperature"), None);
	}

	#[test]
	fn a_directory_of_word_files_is_read_whole() {
		// `vagcan setup` hands the VCDS `Labels/` tree straight in, and the
		// label files' own language is the strongest prior the attack has.
		let dir = std::env::temp_dir().join(format!("vagcan-words-{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&dir);
		std::fs::create_dir_all(dir.join("nested")).unwrap();
		std::fs::write(dir.join("a.lbl"), "one").unwrap();
		std::fs::write(dir.join("nested/b.lbl"), "two").unwrap();
		let files = word_files(&dir);
		assert_eq!(files.len(), 2, "{files:?}");
		assert_eq!(word_files(&dir.join("a.lbl")).len(), 1);
		assert!(word_files(&dir.join("absent")).is_empty());
		let _ = std::fs::remove_dir_all(&dir);
	}

	/// Encipher with a rotation, standing in for a record's own key.
	fn encipher(plain: &str, shift: u8) -> String {
		plain
			.chars()
			.map(|c| match c {
				'a'..='z' => (b'a' + (c as u8 - b'a' + shift) % 26) as char,
				'A'..='Z' => (b'A' + (c as u8 - b'A' + shift) % 26) as char,
				_ => c,
			})
			.collect()
	}

	/// A one-record "cluster" table, so [`resolve_pass`] can be driven on a
	/// single crafted record.
	fn one_cluster(records: &[Record]) -> BTreeMap<Vec<u8>, Vec<usize>> {
		let mut clusters = BTreeMap::new();
		for (index, record) in records.iter().enumerate() {
			let tokens = tttext::tokens(&record.cipher);
			clusters.entry(tttext::pattern(&tokens.join("|"))).or_insert_with(Vec::new).push(index);
		}
		clusters
	}

	#[test]
	fn the_prior_is_the_word_count_of_the_decoded_names() {
		// The statistic §7 calls "a word-frequency prior measured on the decoded
		// label files themselves": every word of every reading, two-letter words
		// included, since those are the function words a leader misreads.
		let solved = HashMap::from([(0usize, "oil temperature: oil bank".to_string()), (1usize, "of oil".to_string())]);
		let freq = word_frequency(&solved);
		assert_eq!(freq["oil"], 3);
		assert_eq!(freq["temperature"], 1);
		assert_eq!(freq["of"], 1);
	}

	#[test]
	fn the_prior_settles_a_tie_the_seed_left_to_alphabetical_order() {
		// `oil` and `bil` share the shape `0 1 2`, and a two-token record pins
		// neither of the letters that tell them apart, so the seed — which
		// weighs every in-domain word alike — breaks the tie by alphabet and
		// reads the record as the wrong word. The decoded label files, where `oil`
		// outnumbers `bil` fifty to one, is the evidence that flips it: the
		// frequency dictionary weighs `oil` far above `bil`, and re-solving
		// under it reads the record right. This is the machinery whose absence
		// took the catalog from seventeen thousand names to four.
		let records = vec![Record {
			id: 7,
			cipher: encipher("oil level", 3),
		}];
		let clusters = one_cluster(&records);

		let seed_words: HashSet<String> = ["oil", "bil", "level"].iter().map(|w| w.to_string()).collect();
		// Flat seed: both readings weigh the same, and `bil` sorts first.
		let mut flat = Dictionary::default();
		for word in &seed_words {
			flat.insert(word, 8.0);
		}
		flat.finish();
		assert_eq!(
			resolve_pass(&records, &clusters, &flat, Limits::default()).get(&0).map(String::as_str),
			Some("bil level"),
			"without the prior the alphabetically-first word wins",
		);

		// The prior, measured off label files in which `oil` is common and `bil`
		// is a fluke, reweights the dictionary and the re-solve reads `oil`.
		let label_files = HashMap::from([("oil".to_string(), 50u32), ("bil".to_string(), 1)]);
		let prior = frequency_dictionary(&seed_words, &label_files);
		assert_eq!(
			resolve_pass(&records, &clusters, &prior, Limits::default()).get(&0).map(String::as_str),
			Some("oil level"),
			"the frequency prior corrects the pick",
		);
	}

	#[test]
	fn the_frequency_dictionary_keeps_a_known_word_the_label_files_never_showed() {
		// A rare-but-real term must survive at a floor of one, or re-solving
		// would lose every word that happened not to be decoded the first time.
		let known: HashSet<String> = ["oil", "reluctance"].iter().map(|w| w.to_string()).collect();
		let label_files = HashMap::from([("oil".to_string(), 40u32)]);
		let dict = frequency_dictionary(&known, &label_files);
		// `abc` shares `oil`'s shape; `reluctance` is its own shape.
		assert_eq!(dict.candidates("abc").iter().find(|(w, _)| w == "oil").map(|(_, w)| *w), Some(41.0));
		assert_eq!(
			dict.candidates("reluctance").iter().find(|(w, _)| w == "reluctance").map(|(_, w)| *w),
			Some(1.0),
			"a word the label files never showed is still a candidate",
		);
	}

	#[test]
	fn a_capped_seed_ties_the_enum_giants_and_keeps_the_content_words() {
		// The whole point of the cap: on the reference label files a label file says
		// `ok` far more often than `of`, purely because `OK` is a status value,
		// and left uncapped that made the search read `Status ok` for
		// `Status of`. Above the cap the two are tied and the decoded-label files
		// frequency decides; below it a content word keeps its true count. The
		// `!=` guards the cap from ever being raised above the giants.
		let (of, ok, voltage) = (3_947u32, 5_403u32, 30u32);
		assert_ne!(of, ok, "the raw counts really do disagree");
		assert_eq!(
			of.min(SEED_FREQUENCY_CAP),
			ok.min(SEED_FREQUENCY_CAP),
			"the enum giants must tie once saturated",
		);
		assert_eq!(voltage.min(SEED_FREQUENCY_CAP), voltage, "a content word is untouched");
	}

	/// End-to-end pin against the vendored label files, run on demand.
	///
	/// `#[ignore]` because it needs the decrypted `[TXT]` section, which is
	/// minutes of key search to produce and is not in the checkout. Dump it once
	/// (`vagcan vcds rod vendor/vcds-en/UDS_EV/TTTEXT.ROD --dump DIR`) and point
	/// the test at it:
	///
	/// ```text
	/// VAGCAN_TTTEXT_TXT=DIR/TXT.bin \
	/// VAGCAN_TTTEXT_LABELS=vendor/vcds-en/Labels \
	///   cargo test -p vagcan --bin vagcan -- --ignored tttext_reproduces
	/// ```
	///
	/// It pins the recovered-name count and a sample of id → name pairs so the
	/// prior cannot silently regress the way it did to 3,987.
	#[test]
	#[ignore = "needs the decrypted TTTEXT [TXT] section; see the doc comment"]
	fn tttext_reproduces_the_reference_names() {
		let (Ok(txt), Ok(labels)) = (std::env::var("VAGCAN_TTTEXT_TXT"), std::env::var("VAGCAN_TTTEXT_LABELS")) else {
			eprintln!("set VAGCAN_TTTEXT_TXT and VAGCAN_TTTEXT_LABELS to run this");
			return;
		};
		assert!(
			std::path::Path::new("/usr/share/dict/words").exists(),
			"the pinned count was measured with the system word list present",
		);
		let out = std::env::temp_dir().join(format!("vagcan-tttext-pin-{}.json", std::process::id()));
		run(Options {
			file: &txt,
			words: &[format!("{labels}:8"), "/usr/share/dict/words:1".to_string()],
			names: None,
			out: None,
			catalog: Some(&out.to_string_lossy()),
			partial: None,
			passes: 4,
			steps: None,
			check: 0,
			gated: false,
		})
		.expect("the run should succeed against the label files");

		let catalog: BTreeMap<String, String> = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
		let _ = std::fs::remove_file(&out);

		assert_eq!(catalog.len(), 14_738, "recovered-name count moved");
		for (id, name) in [
			("000035", "Regulation due to excessive temperature"),
			("000080", "Absolute intake pressure"),
			("000089", "Activation condition ACC interface"),
			("000114", "Speed regulation requested torque"),
			("000128", "A/C compressor activation"),
		] {
			assert_eq!(catalog.get(id).map(String::as_str), Some(name), "id {id} moved");
		}
	}
}
