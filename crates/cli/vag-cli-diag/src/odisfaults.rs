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
//! chooses between sources. With one source there is nothing to choose and
//! the setting is never needed. With several and the setting unset, the first
//! ODIS source written wins and the run says which — a silent choice would
//! look like the only answer.
//!
//! The VCDS chain ([`crate::faultnames`]) stays beside this as the fallback:
//! a unit the project has no fault table for, or a source language the
//! setting names that only the VCDS build declares, goes to it.

use std::collections::BTreeMap;
use std::path::PathBuf;

use vag_data_db::CachedFault;

/// The project's fault tables, open for naming.
pub struct OdisFaults {
	cache: PathBuf,
	/// Every variant the cache holds fault rows for.
	variants: Vec<String>,
	/// `[faults] language`, lower-cased, if set.
	preferred: Option<String>,
	/// `(kind, dir, language)` of every source in the cache.
	sources: Vec<(String, String, Option<String>)>,
	/// Rows per `(F19E, F1A2)`, read once per unit rather than once per code.
	units: BTreeMap<(String, String), UnitTexts>,
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
		// §3), which is evidence for a decoder and not one — so the number is
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
		Some(OdisFaults {
			cache,
			variants,
			preferred: crate::config::fault_language(&crate::config::load()),
			sources,
			units: BTreeMap::new(),
		})
	}

	/// One line saying where the names come from, for the top of a listing.
	pub fn describe(&self) -> String {
		let (variants, codes) = vag_data_db::fault_counts(&self.cache).unwrap_or_default();
		let odis: Vec<String> = self
			.sources
			.iter()
			.filter(|(kind, _, _)| kind == vag_data_db::ODIS)
			.map(|(_, dir, language)| match language {
				Some(language) => format!("{dir} ({language})"),
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
	/// `None` when there is one source, or when the setting decided. Otherwise
	/// which source won by default and how to choose — once per run, above the
	/// codes, so that a text in an unexpected language is never a surprise.
	pub fn choice_note(&self) -> Option<String> {
		let odis: Vec<&(String, String, Option<String>)> = self.sources.iter().filter(|(kind, _, _)| kind == vag_data_db::ODIS).collect();
		let mut languages: Vec<&str> = self.sources.iter().filter_map(|(_, _, l)| l.as_deref()).collect();
		languages.sort_unstable();
		languages.dedup();
		if languages.len() < 2 {
			return None;
		}
		if let Some(preferred) = &self.preferred {
			return match languages.contains(&preferred.as_str()) {
				true => None,
				false => Some(format!(
					"config.toml sets [faults] language = {preferred:?}, which no source here declares ({}) — using the first ODIS source.",
					languages.join(", ")
				)),
			};
		}
		let (_, dir, language) = odis.first()?;
		Some(format!(
			"Fault text is read from {dir} ({}) first; the sources here declare {}. \
			 Set [faults] language in config.toml to choose.",
			language.as_deref().unwrap_or("no language declared"),
			languages.join(", ")
		))
	}

	/// Whether the setting names a language only the VCDS build declares, so
	/// the VCDS chain should be asked first and this project second.
	pub fn prefers_vcds(&self) -> bool {
		let Some(preferred) = &self.preferred else { return false };
		let declares = |kind: &str| {
			self
				.sources
				.iter()
				.any(|(k, _, language)| k == kind && language.as_deref() == Some(preferred.as_str()))
		};
		declares(vag_data_db::VCDS) && !declares(vag_data_db::ODIS)
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
		let chosen = match &self.preferred {
			Some(preferred) => candidates
				.iter()
				.find(|(_, row)| row.language.as_deref() == Some(preferred.as_str()))
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

	fn resolver(preferred: Option<&str>, sources: &[(&str, &str, Option<&str>)]) -> OdisFaults {
		OdisFaults {
			cache: PathBuf::new(),
			variants: Vec::new(),
			preferred: preferred.map(str::to_owned),
			sources: sources
				.iter()
				.map(|(k, d, l)| (k.to_string(), d.to_string(), l.map(str::to_owned)))
				.collect(),
			units: BTreeMap::new(),
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

	#[test]
	fn the_setting_chooses_between_sources_and_an_unset_one_takes_the_first_and_says_so() {
		let unit = UnitTexts {
			family: "EV_X".into(),
			confirmed: true,
			variants: vec!["EV_X".into()],
			rows: vec![
				("EV_X".into(), row(297, "Lenkwinkelsensor", Some("deu"), "/de")),
				("EV_X".into(), row(297, "Steering angle sensor", Some("eng"), "/en")),
			],
		};
		let sources = [("odis", "/de", Some("deu")), ("odis", "/en", Some("eng"))];
		let unset = resolver(None, &sources);
		assert_eq!(unset.name(&unit, [0, 1, 0x29]).unwrap().text.as_deref(), Some("Lenkwinkelsensor"));
		let note = unset.choice_note().expect("two languages and no setting is a choice worth saying");
		assert!(note.contains("/de (deu)"), "{note}");
		assert!(note.contains("[faults] language"), "{note}");

		let english = resolver(Some("eng"), &sources);
		assert_eq!(english.name(&unit, [0, 1, 0x29]).unwrap().text.as_deref(), Some("Steering angle sensor"));
		assert_eq!(english.choice_note(), None, "a setting that was honoured needs no note");

		let nobody = resolver(Some("fra"), &sources);
		assert_eq!(nobody.name(&unit, [0, 1, 0x29]).unwrap().text.as_deref(), Some("Lenkwinkelsensor"));
		assert!(nobody.choice_note().unwrap().contains("\"fra\""));
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
		assert!(!resolver(Some("deu"), &sources).prefers_vcds());
		assert!(!resolver(None, &sources).prefers_vcds());
		// Both declaring the language: the project is asked first, as always.
		let both = [("vcds", "/vcds/Labels", Some("deu")), ("odis", "/x", Some("deu"))];
		assert!(!resolver(Some("deu"), &both).prefers_vcds());
	}
}
