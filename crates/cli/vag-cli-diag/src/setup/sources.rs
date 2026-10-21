//! `vagcan setup --list` and `vagcan setup --drop` — a project's sources, as its cache
//! holds them.
//!
//! The `source` table in `cache.sqlite` is the one list of what a project was read from
//! ([`crate::project::sources`]). Deleting a line from a note beside it never removed a
//! source's rows, and every reader went on using them; dropping a source here removes
//! the row and everything it wrote, in one transaction ([`vag_data_db::drop_source`]).
//!
//! Data on stdout, notes on stderr: `--list` is a table a script can cut columns out of.

use std::path::Path;

use anyhow::{Result, anyhow, bail};
use vag_cli_core::report::Advice as _;

use crate::project::Project;
use crate::ui::menu::Asker;

/// `vagcan setup --list`: the current project's sources, one row each.
pub fn list() -> Result<()> {
	let project = current()?;
	let sources = crate::project::sources(&project)?;
	let (rows, unowned) = match sources.is_empty() {
		true => Default::default(),
		false => vag_data_db::source_rows(&project.cache()).map_err(|e| anyhow::anyhow!("{e}"))?,
	};
	vag_cli_core::out!("{}", render(&project, &sources, &rows));
	for note in notes(&sources, &unowned) {
		vag_cli_core::say!("{note}");
	}
	Ok(())
}

/// `vagcan setup --drop <SOURCE>`: remove one source and every row it wrote, after asking
/// on a terminal; `yes` skips the question.
pub fn drop_source(selector: &str, yes: bool) -> Result<()> {
	let project = current()?;
	let mut io = crate::ui::Console::new(format!("vagcan setup --drop {selector} --yes"));
	drop_with(&mut io, &project, selector, yes, crate::ui::can_ask())
}

/// The project `--project`, the environment or `config.toml` names. Its "nothing is set up"
/// loses its type, hints kept: it must not reach `rescue`, whose answer to that is a download.
fn current() -> Result<Project> {
	crate::project::current().map_err(|e| vag_cli_core::report::untyped(&e))
}

/// The rule behind [`drop_source`], with the asking and the terminal passed in.
fn drop_with(io: &mut impl Asker, project: &Project, selector: &str, yes: bool, can_ask: bool) -> Result<()> {
	let sources = crate::project::sources(project)?;
	if sources.is_empty() {
		bail!("the project {} has no sources: nothing to drop", project.id);
	}
	let source = select(&sources, selector)?;
	let cache = project.cache();
	let (rows, _) = vag_data_db::source_rows(&cache).map_err(|e| anyhow::anyhow!("{e}"))?;
	let held = rows.get(&source.id).copied().unwrap_or_default();
	io.say(&describe(source, &held))?;
	if !yes {
		if !can_ask {
			return Err(
				anyhow!("no terminal to confirm on: nothing dropped").help(format!("to drop without asking: `vagcan setup --drop {} --yes`", source.id)),
			);
		}
		let typed = io.line(&format!("Drop source {} and its {} rows? [y/N]", source.id, held.total()), "")?;
		if !matches!(typed.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
			vag_cli_core::say!("nothing dropped");
			return Ok(());
		}
	}
	let gone =
		vag_data_db::drop_source(&cache, source.id).map_err(|e| anyhow::anyhow!("dropping source {} from {}: {e}", source.id, cache.display()))?;
	io.say(&dropped(source, &gone))?;
	for note in leftovers(project, &source.kind)? {
		vag_cli_core::say!("{note}");
	}
	Ok(())
}

/// The one source `selector` names: an id from `--list`, or a kind that exactly one
/// source is. Anything else is refused with the candidates.
fn select<'a>(sources: &'a [vag_data_db::Source], selector: &str) -> Result<&'a vag_data_db::Source> {
	let wanted = selector.trim();
	if let Ok(id) = wanted.parse::<i64>() {
		return match sources.iter().find(|s| s.id == id) {
			Some(source) => Ok(source),
			None => Err(anyhow!("no source {id} in this project").note(format!("its sources:\n{}", candidates(sources.iter())))),
		};
	}
	let kind = wanted.to_ascii_lowercase();
	if kind != vag_data_db::VCDS && kind != vag_data_db::ODIS {
		return Err(
			anyhow!("{selector:?} is not a source")
				.note(format!("its sources:\n{}", candidates(sources.iter())))
				.help("give `odis`, `vcds` or an id from `vagcan setup --list`"),
		);
	}
	// A stale row is not a source of its kind ([`crate::project::has_source`]): it is
	// dropped by id, and never makes the kind ambiguous.
	let of_kind: Vec<&vag_data_db::Source> = sources.iter().filter(|s| s.kind == kind && !s.is_stale()).collect();
	match of_kind.as_slice() {
		[one] => Ok(one),
		[] => Err(anyhow!("no {kind} source in this project").note(format!("its sources:\n{}", candidates(sources.iter())))),
		_ => Err(
			anyhow!("{} {kind} sources in this project", of_kind.len())
				.note(format!("they are:\n{}", candidates(of_kind.iter().copied())))
				.help("say which by id: `vagcan setup --drop <id>`"),
		),
	}
}

/// One line per source, for a refusal: id, kind, path.
fn candidates<'a>(sources: impl Iterator<Item = &'a vag_data_db::Source>) -> String {
	sources
		.map(|s| format!("  {:>3}  {:<4}  {}", s.id, s.kind, s.path()))
		.collect::<Vec<_>>()
		.join("\n")
}

/// The state column: `stale` for a relative path nothing can resolve, `gone` for a folder
/// no longer on this machine (its rows still serve), `ok` otherwise.
fn state(source: &vag_data_db::Source) -> &'static str {
	match (source.is_stale(), Path::new(&source.dir).is_dir()) {
		(true, _) => "stale",
		(false, false) => "gone",
		(false, true) => "ok",
	}
}

/// `parsed_at` to the minute, as a column: `2026-10-20 07:26`.
///
/// With the `Z`: the stamp is UTC, and a time with no zone reads as local. A stamp of
/// another shape — an older note's, hand-edited — is shown as it is, never cut inside a
/// character.
fn when(parsed_at: Option<&str>) -> String {
	match parsed_at {
		Some(stamp) => match stamp.get(..16) {
			Some(minute) if minute.is_ascii() && minute.as_bytes()[10] == b'T' => format!("{}Z", minute.replace('T', " ")),
			_ => stamp.to_string(),
		},
		None => "-".to_string(),
	}
}

/// The listing: the project and its cache as `key  value`, then a table with one row per
/// source and the path last, since it is the one column of no fixed width.
fn render(project: &Project, sources: &[vag_data_db::Source], rows: &std::collections::BTreeMap<i64, vag_data_db::Rows>) -> String {
	let mut out = format!("project  {}\ncache    {}\n", project.id, project.cache().display());
	if sources.is_empty() {
		return out;
	}
	let mut table = vag_cli_core::table::Table::new(&[
		"id",
		"kind",
		"state",
		"lang",
		"version",
		"read",
		"faults",
		"channels",
		"labels",
		"measurements",
		"path",
	])
	.right(0)
	.right(6)
	.right(7)
	.right(8)
	.right(9)
	// The path last and never cut: a cut path is no path.
	.no_cut();
	for source in sources {
		let held = rows.get(&source.id).copied().unwrap_or_default();
		table.row(vec![
			source.id.to_string(),
			source.kind.clone(),
			state(source).to_string(),
			source.language.clone().unwrap_or_else(|| "-".into()),
			source.meta.version.clone().unwrap_or_else(|| "-".into()),
			when(source.meta.parsed_at.as_deref()),
			held.faults.to_string(),
			held.readings.to_string(),
			held.label_files.to_string(),
			held.measurements.to_string(),
			source.path().to_string(),
		]);
	}
	out.push('\n');
	out.push_str(&table.render(vag_cli_core::table::terminal_width()));
	out
}

/// What `--list` says beside the table, on stderr.
fn notes(sources: &[vag_data_db::Source], unowned: &vag_data_db::Rows) -> Vec<String> {
	let mut out = Vec::new();
	if sources.is_empty() {
		out.push("no sources: vagcan setup <ODIS project or VCDS installation>".to_string());
	}
	for source in sources.iter().filter(|s| s.is_stale()) {
		out.push(format!(
			"stale: source {} is a relative path an older vagcan recorded; nothing resolves it. Drop it: vagcan setup --drop {}",
			source.id, source.id
		));
	}
	if unowned.label_files > 0 {
		out.push(format!(
			"{} label files belong to no source; dropping the last VCDS source removes them",
			unowned.label_files
		));
	}
	out
}

/// One source and what it holds, as `key  value` lines: what `--drop` is about to remove.
fn describe(source: &vag_data_db::Source, held: &vag_data_db::Rows) -> String {
	let mut lines = vec![
		format!("id            {}", source.id),
		format!("kind          {}", source.kind),
		format!("path          {}", source.path()),
	];
	if source.dir != source.path() {
		lines.push(format!("dir           {}", source.dir));
	}
	lines.push(format!("language      {}", source.language.as_deref().unwrap_or("-")));
	lines.push(format!("read          {}", when(source.meta.parsed_at.as_deref())));
	lines.extend(counts(held));
	lines.join("\n")
}

fn counts(held: &vag_data_db::Rows) -> [String; 4] {
	[
		format!("faults        {}", held.faults),
		format!("channels      {}", held.readings),
		format!("label files   {}", held.label_files),
		format!("measurements  {}", held.measurements),
	]
}

/// What `--drop` removed, as `key  value` lines.
fn dropped(source: &vag_data_db::Source, gone: &vag_data_db::Rows) -> String {
	let mut lines = vec![format!("dropped       {} {} {}", source.id, source.kind, source.path())];
	lines.extend(counts(gone));
	lines.join("\n")
}

/// The project's files that described the dropped kind, once no source of it is left:
/// removed where nothing else could use them, kept and named where the next `setup` of
/// that kind reuses them. One note per file, for stderr.
fn leftovers(project: &Project, kind: &str) -> Result<Vec<String>> {
	// A stale row left beside it is no source of the kind: the files go as if it were not.
	if crate::project::has_source(project, kind)? {
		return Ok(Vec::new());
	}
	// `registry.json` names the dropped installation and what was read from it: kept, it
	// would send the next `watch` back to that installation to read it in again.
	// `names-odis.json` is the dropped projects' pooled names.
	let (remove, keep) = match kind {
		vag_data_db::VCDS => (vec![project.registry_log()], vec![project.rod_keys(), project.names(), project.odx_ids()]),
		_ => (vec![project.odis_names()], Vec::new()),
	};
	let mut out = Vec::new();
	for file in remove.iter().filter(|f| f.is_file()) {
		std::fs::remove_file(file).map_err(|e| anyhow::anyhow!("removing {}: {e}", file.display()))?;
		out.push(format!("removed: {}", file.display()));
	}
	for file in keep.iter().filter(|f| f.is_file()) {
		out.push(format!(
			"kept: {} — car commands no longer read it; `vagcan dev vcds` still can, and the next {kind} setup reuses it",
			file.display()
		));
	}
	Ok(out)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::ui::menu::{Answer, Scripted};

	fn source(id: i64, kind: &str, dir: &str) -> vag_data_db::Source {
		vag_data_db::Source {
			id,
			kind: kind.into(),
			dir: dir.into(),
			language: None,
			meta: vag_data_db::Meta::default(),
		}
	}

	/// The reference machine's sources on 2026-10-20, as the cache holds them.
	fn reference() -> Vec<vag_data_db::Source> {
		vec![
			source(1, "vcds", "vendor/vcds-en/Labels"),
			source(2, "odis", "/Users/x/Downloads/SK37X"),
			source(3, "vcds", "/Users/x/vcds-en/Labels"),
		]
	}

	#[test]
	fn a_kind_selects_the_one_source_of_it_and_an_id_any_source() {
		let sources = reference();
		assert_eq!(select(&sources, "odis").unwrap().id, 2);
		assert_eq!(select(&sources, "ODIS").unwrap().id, 2);
		assert_eq!(select(&sources, "1").unwrap().id, 1, "a stale row is droppable by id");
		assert_eq!(select(&sources, " 3 ").unwrap().id, 3);
		// Review of #37: the stale row beside the real installation made `vcds` two
		// sources. It is no source of its kind, so `vcds` is the one real row.
		assert_eq!(select(&sources, "vcds").unwrap().id, 3);
	}

	#[test]
	fn a_kind_two_sources_share_is_refused_with_both_listed() {
		let mut sources = reference();
		sources[0].dir = "/Users/x/vcds-ru/Labels".into();
		let why = vag_cli_core::report::render(&select(&sources, "vcds").unwrap_err(), 0, vag_cli_core::report::Colour::Never);
		assert!(why.contains("2 vcds sources") && why.contains("by id"), "{why}");
		assert!(
			why.contains("/Users/x/vcds-ru/Labels") && why.contains("/Users/x/vcds-en/Labels"),
			"{why}"
		);
		assert!(!why.contains("SK37X"), "only the candidates: {why}");
	}

	#[test]
	fn a_stamp_reads_to_the_minute_in_utc_and_an_odd_one_is_shown_as_it_is() {
		assert_eq!(when(Some("2026-10-20T07:26:16.756Z")), "2026-10-20 07:26Z");
		assert_eq!(when(Some("2026-10-20T07:26:16.756324+00:00")), "2026-10-20 07:26Z");
		assert_eq!(when(None), "-");
		// Review of #37: a byte slice through a character panicked.
		for odd in ["2026-10-20Tö7:26", "2026-10-2ö", "zeit: gestern über Mittag"] {
			assert_eq!(when(Some(odd)), odd, "{odd}");
		}
	}

	#[test]
	fn an_id_or_kind_that_is_not_there_is_refused_with_what_is() {
		let sources = reference();
		for (selector, said) in [("9", "no source 9"), ("labels", "not a source"), ("", "not a source")] {
			let why = vag_cli_core::report::render(&select(&sources, selector).unwrap_err(), 0, vag_cli_core::report::Colour::Never);
			assert!(why.contains(said), "{selector:?}: {why}");
			assert!(why.contains("SK37X"), "{selector:?}: the sources are listed: {why}");
		}
		let only_vcds = [source(3, "vcds", "/Users/x/vcds-en/Labels")];
		assert!(
			vag_cli_core::report::render(&select(&only_vcds, "odis").unwrap_err(), 0, vag_cli_core::report::Colour::Never).contains("no odis source")
		);
	}

	#[test]
	fn the_listing_is_a_table_with_the_path_last_and_the_stale_row_marked() {
		let here = tempfile::tempdir().unwrap();
		let project = Project {
			id: "SK37X".into(),
			dir: here.path().join("SK37X"),
		};
		let mut sources = reference();
		sources[1].language = Some("deu".into());
		sources[1].dir = here.path().display().to_string();
		sources[2].meta.root = Some("/Users/x/vcds-en".into());
		sources[2].meta.parsed_at = Some("2026-10-20T07:26:16.756Z".into());
		let rows: std::collections::BTreeMap<i64, vag_data_db::Rows> = [
			(
				2,
				vag_data_db::Rows {
					faults: 282621,
					readings: 399283,
					..Default::default()
				},
			),
			(
				3,
				vag_data_db::Rows {
					label_files: 3035,
					measurements: 101241,
					readings: 5304,
					faults: 0,
				},
			),
		]
		.into();
		let text = render(&project, &sources, &rows);
		let lines: Vec<&str> = text.lines().collect();
		assert_eq!(lines[0], "project  SK37X");
		assert!(lines[1].starts_with("cache    ") && lines[1].ends_with("cache.sqlite"), "{text}");
		assert!(lines[3].starts_with("id  kind  state") && lines[3].ends_with("path"), "{text}");
		assert!(lines[4].contains("stale") && lines[4].ends_with("vendor/vcds-en/Labels"), "{text}");
		assert!(
			lines[5].contains(" ok ") && lines[5].contains("282621") && lines[5].contains("399283"),
			"{text}"
		);
		assert!(lines[6].contains("gone") && lines[6].contains("2026-10-20 07:26"), "{text}");
		assert!(lines[6].ends_with("/Users/x/vcds-en"), "the folder setup was pointed at: {text}");
		// Columns line up: every row puts its path at the header's `path`.
		let at = lines[3].find("path").unwrap();
		for (line, source) in lines[4..].iter().zip(&sources) {
			assert_eq!(line.get(at..), Some(source.path()), "{text}");
		}
		let notes = notes(&sources, &vag_data_db::Rows::default());
		assert_eq!(notes.len(), 1, "{notes:?}");
		assert!(notes[0].contains("vagcan setup --drop 1"), "{notes:?}");
	}

	/// A project with an ODIS project and a VCDS installation in its cache, and the files
	/// each leaves beside it.
	fn project_with_both(here: &Path) -> Project {
		let project = Project {
			id: "SK37X".into(),
			dir: here.join("SK37X"),
		};
		std::fs::create_dir_all(&project.dir).unwrap();
		let fault = vag_data_labels::odis::Fault {
			dop: "DTCDOP_VAGUDS".into(),
			code: 297,
			display_code: Some("P012900".into()),
			text: Some("Fehler".into()),
			text_id: None,
			short_name: None,
			level: 2,
			temporary: false,
		};
		vag_data_db::put_all_faults(&project.cache(), "/x/SK37X", [("EV_Brake", &[fault][..])]).unwrap();
		vag_data_db::record_language(&project.cache(), vag_data_db::ODIS, "/x/SK37X", "deu").unwrap();
		vag_data_db::record_language(&project.cache(), vag_data_db::VCDS, "/x/vcds-en/Labels", "eng").unwrap();
		for file in [project.registry_log(), project.odis_names(), project.rod_keys()] {
			std::fs::write(file, b"{}").unwrap();
		}
		project
	}

	#[test]
	fn a_drop_on_a_terminal_asks_and_a_no_drops_nothing() {
		let here = tempfile::tempdir().unwrap();
		let project = project_with_both(here.path());
		let mut io = Scripted::new(vec![Answer::Type("n".into())]);
		let capture = vag_cli_core::verbosity::Capture::start(0);
		drop_with(&mut io, &project, "odis", false, true).unwrap();
		let said = capture.finish();
		// A refusal is said on stderr; stdout is the listing's alone.
		assert_eq!((said.stdout.as_str(), said.stderr.as_str()), ("", "nothing dropped\n"));
		assert!(
			io.all_said().contains("faults        1"),
			"what would go is shown first: {}",
			io.all_said()
		);
		assert_eq!(crate::project::sources(&project).unwrap().len(), 2, "a no dropped it");

		let mut io = Scripted::new(vec![Answer::Type("y".into())]);
		drop_with(&mut io, &project, "odis", false, true).unwrap();
		let left = crate::project::sources(&project).unwrap();
		assert_eq!(left.iter().map(|s| s.kind.as_str()).collect::<Vec<_>>(), ["vcds"]);
		assert!(io.all_said().contains("dropped"), "{}", io.all_said());
		assert!(vag_data_db::fault_variants(&project.cache()).unwrap().is_empty(), "its fault text stayed");
		assert!(!project.odis_names().exists(), "the last ODIS source's pooled names stayed");
		assert!(project.registry_log().exists(), "the VCDS installation's log went with the ODIS source");
		// What is left answers on its own: the VCDS source is the one source there is.
		assert!(!crate::project::has_source(&project, vag_data_db::ODIS).unwrap());
		assert!(crate::project::has_source(&project, vag_data_db::VCDS).unwrap());
	}

	#[test]
	fn without_a_terminal_a_drop_needs_yes() {
		let here = tempfile::tempdir().unwrap();
		let project = project_with_both(here.path());
		let mut io = Scripted::new(vec![]);
		let why = vag_cli_core::report::render(
			&drop_with(&mut io, &project, "vcds", false, false).unwrap_err(),
			0,
			vag_cli_core::report::Colour::Never,
		);
		assert!(why.contains("--yes") && why.contains("nothing dropped"), "{why}");
		assert_eq!(crate::project::sources(&project).unwrap().len(), 2);

		let mut io = Scripted::new(vec![]);
		drop_with(&mut io, &project, "vcds", true, false).unwrap();
		assert!(io.typed.is_empty(), "--yes asked nothing");
		let left = crate::project::sources(&project).unwrap();
		assert_eq!(left.iter().map(|s| s.kind.as_str()).collect::<Vec<_>>(), ["odis"]);
		assert!(!project.registry_log().exists(), "the dropped installation's log would bring it back");
		assert!(project.rod_keys().exists(), "the keys are kept for the next VCDS setup");
		assert_eq!(vag_data_db::fault_variants(&project.cache()).unwrap(), ["EV_Brake"], "the ODIS rows stay");
	}

	#[test]
	fn a_stale_row_beside_the_installation_neither_blocks_its_drop_nor_keeps_its_files() {
		// Review of #37, on the reference machine's shape: a relative VCDS row beside the
		// real one (one the upgrade keeps, or one written after it). `--drop vcds` is the
		// real one, and once it is gone the project has no VCDS source: its log goes, and
		// the stale row does not keep VCDS in use.
		let here = tempfile::tempdir().unwrap();
		let project = project_with_both(here.path());
		vag_data_db::record_language(&project.cache(), vag_data_db::VCDS, "vendor/vcds-en/Labels", "eng").unwrap();
		assert!(
			crate::project::sources(&project)
				.unwrap()
				.iter()
				.any(|s| s.kind == "vcds" && s.is_stale())
		);

		let mut io = Scripted::new(vec![]);
		drop_with(&mut io, &project, "vcds", true, false).expect("`vcds` is the one real VCDS source");
		assert!(
			!crate::project::has_source(&project, vag_data_db::VCDS).unwrap(),
			"the stale row counts as VCDS"
		);
		assert!(
			!project.registry_log().exists(),
			"the dropped installation's log stayed beside a stale row"
		);
	}

	#[test]
	fn a_drop_of_a_project_with_no_cache_says_so() {
		let here = tempfile::tempdir().unwrap();
		let project = Project {
			id: "SK37X".into(),
			dir: here.path().join("SK37X"),
		};
		let why = drop_with(&mut Scripted::new(vec![]), &project, "odis", true, false)
			.unwrap_err()
			.to_string();
		assert!(why.contains("no sources"), "{why}");
		assert!(!project.cache().exists(), "asking made a cache");
	}
}
