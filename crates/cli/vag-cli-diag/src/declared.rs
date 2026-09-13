//! What a source says a control unit answers — and the rule that a sweep asks
//! nothing else.
//!
//! A sweep used to ask every unit the same nine pages of identifiers, 2816 of
//! them, with no evidence any one existed. That is a fuzz test of a diagnostic
//! server: each request takes a path through firmware nothing has exercised,
//! and a path with a defect in it crashes the server — and the server is a
//! control unit the car is relying on.
//!
//! Blind sweeping was once the only thing available. It is not any more. This
//! tool resolves a unit's ODIS variant from what the unit itself reports —
//! `F19E` names the ODX file, `F1A2` picks the version, `F187` is the part
//! number — and that variant *declares which identifiers it defines*. Asking a
//! control unit the questions its own manufacturer's data says it answers is
//! not a fuzz test. So that is the default, and the blind sweep is what
//! somebody has to ask for, per unit, in so many words.
//!
//! Nothing here knows anything about any particular car. The declaration comes
//! from the project data under `~/.vagcan`, keyed by what the vehicle in front
//! of the tool said about itself; a car with no matching variant gets an empty
//! answer rather than another car's identifiers.

use std::collections::BTreeSet;
use std::ops::RangeInclusive;

use vag_data_labels::catalog::{CatalogStore, ReadId};

use crate::extracted::Extracted;

/// Where a unit's identifier list came from — and therefore whether asking it
/// is a read or an experiment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
	/// A catalog proven on a car, or an ODIS variant the unit's own answers
	/// resolved to, declares these identifiers.
	Declared,
	/// Someone aimed a blind sweep at this unit by hand. The declared
	/// identifiers are still included — `--blind` widens, it never narrows.
	Blind,
	/// Nothing declares anything for this unit.
	Unknown,
}

/// What a sweep will ask one control unit, and on whose authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
	pub ranges: Vec<RangeInclusive<u16>>,
	pub source: Source,
}

impl Ask {
	/// Whether there is anything to ask at all.
	pub fn is_empty(&self) -> bool {
		self.ranges.is_empty()
	}

	/// How many identifiers this comes to.
	pub fn total(&self) -> usize {
		crate::scan::total_dids(&self.ranges)
	}

	/// The ranges as a survey file writes them down: `0102-0104`, or a bare
	/// `F187` where a span is one identifier wide.
	///
	/// A survey has always recorded what *answered* and never what was *asked*,
	/// and the two are only the same question for a sweep that covered
	/// everything. Anything reading a survey to decide whether a control unit
	/// has an identifier — `watch` holds back the ones this car answers nothing
	/// to — is otherwise guessing on a run aimed with `--blind --range`.
	pub fn spans_text(&self) -> Vec<String> {
		self
			.ranges
			.iter()
			.map(|r| match r.start() == r.end() {
				true => format!("{:04X}", r.start()),
				false => format!("{:04X}-{:04X}", r.start(), r.end()),
			})
			.collect()
	}
}

/// The identifiers some source declares for a unit, given what the unit said
/// about itself.
///
/// `part_number`, `odx_name` and `version` are `F187`, `F19E` and `F1A2`
/// **exactly as the control unit answered them** — the match rule normalises
/// inside, and tidying here is how a caller ends up reimplementing half of it.
///
/// The join is [`crate::extracted::for_unit`], the same one `watch` and
/// `measure` resolve channels through, so a sweep can never ask for an
/// identifier those two would not know how to read, nor skip one they would.
pub fn declared(
	store: &CatalogStore,
	extracted: &Extracted,
	part_number: Option<&str>,
	odx_name: Option<&str>,
	version: Option<&str>,
) -> BTreeSet<u16> {
	crate::extracted::for_unit(store, extracted, part_number, odx_name, version)
		.into_iter()
		.map(|def| {
			let ReadId::Uds(did) = def.address;
			did
		})
		.collect()
}

/// Turn a set of identifiers into the spans a sweep walks.
///
/// Adjacent identifiers collapse into one span so group testing still gets
/// whole batches out of a declared block — and only ever out of declared ones,
/// because a span is built from members and never from a gap between them.
pub fn spans(dids: &BTreeSet<u16>) -> Vec<RangeInclusive<u16>> {
	let mut out: Vec<RangeInclusive<u16>> = Vec::new();
	for did in dids {
		match out.last_mut() {
			// `checked_add` rather than `+ 1`: `0xFFFF` is a legal identifier
			// and a sweep must not panic on the last one in the space.
			Some(last) if last.end().checked_add(1) == Some(*did) => *last = *last.start()..=*did,
			_ => out.push(*did..=*did),
		}
	}
	out
}

/// What to ask one unit: what a source declares, widened by a blind range if
/// somebody aimed one at this unit.
///
/// `blind` is `None` for every unit nobody named. That is the whole of the
/// opt-in: there is no value of any flag that turns blind sweeping on for a
/// car, only for units listed one by one.
pub fn ask(declared: &BTreeSet<u16>, blind: Option<&[RangeInclusive<u16>]>) -> Ask {
	match blind {
		Some(ranges) => {
			// The union, not a replacement. Widening a sweep must never lose an
			// identifier the data already vouched for — a `--blind` run is for
			// finding what nothing declares, on top of what something does.
			let mut all: BTreeSet<u16> = declared.clone();
			for range in ranges {
				all.extend(range.clone());
			}
			Ask {
				ranges: spans(&all),
				source: Source::Blind,
			}
		}
		None if declared.is_empty() => Ask {
			ranges: Vec::new(),
			source: Source::Unknown,
		},
		None => Ask {
			ranges: spans(declared),
			source: Source::Declared,
		},
	}
}

/// The blind range for this run, or `None` because nobody asked for one.
///
/// `--range` describes a blind sweep and describes nothing else, so naming one
/// without `--blind` is refused rather than quietly ignored. A run that did
/// less than its flags said is how somebody concludes the tool is broken and
/// reaches for the biggest hammer they can find.
pub fn blind_ranges(range: Option<&str>, blind: bool, default: &str) -> anyhow::Result<Option<Vec<RangeInclusive<u16>>>> {
	if !blind {
		if range.is_some() {
			anyhow::bail!(
				"--range says what to sweep blind, and no unit was named to sweep. \n\
				 A sweep of identifiers nothing declares is a fuzz test of a control unit's \n\
				 diagnostic server, and a path with a defect in it crashes the server. Name \n\
				 the unit you mean with --blind, or drop --range and read what the car's own \n\
				 data declares."
			);
		}
		return Ok(None);
	}
	let spec = range.unwrap_or(default);
	let ranges = crate::scan::parse_ranges(spec).map_err(|e| anyhow::anyhow!("--range: {e}"))?;
	Ok(Some(ranges))
}

/// The request ids a `--ecu` / `--only` / `--blind` list names.
///
/// **`flag` is the only thing that differed between the three copies**, and it
/// is the whole value of passing it: `faults --ecu`, `dev survey --only` and
/// `dev survey --blind` all take a unit list, and somebody who mistyped one of
/// them has to be told which. Everything else — parse, take the request id of
/// each unit — was written out three times.
///
/// Checked before the adapter is opened, wherever it is called: the cable is a
/// single-user resource, and holding it open to fail on a typo blocks the next
/// attempt.
pub fn unit_list(flag: &str, spec: &str) -> anyhow::Result<Vec<u16>> {
	Ok(
		vag_uds_client::address::parse_list(spec)
			.map_err(|e| anyhow::anyhow!("{flag}: {e}"))?
			.iter()
			.map(|u| u.request)
			.collect(),
	)
}

/// What to say about a unit no source describes.
///
/// **This is the case the default cannot sweep**, and on a real car it is a
/// couple of units out of fifteen. The old behaviour — ask it the nine pages
/// anyway — is exactly the fuzz test described above, performed on the
/// units the tool understands *least*. So it is not swept, its identification
/// block and its faults are still read and still filed, and the command says
/// what it would take to go further.
///
/// The invocation that would sweep it blind is built here rather than passed
/// in. It was a parameter while `scan` and `survey` each needed their own
/// spelling; `scan` is gone, one caller is left, and a parameter with one
/// argument is a place for the two to disagree with nobody left to disagree
/// with.
pub fn no_source_notice(unit: &str) -> String {
	format!(
		"  {unit:<4}      nothing declares identifiers for this unit — identified, not swept\n\
		 \x20              to sweep it blind (a fuzz test of its diagnostic server):\n\
		 \x20                vagcan dev survey --only {unit} --blind {unit}"
	)
}

/// The sources this run has: rows proven on a car, and the extracted project
/// under them.
///
/// A machine where `vagcan setup` has never run has neither, and that is not an
/// error — it is a car whose units will all report no source, which is a true
/// statement about what this tool knows rather than a reason to start guessing.
pub fn sources() -> (CatalogStore, Extracted) {
	let store = match crate::project::current() {
		Ok(project) => CatalogStore::open(project.measurements_dir()),
		// No project means no proven rows. `CatalogStore` over a path that does
		// not exist finds nothing, which is the answer we want anyway.
		Err(_) => CatalogStore::open(std::path::PathBuf::new()),
	};
	(store, crate::extracted::current())
}

#[cfg(test)]
mod tests {
	// `&[0x2000..=0x20FF]` is one range inside a slice of ranges, which is what
	// `scan_dids` and friends take. Clippy reads it as a possible typo for
	// `(0x2000..=0x20FF).collect()`; the parameter type makes that reading
	// impossible, and spelling one range as a vec! of one would be worse.
	#![allow(clippy::single_range_in_vec_init)]
	use super::*;
	use vag_data_labels::catalog::{MeasurementDef, Scaling};
	use vag_data_labels::measure::{LinearScale, RawForm};

	fn def(did: u16) -> MeasurementDef {
		MeasurementDef {
			name: format!("channel {did:04X}").into(),
			unit: "".into(),
			address: ReadId::Uds(did),
			raw_form: RawForm::U16Be,
			scaling: Scaling::Linear(LinearScale { factor: 1.0, offset: 0.0 }),
		}
	}

	/// A store holding one unit's proven rows, written to a temp directory.
	/// **Every byte is synthetic** — no test reads the owner's `~/.vagcan`.
	///
	/// The file is named the way `CatalogStore` looks it up — alphanumerics
	/// only, upper case — while the query below passes the ODX name as a
	/// control unit actually answers it, punctuation and all.
	fn store_with(dir: &std::path::Path, key: &str, dids: &[u16]) -> CatalogStore {
		let catalog = vag_data_labels::catalog::MeasurementCatalog::new(dids.iter().copied().map(def).collect());
		let file = key.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
		std::fs::write(dir.join(format!("{file}.json")), catalog.to_json().unwrap()).unwrap();
		CatalogStore::open(dir)
	}

	#[test]
	fn a_sweep_asks_only_the_identifiers_a_source_declares() {
		// The whole point. 2816 blind requests per unit is a fuzz test of the
		// unit's diagnostic server; a unit the data describes gets asked its
		// own identifiers and nothing else.
		let here = tempfile::tempdir().unwrap();
		let store = store_with(here.path(), "EV_BCMMQB", &[0x1000, 0x1001, 0x2000]);
		let declared = declared(&store, &Extracted::none(), None, Some("EV_BCMMQB"), None);
		assert_eq!(declared.iter().copied().collect::<Vec<_>>(), [0x1000, 0x1001, 0x2000]);

		let ask = ask(&declared, None);
		assert_eq!(ask.source, Source::Declared);
		assert_eq!(ask.total(), 3, "three identifiers, not three pages: {:?}", ask.ranges);
		// And nothing between them: the run 1000..=1001 collapses, 2000 stands
		// alone, and 1002..1FFF is never asked.
		assert_eq!(ask.ranges, vec![0x1000..=0x1001, 0x2000..=0x2000]);
	}

	#[test]
	fn a_unit_no_source_describes_is_not_swept_at_all() {
		// Two of the reference car's fifteen units resolve to no variant. The
		// old default swept them the hardest — nine pages of identifiers at the
		// units this tool understands least.
		let here = tempfile::tempdir().unwrap();
		let store = CatalogStore::open(here.path());
		let declared = declared(&store, &Extracted::none(), Some("1K0907379AQ"), Some("EV_Unheard_Of"), None);
		assert!(declared.is_empty());

		let ask = ask(&declared, None);
		assert_eq!(ask.source, Source::Unknown);
		assert!(ask.is_empty(), "an unknown unit gets asked nothing: {:?}", ask.ranges);

		// And it is told how to go further, in terms that say what it costs.
		let notice = no_source_notice("44");
		assert!(notice.contains("not swept"), "{notice}");
		assert!(notice.contains("--blind 44"), "{notice}");
		assert!(notice.contains("fuzz test"), "{notice}");
	}

	#[test]
	fn what_was_asked_is_written_down_in_a_form_a_reader_can_take_back_apart() {
		// The other half of a contract `watch` depends on: it decides which
		// declared channels this car does not have by comparing what answered
		// against what was asked, and it can only do that if the survey wrote
		// the range in a shape that survives the round trip.
		let declared: BTreeSet<u16> = [0x0102, 0x0103, 0x0104, 0xF187].into_iter().collect();
		let asked = ask(&declared, None);
		assert_eq!(asked.spans_text(), vec!["0102-0104", "F187"], "a run of three, then one on its own");

		// A unit nobody asked anything writes an empty list, not a missing one:
		// "asked nothing" and "no record of what was asked" are different
		// statements and only the second is a shrug.
		assert!(ask(&BTreeSet::new(), None).spans_text().is_empty());

		// And the last identifier in the space, which is where an off-by-one
		// in the span walk would show up.
		let edge: BTreeSet<u16> = [0xFFFE, 0xFFFF].into_iter().collect();
		assert_eq!(ask(&edge, None).spans_text(), vec!["FFFE-FFFF"]);
	}

	#[test]
	fn blind_widens_a_sweep_and_never_narrows_it() {
		// Somebody hunting new identifiers must not silently lose the ones the
		// data already vouched for.
		let here = tempfile::tempdir().unwrap();
		let store = store_with(here.path(), "EV_Test", &[0xF187, 0x2000]);
		let declared = declared(&store, &Extracted::none(), None, Some("EV_Test"), None);

		let ask = ask(&declared, Some(&[0x3800..=0x3803]));
		assert_eq!(ask.source, Source::Blind);
		assert_eq!(ask.ranges, vec![0x2000..=0x2000, 0x3800..=0x3803, 0xF187..=0xF187]);
		assert_eq!(ask.total(), 6);
	}

	#[test]
	fn a_range_without_a_unit_to_aim_it_at_is_refused() {
		// `--range 0000-FFFF` with nothing to aim it at used to be the whole
		// of the blind sweep. Ignoring it quietly would be worse than refusing:
		// somebody would conclude the sweep was broken.
		let err = blind_ranges(Some("0000-FFFF"), false, "F100-F1FF").unwrap_err().to_string();
		assert!(err.contains("--blind"), "{err}");
		assert!(err.contains("fuzz test"), "it says what it costs: {err}");

		// Nothing asked for, nothing swept blind.
		assert_eq!(blind_ranges(None, false, "F100-F1FF").unwrap(), None);
		// Aimed, with no range: the default range, which is what --blind means
		// on its own.
		assert_eq!(blind_ranges(None, true, "F100-F1FF").unwrap(), Some(vec![0xF100..=0xF1FF]));
		assert_eq!(blind_ranges(Some("2000-2001"), true, "F100-F1FF").unwrap(), Some(vec![0x2000..=0x2001]));
		assert!(blind_ranges(Some("nonsense"), true, "F100-F1FF").is_err());
	}

	#[test]
	fn adjacent_identifiers_become_one_span_and_a_gap_never_does() {
		// Group testing walks spans in batches of eight. A span built across a
		// gap would ask for identifiers nothing declared — which is the whole
		// thing this module exists to prevent.
		let dids: BTreeSet<u16> = [0x2000, 0x2001, 0x2002, 0x2004, 0xFFFF].into_iter().collect();
		assert_eq!(spans(&dids), vec![0x2000..=0x2002, 0x2004..=0x2004, 0xFFFF..=0xFFFF]);
		assert_eq!(crate::scan::total_dids(&spans(&dids)), dids.len());
		assert!(spans(&BTreeSet::new()).is_empty());
	}
}
