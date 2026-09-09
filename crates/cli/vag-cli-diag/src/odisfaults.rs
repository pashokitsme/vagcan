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
	/// The object's `LEVEL`, recorded and not interpreted.
	pub level: u32,
}

impl Naming {
	/// One line for the console: the display code, then the text with its
	/// line breaks flattened — a fault text is one row on a screen.
	pub fn line(&self) -> String {
		let text = self.text.as_deref().map(|t| t.replace('\n', " / "));
		match (self.display_code.as_deref(), text) {
			(Some(code), Some(text)) => format!("{code}  {text}"),
			(Some(code), None) => format!("{code}  (no text in the project)"),
			(None, Some(text)) => text,
			(None, None) => "(no text in the project)".to_string(),
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
		let mut rows = Vec::new();
		for variant in &variants {
			for row in vag_data_db::faults_of(&self.cache, variant).unwrap_or_default() {
				rows.push((variant.clone(), row));
			}
		}
		let texts = UnitTexts { variants, rows };
		self.units.insert(key, texts.clone());
		texts
	}

	/// Name one code from a unit's rows, or `None` when no table lists it.
	///
	/// Among the rows for the number, one whose source declares the preferred
	/// language wins; failing that, the first written — the first source, in
	/// its first-named variant — which is what makes two runs agree.
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
		Some(Naming {
			display_code: row.fault.display_code.clone(),
			text: row.fault.text.clone(),
			language: row.language.clone(),
			variant: variant.clone(),
			level: row.fault.level,
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn row(code: u32, text: &str, language: Option<&str>, dir: &str) -> CachedFault {
		CachedFault {
			fault: vag_data_labels::odis::Fault {
				dop: "DTCDOP_VAGUDS".into(),
				code,
				display_code: Some("B1168F2".into()),
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
			variants: vec!["EV_Brake_035".into()],
			rows: vec![("EV_Brake_035".into(), row(297, "Swa_lost_initialisation", Some("deu"), "/x"))],
		};
		let r = resolver(None, &[("odis", "/x", Some("deu"))]);
		let named = r.name(&unit, [0x00, 0x01, 0x29]).expect("297 is in the table");
		assert_eq!(named.line(), "B1168F2  Swa_lost_initialisation");
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
			variant: String::new(),
			level: 2,
		};
		assert_eq!(naming.line(), "P150B00  Acceleration monitoring / Control limit exceeded");
	}

	#[test]
	fn the_setting_chooses_between_sources_and_an_unset_one_takes_the_first_and_says_so() {
		let unit = UnitTexts {
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
