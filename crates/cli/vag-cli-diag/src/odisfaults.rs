//! Naming a fault from the ODIS project's own fault table.
//!
//! The rows come out of `cache.sqlite` (`vag_data_db::faults_of`), written by
//! `vagcan setup` from `Project::faults` — one row per code per ECU variant,
//! keyed by the 24-bit number the unit sends. This module is the policy over
//! them: which variants a unit's `F19E`/`F1A2` pick (the same rule the
//! channels use, [`vag_cli_core::extracted::best_variants`]), and which
//! source's text wins when more than one is set up.
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
//! setting unset, **English wins wherever a source has it** — an ODIS project
//! that declares `eng`, else a VCDS installation's English text — and failing
//! that the first ODIS source written; the run says which in one line, since a
//! silent choice would look like the only answer (2026-10-20: an English VCDS
//! set up beside a German project still printed German).
//!
//! The VCDS chain ([`crate::faultnames`]) stays beside this as the fallback:
//! a unit the project has no fault table for, or a preferred language only the
//! VCDS build declares, goes to it.

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
	/// `(kind, dir, language)` of every source in the cache, the language as
	/// [`crate::config::language_code`] gives it.
	sources: Vec<(String, String, Option<String>)>,
	/// The VCDS build the chain reads, when it opened — set by
	/// [`OdisFaults::set_vcds`]. **Its language is the pool's, not the source
	/// rows'**: the cache keeps a row for every VCDS installation ever set up,
	/// and the pool holds one build (`setup` clears it when the build changes),
	/// so a row saying `eng` beside a pool holding `Code-RUS.dat` would print
	/// Russian under a header saying English (found in review, PR #36).
	vcds: Option<VcdsBuild>,
	/// Rows per `(F19E, F1A2)`, read once per unit rather than once per code.
	units: BTreeMap<(String, String), UnitTexts>,
}

/// The VCDS build behind the chain: what the pool's own files say it is, and
/// where the pool is.
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
	/// One line for the console: the display code, the text with its line
	/// breaks flattened — a fault text is one row on a screen — the level, and
	/// where the row came from when that was not settled by the car.
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
	fn provenance(&self) -> Option<String> {
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
	/// Open the current project's fault tables, or `None` when there are no
	/// rows — a project set up from a VCDS installation alone, or none at all.
	pub fn open() -> Option<OdisFaults> {
		let project = crate::project::current().ok()?;
		let cache = project.cache();
		let variants = vag_data_db::fault_variants(&cache).ok()?;
		if variants.is_empty() {
			return None;
		}
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
			vcds: None,
			units: BTreeMap::new(),
		}
	}

	/// Say which VCDS build the chain beside this reads, or `None` when it did
	/// not open (see the field).
	pub fn set_vcds(&mut self, build: Option<VcdsBuild>) {
		self.vcds = build.map(|build| VcdsBuild {
			language: build.language.map(|l| crate::config::language_code(&l)),
			..build
		});
	}

	/// The language of the VCDS text this run can read, if any.
	fn vcds_language(&self) -> Option<&str> {
		self.vcds.as_ref()?.language.as_deref()
	}

	/// Whether an ODIS source declares this language.
	fn odis_declares(&self, language: &str) -> bool {
		self
			.sources
			.iter()
			.any(|(k, _, l)| k == vag_data_db::ODIS && l.as_deref() == Some(language))
	}

	/// Every language this run can show, sorted: the ODIS sources' declared
	/// ones and the language of the VCDS build the pool holds, when it opened.
	fn languages(&self) -> Vec<&str> {
		let mut languages: Vec<&str> = self
			.sources
			.iter()
			.filter(|(kind, _, _)| kind == vag_data_db::ODIS)
			.filter_map(|(_, _, l)| l.as_deref())
			.chain(self.vcds_language())
			.collect();
		languages.sort_unstable();
		languages.dedup();
		languages
	}

	/// The language this run prefers: the setting; unset, English where an ODIS
	/// source declares it or the VCDS build the pool holds is English; else
	/// none, and the first ODIS source's text is shown.
	pub fn preferred(&self) -> Option<&str> {
		if let Some(setting) = &self.setting {
			return Some(setting);
		}
		let english = self.odis_declares(ENGLISH) || self.vcds_language() == Some(ENGLISH);
		english.then_some(ENGLISH)
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
	/// `en`), and only those this run can show count: the ODIS sources' and the
	/// VCDS pool's, when the chain opened ([`OdisFaults::languages`]). Three cases:
	///
	/// - **`[faults] language` names a language nothing here has** — always
	///   said, however many sources there are: which language was asked for,
	///   which ones are available, and whose text is shown instead.
	/// - **Unset, and two or more languages are available** — one line: which
	///   language and whose text, what else is available, and the setting that
	///   changes it.
	/// - Otherwise — the setting was honoured, or there is nothing to choose —
	///   `None`.
	///
	/// A VCDS source row that recorded no language used to get a line saying how
	/// to record it. The VCDS language is the pool's own now, read off its files,
	/// so such a row changes nothing and the line is gone.
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
				format!("{dir} ({})", language.as_deref().unwrap_or("no language declared"))
			});
			return Some(format!(
				"config.toml sets [faults] language = {:?}, which no source here has (available: {available}) — showing {}.",
				crate::config::short_language(setting),
				shown.as_deref().unwrap_or("the first source")
			));
		}
		if languages.len() < 2 {
			return None;
		}
		// Whose text this run reads first: the VCDS build where only it has the
		// preferred language, else the ODIS source that declares it, else the
		// first ODIS one.
		let (language, from) = match self.preferred() {
			Some(preferred) if self.prefers_vcds() => (preferred, self.vcds_installation()?),
			Some(preferred) => {
				let (_, dir, _) = self
					.sources
					.iter()
					.find(|(k, _, l)| k == vag_data_db::ODIS && l.as_deref() == Some(preferred))?;
				(preferred, format!("the ODIS project {dir}"))
			}
			None => {
				let (_, dir, language) = first_odis?;
				(language.as_deref()?, format!("the ODIS project {dir}"))
			}
		};
		Some(format!(
			"Fault text in {} from {from} (available: {available}). Set [faults] language in config.toml to change it.",
			crate::config::short_language(language)
		))
	}

	/// Where the VCDS build the pool holds came from, for the header: the latest
	/// VCDS source row in the pool's language — `setup` writes a row per
	/// installation and the pool keeps only the last build — else the pool.
	fn vcds_installation(&self) -> Option<String> {
		let build = self.vcds.as_ref()?;
		let row = self
			.sources
			.iter()
			.rev()
			.find(|(kind, _, l)| kind == vag_data_db::VCDS && l.is_some() && *l == build.language);
		Some(match row {
			Some((_, dir, _)) => format!("the VCDS installation {}", installation_of(dir).display()),
			None => format!("the VCDS files in {}", build.pool.display()),
		})
	}

	/// Whether the preferred language ([`OdisFaults::preferred`]) is one only
	/// the VCDS build the pool holds has, so the VCDS chain should be asked
	/// first and this project second.
	pub fn prefers_vcds(&self) -> bool {
		let Some(preferred) = self.preferred() else { return false };
		self.vcds_language() == Some(preferred) && !self.odis_declares(preferred)
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
		let variants: Vec<String> = crate::extracted::best_variants(&self.variants, Some(odx_name), Some(version))
			.into_iter()
			.cloned()
			.collect();
		// How the identity picked them. `Exact` is the unit's own file and
		// `Version` is `F1A2` confirming a variant; `Family` is the right
		// family with `F1A2` unanswered or unmatched, which is a guess and is
		// reported as one on every line taken from it.
		let confirmed = variants
			.first()
			.and_then(|name| vag_data_labels::label_files::odx_match(name, odx_name, version))
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

/// The installation a VCDS source row was read from.
///
/// The row records the `Labels/` directory the label files were parsed out of
/// (`setup`'s `label_cache`), and `vagcan setup` wants the installation root
/// above it — which is also where the fault-text file whose name records the
/// language sits. A row that is not a `Labels/` directory is given back as it is.
fn installation_of(dir: &str) -> &std::path::Path {
	let path = std::path::Path::new(dir);
	match path.file_name().and_then(|name| name.to_str()) {
		Some(name) if name.eq_ignore_ascii_case("labels") => path.parent().unwrap_or(path),
		_ => path,
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

	/// A resolver over these source rows. Where there is a VCDS row the chain is
	/// open and the pool holds the build the last one was set up from — what
	/// `setup` leaves behind; [`pool`] says otherwise where a test needs to.
	fn resolver(preferred: Option<&str>, sources: &[(&str, &str, Option<&str>)]) -> OdisFaults {
		let mut r = OdisFaults::new(
			PathBuf::new(),
			Vec::new(),
			preferred.map(str::to_owned),
			sources
				.iter()
				.map(|(k, d, l)| (k.to_string(), d.to_string(), l.map(str::to_owned)))
				.collect(),
		);
		let last = sources.iter().rev().find(|(kind, _, _)| *kind == vag_data_db::VCDS);
		r.set_vcds(last.map(|(_, _, language)| pool(*language)));
		r
	}

	/// The VCDS pool, holding a build in this language.
	fn pool(language: Option<&str>) -> VcdsBuild {
		VcdsBuild {
			language: language.map(str::to_owned),
			pool: PathBuf::from("/home/.vagcan/rod"),
		}
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
		assert!(!unset.prefers_vcds());
		let note = unset.choice_note().expect("two languages and no setting is a choice worth saying");
		assert_eq!(note.lines().count(), 1, "{note}");
		assert!(note.starts_with("Fault text in en from the ODIS project /en"), "{note}");
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
		let r = resolver(None, &[("odis", "/x/SK37X", Some("deu")), ("vcds", "/vcds-ru/Labels", Some("rus"))]);
		assert_eq!(r.preferred(), None);
		assert!(!r.prefers_vcds());
		let note = r.choice_note().unwrap();
		assert!(note.starts_with("Fault text in de from the ODIS project /x/SK37X"), "{note}");
		assert!(note.contains("available: de, ru"), "{note}");
	}

	#[test]
	fn unset_a_german_project_beside_an_english_vcds_reads_english_from_vcds() {
		// The owner's cache (2026-10-20): an old VCDS row with no language, the
		// German ODIS project, and the English VCDS build set up after it, which
		// the pool holds. With nothing set the listing was German; English is
		// there, so it is read first, from VCDS, and the project stays the
		// fallback where VCDS has nothing for a code.
		let sources = [
			("vcds", "vendor/vcds-en/Labels", None),
			("odis", "/x/SK37X", Some("deu")),
			("vcds", "/repo/vendor/vcds-en/Labels", Some("eng")),
		];
		let mut r = resolver(None, &sources);
		assert_eq!(r.preferred(), Some("eng"));
		assert!(r.prefers_vcds(), "only VCDS has English, so VCDS is asked first");
		let note = r.choice_note().unwrap();
		assert_eq!(
			note,
			"Fault text in en from the VCDS installation /repo/vendor/vcds-en (available: de, en). \
			 Set [faults] language in config.toml to change it."
		);
		// The project's own German, set by hand, puts the project first again.
		let mut german = resolver(Some("de"), &sources);
		assert!(!german.prefers_vcds());
		assert_eq!(german.choice_note(), None);
		german.set_vcds(None);
		assert!(!german.prefers_vcds());
		// A VCDS chain that did not open names nothing: no English default, and
		// English is not available — one language left, so nothing to say.
		r.set_vcds(None);
		assert_eq!(r.preferred(), None);
		assert!(!r.prefers_vcds());
		assert_eq!(r.choice_note(), None);
		// Asked for anyway, English is not offered as available.
		let mut english = resolver(Some("en"), &sources);
		english.set_vcds(None);
		assert!(!english.prefers_vcds());
		let note = english.choice_note().expect("a setting nothing here can show is said");
		assert!(note.contains("(available: de)"), "{note}");
	}

	#[test]
	fn the_vcds_language_is_the_build_the_pool_holds_not_any_source_row() {
		// Found in review (PR #36): set up vcds-en, then vcds-ru, beside a German
		// project. The cache keeps both VCDS rows; the pool holds only the
		// Russian build (`setup` clears it when the build changes). Choosing
		// English because a row said `eng` printed Russian text under a header
		// saying "Fault text in en from …/vcds-en".
		let sources = [
			("vcds", "/v/vcds-en/Labels", Some("eng")),
			("odis", "/x/SK37X", Some("deu")),
			("vcds", "/v/vcds-ru/Labels", Some("rus")),
		];
		let mut r = resolver(None, &sources);
		r.set_vcds(Some(pool(Some("rus"))));
		assert_eq!(r.preferred(), None, "no English to be had: the project's own");
		assert!(!r.prefers_vcds());
		let note = r.choice_note().unwrap();
		assert!(note.starts_with("Fault text in de from the ODIS project /x/SK37X"), "{note}");
		assert!(note.contains("(available: de, ru)"), "English is not available: {note}");

		let mut english = resolver(Some("en"), &sources);
		english.set_vcds(Some(pool(Some("rus"))));
		assert!(!english.prefers_vcds(), "Russian VCDS text must not be asked first for English");
		assert!(english.choice_note().unwrap().contains("(available: de, ru)"));

		let mut russian = resolver(Some("ru"), &sources);
		russian.set_vcds(Some(pool(Some("rus"))));
		assert!(russian.prefers_vcds());
		assert_eq!(russian.choice_note(), None);

		// Unset, with the pool back on the English build: English, and the header
		// names the installation that build came from, not the Russian one.
		let mut back = resolver(None, &sources);
		back.set_vcds(Some(pool(Some("eng"))));
		assert!(back.prefers_vcds());
		assert!(
			back
				.choice_note()
				.unwrap()
				.starts_with("Fault text in en from the VCDS installation /v/vcds-en "),
			"{:?}",
			back.choice_note()
		);
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
		assert!(!one.prefers_vcds());
		// Honoured, it says nothing.
		assert_eq!(resolver(Some("deu"), &[("odis", "/x/SK37X", Some("deu"))]).choice_note(), None);
	}

	#[test]
	fn a_vcds_row_that_recorded_no_language_counts_for_nothing_beside_the_pool() {
		// A cache built before `language` existed has a VCDS row that is `NULL`.
		// The language is read off the pool's files now, so the row neither
		// blocks English nor needs a line about recording it.
		let sources = [("vcds", "/Applications/VCDS/Labels", None), ("odis", "/x/SK37X", Some("deu"))];
		let mut r = resolver(None, &sources);
		r.set_vcds(Some(pool(Some("eng"))));
		assert!(r.prefers_vcds());
		assert_eq!(
			r.choice_note().unwrap(),
			"Fault text in en from the VCDS files in /home/.vagcan/rod (available: de, en). \
			 Set [faults] language in config.toml to change it.",
			"no row names the installation, so the pool is named"
		);
		let mut french = resolver(Some("fr"), &sources);
		french.set_vcds(Some(pool(Some("eng"))));
		let note = french.choice_note().unwrap();
		assert_eq!(note.lines().count(), 1, "no hint about the unrecorded row: {note}");
		assert!(note.contains("(available: de, en)"), "{note}");
	}

	#[test]
	fn one_source_needs_no_setting_and_makes_no_note() {
		let one = resolver(None, &[("odis", "/x", Some("deu"))]);
		assert_eq!(one.choice_note(), None);
		assert!(!one.prefers_vcds());
	}

	#[test]
	fn a_language_only_the_vcds_build_declares_sends_the_vcds_chain_first() {
		let sources = [("vcds", "/vcds/Labels", Some("eng")), ("odis", "/x", Some("deu"))];
		assert!(resolver(Some("eng"), &sources).prefers_vcds());
		assert!(resolver(Some("en"), &sources).prefers_vcds());
		assert!(!resolver(Some("deu"), &sources).prefers_vcds());
		// Unset, English is the default, and only VCDS has it.
		assert!(resolver(None, &sources).prefers_vcds());
		// Both declaring the language: the project is asked first, as always.
		let both = [("vcds", "/vcds/Labels", Some("deu")), ("odis", "/x", Some("deu"))];
		assert!(!resolver(Some("deu"), &both).prefers_vcds());
	}
}
