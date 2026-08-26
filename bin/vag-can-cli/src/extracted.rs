//! The channels an extracted source knows, underneath the ones a drive proved.
//!
//! An ODIS project describes 717 ECU variants and 310,734 readable channels, by
//! UDS identifier, with a scaling for each. A car reports which variant it is —
//! `F19E` names the ODX file and `F1A2`'s leading three digits pick the version
//! — and [`vag_data::label_files::odx_match`] is the rule that turns those two
//! answers into a match. That rule is not reimplemented here: it is the same
//! function the `.rod` directory walk uses, so a name the walk would find and a
//! name this finds can never disagree.
//!
//! **Everything here is evidence, not a catalog** (design §4.5). A row proven on
//! the actual car in front of the tool always wins; an extracted row fills in
//! only what no drive has established yet. [`merge`] is where that is enforced,
//! once, so that nothing downstream — `watch`, `scan`, `survey`, `properties` —
//! ever has to ask which source a row came from.
//!
//! ## The gear conflict is not settled here, and must not be
//!
//! The reference car's proven gear row reads raw `0x0C` as reverse; this
//! project's `0x210F` calls the same raw value "Gear 9"
//! (`research/labels/odis-format.md` §7.1). They may simply be different
//! channels on different units, and one minute with the car settles it — select
//! reverse, read `0x210F` on `7E0` and `0x3816` on `7E1`. Until somebody does,
//! [`merge`] keeps the proven row and the extracted one stays unread. No code
//! here picks a winner on the strength of an argument.

use std::collections::BTreeMap;
use std::path::PathBuf;

use vag_data::catalog::{MeasurementDef, ReadId};
use vag_data::label_files::{OdxMatch, odx_match};
use vag_data::measure::RawForm;

/// One project's extracted channels, ready to be asked about a control unit.
///
/// Holds the variant names rather than the channels: a project has hundreds of
/// variants and a third of a million channels, and a car needs the handful
/// belonging to the one variant it turns out to be.
///
/// It also holds the project's `names.json`, because that file is the other
/// half of the same question. An extracted row carries a **text id** and that
/// id is the key `names.json` is written under — the whole finding
/// `research/labels/odis-crib.md` §3 rests on — so a channel's wording is a
/// lookup through an id the row itself carries, never a table of names in this
/// source.
#[derive(Debug)]
pub struct Extracted {
	cache: PathBuf,
	variants: Vec<String>,
	/// text id → what the label files call it. Empty for a project that has
	/// none, which is not an error: `watch --catalogs <dir>` has no project at
	/// all, and a project set up before names were recovered has no file.
	names: BTreeMap<String, String>,
	/// text id → what **this machine's owner** calls it, in the language
	/// `config.toml` names — see [`crate::glossary`].
	///
	/// It outranks everything, because it is the only wording written by
	/// somebody who has to read it at an open driver's door. It is also global
	/// rather than per project: a text id is VW's key for a piece of text, not
	/// a fact about a platform, so a translation written once holds for every
	/// car afterwards.
	mine: BTreeMap<String, String>,
}

/// One channel a unit offers, with everything known about how to name it.
///
/// Three separate facts, and collapsing any two of them loses something a
/// reader is entitled to: `def` is how the bytes are read, `proven` is whether
/// a drive established that, and `named` is the label files' own wording for
/// the same channel. The last one exists because an ODIS long name is written
/// for a diagnostic engineer — `Brake_pedal_information_plausibility` — and the
/// text id it carries reaches a sentence written for a person.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
	pub def: MeasurementDef,
	/// Whether a drive on a car established this scaling — see [`tagged`].
	pub proven: bool,
	/// What this project's `names.json` calls the channel's text id, when the
	/// row carried one and the file knows it.
	pub named: Option<String>,
	/// The row's own text id, kept rather than consumed.
	///
	/// It is the key `~/.vagcan/names.csv` is written under, so a person who
	/// has just read a bad name on screen needs it to write a better one — see
	/// `watch`'s `show_key` setting. `None` on a proven row, which was named by
	/// whoever proved it and has no id to look anything up by.
	pub text_id: Option<String>,
}

/// What this run's project knows, or nothing when there is no project.
///
/// The ordinary entry point: a command that is about to resolve channels has a
/// `CatalogStore` and needs the extracted rows that go underneath it. No project
/// is not an error — `--catalogs <dir>` names proven rows directly, and a
/// machine where `vagcan setup` has not run has none of either.
pub fn current() -> Extracted {
	match crate::project::current() {
		Ok(project) => open(&project),
		Err(_) => Extracted::none(),
	}
}

/// Read a project's variant list, or nothing when it has no extracted rows.
///
/// A project built from a VCDS installation alone has none, and that is the
/// ordinary case rather than an error — the tool worked that way until an ODIS
/// project became a second source, and it still does.
pub fn open(project: &crate::project::Project) -> Extracted {
	let cache = project.cache();
	let variants = vag_db::reading_variants(&cache).unwrap_or_default();
	Extracted {
		cache,
		variants,
		// **Only when a VCDS installation has actually contributed.** The join
		// through a text id is right and stays; what was wrong is trusting
		// `names.json` to hold VCDS wording on a project that has only ever
		// seen ODIS runs. There it holds ODIS's *pooled* text for the id, while
		// the row carries that parameter's name in this ECU variant — and the
		// pooled one is the worse of the two, measured on the owner's car: 0
		// channels gained a name, 340 got different and mostly worse wording,
		// and two distinct channels collapsed onto one label.
		//
		// A file cannot say who wrote it, so the project's own source log is
		// asked instead (`crate::project::has_source`).
		names: match crate::project::has_source(project, vag_db::VCDS) {
			true => read_names(&project.names()),
			false => BTreeMap::new(),
		},
		// Read for every project, whatever its sources: this file is the
		// owner's, not a vendor's, and the id it is keyed by is VW's rather than
		// this platform's.
		mine: crate::glossary::load(crate::config::language(&crate::config::load())),
	}
}

/// A project's `names.json` as a map, or nothing at all.
///
/// A missing or unparseable file is a project whose names have not been
/// recovered, which is an ordinary state and not a failure: every channel then
/// keeps whatever its own source called it. Failing the run over it would take
/// `watch` off a car for the sake of nicer wording.
fn read_names(path: &std::path::Path) -> BTreeMap<String, String> {
	std::fs::read_to_string(path)
		.ok()
		.and_then(|text| serde_json::from_str(&text).ok())
		.unwrap_or_default()
}

impl Extracted {
	/// A project that knows nothing — for a caller with no project to hand.
	///
	/// Not only for tests: `watch --catalogs <dir>` and `measure --catalogs
	/// <dir>` name a directory of proven rows directly, without a project, and
	/// there is nothing extracted to go under them.
	pub fn none() -> Extracted {
		Extracted {
			cache: PathBuf::new(),
			variants: Vec::new(),
			names: BTreeMap::new(),
			mine: BTreeMap::new(),
		}
	}

	/// What the label files call a text id, when this project has recovered it.
	///
	/// The whole of the name join, in one place: nothing else in this tool maps
	/// an id to words, so a caller cannot end up with a name from somewhere
	/// this project cannot point at.
	pub fn name_of(&self, text_id: Option<&str>) -> Option<&str> {
		let id = text_id?;
		// The owner's own glossary first, then whatever the label files
		// recovered. An id the glossary does not mention falls straight
		// through, which is what makes the file worth writing one line at a
		// time instead of all at once.
		self
			.mine
			.get(id)
			.or_else(|| self.names.get(id))
			.map(String::as_str)
			.filter(|name| !name.trim().is_empty())
	}

	/// Every text id this project has recovered wording for, and that wording.
	///
	/// For [`crate::glossary::seed`], which needs the ids to key a translation
	/// by and the current name so that translating is reading rather than
	/// guessing. Not for display — [`Self::name_of`] is the one lookup.
	pub fn names(&self) -> BTreeMap<String, String> {
		self.names.clone()
	}

	/// Whether this project knows any channels at all.
	pub fn is_empty(&self) -> bool {
		self.variants.is_empty()
	}

	/// The channels this project knows for a unit, given what the unit said.
	///
	/// `odx_name` is `F19E` and `version` is `F1A2`, **passed through exactly as
	/// the car answered them** — padding, NULs and full length. [`odx_match`]
	/// normalises inside, and tidying here is how a caller ends up
	/// reimplementing the half of the rule it was reusing.
	///
	/// Only the best-ranked variants are read. A `Family` match is a right
	/// family with an unconfirmed variant, and when an `Exact` or a `Version`
	/// match exists the family ones are guesses standing next to an answer.
	pub fn for_unit(&self, odx_name: Option<&str>, version: Option<&str>) -> Vec<MeasurementDef> {
		self.described(odx_name, version).into_iter().map(|(def, _)| def).collect()
	}

	/// The same, with each row's **text id** still attached.
	///
	/// Split out rather than folded into [`Self::for_unit`] because the id is
	/// what reaches `names.json`, and dropping it here is exactly how every
	/// channel on the selection screen came to be called after the ODIS long
	/// name — or, where there was none, after its own identifier.
	fn described(&self, odx_name: Option<&str>, version: Option<&str>) -> Vec<(MeasurementDef, Option<String>)> {
		// A project that knows nothing answers nothing, without opening a cache
		// that has nothing in it. That is every VCDS-only project, which is
		// still the common case.
		if self.is_empty() {
			return Vec::new();
		}
		let Some(odx_name) = odx_name else { return Vec::new() };
		let version = version.unwrap_or("");
		let mut ranked: Vec<(OdxMatch, &String)> = self
			.variants
			.iter()
			.filter_map(|name| odx_match(name, odx_name, version).map(|rank| (rank, name)))
			.collect();
		if ranked.is_empty() {
			return Vec::new();
		}
		// `OdxMatch` orders best-first, so the minimum is the best rank there is.
		let best = ranked.iter().map(|(rank, _)| *rank).min().expect("ranked is not empty");
		ranked.retain(|(rank, _)| *rank == best);
		ranked.sort_by(|a, b| a.1.cmp(b.1));

		let mut out: Vec<(MeasurementDef, Option<String>)> = Vec::new();
		for (_, name) in ranked {
			let Ok(readings) = vag_db::readings_of(&self.cache, name) else {
				continue;
			};
			for reading in readings {
				let text_id = reading.text_id.clone();
				let Some(def) = to_def(&reading) else { continue };
				// Two variants of one family can describe the same **field**. The
				// first wins, which is the alphabetically first — arbitrary, but
				// stable, and a run that reported a different name each time
				// would be worse than one that reports a fixed one.
				//
				// The field, not the identifier. Keying this by DID alone was
				// worth 1,952 channels of 3,963 on the reference car: everything
				// a control unit packed into a response after the first field
				// was parsed, scaled, named — and then dropped one line before
				// it could be shown.
				if !out.iter().any(|(held, _)| same_field(held, &def)) {
					out.push((def, text_id));
				}
			}
		}
		out
	}
}

/// One extracted channel as the rest of this tool speaks about channels.
///
/// **`None` for anything [`RawForm`] cannot say exactly**, which since the
/// widening means a field that does not start on a byte, does not fill whole
/// bytes, or is wider than four of them — a one-bit flag, a 3-bit field at bit
/// 19, a 32-byte block. Approximating one of those would produce a confident
/// wrong number, which is worse than the raw bytes the reader gets today.
/// [`RawForm::for_field`] is the whole rule and it lives with the enum, so the
/// vocabulary is defined in one place rather than half here.
fn to_def(reading: &vag_data::odis::Reading) -> Option<MeasurementDef> {
	let form = RawForm::for_field(reading.bit_offset, reading.bit_length, reading.signed, reading.big_endian)?;
	Some(MeasurementDef {
		name: reading.name.clone().into(),
		unit: reading.unit.clone().unwrap_or_default().into(),
		address: ReadId::Uds(reading.did),
		raw_form: form,
		scaling: reading.scaling.clone(),
	})
}

/// Whether two definitions describe the same field of the same identifier.
///
/// Same address **and** same starting byte. Two fields of one response are two
/// channels; the same field said twice by two variants of a family is one.
fn same_field(a: &MeasurementDef, b: &MeasurementDef) -> bool {
	a.address == b.address && a.raw_form.bit_offset() == b.raw_form.bit_offset()
}

/// Put the proven rows on top of the extracted ones, by identifier.
///
/// **Design §4.5, and the one place it is enforced.** A `measurements/` row is
/// the only data proven on the actual car in front of the tool; everything
/// extracted — from a VCDS installation or an ODIS project — is evidence for a
/// catalog and not the catalog itself, until a drive confirms it. So a proven
/// row wins at its identifier and an extracted row fills in only where none
/// exists.
///
/// The proven rows keep their order and come first, because that is the order
/// somebody established by driving and the order they will look for.
pub fn merge(proven: Vec<MeasurementDef>, extracted: Vec<MeasurementDef>) -> Vec<MeasurementDef> {
	let mut out = proven;
	for def in extracted {
		// By field, not by identifier. A proven row for byte 0 of `0x2029` says
		// nothing about byte 2 of it, and letting it suppress that field would
		// make a drive *cost* channels — the opposite of what proving one is
		// for.
		if !out.iter().any(|held| same_field(held, &def)) {
			out.push(def);
		}
	}
	out
}

/// The channels for one unit, each tagged with whether a drive proved it.
///
/// The whole join in one call. A caller says what the unit reported and gets
/// back what can be read off it, in precedence order — but it is told which
/// rows are proven, because that is the one thing about a row a reader is
/// entitled to know. Everything else about where a row came from stays here.
///
/// **The tag is not a source label, it is a claim about confidence.** "Proven"
/// means a drive established this scaling on a car; "not proven" covers both an
/// ODIS compu formula and a standard OBD-II parameter, and neither has been
/// confirmed against the vehicle in front of the tool. Reporting an extracted
/// row as proven would be the single most misleading thing this join could do.
pub fn tagged(
	store: &vag_data::catalog::CatalogStore,
	extracted: &Extracted,
	part_number: Option<&str>,
	odx_name: Option<&str>,
	version: Option<&str>,
) -> Vec<Resolved> {
	let proven = store.for_unit(part_number, odx_name);
	let mut out: Vec<Resolved> = proven
		.into_iter()
		.map(|def| Resolved {
			def,
			proven: true,
			// A proven row was named by whoever proved it on the car, and that
			// name is the one they will look for. There is no text id on it to
			// look anything else up by, either.
			named: None,
			text_id: None,
		})
		.collect();
	for (def, text_id) in extracted.described(odx_name, version) {
		// **By field, not by address** — the same rule [`merge`] enforces, and
		// missed here when it was written there. A proven row for the first two
		// bytes of an identifier says nothing about the field beside it, and
		// dropping the neighbours undid the whole of "every field of a response
		// is its own channel" on the one path `watch` actually takes.
		if !out.iter().any(|held| same_field(&held.def, &def)) {
			out.push(Resolved {
				named: extracted.name_of(text_id.as_deref()).map(str::to_string),
				text_id,
				def,
				proven: false,
			});
		}
	}
	out
}

/// The same, without the tag, for a caller that keeps its own.
pub fn for_unit(
	store: &vag_data::catalog::CatalogStore,
	extracted: &Extracted,
	part_number: Option<&str>,
	odx_name: Option<&str>,
	version: Option<&str>,
) -> Vec<MeasurementDef> {
	merge(store.for_unit(part_number, odx_name), extracted.for_unit(odx_name, version))
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::path::Path;
	use vag_data::catalog::Scaling;
	use vag_data::measure::LinearScale;

	/// A cache holding one project's channels. **Every byte is synthetic** — no
	/// test reads a real ODIS project or the owner's own `~/.vagcan`.
	fn cache_with(dir: &Path, variants: &[(&str, Vec<vag_data::odis::Reading>)]) -> Extracted {
		named_cache_with(dir, variants, &[])
	}

	/// The same, with a `names.json` beside it — text id to what the label
	/// files call it. **Every byte is synthetic**, ids included.
	fn named_cache_with(dir: &Path, variants: &[(&str, Vec<vag_data::odis::Reading>)], names: &[(&str, &str)]) -> Extracted {
		let cache = dir.join("cache.sqlite");
		for (name, readings) in variants {
			vag_db::put_readings(&cache, "/nowhere/SK37X", name, readings).expect("the fixture writes");
		}
		Extracted {
			variants: vag_db::reading_variants(&cache).unwrap(),
			cache,
			names: names.iter().map(|(id, text)| (id.to_string(), text.to_string())).collect(),
			// Never the owner's real one: a test must not read `~/.vagcan`.
			mine: BTreeMap::new(),
		}
	}

	fn reading(did: u16, name: &str, bit_offset: u32, bit_length: u32, big_endian: bool) -> vag_data::odis::Reading {
		vag_data::odis::Reading {
			did,
			name: name.to_string(),
			unit: Some("/min".to_string()),
			bit_offset,
			bit_length,
			signed: false,
			big_endian,
			scaling: Scaling::Linear(LinearScale { factor: 1.0, offset: 0.0 }),
			text_id: None,
		}
	}

	fn proven(did: u16, name: &str) -> MeasurementDef {
		MeasurementDef {
			name: name.to_string().into(),
			unit: "".into(),
			address: ReadId::Uds(did),
			raw_form: RawForm::U16Le,
			scaling: Scaling::Linear(LinearScale { factor: 0.25, offset: 0.0 }),
		}
	}

	#[test]
	fn every_field_of_one_response_is_its_own_channel() {
		// The loss this fixes. A `0x22` answer carries as many fields as the
		// control unit put in it, and keying a channel by its identifier held
		// the first and dropped the rest: 1,952 of 3,963 sayable fields on the
		// reference car's fifteen units, all of them parsed, scaled and named
		// before being thrown away one line from the screen.
		let here = tempfile::tempdir().unwrap();
		let extracted = cache_with(
			here.path(),
			&[(
				"EV_Test_001",
				vec![
					reading(0x2029, "Boost, specified", 0, 16, true),
					reading(0x2029, "Boost, actual", 16, 16, true),
					reading(0x2029, "Charge air temperature", 32, 8, true),
				],
			)],
		);

		let defs = extracted.for_unit(Some("EV_Test"), Some("001007"));
		assert_eq!(defs.len(), 3, "three fields, three channels: {defs:#?}");
		let offsets: Vec<u32> = defs.iter().map(|d| d.raw_form.bit_offset()).collect();
		assert_eq!(offsets, vec![0, 16, 32], "and each reads from its own place in the response");
		assert!(defs.iter().all(|d| d.address == ReadId::Uds(0x2029)), "one identifier, one request");
	}

	#[test]
	fn a_proven_field_does_not_suppress_the_others_beside_it() {
		// §4.5 says a proven row wins **at its identifier**, which was enforced
		// as "and everything else at that identifier disappears". A drive that
		// proved byte 0 would then cost the channels at bytes 2 and 4 — proving
		// a scaling would *lose* measurements, which is the opposite of the
		// rule's purpose.
		let here = tempfile::tempdir().unwrap();
		let extracted = cache_with(
			here.path(),
			&[(
				"EV_Test_001",
				vec![
					reading(0x2029, "Boost, specified", 0, 16, true),
					reading(0x2029, "Boost, actual", 16, 16, true),
				],
			)],
		);
		let merged = merge(
			vec![proven(0x2029, "Boost, specified")],
			extracted.for_unit(Some("EV_Test"), Some("001007")),
		);

		assert_eq!(merged.len(), 2, "the proven field and the one beside it: {merged:#?}");
		assert_eq!(
			merged[0].name.as_ref(),
			"Boost, specified",
			"the proven row is still first and still wins its own byte"
		);
		assert_eq!(merged[0].raw_form, RawForm::U16Le, "with the form a drive established, not the file's");
		assert_eq!(merged[1].raw_form.bit_offset(), 16);
	}

	#[test]
	fn the_owners_own_wording_outranks_both_vendors() {
		// The reason the glossary exists. `Brake_pedal_information_plausibility`
		// is what a diagnostic engineer wrote and it is unreadable at an open
		// driver's door; the label files' own recovery is often no better. A
		// line in ~/.vagcan/names.csv beats both, and an id it does not mention
		// falls straight through — which is what makes the file worth writing
		// one line at a time.
		let here = tempfile::tempdir().unwrap();
		let mut x = named_cache_with(
			here.path(),
			&[("EV_Test_001", vec![reading(0x2029, "Ladedruck", 0, 16, true)])],
			&[("IDE00022", "Ladedruck-Ist"), ("MAS18568", "Oil temp")],
		);
		x.mine = crate::glossary::parse("text_id,en,ru\nIDE00022,Boost pressure,Давление наддува\n", crate::config::Language::Ru);

		assert_eq!(x.name_of(Some("IDE00022")), Some("Давление наддува"), "the owner's line wins");
		assert_eq!(
			x.name_of(Some("MAS18568")),
			Some("Oil temp"),
			"and an id it says nothing about is untouched"
		);
		assert_eq!(x.name_of(Some("IDE99999")), None);
		assert_eq!(x.name_of(None), None);
	}

	#[test]
	fn an_odis_only_project_does_not_borrow_its_own_pooled_wording_back() {
		// The regression, at the reader end. A project that has only ever seen
		// ODIS runs has `names.json` full of ODIS's *pooled* text, and
		// preferring it over the row's own name replaced the parameter's
		// wording in this ECU variant with the generic one for the id — 340
		// channels reworded on the owner's car, none gained, and two distinct
		// channels collapsed onto a single label.
		let here = tempfile::tempdir().unwrap();
		let project = crate::project::Project {
			id: "SK37X".into(),
			dir: here.path().to_path_buf(),
		};
		std::fs::write(project.names(), r#"{"MAS14374": "Total_CarWakeup_Events_Counter"}"#).unwrap();

		// No source log at all: the cautious answer, and the one that matters
		// here — a project that cannot say where its data came from is not
		// assumed to hold VCDS wording.
		assert!(open(&project).names.is_empty(), "it trusted a file nothing vouched for");

		// A log naming only ODIS runs: same answer, and this is the owner's
		// actual machine.
		crate::project::record_source(
			&project,
			crate::project::SourceEntry {
				kind: vag_db::ODIS,
				path: "/nowhere/SK37X".into(),
				version: None,
				detail: None,
			},
		)
		.unwrap();
		assert!(open(&project).names.is_empty(), "{:?}", open(&project).names);

		// Once an installation has contributed, the file is what it promises
		// and the join through the text id does its job.
		crate::project::record_source(
			&project,
			crate::project::SourceEntry {
				kind: vag_db::VCDS,
				path: "/nowhere/VCDS".into(),
				version: None,
				detail: None,
			},
		)
		.unwrap();
		assert_eq!(
			open(&project).names.get("MAS14374").map(String::as_str),
			Some("Total_CarWakeup_Events_Counter")
		);
	}

	#[test]
	fn a_car_finds_its_variant_by_what_it_reports_about_itself() {
		// `F19E` names the ODX file and `F1A2`'s leading three digits pick the
		// version. Both come off the car, so this stays a lookup the vehicle
		// answers rather than a table about one vehicle.
		let here = tempfile::tempdir().unwrap();
		let x = cache_with(
			here.path(),
			&[
				("EV_TCMDQ200021_001", vec![reading(0x380A, "Getriebe-Eingangsdrehzahl", 0, 16, false)]),
				("EV_ECM18TFS0208V0906264H_001", vec![reading(0x2000, "Motordrehzahl", 0, 16, true)]),
			],
		);

		// Padded exactly as a control unit answers, and passed through untidied.
		let defs = x.for_unit(Some("EV_TCMDQ200021\0\0 "), Some("001010 "));
		assert_eq!(defs.len(), 1, "{defs:#?}");
		assert_eq!(defs[0].name, "Getriebe-Eingangsdrehzahl");
		assert_eq!(defs[0].address, ReadId::Uds(0x380A));
		// The one channel a drive proved is little-endian, and it survived.
		assert_eq!(defs[0].raw_form, RawForm::U16Le);

		// A unit this project does not describe gets nothing, not a guess.
		assert!(x.for_unit(Some("EV_SomethingElse"), Some("001")).is_empty());
		// And a unit that reported no ODX name at all is not matched by luck.
		assert!(x.for_unit(None, Some("001")).is_empty());
	}

	#[test]
	fn an_exact_or_versioned_variant_beats_the_rest_of_its_family() {
		// A `Family` match is the right family with an unconfirmed variant.
		// Beside an answer it is a guess, and reading both would put two
		// descriptions of one identifier in front of somebody.
		let here = tempfile::tempdir().unwrap();
		let x = cache_with(
			here.path(),
			&[
				("EV_TCMDQ200021_001", vec![reading(0x380A, "the versioned one", 0, 16, false)]),
				("EV_TCMDQ200021_099", vec![reading(0x380A, "another of the family", 0, 16, false)]),
			],
		);
		let defs = x.for_unit(Some("EV_TCMDQ200021"), Some("001010"));
		assert_eq!(defs.len(), 1, "{defs:#?}");
		assert_eq!(defs[0].name, "the versioned one");

		// With no version to go on, nothing can rank `Version` — both are
		// family, and both are read rather than one being picked arbitrarily.
		let defs = x.for_unit(Some("EV_TCMDQ200021"), None);
		assert_eq!(defs.len(), 1, "one identifier, described once: {defs:#?}");
		assert_eq!(
			defs[0].name, "the versioned one",
			"the first by name (`_001` before `_099`), so a rerun says the same"
		);
	}

	#[test]
	fn a_proven_row_wins_and_an_extracted_one_only_fills_in() {
		// Design §4.5. The owner has one proven unit and it must not be
		// overwritten by a row that disagrees with what a drive established.
		let extracted = vec![
			MeasurementDef {
				name: "extracted, same identifier".into(),
				unit: "".into(),
				address: ReadId::Uds(0x380A),
				raw_form: RawForm::U16Be,
				scaling: Scaling::Linear(LinearScale { factor: 9.0, offset: 0.0 }),
			},
			MeasurementDef {
				name: "extracted, nothing proven here".into(),
				unit: "".into(),
				address: ReadId::Uds(0x2000),
				raw_form: RawForm::U16Be,
				scaling: Scaling::Linear(LinearScale { factor: 1.0, offset: 0.0 }),
			},
		];
		let merged = merge(vec![proven(0x380A, "proven by driving")], extracted);
		assert_eq!(merged.len(), 2);
		assert_eq!(merged[0].name, "proven by driving", "an extracted row overwrote a proven one");
		assert_eq!(merged[0].scaling, Scaling::Linear(LinearScale { factor: 0.25, offset: 0.0 }));
		assert_eq!(merged[1].name, "extracted, nothing proven here");
	}

	#[test]
	fn a_channel_this_tool_cannot_read_exactly_is_skipped_rather_than_approximated() {
		// What is left after the widening, and why. A field inside one byte is
		// now sayable — bit order established on the car itself, see
		// `RawForm::Bits`. What stays refused is a field that **crosses** a byte
		// boundary, which needs a rule for which end the bits continue from that
		// nothing here has evidence for, and one wider than the `i32` carrier.
		// Approximating either produces a confident wrong number, which is worse
		// than the raw bytes a reader gets instead.
		let here = tempfile::tempdir().unwrap();
		let x = cache_with(
			here.path(),
			&[(
				"EV_Test",
				vec![
					reading(0x1000, "three bits, part way into a byte", 19, 3, true),
					reading(0x1001, "a whole big-endian word", 0, 16, true),
					reading(0x1002, "twelve bits", 0, 12, true),
					reading(0x1003, "a byte at the second position", 8, 8, true),
					reading(0x1004, "a one-bit flag", 24, 1, true),
					reading(0x1005, "eight bytes, wider than an i32", 0, 64, true),
				],
			)],
		);
		let defs = x.for_unit(Some("EV_Test"), None);
		let names: Vec<&str> = defs.iter().map(|d| d.name.as_ref()).collect();
		assert_eq!(
			names,
			[
				"three bits, part way into a byte",
				"a whole big-endian word",
				"a byte at the second position",
				"a one-bit flag"
			],
			"{defs:#?}"
		);
		assert_eq!(
			defs[0].raw_form,
			RawForm::Bits {
				bit_offset: 19,
				bit_length: 3,
				signed: false
			}
		);
		assert_eq!(defs[1].raw_form, RawForm::U16Be);
		assert_eq!(defs[2].raw_form, RawForm::U8Second);
		assert_eq!(
			defs[3].raw_form,
			RawForm::Bits {
				bit_offset: 24,
				bit_length: 1,
				signed: false
			}
		);
	}

	#[test]
	fn the_shapes_the_old_vocabulary_could_not_say_now_come_through() {
		// The measured gap on the reference car, in miniature. Each of these
		// three was found by the project, described exactly, and dropped for
		// want of a form: 146 channels of the first shape on the gearbox alone,
		// 51 of the second, and the rest at a byte offset above 1.
		let here = tempfile::tempdir().unwrap();
		let mut signed_word = reading(0x2000, "signed word, little end first", 0, 16, false);
		signed_word.signed = true;
		let x = cache_with(
			here.path(),
			&[(
				"EV_Test",
				vec![
					signed_word,
					reading(0x2001, "a little-endian counter", 0, 32, false),
					reading(0x2002, "a word four bytes into the answer", 32, 16, true),
				],
			)],
		);
		let defs = x.for_unit(Some("EV_Test"), None);
		assert_eq!(defs.len(), 3, "{defs:#?}");
		assert_eq!(
			defs[0].raw_form,
			RawForm::Int {
				byte_offset: 0,
				byte_length: 2,
				signed: true,
				big_endian: false
			}
		);
		assert_eq!(
			defs[1].raw_form,
			RawForm::Int {
				byte_offset: 0,
				byte_length: 4,
				signed: false,
				big_endian: false
			}
		);
		assert_eq!(
			defs[2].raw_form,
			RawForm::Int {
				byte_offset: 4,
				byte_length: 2,
				signed: false,
				big_endian: true
			}
		);
		// And they decode, rather than merely existing: -208, and 0x1234 read
		// where it actually sits (the factor is 1.0 in this fixture).
		assert_eq!(defs[0].interpret(&[0x30, 0xFF]), Some(-208.0));
		assert_eq!(defs[2].interpret(&[0, 0, 0, 0, 0x12, 0x34]), Some(4660.0));
	}

	#[test]
	fn a_channel_is_named_through_the_text_id_its_own_row_carries() {
		// The complaint this answers: every row on the selection screen was
		// called either after an ODIS long name written for a diagnostic
		// engineer, or — where the project had none — after its own identifier.
		// The row carries a text id, `names.json` is keyed by that id, and the
		// join is a lookup rather than a table of names in this source.
		let here = tempfile::tempdir().unwrap();
		let mut named = reading(0x0283, "Brake_pedal_information_plausibility", 0, 16, true);
		named.text_id = Some("MAS11563".to_string());
		let mut unknown_id = reading(0x0284, "Ambient_pressure", 0, 16, true);
		unknown_id.text_id = Some("MAS04415".to_string());
		let x = named_cache_with(
			here.path(),
			&[("EV_Test", vec![named, unknown_id, reading(0x0285, "no text id at all", 0, 16, true)])],
			&[("MAS11563", "Brake pedal plausibility"), ("MAS99999", "some other channel")],
		);

		let store = vag_data::catalog::CatalogStore::open(here.path().join("nothing"));
		let rows = tagged(&store, &x, None, Some("EV_Test"), None);
		assert_eq!(rows.len(), 3, "{rows:#?}");
		assert_eq!(rows[0].named.as_deref(), Some("Brake pedal plausibility"));
		// An id `names.json` does not know leaves the row with what its own
		// source called it — never with a name from somewhere else.
		assert_eq!(rows[1].named, None);
		assert_eq!(rows[2].named, None);
	}

	#[test]
	fn a_proven_row_keeps_the_name_of_whoever_proved_it() {
		// A `measurements/` row was named by the person who established it on a
		// car, and that is the name they will look for. It carries no text id
		// to look anything else up by either.
		let here = tempfile::tempdir().unwrap();
		let mut row = reading(0x380A, "Getriebe-Eingangsdrehzahl", 0, 16, false);
		row.text_id = Some("IDE00116".to_string());
		let x = named_cache_with(
			here.path(),
			&[("EV_TCMDQ200021_001", vec![row])],
			&[("IDE00116", "Transmission input speed")],
		);
		// A store with the same identifier proven on a car.
		let dir = here.path().join("measured");
		std::fs::create_dir_all(&dir).unwrap();
		let catalog = vag_data::catalog::MeasurementCatalog::new(vec![proven(0x380A, "Input shaft speed")]);
		std::fs::write(dir.join("0CW300041G.json"), serde_json::to_string(&catalog).unwrap()).unwrap();
		let store = vag_data::catalog::CatalogStore::open(&dir);
		let rows = tagged(&store, &x, Some("0CW300041G"), Some("EV_TCMDQ200021"), Some("001"));
		assert_eq!(rows.len(), 1, "{rows:#?}");
		assert!(rows[0].proven);
		assert_eq!(rows[0].def.name, "Input shaft speed");
		assert_eq!(rows[0].named, None, "a proven row is not renamed under the person who proved it");
	}

	#[test]
	fn a_blank_name_in_the_catalog_is_no_name_rather_than_an_empty_row() {
		// `names.json` is merged from more than one source and an empty string
		// is a value it can hold. Preferring one would replace a readable ODIS
		// name with nothing at all.
		let here = tempfile::tempdir().unwrap();
		let mut row = reading(0x0283, "Ambient_pressure", 0, 16, true);
		row.text_id = Some("MAS04415".to_string());
		let x = named_cache_with(here.path(), &[("EV_Test", vec![row])], &[("MAS04415", "   ")]);
		assert_eq!(x.name_of(Some("MAS04415")), None);
		assert_eq!(x.name_of(None), None);
	}

	#[test]
	fn a_project_with_no_extracted_rows_is_the_ordinary_case_and_not_an_error() {
		// The tool worked this way until an ODIS project became a second source,
		// and a VCDS-only project still does.
		let here = tempfile::tempdir().unwrap();
		let x = Extracted {
			cache: here.path().join("nothing.sqlite"),
			variants: Vec::new(),
			names: BTreeMap::new(),
			mine: BTreeMap::new(),
		};
		assert!(x.is_empty());
		assert!(x.for_unit(Some("EV_TCMDQ200021"), Some("001")).is_empty());
		// And the join over it is exactly the proven rows, unchanged.
		let store = vag_data::catalog::CatalogStore::open(here.path());
		assert!(for_unit(&store, &x, Some("0CW300041G"), Some("EV_TCMDQ200021"), Some("001")).is_empty());
	}
}
