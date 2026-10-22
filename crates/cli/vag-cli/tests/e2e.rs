//! The e2e cases: `vagcan` as a person runs it, against a virtual car, compared with what
//! it printed last time (`tests/e2e/`, docs/virtual-car.md).
//!
//! Each directory under `tests/e2e/` is one car: its `car.toml` is the profile the sim plays
//! (`vag-uds-sim`), and its `*.md` and `*.toml` files are trycmd cases — literate terminal
//! sessions, and single commands whose stdout and stderr are checked apart. For every case
//! file this starts a sim of that car in-process and a fresh `~/.vagcan` (`VAGCAN_HOME`)
//! holding a made-up project, and hands the command its port in `VAGCAN_DEVICE`, so a case
//! reads `$ vagcan faults` and nothing more.
//!
//! **Run through `just e2e`, not `cargo test`.** The binary under test must be the one a
//! person installs, and the `target/debug/vagcan` a `cargo test` builds is a test build
//! (`vag-cli-core`'s `test-root`), which starts every run with a line about its data dir.
//! The recipe builds the real one, copies it aside and names it in `VAGCAN_E2E_BIN`; this
//! target is `test = false`, so the workspace's `cargo test` does not run it. `just
//! e2e-bless` rewrites what the cases expect (`TRYCMD=overwrite`).
//!
//! What varies between runs is never in a case: the port is `[PORT]`, the data dir `[HOME]`,
//! the version `[VERSION]` — trycmd's variables, each the exact text it stands for — and
//! anything else that moves is elided where it stands, with `[..]`. Nothing here is a fact
//! about any car: the units, part numbers, ODX names, channels and fault texts are made up.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where the cases are: `tests/e2e/` at the repository's root.
fn cases_root() -> PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/e2e")
}

/// The binary under test, as `just e2e` names it.
fn binary() -> PathBuf {
	let Some(path) = std::env::var_os("VAGCAN_E2E_BIN") else {
		panic!(
			"VAGCAN_E2E_BIN is not set: run the e2e cases with `just e2e`, which builds the vagcan a person installs \
             (a `cargo test` build is a test build, and says so on every run)"
		);
	};
	let path = PathBuf::from(path);
	assert!(path.is_file(), "VAGCAN_E2E_BIN={} is not a file", path.display());
	path
}

/// The made-up project every car's home holds, under `data/<PROJECT>/`.
const PROJECT: &str = "SIM00";

/// What an ODIS project names a unit: its variant, the logical link that reaches it, the
/// link's long name and the address the project's table of links gives it. Made up.
fn odis_unit(variant: &str, link: &str, name: &str, address: Option<u32>) -> vag_data_labels::odis::UnitName {
	vag_data_labels::odis::UnitName {
		variant: variant.to_string(),
		base_variant: format!("BV_{}", link.trim_start_matches("LL_")),
		link: link.to_string(),
		name: Some(name.to_string()),
		address,
	}
}

/// VCDS label files into the VCDS store's cache, as `setup` reads them, each named after the
/// part number it describes and saying which unit it is, `(file, address, name)`; the store's
/// source keeps the directory `source` names. Made up.
fn vcds_label_files(cache: &Path, source: &str, files: &[(&str, u8, &str)]) {
	let scratch = tempfile::tempdir().unwrap();
	let labels = scratch.path().join("Labels");
	std::fs::create_dir_all(&labels).unwrap();
	for (file, address, name) in files {
		std::fs::write(
			labels.join(format!("{file}.lbl")),
			format!("; Component: {name} (#{address:02X})\n001,1,Sim value,,\n"),
		)
		.unwrap();
	}
	vag_data_db::build_db(&labels, cache).unwrap();
	// Read from a folder that is gone by now: the source keeps the made-up one.
	rusqlite::Connection::open(cache)
		.unwrap()
		.execute("UPDATE source SET dir = ?1 WHERE kind = 'vcds'", [source])
		.unwrap();
}

/// A `~/.vagcan` with one made-up project: channels, fault texts and unit names for the made-up
/// ODX variants the cars' profiles name, as `vagcan setup` would have cached them from an ODIS
/// project, and the project named in `config.toml`.
fn home(at: &Path) {
	use vag_data_labels::catalog::Scaling;
	use vag_data_labels::measure::LinearScale;
	use vag_data_labels::odis::{Fault, Reading};
	let project = at.join("data").join(PROJECT);
	std::fs::create_dir_all(&project).unwrap();
	std::fs::write(at.join("config.toml"), format!("project = \"{PROJECT}\"\n")).unwrap();
	let cache = project.join("cache.sqlite");
	let reading = |did: u16, name: &str, unit: &str, bits: u32, factor: f64, offset: f64| Reading {
		did,
		name: name.to_string(),
		unit: Some(unit.to_string()),
		bit_offset: 0,
		bit_length: bits,
		signed: false,
		big_endian: true,
		scaling: Scaling::Linear(LinearScale { factor, offset }),
		text_id: None,
	};
	let source = "/nowhere/SIM00-odis";
	vag_data_db::put_readings(
		&cache,
		source,
		"EV_SimEngine_001",
		&[
			reading(0x1001, "Sim_oil_temperature", "°C", 8, 1.0, -40.0),
			reading(0x1002, "Sim_boost_pressure", "hPa", 16, 1.0, 0.0),
		],
	)
	.unwrap();
	let fault = |code: u32, text: &str| Fault {
		dop: "DTCDOP_SIM".into(),
		code,
		display_code: Some(format!("P{code:06X}")),
		text: Some(text.into()),
		text_id: None,
		short_name: None,
		level: 2,
		temporary: false,
	};
	vag_data_db::put_faults(
		&cache,
		source,
		"EV_SimEngine_001",
		&[fault(0x000129, "Made-up sensor A: signal too high")],
	)
	.unwrap();
	vag_data_db::put_faults(&cache, source, "EV_SimBrake_001", &[fault(0x047120, "Made-up valve B: open circuit")]).unwrap();
	vag_data_db::record_language(&cache, vag_data_db::ODIS, source, "eng").unwrap();
	// What the project calls the engine and the gearbox, and their numbers. The brake unit
	// names no ODX file, so the project names nothing of it.
	vag_data_db::put_unit_names(
		&cache,
		source,
		&[
			odis_unit("EV_SimEngine_001", "LL_SimEngine", "Sim Engine", Some(0x01)),
			odis_unit("EV_SimGear_001", "LL_SimGearbox", "Sim Gearbox", Some(0x02)),
		],
	)
	.unwrap();
	// Adaptation channels of the engine's and the gearbox's variants. The brake unit names no
	// ODX file, so nothing describes its adaptations; 0612 is described and not on the car.
	vag_data_db::put_all_adaptations(
		&cache,
		source,
		[
			(
				"EV_SimEngine_001",
				&[
					reading(0x0610, "Sim_idle_speed_offset", "rpm", 8, 10.0, 0.0),
					reading(0x0611, "Sim_service_interval", "km", 16, 100.0, 0.0),
					reading(0x0612, "Sim_not_on_the_car", "s", 8, 1.0, 0.0),
				][..],
			),
			("EV_SimGear_001", &[reading(0x0620, "Sim_clutch_adaptation", "%", 8, 0.5, 0.0)][..]),
		],
	)
	.unwrap();

	// The engine's fault records, as `setup` stores the variant's extended-data multiplexer:
	// the two standard records and an own record 0x70 of four bytes. Made up.
	{
		use vag_data_labels::odis::{EnvironmentField, EnvironmentRecord};
		let field = |name: &str, bit: u32, bits: u32, factor: f64, unit: Option<&str>| EnvironmentField {
			name: Some(name.to_string()),
			text_id: None,
			unit: unit.map(str::to_string),
			bit_offset: Some(bit),
			bit_length: Some(bits),
			signed: false,
			big_endian: true,
			scaling: Some(Scaling::Linear(LinearScale { factor, offset: 0.0 })),
			constant: false,
		};
		let record = |number: i32, length: u32, fields: Vec<EnvironmentField>| EnvironmentRecord {
			range: Some((number, number)),
			name: None,
			length: Some(length),
			fields,
		};
		let records = [
			record(1, 2, vec![field("Sim priority", 0, 8, 1.0, None), field("Sim count", 8, 8, 1.0, None)]),
			record(
				2,
				10,
				vec![field("Sim aging", 0, 8, 1.0, None), field("Sim mileage", 8, 24, 1.0, Some("km"))],
			),
			record(
				0x70,
				4,
				vec![
					field("Sim Engine Speed", 0, 16, 0.25, Some("1/min")),
					field("Sim Terminal 30", 16, 8, 0.1, Some("V")),
				],
			),
		];
		vag_data_db::put_all_odis_freeze_frames(&cache, source, [("EV_SimEngine_001", &records[..])]).unwrap();
	}
	// The demo car's dash: one value on one page, the engine's coolant temperature.
	let dash = at.join("dash").join("SIMVIN00000000001");
	std::fs::create_dir_all(&dash).unwrap();
	std::fs::write(
		dash.join("dash.toml"),
		"vin = \"SIMVIN00000000001\"\n\n[[channel]]\nref = \"01:F405\"\n\n[[page]]\nkind = \"values\"\ncells = [\"01:F405\"]\n",
	)
	.unwrap();
	// When the project was read, fixed: `setup --list` prints it.
	rusqlite::Connection::open(&cache)
		.unwrap()
		.execute("UPDATE source SET parsed_at = '2026-10-21T00:00:00.000Z'", [])
		.unwrap();
}

/// The car whose home holds two sources, one with a path too long for 80 columns beside its
/// row: `setup --list` puts each path under its row and keeps every row.
const TWO_SOURCES: &str = "two-sources";

/// A `~/.vagcan` with the VCDS store and one ODIS project, both read at a fixed time, each
/// describing the engine its own way — one source per run, so a case shows which one a run
/// read (CLAUDE.md, "ODIS and VCDS are independent sources"). Made-up paths, names and rows:
///
/// - the ODIS project: `0x1001` as `Odis_oil_temperature` (×1 −40), the engine's fault
///   `000129` with its text, the engine's record 70 as `Odis Engine Speed`, and the engine
///   and the brakes named and numbered (`01`, `03`);
/// - the VCDS store: `0x1001` as `Vcds oil temperature` (×0.5 −20), `0x1003` that ODIS does
///   not describe, the engine's record 70 as `Vcds engine speed`, and a registry log saying
///   the car's units were read — no fault text (no `.rod` pool) — and label files naming and
///   numbering the engine and the gearbox (`01`, `02`);
/// - the car's dash, `dash.toml`, reading `01:1001` and `01:1003`.
fn two_sources_home(at: &Path) {
	use vag_data_labels::catalog::Scaling;
	use vag_data_labels::measure::LinearScale;
	use vag_data_labels::odis::{EnvironmentField, EnvironmentRecord, Fault, Reading};
	let store = at.join("vcds");
	let project = at.join("data").join(PROJECT);
	let vcds = "/nowhere/a-long-way-down/to-a-vcds-installation/Labels";
	let odis = "/nowhere/SIM00-odis";
	std::fs::create_dir_all(&store).unwrap();
	// VCDS names the engine and the gearbox, each by its part number; not the brakes.
	vcds_label_files(
		&store.join("cache.sqlite"),
		vcds,
		&[("SIM906000A", 0x01, "Vcds Engine"), ("SIM300000B", 0x02, "Vcds Gearbox")],
	);
	for (dir, kind, source) in [(&store, vag_data_db::VCDS, vcds), (&project, vag_data_db::ODIS, odis)] {
		std::fs::create_dir_all(dir).unwrap();
		vag_data_db::record_language(&dir.join("cache.sqlite"), kind, source, "eng").unwrap();
	}
	// ODIS names the engine and the brakes, each by what its `F19E` says it is; not the
	// gearbox, which names no ODX file.
	vag_data_db::put_unit_names(
		&project.join("cache.sqlite"),
		odis,
		&[
			odis_unit("EV_SimEngine_001", "LL_SimEngine", "Odis Engine", Some(0x01)),
			odis_unit("EV_SimBrake_001", "LL_SimBrake", "Odis Brakes", Some(0x03)),
		],
	)
	.unwrap();
	let reading = |did: u16, name: &str, factor: f64, offset: f64| Reading {
		did,
		name: name.to_string(),
		unit: Some("°C".to_string()),
		bit_offset: 0,
		bit_length: 8,
		signed: false,
		big_endian: true,
		scaling: Scaling::Linear(LinearScale { factor, offset }),
		text_id: None,
	};
	let odis_cache = project.join("cache.sqlite");
	vag_data_db::put_readings(
		&odis_cache,
		odis,
		"EV_SimEngine_001",
		&[reading(0x1001, "Odis_oil_temperature", 1.0, -40.0)],
	)
	.unwrap();
	let fault = Fault {
		dop: "DTCDOP_SIM".into(),
		code: 0x000129,
		display_code: Some("P000129".into()),
		text: Some("Made-up sensor A: signal too high".into()),
		text_id: None,
		short_name: None,
		level: 2,
		temporary: false,
	};
	vag_data_db::put_faults(&odis_cache, odis, "EV_SimEngine_001", &[fault]).unwrap();
	let field = |name: &str, bit: u32, bits: u32, factor: f64, unit: Option<&str>| EnvironmentField {
		name: Some(name.to_string()),
		text_id: None,
		unit: unit.map(str::to_string),
		bit_offset: Some(bit),
		bit_length: Some(bits),
		signed: false,
		big_endian: true,
		scaling: Some(Scaling::Linear(LinearScale { factor, offset: 0.0 })),
		constant: false,
	};
	let record = |number: i32, length: u32, fields: Vec<EnvironmentField>| EnvironmentRecord {
		range: Some((number, number)),
		name: None,
		length: Some(length),
		fields,
	};
	let records = [
		record(1, 2, vec![field("Sim priority", 0, 8, 1.0, None), field("Sim count", 8, 8, 1.0, None)]),
		record(
			2,
			10,
			vec![field("Sim aging", 0, 8, 1.0, None), field("Sim mileage", 8, 24, 1.0, Some("km"))],
		),
		record(0x70, 4, vec![field("Odis Engine Speed", 0, 16, 0.25, Some("1/min"))]),
	];
	vag_data_db::put_all_odis_freeze_frames(&odis_cache, odis, [("EV_SimEngine_001", &records[..])]).unwrap();

	let vcds_cache = store.join("cache.sqlite");
	vag_data_db::put_all_vcds_readings(
		&vcds_cache,
		vcds,
		[(
			"EV_SimEngine",
			&[
				reading(0x1001, "Vcds oil temperature", 0.5, -20.0),
				reading(0x1003, "Vcds intake temperature", 1.0, -40.0),
			][..],
		)],
	)
	.unwrap();
	let row = |bit: u32, bits: u32, kind: u8, scale: Option<(f64, f64)>, unit: Option<&str>, name: &str| {
		use vag_data_labels::registries::{Described, Entry, Value};
		Described {
			section: vag_data_db::FREEZE_FRAME_SECTION,
			entry: Entry {
				row: 1,
				key: 0x70,
				identifier: None,
				ordinal: None,
				lead: Some(104),
				case: None,
				value: Value {
					kind: Some(kind),
					big_endian: true,
					bit_offset: Some(bit),
					bit_length: Some(bits),
					scale,
					..Default::default()
				},
				raw: String::new(),
			},
			name: Some(name.to_string()),
			text_id: None,
			unit: unit.map(str::to_string),
			levels: Vec::new(),
		}
	};
	let rows = [
		row(0, 16, 0, Some((0.25, 0.0)), Some("/min"), "Vcds engine speed"),
		row(16, 8, 0, Some((0.1, 0.0)), Some("V"), "Vcds supply voltage"),
		row(24, 8, 11, None, None, "Reserved"),
	];
	vag_data_db::put_all_vcds_registry_rows(&vcds_cache, vcds, [("EV_SimEngine", &rows[..])]).unwrap();
	// The car's units were read from the installation: a VCDS run has nothing left to read.
	std::fs::write(
		store.join("registry.json"),
		r#"{"installation": "/nowhere/a-long-way-down/to-a-vcds-installation", "read_version": 3, "units": [
			{"odx_name": "EV_SimEngine", "odx_version": "001001", "outcome": "read", "stem": "EV_SimEngine"},
			{"odx_name": "EV_SimBrake", "odx_version": "001001", "outcome": "no file or redirect"}]}"#,
	)
	.unwrap();
	for dir in [&store, &project] {
		rusqlite::Connection::open(dir.join("cache.sqlite"))
			.unwrap()
			.execute("UPDATE source SET parsed_at = '2026-10-21T00:00:00.000Z'", [])
			.unwrap();
	}
	let dash = at.join("dash").join("SIMVIN00000000002");
	std::fs::create_dir_all(&dash).unwrap();
	std::fs::write(
		dash.join("dash.toml"),
		"vin = \"SIMVIN00000000002\"\n\n[[channel]]\nref = \"01:1001\"\n\n[[channel]]\nref = \"01:1003\"\n\n\
		 [[page]]\nkind = \"values\"\ncells = [\"01:1001\", \"01:1003\"]\n",
	)
	.unwrap();
}

/// The car whose home holds a VCDS install alone, no ODIS project: each source is tested on
/// its own (CLAUDE.md, "ODIS and VCDS are independent sources").
const VCDS_ONLY: &str = "vcds-only";

/// A `~/.vagcan` set up from a VCDS install alone: the machine's VCDS store holds the engine's
/// freeze-frame layout, as `setup` stores `RF.rod`'s rows, and a label file naming the engine;
/// no ODIS project.
fn vcds_home(at: &Path) {
	let store = at.join("vcds");
	std::fs::create_dir_all(&store).unwrap();
	let cache = store.join("cache.sqlite");
	// Record 0x70, four bytes — an engine speed, a voltage, a reserved byte. Made up.
	let vcds = "/nowhere/sim-vcds/Labels";
	// VCDS names the engine by its part number.
	vcds_label_files(&cache, vcds, &[("SIM906000A", 0x01, "Sim Engine (VCDS)")]);
	vag_data_db::record_language(&cache, vag_data_db::VCDS, vcds, "eng").unwrap();
	let row = |bit: u32, bits: u32, kind: u8, scale: Option<(f64, f64)>, unit: Option<&str>, name: &str| {
		use vag_data_labels::registries::{Described, Entry, Value};
		Described {
			section: vag_data_db::FREEZE_FRAME_SECTION,
			entry: Entry {
				row: 1,
				key: 0x70,
				identifier: None,
				ordinal: None,
				lead: Some(104),
				case: None,
				value: Value {
					kind: Some(kind),
					big_endian: true,
					bit_offset: Some(bit),
					bit_length: Some(bits),
					scale,
					..Default::default()
				},
				raw: String::new(),
			},
			name: Some(name.to_string()),
			text_id: None,
			unit: unit.map(str::to_string),
			levels: Vec::new(),
		}
	};
	let rows = [
		row(0, 16, 0, Some((0.25, 0.0)), Some("/min"), "Sim engine speed"),
		row(16, 8, 0, Some((0.1, 0.0)), Some("V"), "Sim supply voltage"),
		row(24, 8, 11, None, None, "Reserved"),
	];
	vag_data_db::put_all_vcds_registry_rows(&cache, vcds, [("EV_SimEngine_001", &rows[..])]).unwrap();
}

/// Copy the directory `from` to `to`, recursively.
fn copy(from: &Path, to: &Path) {
	std::fs::create_dir_all(to).unwrap();
	for entry in std::fs::read_dir(from).unwrap().flatten() {
		let target = to.join(entry.file_name());
		match entry.file_type().unwrap().is_dir() {
			true => copy(&entry.path(), &target),
			false => {
				std::fs::copy(entry.path(), target).unwrap();
			}
		}
	}
}

/// The case files of one car, in name order.
fn case_files(car: &Path) -> Vec<PathBuf> {
	let mut files: Vec<PathBuf> = std::fs::read_dir(car)
		.unwrap()
		.flatten()
		.map(|e| e.path())
		.filter(|p| {
			let name = p.file_name().unwrap().to_string_lossy();
			match p.extension().and_then(|e| e.to_str()) {
				Some("md") => true,
				// `car.toml` is the profile, not a case.
				Some("toml") => name != "car.toml",
				_ => false,
			}
		})
		.collect();
	files.sort();
	files
}

/// Run one case file against a sim of `profile`, in a home of its own.
fn run_case(bin: &Path, profile: &Path, template: &Path, case: &Path) {
	let text = std::fs::read_to_string(profile).unwrap();
	let car = vag_uds_sim::Profile::parse(&text)
		.and_then(|p| p.resolve())
		.unwrap_or_else(|e| panic!("{}: {e}", profile.display()));
	let sim = vag_uds_sim::Sim::start(car, |_| {}).expect("a pseudo-terminal for the sim");
	let scratch = tempfile::tempdir().unwrap();
	let home = scratch.path().join(".vagcan");
	copy(template, &home);
	let port = sim.path().display().to_string();
	let cases = trycmd::TestCases::new();
	cases
		.register_bin("vagcan", bin)
		.case(case)
		.timeout(Duration::from_secs(60))
		// A variable set here wins over the same one a case's `$ VAR=value vagcan …` sets
		// (trycmd's order), so nothing is set here that a case varies: a case names another
		// port with `--device`, and a width with `VAGCAN_COLUMNS=`, left unset here.
		.env("VAGCAN_DEVICE", &port)
		.env("VAGCAN_HOME", home.display().to_string())
		.env("HOME", scratch.path().display().to_string())
		// What a run may read from the person's environment, made the same everywhere. A home
		// with no project (VCDS alone) names none: empty is read as unset.
		.env("VAGCAN_PROJECT", if template.join("data").join(PROJECT).is_dir() { PROJECT } else { "" })
		.env("TZ", "UTC")
		.env("LANG", "C")
		.env("NO_COLOR", "1");
	cases
		.insert_var("[PORT]", port)
		.unwrap()
		.insert_var("[HOME]", home.display().to_string())
		.unwrap()
		.insert_var("[VERSION]", env!("CARGO_PKG_VERSION"))
		.unwrap();
	cases.run();
	drop(sim);
}

#[test]
fn e2e() {
	let bin = binary();
	let template = tempfile::tempdir().unwrap();
	home(template.path());
	let vcds_template = tempfile::tempdir().unwrap();
	vcds_home(vcds_template.path());
	let two_template = tempfile::tempdir().unwrap();
	two_sources_home(two_template.path());
	let root = cases_root();
	let mut cars: Vec<PathBuf> = std::fs::read_dir(&root)
		.unwrap()
		.flatten()
		.map(|e| e.path())
		.filter(|p| p.join("car.toml").is_file())
		.collect();
	cars.sort();
	assert!(!cars.is_empty(), "no car under {}", root.display());
	let mut failed = Vec::new();
	for car in &cars {
		for case in case_files(car) {
			let home = match car.file_name().and_then(|n| n.to_str()) {
				Some(VCDS_ONLY) => vcds_template.path(),
				Some(TWO_SOURCES) => two_template.path(),
				_ => template.path(),
			};
			let ran = std::panic::catch_unwind(|| run_case(&bin, &car.join("car.toml"), home, &case));
			if ran.is_err() {
				failed.push(case.strip_prefix(&root).unwrap_or(&case).display().to_string());
			}
		}
	}
	assert!(
		failed.is_empty(),
		"e2e cases that failed: {failed:?} — `just e2e-bless` takes the new output, after reading the diff"
	);
}
