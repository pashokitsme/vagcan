//! Naming a fault from the ODIS project's own fault table.
//!
//! The rows come out of `cache.sqlite` (`vag_data_db::faults_of`), written by
//! `vagcan setup` from `Project::faults` — one row per code per ECU variant,
//! keyed by the 24-bit number the unit sends. This module is the policy over
//! them: which variants a unit's `F19E`/`F1A2` pick (the same rule the
//! channels use, [`vag_cli_core::extracted::best_variants`]), and which
//! ODIS source's text wins when the project holds more than one.
//!
//! ## Language is a property of the source
//! The object model has no language field: a code carries one text, in
//! whichever language its supplier wrote it, and a project declares one
//! language for itself (`research/odis-dtc/README.md` §4). So a text's
//! language is its *source's* declared language, a second project in another
//! language is a second source, and `[faults] language` in `config.toml`
//! chooses between sources — `en`, `de`, `ru` or the three-letter code a
//! source declares ([`crate::config::language_code`]). With one language there
//! is nothing to choose and the setting is never needed. With several and the
//! setting unset, **English wins wherever an ODIS source declares it**, and failing
//! that the first ODIS source written; the run says which in one line, since a
//! silent choice would look like the only answer.
//!
//! **ODIS alone.** This is an ODIS run's whole fault naming: a unit the project has no
//! fault table for, or a code a table lacks, is shown as its number. The VCDS chain
//! ([`crate::faultnames`]) is a VCDS run's, and never stands in (CLAUDE.md, "ODIS and VCDS
//! are independent sources").

use std::collections::BTreeMap;
use std::path::PathBuf;

use vag_data_db::CachedFault;

/// English, as sources declare it (ISO 639-2) — the default when a source has it.
const ENGLISH: &str = "eng";

/// The project's fault tables, open for naming.
pub struct OdisFaults {
	cache: PathBuf,
	/// Every variant the cache holds fault rows for.
	variants: Vec<String>,
	/// `[faults] language`, as [`crate::config::language_code`] gives it, if set.
	setting: Option<String>,
	/// `(kind, dir, language)` of every source in the project's cache, the language as
	/// [`crate::config::language_code`] gives it.
	sources: Vec<(String, String, Option<String>)>,
	/// Rows per `(F19E, F1A2)`, read once per unit rather than once per code.
	units: BTreeMap<(String, String), UnitTexts>,
}

/// The VCDS build the pool holds: what its own files say it is, and where the pool is — what
/// a VCDS run's `[faults] language` is checked against. **Its language is the pool's, not
/// any source row's**: the pool holds one build (`setup` clears it when the build changes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VcdsBuild {
	/// ISO 639-2, from [`crate::registry::build_of`] and
	/// [`crate::registry::codes_language`]; `None` for a build neither names.
	pub language: Option<String>,
	/// The pool the chain reads, said when no source row names the installation.
	pub pool: PathBuf,
}

impl VcdsBuild {
	/// The build the pool at `pool` holds, as its files say.
	pub fn of(pool: &std::path::Path) -> VcdsBuild {
		VcdsBuild {
			language: crate::registry::build_of(pool)
				.and_then(crate::registry::codes_language)
				.map(str::to_string),
			pool: pool.to_path_buf(),
		}
	}
}

/// What the project knows for one unit: the variants its identity picked,
/// and every fault row of theirs.
#[derive(Debug, Clone, Default)]
pub struct UnitTexts {
	pub variants: Vec<String>,
	/// The unit's `F19E`, so a variant can be printed as the suffix that
	/// distinguishes it inside the family rather than in full.
	pub family: String,
	/// Whether the car's own identifiers picked these variants or merely their
	/// family — `Exact`/`Version` against `Family`
	/// ([`vag_data_labels::label_files::OdxMatch`]). A family match is a guess
	/// and every line taken from one says so.
	pub confirmed: bool,
	rows: Vec<(String, CachedFault)>,
}

impl UnitTexts {
	/// Whether the project has a fault table for this unit at all.
	pub fn is_empty(&self) -> bool {
		self.rows.is_empty()
	}
}

/// One code, named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Naming {
	/// The code a tester prints, `B1168F2`.
	pub display_code: Option<String>,
	/// The text, as the file has it.
	pub text: Option<String>,
	/// The language the row's source declared.
	pub language: Option<String>,
	/// Which variant's table the row is from.
	pub variant: String,
	/// The `F19E` those variants belong to, so [`Naming::line`] can print the
	/// variant as the suffix that tells it from its siblings.
	pub family: String,
	/// How many variants the unit's identity matched — the size of the set
	/// this row was taken out of.
	pub matched: usize,
	/// Whether that match was the car's own answer rather than the family
	/// ([`UnitTexts::confirmed`]).
	pub confirmed: bool,
	/// Whether the other matching variants that list this number say something
	/// else about it, in the same language.
	pub disagrees: bool,
	/// The object's `LEVEL`, recorded and not interpreted.
	pub level: u32,
}

impl Naming {
	/// One line: the display code, the text with its line breaks flattened, the level,
	/// and where the row came from when that was not settled by the car — what this
	/// module's tests read a row by. `faults` takes the fields apart for its table.
	#[cfg(test)]
	pub fn line(&self) -> String {
		let text = self.text.as_deref().map(|t| t.replace('\n', " / "));
		let mut line = match (self.display_code.as_deref(), text) {
			(Some(code), Some(text)) => format!("{code}  {text}"),
			(Some(code), None) => format!("{code}  (no text in the project)"),
			(None, Some(text)) => text,
			(None, None) => "(no text in the project)".to_string(),
		};
		// `LEVEL` in the file's own word. It matched VCDS's fault priority on
		// both codes it was checked against (`research/odis-dtc/README.md`
		// §2), which is evidence for a decoder and not one — so the number is
		// shown and the word stays the file's.
		if self.level > 0 {
			line.push_str(&format!("  level {}", self.level));
		}
		if let Some(note) = self.provenance() {
			line.push_str(&format!("  ({note})"));
		}
		line
	}

	/// Which variant this row came from, when the car's identifiers did not
	/// settle that on their own, and whether the others agreed with it.
	///
	/// Silent when `F19E`/`F1A2` picked one variant: then there was no choice
	/// to report, and naming the file under every code would be noise. A
	/// family match, or several matching variants, is a choice — the row is
	/// still shown, because it is the project's answer and the alternative is
	/// no name at all, but the line never presents it as settled.
	pub fn provenance(&self) -> Option<String> {
		if self.confirmed && self.matched < 2 {
			return None;
		}
		let mut parts = vec![match self.matched {
			0 | 1 => format!("variant {}, the only one matching", self.short_variant()),
			n => format!("variant {} of {n} matching", self.short_variant()),
		}];
		if self.disagrees {
			parts.push(match self.confirmed {
				true => "they disagree on this number".to_string(),
				false => "they disagree — record F1A2 to settle it".to_string(),
			});
		}
		Some(parts.join("; "))
	}

	/// The part of the variant's name that distinguishes it inside its family:
	/// `EV_Brake1UDSContiMK100ESP_032` under `EV_Brake1UDSContiMK100ESP` is
	/// `_032`. The unit's own line has already printed the family.
	fn short_variant(&self) -> &str {
		let family = self.family.trim_end_matches(['\0', ' ']);
		if family.is_empty() {
			return &self.variant;
		}
		match self.variant.get(..family.len()) {
			Some(head) if head.eq_ignore_ascii_case(family) && family.len() < self.variant.len() => &self.variant[family.len()..],
			_ => &self.variant,
		}
	}
}

impl OdisFaults {
	/// The languages the current project's ODIS sources declare, ISO 639-2 — what a VCDS run
	/// offers `--source odis` for when its own build lacks the one asked. Its source rows only:
	/// no fault row is read.
	pub fn project_languages() -> Vec<String> {
		let Ok(project) = crate::project::current() else { return Vec::new() };
		vag_data_db::source_languages(&project.cache())
			.unwrap_or_default()
			.into_iter()
			.filter(|(kind, _, _)| kind == vag_data_db::ODIS)
			.filter_map(|(_, _, l)| l.map(|l| crate::config::language_code(&l)))
			.collect()
	}

	/// Open the current project's fault tables, or `None` when there are no
	/// rows — a project set up from a VCDS installation alone, or none at all.
	pub fn open() -> Option<OdisFaults> {
		let project = crate::project::current().ok()?;
		let cache = project.cache();
		let variants = vag_data_db::fault_variants(&cache).ok()?;
		if variants.is_empty() {
			return None;
		}
		// The project's own sources: its languages are the ones an ODIS run can show.
		let sources = vag_data_db::source_languages(&cache).unwrap_or_default();
		Some(OdisFaults::new(
			cache,
			variants,
			crate::config::fault_language(&crate::config::load()),
			sources,
		))
	}

	fn new(cache: PathBuf, variants: Vec<String>, setting: Option<String>, sources: Vec<(String, String, Option<String>)>) -> OdisFaults {
		let sources = sources
			.into_iter()
			.map(|(kind, dir, language)| (kind, dir, language.map(|l| crate::config::language_code(&l))))
			.collect();
		OdisFaults {
			cache,
			variants,
			setting: setting.map(|l| crate::config::language_code(&l)),
			sources,
			units: BTreeMap::new(),
		}
	}

	/// Whether an ODIS source declares this language.
	fn odis_declares(&self, language: &str) -> bool {
		self
			.sources
			.iter()
			.any(|(k, _, l)| k == vag_data_db::ODIS && l.as_deref() == Some(language))
	}

	/// Every language this run can show, sorted: the ODIS sources' declared ones.
	pub fn languages(&self) -> Vec<&str> {
		let mut languages: Vec<&str> = self
			.sources
			.iter()
			.filter(|(kind, _, _)| kind == vag_data_db::ODIS)
			.filter_map(|(_, _, l)| l.as_deref())
			.collect();
		languages.sort_unstable();
		languages.dedup();
		languages
	}

	/// The language this run prefers: the setting; unset, English where an ODIS
	/// source declares it; else none, and the first ODIS source's text is shown.
	pub fn preferred(&self) -> Option<&str> {
		if let Some(setting) = &self.setting {
			return Some(setting);
		}
		self.odis_declares(ENGLISH).then_some(ENGLISH)
	}

	/// The project's fault sources in a few words for the `names` line: `ODIS SK37X (deu)`.
	/// A cache with fault rows and no source row names the project it is in instead.
	pub fn short(&self) -> String {
		let odis: Vec<String> = self
			.sources
			.iter()
			.filter(|(kind, _, _)| kind == vag_data_db::ODIS)
			.map(|(_, dir, language)| {
				let name = std::path::Path::new(dir)
					.file_name()
					.map(|n| n.to_string_lossy().into_owned())
					.unwrap_or_else(|| dir.clone());
				match language {
					Some(language) => format!("{name} ({language})"),
					None => name,
				}
			})
			.collect();
		if odis.is_empty() {
			let project = self.cache.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned());
			return format!("ODIS {}", project.unwrap_or_else(|| "project".to_string()));
		}
		format!("ODIS {}", odis.join(", "))
	}

	/// One line saying where the names come from, for the top of a listing.
	pub fn describe(&self) -> String {
		let (variants, codes) = vag_data_db::fault_counts(&self.cache).unwrap_or_default();
		let odis: Vec<String> = self
			.sources
			.iter()
			.filter(|(kind, _, _)| kind == vag_data_db::ODIS)
			.map(|(_, dir, language)| match language {
				Some(language) => format!("{dir} ({})", crate::config::short_language(language)),
				None => dir.clone(),
			})
			.collect();
		format!(
			"{codes} fault {} for {variants} {} from {}",
			crate::render::plural(codes as usize, "text"),
			crate::render::plural(variants as usize, "variant"),
			odis.join(", ")
		)
	}

	/// What to say about the choice of language, if a choice is being made.
	///
	/// Once per run, above the codes, so that a text in an unexpected language is
	/// never a surprise. Languages are said the way a person types them (`de`,
	/// `en`), and only those this run can show count: the ODIS sources'
	/// ([`OdisFaults::languages`]). Three cases:
	///
	/// - **`[faults] language` names a language nothing here has** — always
	///   said, however many sources there are: which language was asked for,
	///   which ones are available, and whose text is shown instead.
	/// - **Unset, and two or more languages are available** — one line: which
	///   language and whose text, what else is available, and the setting that
	///   changes it.
	/// - Otherwise — the setting was honoured, or there is nothing to choose —
	///   `None`.
	pub fn choice_note(&self) -> Option<String> {
		let languages = self.languages();
		// Sorted as they are printed: `es` is `spa` and `sv` is `swe`, so the
		// three-letter order is not the two-letter one.
		let mut short: Vec<String> = languages.iter().map(|l| crate::config::short_language(l)).collect();
		short.sort_unstable();
		let available = match short.is_empty() {
			true => "none".to_string(),
			false => short.join(", "),
		};
		let first_odis = self.sources.iter().find(|(kind, _, _)| kind == vag_data_db::ODIS);
		if let Some(setting) = &self.setting {
			if languages.contains(&setting.as_str()) {
				return None;
			}
			// The source whose text [`OdisFaults::name`] falls back to: the first
			// ODIS source written, since rows come back in source order.
			let shown = first_odis.map(|(_, dir, language)| {
				let language = language.as_deref().map(crate::config::short_language);
				format!(
					"{} ({})",
					vag_cli_core::datadir::tilde(std::path::Path::new(dir)),
					language.as_deref().unwrap_or("no language declared")
				)
			});
			return Some(format!(
				"note: `[faults] language = {:?}` in config.toml is in no source here (available: {available}); showing {}",
				crate::config::short_language(setting),
				shown.as_deref().unwrap_or("the first source")
			));
		}
		if languages.len() < 2 {
			return None;
		}
		// Whose text this run reads: the ODIS source that declares the preferred language,
		// else the first ODIS one.
		let (language, from) = match self.preferred() {
			Some(preferred) => {
				let (_, dir, _) = self
					.sources
					.iter()
					.find(|(k, _, l)| k == vag_data_db::ODIS && l.as_deref() == Some(preferred))?;
				(
					preferred,
					format!("the ODIS project {}", vag_cli_core::datadir::tilde(std::path::Path::new(dir))),
				)
			}
			None => {
				let (_, dir, language) = first_odis?;
				(
					language.as_deref()?,
					format!("the ODIS project {}", vag_cli_core::datadir::tilde(std::path::Path::new(dir))),
				)
			}
		};
		Some(format!(
			"note: fault text in {} from {from} (available: {available}); `[faults] language` in config.toml picks another",
			crate::config::short_language(language)
		))
	}

	/// The rows for a unit that named itself, read once and kept.
	///
	/// Cloned for the same reason [`crate::faultnames::Namer::unit`] clones: a
	/// caller holding a borrow of this could not then name a code through
	/// `&self`. A unit's rows are a few hundred small strings.
	pub fn unit(&mut self, odx_name: &str, version: &str) -> UnitTexts {
		let key = (odx_name.to_string(), version.to_string());
		if let Some(found) = self.units.get(&key) {
			return found.clone();
		}
		let variants: Vec<String> = crate::extracted::best_variants(&self.variants, Some(odx_name), Some(version), &[])
			.into_iter()
			.cloned()
			.collect();
		// How the identity picked them. `Exact` is the unit's own file and
		// `Version` is `F1A2` confirming a variant; `Family` is the right
		// family with `F1A2` unanswered or unmatched, which is a guess and is
		// reported as one on every line taken from it.
		let confirmed = variants
			.first()
			.and_then(|name| vag_data_labels::label_files::odx_match(name, odx_name, version, &[]))
			.is_some_and(|rank| rank != vag_data_labels::label_files::OdxMatch::Family);
		let mut rows = Vec::new();
		for variant in &variants {
			for row in vag_data_db::faults_of(&self.cache, variant).unwrap_or_default() {
				rows.push((variant.clone(), row));
			}
		}
		let texts = UnitTexts {
			variants,
			family: odx_name.trim_end_matches(['\0', ' ']).to_string(),
			confirmed,
			rows,
		};
		self.units.insert(key, texts.clone());
		texts
	}

	/// Name one code from a unit's rows, or `None` when no table lists it.
	///
	/// Among the rows for the number, one whose source declares the preferred
	/// language wins; failing that, the first written — the first source, in
	/// its first-named variant — which is what makes two runs agree.
	///
	/// **That first row is a choice, not an answer**, whenever the unit's
	/// identity matched a family rather than a variant: the reference car's
	/// unit 03 answers no `F1A2`, seven variants match, and they do not all
	/// write 297 the same way. The row carries where it came from and whether
	/// the others agreed, so [`Naming::line`] can say so rather than letting
	/// the alphabetically first look like the car's own answer.
	pub fn name(&self, unit: &UnitTexts, code: [u8; 3]) -> Option<Naming> {
		let number = u32::from_be_bytes([0, code[0], code[1], code[2]]);
		let candidates: Vec<&(String, CachedFault)> = unit.rows.iter().filter(|(_, row)| row.fault.code == number).collect();
		let chosen = match self.preferred() {
			Some(preferred) => candidates
				.iter()
				.find(|(_, row)| row.language.as_deref().map(crate::config::language_code).as_deref() == Some(preferred))
				.or_else(|| candidates.first()),
			None => candidates.first(),
		}?;
		let (variant, row) = chosen;
		// Only rows in the chosen row's language are compared: a second source
		// in another language differs by design, and calling that a
		// disagreement would flag every code on a two-language machine.
		let disagrees = candidates.iter().any(|(other, r)| {
			other != variant && r.language == row.language && (r.fault.display_code != row.fault.display_code || r.fault.text != row.fault.text)
		});
		Some(Naming {
			display_code: row.fault.display_code.clone(),
			text: row.fault.text.clone(),
			language: row.language.clone(),
			variant: variant.clone(),
			family: unit.family.clone(),
			matched: unit.variants.len(),
			confirmed: unit.confirmed,
			disagrees,
			level: row.fault.level,
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn row(code: u32, text: &str, language: Option<&str>, dir: &str) -> CachedFault {
		display_row(code, text, language, dir, "B1168F2")
	}

	fn display_row(code: u32, text: &str, language: Option<&str>, dir: &str, display: &str) -> CachedFault {
		CachedFault {
			fault: vag_data_labels::odis::Fault {
				dop: "DTCDOP_VAGUDS".into(),
				code,
				display_code: Some(display.into()),
				text: Some(text.into()),
				text_id: None,
				short_name: None,
				level: 2,
				temporary: false,
			},
			language: language.map(str::to_owned),
			source_dir: dir.into(),
		}
	}

	#[test]
	fn the_names_line_says_the_project_when_no_source_row_names_one() {
		let r = resolver(None, &[(vag_data_db::ODIS, "/x/Projects/SK37X", Some("deu"))]);
		assert_eq!(r.short(), "ODIS SK37X (deu)");
		// Fault rows and no source row: `ODIS ` with nothing after it, before (review of
		// PR #38). The project the cache is in, instead.
		let bare = OdisFaults::new(PathBuf::from("/h/.vagcan/data/SK37X/cache.sqlite"), Vec::new(), None, Vec::new());
		assert_eq!(bare.short(), "ODIS SK37X");
	}

	/// A resolver over these source rows.
	fn resolver(preferred: Option<&str>, sources: &[(&str, &str, Option<&str>)]) -> OdisFaults {
		OdisFaults::new(
			PathBuf::new(),
			Vec::new(),
			preferred.map(str::to_owned),
			sources
				.iter()
				.map(|(k, d, l)| (k.to_string(), d.to_string(), l.map(str::to_owned)))
				.collect(),
		)
	}

	#[test]
	fn a_code_is_named_by_its_number_and_prints_as_vcds_would() {
		// The reference car's brake unit: `00 01 29` is 297, and the project's
		// display code for it is the `B1168 F2` VCDS printed.
		let unit = UnitTexts {
			family: "EV_Brake".into(),
			confirmed: true,
			variants: vec!["EV_Brake_035".into()],
			rows: vec![("EV_Brake_035".into(), row(297, "Swa_lost_initialisation", Some("deu"), "/x"))],
		};
		let r = resolver(None, &[("odis", "/x", Some("deu"))]);
		let named = r.name(&unit, [0x00, 0x01, 0x29]).expect("297 is in the table");
		// The unit's own `F1A2` picked that one variant, so the line is the
		// name and the level and nothing about where it came from.
		assert_eq!(named.line(), "B1168F2  Swa_lost_initialisation  level 2");
		assert_eq!(named.variant, "EV_Brake_035");
		assert_eq!(named.language.as_deref(), Some("deu"));
		assert!(r.name(&unit, [0x00, 0x01, 0x2A]).is_none(), "a number no table lists is not named");
	}

	#[test]
	fn a_text_with_line_breaks_is_one_line_on_the_console() {
		let naming = Naming {
			display_code: Some("P150B00".into()),
			text: Some("Acceleration monitoring\nControl limit exceeded".into()),
			language: None,
			variant: "EV_ECM_001".into(),
			family: "EV_ECM".into(),
			matched: 1,
			confirmed: true,
			disagrees: false,
			level: 2,
		};
		assert_eq!(naming.line(), "P150B00  Acceleration monitoring / Control limit exceeded  level 2");
	}

	#[test]
	fn a_family_match_names_the_variant_it_took_and_says_when_they_disagree() {
		// Unit 03 of the reference car answers no `F1A2`, so seven variants of
		// `EV_Brake1UDSContiMK100ESP` match at family rank — and they do not
		// agree about 297: `_032` writes it `B116816`, `_035`–`_038` write
		// `B1168F2`. Taking the alphabetically first and printing it as the
		// answer was the bug; the row is still shown, and the line says it was
		// picked out of seven and that they differ.
		let variants: Vec<String> = ["_032", "_035", "_036", "_037", "_038", "_039", "_040"]
			.iter()
			.map(|suffix| format!("EV_Brake{suffix}"))
			.collect();
		let unit = UnitTexts {
			family: "EV_Brake".into(),
			confirmed: false,
			variants,
			rows: vec![
				(
					"EV_Brake_032".into(),
					display_row(297, "Swa_lost_initialisation", Some("deu"), "/x", "B116816"),
				),
				(
					"EV_Brake_035".into(),
					display_row(297, "Swa_lost_initialisation", Some("deu"), "/x", "B1168F2"),
				),
			],
		};
		let named = resolver(None, &[("odis", "/x", Some("deu"))])
			.name(&unit, [0x00, 0x01, 0x29])
			.expect("297 is in the table");
		assert_eq!(named.variant, "EV_Brake_032");
		assert_eq!(
			named.line(),
			"B116816  Swa_lost_initialisation  level 2  (variant _032 of 7 matching; they disagree — record F1A2 to settle it)"
		);
	}

	#[test]
	fn variants_that_agree_are_not_reported_as_a_disagreement() {
		// Two localisations of one variant say the same thing; only that the
		// choice was made at family rank is worth a word.
		let unit = UnitTexts {
			family: "EV_Brake".into(),
			confirmed: false,
			variants: vec!["EV_Brake_035".into(), "EV_Brake_035_SK37".into()],
			rows: vec![
				("EV_Brake_035".into(), row(297, "Swa_lost_initialisation", Some("deu"), "/x")),
				("EV_Brake_035_SK37".into(), row(297, "Swa_lost_initialisation", Some("deu"), "/x")),
			],
		};
		let named = resolver(None, &[("odis", "/x", Some("deu"))]).name(&unit, [0, 1, 0x29]).unwrap();
		assert_eq!(named.line(), "B1168F2  Swa_lost_initialisation  level 2  (variant _035 of 2 matching)");
	}

	/// A unit two ODIS projects name, one German and one English.
	fn two_project_unit() -> UnitTexts {
		UnitTexts {
			family: "EV_X".into(),
			confirmed: true,
			variants: vec!["EV_X".into()],
			rows: vec![
				("EV_X".into(), row(297, "Lenkwinkelsensor", Some("deu"), "/de")),
				("EV_X".into(), row(297, "Steering angle sensor", Some("eng"), "/en")),
			],
		}
	}

	#[test]
	fn unset_an_english_odis_project_is_read_first_and_the_line_says_so() {
		// Unset, with the projects declaring German and English: English, from the
		// project that has it, and one line saying which, what else there is, and
		// the setting that changes it.
		let unit = two_project_unit();
		let unset = resolver(None, &[("odis", "/de", Some("deu")), ("odis", "/en", Some("eng"))]);
		assert_eq!(unset.preferred(), Some("eng"));
		assert_eq!(unset.name(&unit, [0, 1, 0x29]).unwrap().text.as_deref(), Some("Steering angle sensor"));
		let note = unset.choice_note().expect("two languages and no setting is a choice worth saying");
		assert_eq!(note.lines().count(), 1, "{note}");
		assert!(note.starts_with("note: fault text in en from the ODIS project /en"), "{note}");
		assert!(note.contains("available: de, en"), "{note}");
		assert!(note.contains("[faults] language"), "{note}");
	}

	#[test]
	fn the_setting_overrides_the_english_default_in_either_spelling() {
		let unit = two_project_unit();
		let sources = [("odis", "/de", Some("deu")), ("odis", "/en", Some("eng"))];
		for german in ["deu", "de", "DE", "ger"] {
			let r = resolver(Some(german), &sources);
			assert_eq!(r.name(&unit, [0, 1, 0x29]).unwrap().text.as_deref(), Some("Lenkwinkelsensor"), "{german}");
			assert_eq!(r.choice_note(), None, "a setting that was honoured needs no note: {german}");
		}
		let english = resolver(Some("en"), &sources);
		assert_eq!(english.name(&unit, [0, 1, 0x29]).unwrap().text.as_deref(), Some("Steering angle sensor"));
	}

	#[test]
	fn a_setting_nobody_declares_is_said_with_what_is_available() {
		let unit = two_project_unit();
		let nobody = resolver(Some("fr"), &[("odis", "/de", Some("deu")), ("odis", "/en", Some("eng"))]);
		assert_eq!(nobody.name(&unit, [0, 1, 0x29]).unwrap().text.as_deref(), Some("Lenkwinkelsensor"));
		let note = nobody.choice_note().unwrap();
		assert!(note.contains("language = \"fr\""), "{note}");
		assert!(note.contains("(available: de, en)"), "{note}");
		assert!(note.contains("showing /de (de)"), "{note}");
	}

	#[test]
	fn unset_with_no_english_anywhere_the_project_reads_in_its_own_language() {
		let r = resolver(None, &[("odis", "/x/SK37X", Some("deu")), ("odis", "/x/SK37X-ru", Some("rus"))]);
		assert_eq!(r.preferred(), None);
		let note = r.choice_note().unwrap();
		assert!(note.starts_with("note: fault text in de from the ODIS project /x/SK37X"), "{note}");
		assert!(note.contains("available: de, ru"), "{note}");
	}

	#[test]
	fn a_vcds_source_beside_the_project_is_no_language_of_an_odis_run() {
		// The owner's cache before one source per run (2026-10-20): a German ODIS project
		// and an English VCDS build. English then came from VCDS; now an ODIS run reads the
		// project alone, in its own German, and VCDS's English is not offered.
		let sources = [
			("vcds", "vendor/vcds-en/Labels", None),
			("odis", "/x/SK37X", Some("deu")),
			("vcds", "/repo/vendor/vcds-en/Labels", Some("eng")),
		];
		let r = resolver(None, &sources);
		assert_eq!(r.preferred(), None, "no ODIS source declares English");
		assert_eq!(r.choice_note(), None, "one language: nothing to choose");
		let english = resolver(Some("en"), &sources);
		let note = english.choice_note().expect("a setting the project cannot show is said");
		assert!(note.contains("(available: de)"), "{note}");
	}

	#[test]
	fn a_language_nothing_here_declares_is_said_even_with_one_source() {
		// The defect: the note sat behind "fewer than two languages recorded →
		// nothing to choose", so an ODIS-only project with `language = "rus"`
		// printed German and never said the setting had been ignored.
		let one = resolver(Some("rus"), &[("odis", "/x/SK37X", Some("deu"))]);
		let note = one.choice_note().expect("a setting that cannot be honoured is always said");
		assert_eq!(note.lines().count(), 1, "{note}");
		assert!(note.contains("\"ru\""), "which setting, as a person types it: {note}");
		assert!(note.contains("available: de"), "what is available: {note}");
		assert!(note.contains("showing /x/SK37X (de)"), "what is shown instead: {note}");
		// Honoured, it says nothing.
		assert_eq!(resolver(Some("deu"), &[("odis", "/x/SK37X", Some("deu"))]).choice_note(), None);
	}

	#[test]
	fn one_source_needs_no_setting_and_makes_no_note() {
		let one = resolver(None, &[("odis", "/x", Some("deu"))]);
		assert_eq!(one.choice_note(), None);
	}
}
