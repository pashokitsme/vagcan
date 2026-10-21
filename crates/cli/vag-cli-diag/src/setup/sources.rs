//! `vagcan setup --list` and `vagcan setup --drop` — the sources this machine holds, as
//! their caches hold them: the VCDS store's, then each ODIS project's.
//!
//! The `source` table in a `cache.sqlite` is the one list of what that store was read
//! from ([`crate::project::sources`]). Dropping a source here removes the row and
//! everything it wrote, in one transaction ([`vag_data_db::drop_source`]). `--drop vcds`
//! acts on the VCDS store, `--drop odis` on this run's project, an id on the store it
//! names.
//!
//! **An id names its store.** Each cache numbers its own sources, so a fresh machine has a
//! source 1 in both: the listing shows `v1` for the VCDS store's and `p1` for a project's,
//! and `--drop` takes either. A bare number still works where only one store has it.
//!
//! Data on stdout, notes on stderr: `--list` is a table a script can cut columns out of.

use std::path::Path;

use anyhow::{Result, anyhow};
use vag_cli_core::report::Advice as _;

use crate::project::Project;
use crate::ui::menu::Asker;

/// What every refusal of a selector ends with.
const GIVE_A_SOURCE: &str = "give `vcds`, `odis` or an id from `vagcan setup --list`";

/// `vagcan setup --list`: every source on this machine, one row each — the VCDS store's,
/// then every project's — under a one-line summary, as `gh` lists. At `-v` each row has
/// its details on a line of its own under it.
pub fn list() -> Result<()> {
	let vcds = crate::project::vcds()?;
	let projects = crate::datadir::projects_dir()?;
	let mut stores: Vec<Project> = Vec::new();
	if vcds.cache().is_file() {
		stores.push(vcds);
	}
	for id in crate::project::list()? {
		stores.push(Project { dir: projects.join(&id), id });
	}
	// Nothing set up is a listing of nothing, as an empty store is: not an error.
	if stores.is_empty() {
		vag_cli_core::out!("{}", render(&[]).0);
		vag_cli_core::say!("help: run `vagcan setup {}` to read one", crate::missing::SOURCE_PLACEHOLDER);
		return Ok(());
	}
	let mut listed: Vec<Listed> = Vec::new();
	let mut said = Vec::new();
	for store in stores {
		let sources = crate::project::sources(&store)?;
		let (rows, unowned) = match sources.is_empty() {
			true => Default::default(),
			false => vag_data_db::source_rows(&store.cache()).map_err(|e| anyhow::anyhow!("{e}"))?,
		};
		said.extend(notes(&store, &sources, &unowned));
		listed.push(Listed { store, sources, rows });
	}
	let (head, rows) = render(&listed);
	vag_cli_core::out!("{head}");
	for (line, details) in rows {
		vag_cli_core::out!("{line}\n");
		if let Some(details) = details {
			vag_cli_core::note!("    {details}");
		}
	}
	for note in said {
		vag_cli_core::say!("{note}");
	}
	for line in migrations(&crate::project::vcds()?) {
		vag_cli_core::note!("{line}");
	}
	Ok(())
}

/// What `-v` says of the migrations the VCDS store's record keeps
/// ([`crate::project::migration_record`]): one line each, when, from where, what it replaced.
fn migrations(store: &Project) -> Vec<String> {
	let Some(entries) = std::fs::read_to_string(crate::project::migration_record(store))
		.ok()
		.and_then(|text| serde_json::from_str::<Vec<serde_json::Value>>(&text).ok())
	else {
		return Vec::new();
	};
	let path = |value: &serde_json::Value| super::home_short(Path::new(value.as_str().unwrap_or("?")));
	entries
		.iter()
		.map(|entry| {
			// The day, as the listing's READ column shows one.
			let mut parts = vec![format!("migrated {}", day(entry["at"].as_str()))];
			if entry["moved"].is_object() {
				parts.push(format!(
					"moved the VCDS installation {} from {}",
					path(&entry["moved"]["installation"]),
					path(&entry["moved"]["from"])
				));
			}
			for replaced in entry["replaced"].as_array().into_iter().flatten() {
				parts.push(format!("replaced {}", path(&replaced["installation"])));
			}
			format!("note: {}", parts.join(vag_cli_core::marks::SEP))
		})
		.collect()
}

/// One store's part of the listing.
struct Listed {
	store: Project,
	sources: Vec<vag_data_db::Source>,
	rows: std::collections::BTreeMap<i64, vag_data_db::Rows>,
}

/// Whether a store is the machine's VCDS store, whose ids read `v<n>`.
fn is_vcds_store(store: &Project) -> bool {
	store.id == crate::project::VCDS_STORE
}

/// A source's id as the listing shows it and `--drop` takes it: its store's letter, then
/// the cache's own number — `v1`, `p2`.
fn shown_id(store: &Project, source: &vag_data_db::Source) -> String {
	format!("{}{}", if is_vcds_store(store) { 'v' } else { 'p' }, source.id)
}

/// `vagcan setup --drop <SOURCE>`: remove one source and every row it wrote, after asking
/// on a terminal; `yes` skips the question.
pub fn drop_source(selector: &str, yes: bool) -> Result<()> {
	let vcds = crate::project::vcds()?;
	// A project named that is not there, or several and none chosen: said as text, for it
	// must not reach `rescue`, whose answer to "nothing set up" is a download.
	let project = crate::project::selected().map_err(|e| anyhow::anyhow!("{e:#}"))?;
	let store = which_store(selector, &vcds, project.as_ref())?;
	let mut io = crate::ui::Console::new(format!("vagcan setup --drop {selector} --yes"));
	drop_with(&mut io, &store, selector, yes, crate::ui::can_ask())
}

/// The store `selector` is about: the VCDS store for `vcds` or `v<n>`, the project for
/// `odis` or `p<n>`, and for a bare number the one of the two that holds it — refused when
/// both do.
fn which_store(selector: &str, vcds: &Project, project: Option<&Project>) -> Result<Project> {
	let wanted = selector.trim().to_ascii_lowercase();
	let no_project = || anyhow!("no ODIS project is set up: nothing to drop").help(GIVE_A_SOURCE);
	if wanted == vag_data_db::VCDS || wanted.strip_prefix('v').is_some_and(is_number) {
		return Ok(vcds.clone());
	}
	if wanted == vag_data_db::ODIS || wanted.strip_prefix('p').is_some_and(is_number) {
		return project.cloned().ok_or_else(no_project);
	}
	let Ok(id) = wanted.parse::<i64>() else {
		return Err(anyhow!("`{}` is not a source", selector.trim()).help(GIVE_A_SOURCE));
	};
	let holds = |store: &Project| crate::project::sources(store).is_ok_and(|rows| rows.iter().any(|s| s.id == id));
	let held: Vec<&Project> = std::iter::once(vcds).chain(project).filter(|store| holds(store)).collect();
	match held.as_slice() {
		[one] => Ok((*one).clone()),
		[] => Err(anyhow!("no source {id} on this machine").help(GIVE_A_SOURCE)),
		_ => Err(
			anyhow!("source {id} is in the VCDS store and in the project").help(format!("give `v{id}` or `p{id}`, as `vagcan setup --list` shows them")),
		),
	}
}

fn is_number(text: &str) -> bool {
	!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// The rule behind [`drop_source`], on one store, with the asking and the terminal
/// passed in. What it says is a heading, then indented lines, as `git` reports.
fn drop_with(io: &mut impl Asker, project: &Project, selector: &str, yes: bool, can_ask: bool) -> Result<()> {
	let sources = crate::project::sources(project)?;
	if sources.is_empty() {
		let read_one = match is_vcds_store(project) {
			true => "run `vagcan setup <VCDS installation>` to read one",
			false => "run `vagcan setup <ODIS project>` to read one",
		};
		return Err(anyhow!("{} has no sources: nothing to drop", name(project)).help(read_one));
	}
	let source = select(project, &sources, selector)?;
	let cache = project.cache();
	let (rows, _) = vag_data_db::source_rows(&cache).map_err(|e| anyhow::anyhow!("{e}"))?;
	let held = rows.get(&source.id).copied().unwrap_or_default();
	let id = shown_id(project, source);
	io.say(&format!(
		"Drop source {id} ({}) {}",
		source.kind,
		super::home_short(Path::new(source.path()))
	))?;
	if !yes {
		if !can_ask {
			return Err(anyhow!("no terminal to confirm on: nothing dropped").help(format!("run `vagcan setup --drop {} --yes`", selector.trim())));
		}
		io.say(&format!("  holds {}", counts(&held)))?;
		let typed = io.line("Drop it? [y/N]", "")?;
		if !matches!(typed.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
			vag_cli_core::say!("nothing dropped");
			return Ok(());
		}
	}
	let gone = vag_data_db::drop_source(&cache, source.id).map_err(|e| anyhow::anyhow!("dropping source {id} from {}: {e}", cache.display()))?;
	io.say(&format!("  removed {}", counts(&gone)))?;
	for line in leftovers(project, &source.kind)? {
		io.say(&line)?;
	}
	Ok(())
}

/// The one source `selector` names in this store: an id as `--list` shows it, the cache's
/// number alone, or a kind exactly one source is.
fn select<'a>(store: &Project, sources: &'a [vag_data_db::Source], selector: &str) -> Result<&'a vag_data_db::Source> {
	let wanted = selector.trim().to_ascii_lowercase();
	let number = wanted.trim_start_matches(['v', 'p']);
	if is_number(number) {
		return match sources.iter().find(|s| shown_id(store, s) == wanted || s.id.to_string() == wanted) {
			Some(source) => Ok(source),
			None => Err(anyhow!("no source {} on this machine", selector.trim()).help(GIVE_A_SOURCE)),
		};
	}
	if wanted != vag_data_db::VCDS && wanted != vag_data_db::ODIS {
		return Err(anyhow!("`{}` is not a source", selector.trim()).help(GIVE_A_SOURCE));
	}
	// A stale row is not a source of its kind ([`crate::project::has_source`]): it is
	// dropped by id, and never makes the kind ambiguous.
	let of_kind: Vec<&vag_data_db::Source> = sources.iter().filter(|s| s.kind == wanted && !s.is_stale()).collect();
	match of_kind.as_slice() {
		[one] => Ok(one),
		[] => Err(anyhow!("no {wanted} source on this machine").help(GIVE_A_SOURCE)),
		_ => Err(anyhow!("{} {wanted} sources", of_kind.len()).help(format!(
			"give one of {}",
			of_kind.iter().map(|s| format!("`{}`", shown_id(store, s))).collect::<Vec<_>>().join(", ")
		))),
	}
}

/// Whether a source can be read from: not a relative path an older vagcan left, and its
/// folder still on this machine.
fn usable(source: &vag_data_db::Source) -> bool {
	!source.is_stale() && Path::new(&source.dir).is_dir()
}

/// `parsed_at` to the minute: `2026-10-20 07:26Z`.
///
/// With the `Z`: the stamp is UTC, and a time with no zone reads as local. A stamp of
/// another shape — an older note's, hand-edited — is shown as it is, never cut inside a
/// character.
fn when(parsed_at: Option<&str>) -> String {
	match parsed_at {
		Some(stamp) => match stamp.get(..16) {
			Some(minute) if minute.is_ascii() && minute.as_bytes()[10] == b'T' => {
				format!("{}Z", minute.replace('T', " "))
			}
			_ => stamp.to_string(),
		},
		None => "-".to_string(),
	}
}

/// `parsed_at` to the day, as the listing's column: `2026-10-20`.
fn day(parsed_at: Option<&str>) -> String {
	match when(parsed_at) {
		minute if minute.len() == 17 => minute[..10].to_string(),
		other => other,
	}
}

/// What a sentence calls a store: the VCDS store, or a project by its name.
fn name(store: &Project) -> String {
	match is_vcds_store(store) {
		true => "the VCDS store".to_string(),
		false => format!("the project {}", store.id),
	}
}

/// The listing: the summary line and the header as one string, then each row with what
/// `-v` says under it — `gh pr list`'s shape, a `✓`/`✗` first, short columns, the path
/// last with `~`.
fn render(listed: &[Listed]) -> (String, Vec<(String, Option<String>)>) {
	let total: usize = listed.iter().map(|l| l.sources.len()).sum();
	let mut head = format!("{total} {} on this machine\n", crate::render::plural(total, "source"));
	if total == 0 {
		return (head, Vec::new());
	}
	let mut table = vag_cli_core::table::Table::new(&["", "ID", "KIND", "PROJECT", "LANG", "READ", "CHANNELS", "PATH"])
		.right(6)
		// The path last and never cut: a cut path is no path.
		.no_cut();
	let mut details = Vec::new();
	for l in listed {
		for source in &l.sources {
			let held = l.rows.get(&source.id).copied().unwrap_or_default();
			table.row(vec![
				(if usable(source) {
					vag_cli_core::marks::OK
				} else {
					vag_cli_core::marks::FAIL
				})
				.to_string(),
				shown_id(&l.store, source),
				source.kind.clone(),
				(if is_vcds_store(&l.store) { "-" } else { l.store.id.as_str() }).to_string(),
				source.language.clone().unwrap_or_else(|| "-".into()),
				day(source.meta.parsed_at.as_deref()),
				held.readings.to_string(),
				super::home_short(Path::new(source.path())),
			]);
			details.push(detail(&l.store, source, &held));
		}
	}
	let rendered = table.render(vag_cli_core::table::terminal_width());
	let mut lines = rendered.lines();
	head.push('\n');
	head.push_str(lines.next().unwrap_or_default());
	head.push('\n');
	(head, rows_of(lines, details))
}

/// The table's lines after the header, one entry per source with its `-v` details. A row
/// is one line, or two when its path goes under it: every row starts with its `✓`/`✗`, and
/// a line that starts with a space is the row above's path — so a wrapped path never shifts
/// a later row onto another's details, or out of the listing.
fn rows_of<'a>(lines: impl Iterator<Item = &'a str>, details: Vec<String>) -> Vec<(String, Option<String>)> {
	let mut rows: Vec<String> = Vec::new();
	for line in lines {
		match rows.last_mut() {
			Some(row) if line.starts_with(' ') => {
				row.push('\n');
				row.push_str(line);
			}
			_ => rows.push(line.to_string()),
		}
	}
	debug_assert_eq!(rows.len(), details.len(), "one rendered row per source");
	let mut details = details.into_iter();
	rows.into_iter().map(|row| (row, details.next())).collect()
}

/// What `-v` says under one row: where it is kept, what the source calls its version, why
/// it is marked `✗`, and what else it holds, joined by the one separator.
fn detail(store: &Project, source: &vag_data_db::Source, held: &vag_data_db::Rows) -> String {
	let mut parts = vec![super::home_short(&store.cache())];
	match (source.kind.as_str(), source.meta.version.as_deref()) {
		(vag_data_db::VCDS, Some(file)) => parts.push(format!("fault text from {file}")),
		(_, Some(version)) => parts.push(format!("version {version}")),
		(_, None) => {}
	}
	if source.is_stale() {
		parts.push("a relative path an older vagcan recorded; nothing resolves it".to_string());
	} else if !Path::new(&source.dir).is_dir() {
		parts.push("its folder is gone; its rows still serve".to_string());
	}
	if held.label_files + held.measurements + held.faults > 0 {
		parts.push(counts(&vag_data_db::Rows { readings: 0, ..*held }));
	}
	parts.join(vag_cli_core::marks::SEP)
}

/// What `--list` says beside the listing, on stderr.
fn notes(store: &Project, sources: &[vag_data_db::Source], unowned: &vag_data_db::Rows) -> Vec<String> {
	let mut out = Vec::new();
	if sources.is_empty() {
		let read_one = match is_vcds_store(store) {
			true => "help: run `vagcan setup <VCDS installation>` to read one",
			false => "help: run `vagcan setup <ODIS project>` to read one",
		};
		out.push(format!("note: {} has no sources\n{read_one}", name(store)));
	}
	for source in sources.iter().filter(|s| s.is_stale()) {
		let id = shown_id(store, source);
		out.push(format!(
			"warning: source {id} is a relative path an older vagcan recorded; nothing resolves it\n\
			 help: run `vagcan setup --drop {id}`"
		));
	}
	if unowned.label_files > 0 {
		out.push(format!(
			"note: {} label files belong to no source; dropping the last VCDS source removes them",
			unowned.label_files
		));
	}
	out
}

/// What a source holds, as one line of facts: `6085 channels · 3035 label files`. Kinds
/// it holds none of are left out.
fn counts(held: &vag_data_db::Rows) -> String {
	let parts: Vec<String> = [
		(held.readings, "channel"),
		(held.faults, "fault code"),
		(held.label_files, "label file"),
		(held.measurements, "measurement"),
	]
	.into_iter()
	.filter(|(n, _)| *n > 0)
	.map(|(n, what)| format!("{n} {}", crate::render::plural(n as usize, what)))
	.collect();
	match parts.is_empty() {
		true => "no rows".to_string(),
		false => parts.join(vag_cli_core::marks::SEP),
	}
}

/// The store's files that described the dropped kind, once no source of it is left:
/// removed where nothing else could use them, kept and named where the next `setup` of
/// that kind reuses them — as lines under the drop's heading.
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
	let file_name = |file: &std::path::PathBuf| file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
	let mut removed = Vec::new();
	for file in remove.iter().filter(|f| f.is_file()) {
		std::fs::remove_file(file).map_err(|e| anyhow::anyhow!("removing {}: {e}", file.display()))?;
		removed.push(file_name(file));
	}
	let kept: Vec<String> = keep.iter().filter(|f| f.is_file()).map(file_name).collect();
	let mut out = Vec::new();
	if !removed.is_empty() {
		out.push(format!("  removed {}", removed.join(vag_cli_core::marks::SEP)));
	}
	if !kept.is_empty() {
		out.push(format!("  kept {}", kept.join(vag_cli_core::marks::SEP)));
		out.push(format!("    (the next {} setup reuses them)", kind.to_ascii_uppercase()));
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

	/// An error as `vagcan` prints it, with no colour.
	fn rendered(err: &anyhow::Error) -> String {
		vag_cli_core::report::render(err, 0, vag_cli_core::report::Colour::Never)
	}

	/// A project the selector tests' sources are in: its ids read `p<n>`.
	fn store() -> Project {
		Project {
			id: "SK37X".into(),
			dir: "/x/SK37X".into(),
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
		assert_eq!(select(&store(), &sources, "odis").unwrap().id, 2);
		assert_eq!(select(&store(), &sources, "ODIS").unwrap().id, 2);
		assert_eq!(select(&store(), &sources, "1").unwrap().id, 1, "a stale row is droppable by id");
		assert_eq!(select(&store(), &sources, "p1").unwrap().id, 1, "and by the id the listing shows");
		assert!(select(&store(), &sources, "v1").is_err(), "another store's id is not this one's");
		assert_eq!(select(&store(), &sources, " 3 ").unwrap().id, 3);
		// Review of #37: the stale row beside the real installation made `vcds` two
		// sources. It is no source of its kind, so `vcds` is the one real row.
		assert_eq!(select(&store(), &sources, "vcds").unwrap().id, 3);
	}

	#[test]
	fn a_kind_two_sources_share_is_refused_with_both_listed() {
		let mut sources = reference();
		sources[0].dir = "/Users/x/vcds-ru/Labels".into();
		let why = rendered(&select(&store(), &sources, "vcds").unwrap_err());
		assert_eq!(why, "error: 2 vcds sources\n\nhelp: give one of `p1`, `p3`\n");
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
			let why = rendered(&select(&store(), &sources, selector).unwrap_err());
			assert!(why.contains(said), "{selector:?}: {why}");
			assert!(why.ends_with(&format!("help: {GIVE_A_SOURCE}\n")), "{selector:?}: {why}");
		}
		let only_vcds = [source(3, "vcds", "/Users/x/vcds-en/Labels")];
		assert!(rendered(&select(&store(), &only_vcds, "odis").unwrap_err()).contains("no odis source"));
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
		let listed = [Listed {
			store: project.clone(),
			sources: sources.clone(),
			rows: rows.clone(),
		}];
		let (head, rows_shown) = render(&listed);
		assert!(head.starts_with("3 sources on this machine\n\n"), "{head}");
		let header = head.lines().nth(2).unwrap();
		assert!(
			header.starts_with("   ID  KIND  PROJECT  LANG  READ") && header.ends_with("PATH"),
			"{header}"
		);
		let lines: Vec<&str> = rows_shown.iter().map(|(line, _)| line.as_str()).collect();
		assert!(
			lines[0].starts_with("✗  p1  vcds  SK37X") && lines[0].ends_with("vendor/vcds-en/Labels"),
			"{lines:?}"
		);
		assert!(lines[1].starts_with("✓  p2  odis") && lines[1].contains("399283"), "{lines:?}");
		assert!(lines[2].starts_with("✗  p3") && lines[2].contains("2026-10-20 "), "{lines:?}");
		assert!(lines[2].ends_with("/Users/x/vcds-en"), "the folder setup was pointed at: {lines:?}");
		assert!(header.find("PATH").unwrap() <= 60, "short columns before the path: {header}");
		// Columns line up: every row puts its path at the header's `PATH`.
		let at = header.find("PATH").unwrap();
		for (line, source) in lines.iter().zip(&sources) {
			let path = super::super::home_short(Path::new(source.path()));
			assert_eq!(line.chars().skip(at).collect::<String>(), path, "{line:?}");
		}
		// At -v, each row's details under it, joined by the one separator.
		let details: Vec<String> = rows_shown.iter().filter_map(|(_, d)| d.clone()).collect();
		assert!(details[0].contains("a relative path"), "{details:?}");
		assert!(details[1].contains("282621 fault codes"), "{details:?}");
		assert!(
			details[2].contains("fault text from") || details[2].contains("3035 label files · 101241 measurements"),
			"{details:?}"
		);
		assert!(details[2].contains("its folder is gone"), "{details:?}");
		assert_eq!(render(&[]).0, "0 sources on this machine\n");
		let notes = notes(&project, &sources, &vag_data_db::Rows::default());
		assert_eq!(notes.len(), 1, "{notes:?}");
		assert!(notes[0].contains("help: run `vagcan setup --drop p1`"), "{notes:?}");
	}

	#[test]
	fn a_path_under_its_row_keeps_every_row_with_its_own_details() {
		// At 80 columns a long path goes on a line of its own under its row: two lines per
		// row, which paired line by line with the details dropped the second row (UX review
		// of PR #50).
		let mut table = vag_cli_core::table::Table::new(&["", "ID", "KIND", "PROJECT", "LANG", "READ", "CHANNELS", "PATH"]).no_cut();
		let long = "/nowhere/a-long-way-down/to-a-vcds-installation/Labels";
		table.row(["✓", "v1", "vcds", "-", "eng", "2026-10-21", "0", long].map(String::from).to_vec());
		table.row(
			["✓", "p2", "odis", "SIM00", "eng", "2026-10-21", "2", "/nowhere/SIM00-odis"]
				.map(String::from)
				.to_vec(),
		);
		let rendered = table.render(Some(80));
		assert_eq!(rendered.lines().count(), 4, "one path under its row: {rendered}");
		let rows = rows_of(rendered.lines().skip(1), vec!["store v1".into(), "project p2".into()]);
		assert_eq!(rows.len(), 2, "{rows:?}");
		assert!(rows[0].0.starts_with("✓  v1") && rows[0].0.ends_with(long), "{rows:?}");
		assert_eq!(rows[0].1.as_deref(), Some("store v1"));
		assert!(rows[1].0.starts_with("✓  p2") && rows[1].0.ends_with("/nowhere/SIM00-odis"), "{rows:?}");
		assert_eq!(rows[1].1.as_deref(), Some("project p2"));
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
			io.all_said().contains("  holds 1 fault code"),
			"what would go is shown first: {}",
			io.all_said()
		);
		assert_eq!(crate::project::sources(&project).unwrap().len(), 2, "a no dropped it");

		let mut io = Scripted::new(vec![Answer::Type("y".into())]);
		drop_with(&mut io, &project, "odis", false, true).unwrap();
		let left = crate::project::sources(&project).unwrap();
		assert_eq!(left.iter().map(|s| s.kind.as_str()).collect::<Vec<_>>(), ["vcds"]);
		assert!(io.all_said().contains("  removed 1 fault code"), "{}", io.all_said());
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

	#[test]
	fn vcds_is_the_machine_store_odis_the_project_and_an_id_the_one_that_holds_it() {
		// The VCDS store and the project each keep their own `source` ids, so an id is
		// looked for in both and refused when both hold it.
		let here = tempfile::tempdir().unwrap();
		let project = Project {
			id: "SK37X".into(),
			dir: here.path().join("data").join("SK37X"),
		};
		let vcds = crate::project::vcds_at(&here.path().join("vcds"));
		std::fs::create_dir_all(&project.dir).unwrap();
		std::fs::create_dir_all(&vcds.dir).unwrap();
		vag_data_db::record_language(&project.cache(), vag_data_db::ODIS, "/x/SK37X", "deu").unwrap();
		vag_data_db::record_language(&vcds.cache(), vag_data_db::ODIS, "/x/placeholder", "deu").unwrap();
		vag_data_db::record_language(&vcds.cache(), vag_data_db::VCDS, "/x/vcds-en/Labels", "eng").unwrap();
		assert_eq!(which_store("vcds", &vcds, Some(&project)).unwrap(), vcds);
		assert_eq!(which_store("ODIS", &vcds, Some(&project)).unwrap(), project);
		assert_eq!(which_store("2", &vcds, Some(&project)).unwrap(), vcds, "id 2 is the store's alone");
		let why = rendered(&which_store("1", &vcds, Some(&project)).unwrap_err());
		assert!(why.contains("help: give `v1` or `p1`"), "{why}");
		// With no project there is nothing ODIS to drop.
		assert!(rendered(&which_store("odis", &vcds, None).unwrap_err()).contains("no ODIS project"));
		assert_eq!(which_store("1", &vcds, None).unwrap(), vcds);
	}

	#[test]
	fn dropping_vcds_from_the_store_takes_its_log_and_keeps_its_keys_and_leaves_the_project() {
		let here = tempfile::tempdir().unwrap();
		let project = project_with_both(&here.path().join("data"));
		// The project as the migration leaves it: its ODIS rows alone.
		vag_data_db::drop_kind(&project.cache(), vag_data_db::VCDS).unwrap();
		let vcds = crate::project::vcds_at(&here.path().join("vcds"));
		std::fs::create_dir_all(&vcds.dir).unwrap();
		vag_data_db::record_language(&vcds.cache(), vag_data_db::VCDS, "/x/vcds-en/Labels", "eng").unwrap();
		for file in [vcds.registry_log(), vcds.rod_keys(), vcds.names()] {
			std::fs::write(file, b"{}").unwrap();
		}
		let store = which_store("vcds", &vcds, Some(&project)).unwrap();
		drop_with(&mut Scripted::new(vec![]), &store, "vcds", true, false).unwrap();
		assert!(!crate::project::has_source(&vcds, vag_data_db::VCDS).unwrap());
		assert!(!vcds.registry_log().exists(), "the dropped installation's log stayed");
		assert!(vcds.rod_keys().exists() && vcds.names().exists(), "kept for the next VCDS setup");
		assert_eq!(
			vag_data_db::fault_variants(&project.cache()).unwrap(),
			["EV_Brake"],
			"the project lost its ODIS rows"
		);
		// And `--drop odis` acts within the project, leaving the store alone.
		let store = which_store("odis", &vcds, Some(&project)).unwrap();
		drop_with(&mut Scripted::new(vec![]), &store, "odis", true, false).unwrap();
		assert!(!crate::project::has_source(&project, vag_data_db::ODIS).unwrap());
		assert!(vcds.cache().is_file());
	}
}
