//! **The one-source guard, end to end** (CLAUDE.md, "ODIS and VCDS are independent sources").
//!
//! A home holding both sources, each marking every name, id and wording with its own word, and
//! the readers a car command goes through — `extracted::current`, the fault namers, the VCDS
//! causes, the dash build's resolve — run over it as a command runs them: through the data
//! dir, the selector and the stores, not handed a store by the test. A reader that reaches
//! the other source in one run fails here, however it reaches it.
//!
//! **Each scenario runs in a child process of this test binary**, with `VAGCAN_HOME` and
//! `VAGCAN_SOURCE` set in its environment — no `set_var` in a process with other threads.
//! Nothing here is a fact about any car: every row, id and text is made up.

use std::path::Path;
use std::process::{Command, Stdio};

use vag_cli_core::source::Source;

/// Set in the child: which scenario to run.
const CHILD: &str = "VAGCAN_ONE_SOURCE_CHILD";

const VIN: &str = "TESTVIN0000000001";
const PROJECT: &str = "SK37X";

/// Both sources, every word of each marked `odis` or `vcds`. The project's fault text is
/// German only; `config.toml` asks for English.
fn home(at: &Path) {
	use vag_data_labels::catalog::Scaling;
	use vag_data_labels::measure::LinearScale;
	let reading = |did: u16, name: &str, id: &str| vag_data_labels::odis::Reading {
		did,
		name: name.to_string(),
		unit: None,
		bit_offset: 0,
		bit_length: 8,
		signed: false,
		big_endian: true,
		scaling: Scaling::Linear(LinearScale { factor: 1.0, offset: 0.0 }),
		text_id: Some(id.to_string()),
	};
	let project = at.join("data").join(PROJECT);
	let vcds = at.join("vcds");
	for dir in [&project, &vcds] {
		std::fs::create_dir_all(dir).unwrap();
	}
	std::fs::write(
		at.join("config.toml"),
		format!("project = \"{PROJECT}\"\n\n[faults]\nlanguage = \"en\"\n"),
	)
	.unwrap();

	let odis = project.join("cache.sqlite");
	vag_data_db::put_readings(&odis, "/nowhere/SK37X", "EV_Unit_001", &[reading(0x1000, "odis channel", "IDE00001")]).unwrap();
	let fault = vag_data_labels::odis::Fault {
		dop: "DTCDOP_TEST".into(),
		code: 0x000129,
		display_code: Some("P000129".into()),
		text: Some("odis fault".into()),
		text_id: None,
		short_name: None,
		level: 1,
		temporary: false,
	};
	vag_data_db::put_faults(&odis, "/nowhere/SK37X", "EV_Unit_001", &[fault]).unwrap();
	vag_data_db::record_language(&odis, vag_data_db::ODIS, "/nowhere/SK37X", "deu").unwrap();

	let store = vcds.join("cache.sqlite");
	vag_data_db::record_language(&store, vag_data_db::VCDS, "/nowhere/vcds/Labels", "eng").unwrap();
	vag_data_db::put_all_vcds_readings(
		&store,
		"/nowhere/vcds/Labels",
		[(
			"EV_Unit",
			&[
				reading(0x2000, "vcds channel", "900001"),
				reading(0x1000, "vcds at the shared field", "IDE00001"),
			][..],
		)],
	)
	.unwrap();
	// VCDS's wording, for its own id and for the id the ODIS row carries too.
	std::fs::write(vcds.join("names.json"), r#"{"IDE00001": "vcds wording", "900001": "vcds wording too"}"#).unwrap();
	// A unit the installation had no file for: a cause a VCDS run says, and an ODIS run not.
	std::fs::write(
		vcds.join("registry.json"),
		r#"{"installation": "/nowhere/vcds", "read_version": 3, "units": [
			{"odx_name": "EV_Unit", "odx_version": "001001", "outcome": "no file or redirect"}]}"#,
	)
	.unwrap();

	vag_cli_core::units::record_for(Some(VIN), &[engine()]);
	let dash = at.join("dash").join(VIN);
	std::fs::create_dir_all(&dash).unwrap();
	std::fs::write(
		dash.join("dash.toml"),
		format!("vin = \"{VIN}\"\n\n[[channel]]\nref = \"01:1000\"\n\n[[page]]\nkind = \"values\"\ncells = [\"01:1000\"]\n"),
	)
	.unwrap();
}

fn engine() -> vag_cli_core::plan::UnitIdentity {
	vag_cli_core::plan::UnitIdentity {
		request: 0x7E0,
		part_number: Some("TESTPART1".into()),
		odx_name: Some("EV_Unit".into()),
		odx_version: Some("001001".into()),
		component: None,
	}
}

/// The child's side: build the home, then run every reader as `source`'s run.
fn child(source: Source) {
	let at = std::path::PathBuf::from(std::env::var_os(vag_cli_core::datadir::ROOT_ENV).unwrap());
	home(&at);
	let (mine, theirs) = (source.word(), source.other().word());
	let other_dir = match source {
		Source::Odis => at.join("vcds"),
		Source::Vcds => at.join("data").join(PROJECT),
	};

	// The channels a command resolves, through the stores and the selector.
	let extracted = vag_cli_core::extracted::current().for_car(Some(VIN));
	assert_eq!(extracted.run_source(), Some(source));
	let store = vag_data_labels::catalog::CatalogStore::open(at.join("no-proven-rows"));
	let rows = vag_cli_core::extracted::tagged(&store, &extracted, None, Some("EV_Unit"), Some("001001"));
	assert!(!rows.is_empty(), "{source}: its own channels");
	for row in &rows {
		assert!(row.def.name.starts_with(mine), "{source}: {row:?}");
		assert!(row.named.as_deref().is_none_or(|n| !n.contains(theirs)), "{source}: {row:?}");
	}
	// VCDS's text table never names an ODIS row, by any id.
	if source == Source::Odis {
		assert_eq!(extracted.name_of(Some("IDE00001")), None, "VCDS's names.json named an ODIS row");
		assert!(extracted.names().is_empty());
	}

	// Fault text: the run's half alone, and a language it lacks said at level 0.
	let keys = at.join("vcds").join("rod-keys.json");
	let namers = vag_cli_diag::faults::Namers::open(Some(source), &keys, false).unwrap();
	let said = format!(
		"{}\n{}\n{}\n{}",
		namers.summary(),
		namers.describe().join("\n"),
		namers.language_warning().join("\n"),
		namers.empty_note()
	);
	match source {
		Source::Odis => {
			assert!(namers.summary().starts_with("ODIS"), "{said}");
			assert_eq!(
				namers.language_warning(),
				["warning: no en fault text in the ODIS project (available: de)"],
				"VCDS's English counted as the project's: {said}"
			);
			assert!(!said.to_lowercase().contains("vcds-"), "{said}");
		}
		Source::Vcds => {
			assert!(namers.is_empty(), "no VCDS fault text in the fixture: {said}");
			assert_eq!(namers.none_because(), "the VCDS installation has no fault text");
			assert!(namers.empty_note().contains("`--source odis`"), "{said}");
		}
	}

	// Why a unit has no VCDS channels: a VCDS run's question alone.
	let causes = vag_cli_core::registry::causes(extracted.run_source(), &[engine()]);
	assert_eq!(causes.is_empty(), source == Source::Odis, "{source}: {causes:?}");

	// The dash build: its inputs are the run's store's, and its channel the run's source's.
	let resolved = vag_cli_core::dash::resolve_for_car(VIN, None).unwrap();
	// The rows proven on a car are kept in the project's `measurements/` — a measurement of
	// this car, not a source, and read on either run.
	for input in resolved.inputs.iter().filter(|i| !i.ends_with("measurements")) {
		assert!(!input.starts_with(&other_dir), "{source}: the build read {}", input.display());
	}
	assert_eq!(resolved.built.plan.channels.len(), 1);
	assert!(
		resolved
			.built
			.notes
			.iter()
			.any(|n| n.contains(&format!("channels from the {}", source.name()))),
		"{:?}",
		resolved.built.notes
	);
}

/// Run `source`'s scenario in a child of this binary, with its own home.
fn in_child(test: &str, source: Source) {
	if let Ok(asked) = std::env::var(CHILD) {
		if asked == source.word() {
			child(source);
		}
		return;
	}
	let home = tempfile::tempdir().unwrap();
	let mut command = Command::new(std::env::current_exe().unwrap());
	command
		.args([test, "--exact", "--nocapture", "--test-threads=1"])
		.env(CHILD, source.word())
		.env("HOME", home.path())
		.env(vag_cli_core::datadir::ROOT_ENV, home.path().join(".vagcan"))
		.env_remove(vag_cli_core::project::PROJECT_ENV)
		.env_remove(vag_cli_core::source::SOURCE_ENV)
		.stdin(Stdio::null());
	// The default reads ODIS; VCDS is asked for the way a shell or the firmware build asks.
	if source == Source::Vcds {
		command.env(vag_cli_core::source::SOURCE_ENV, "vcds");
	}
	let out = command.output().unwrap();
	assert!(
		out.status.success(),
		"{source} failed:\n--- stdout\n{}\n--- stderr\n{}",
		String::from_utf8_lossy(&out.stdout),
		String::from_utf8_lossy(&out.stderr)
	);
	assert!(
		String::from_utf8_lossy(&out.stdout).contains("1 passed"),
		"{}",
		String::from_utf8_lossy(&out.stdout)
	);
}

#[test]
fn an_odis_run_over_both_sources_reads_nothing_of_vcds() {
	in_child("an_odis_run_over_both_sources_reads_nothing_of_vcds", Source::Odis);
}

#[test]
fn a_vcds_run_over_both_sources_reads_nothing_of_odis() {
	in_child("a_vcds_run_over_both_sources_reads_nothing_of_odis", Source::Vcds);
}
