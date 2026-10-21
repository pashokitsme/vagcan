//! `vagcan dev vcds …`, every tool through `vcds::run` as the dispatcher calls it, at
//! levels 0, 1 and 2: the data on stdout is the same at each, a level only adds to
//! stderr, and no data line is on stderr. The level-0 stdout is pinned where it is short.
//!
//! The inputs are made here, in a temporary directory: two invented label files, a `.rod`
//! with no sections, a names file, a capture and a VCDS log of one invented scaling.
//! Nothing here is a fact about any car.
//!
//! **Its own test binary, on purpose.** `labels --part` builds the project's label cache
//! under `~/.vagcan`, which no test may write into, so this process points `HOME` at a
//! temporary directory before anything reads it — a process-wide change, safe only while
//! one test runs, which a binary with one test guarantees. One test also keeps the
//! captures from overlapping: a capture is process-wide, and a test in another binary
//! writing while it runs cannot reach this process.

use std::path::Path;

use vag_cli_core::verbosity::{Capture, Captured, assert_split};
use vag_cli_diag::device::DeviceArg;
use vag_cli_diag::vcds::{self, Tool};

/// `tool` run at levels 0, 1 and 2, each run checked to finish.
fn levels(what: &str, tool: impl Fn() -> Tool) -> Vec<Captured> {
	let runs: Vec<Captured> = (0..=2)
		.map(|level| {
			let capture = Capture::start(level);
			let done = vcds::run(tool());
			let captured = capture.finish();
			assert!(done.is_ok(), "{what} at level {level}: {:?}", done.err());
			captured
		})
		.collect();
	assert_split(what, &runs);
	runs
}

fn labels(dir: &Path, part: Option<&str>, block: Option<u16>, odx: Option<&str>, keys: &Path) -> Tool {
	Tool::Labels {
		dir: dir.to_string_lossy().into_owned(),
		part: part.map(str::to_owned),
		block,
		field: None,
		odx: odx.map(str::to_owned),
		from_car: false,
		ecu: "01".into(),
		refresh: false,
		iv_cache: Some(keys.to_string_lossy().into_owned()),
		device: DeviceArg::default(),
	}
}

/// A capture of one identifier, `F40C` on `7E0`, read every 0.1 s and carrying
/// `3200 + 8·i` as two big-endian bytes, anchored on a whole second; and a VCDS log of
/// the same value at 0.25 per bit, every 0.5 s, beside one that never moves.
fn session(dir: &Path) -> (String, String) {
	use vag_uds_capture::{CapturePayload, CaptureRecord, Direction, wall_clock_anchor, write_records};
	use vag_uds_transport::CanId;

	let anchor: u64 = 1_785_536_240_000_000;
	let mut records = vec![CaptureRecord {
		ts_us: 0,
		dir: Direction::Rx,
		payload: CapturePayload::Marker {
			note: wall_clock_anchor(anchor),
		},
	}];
	for i in 0..220u64 {
		let raw = (3200 + 8 * i as u16).to_be_bytes();
		let frame = |ts_us: u64, id: u16, data: Vec<u8>| CaptureRecord {
			ts_us,
			dir: Direction::Rx,
			payload: CapturePayload::CanFrame {
				id: CanId::Standard(id),
				data,
			},
		};
		records.push(frame(i * 100_000, 0x7E0, vec![0x03, 0x22, 0xF4, 0x0C, 0, 0, 0, 0]));
		records.push(frame(i * 100_000 + 10, 0x7E8, vec![0x05, 0x62, 0xF4, 0x0C, raw[0], raw[1], 0, 0]));
	}
	let capture = dir.join("capture.jsonl");
	write_records(std::fs::File::create(&capture).unwrap(), &records).unwrap();

	// The log's header names a time of day to the second, read as the middle of that
	// second: the anchor's own second puts the log's t = 0 at 0.5 s into the capture.
	let offset_at_midnight = vag_cli_core::analyse::log_offset_seconds(anchor, (0, 0, 0)).unwrap();
	let day = (0.5 - offset_at_midnight).round() as u32;
	let mut log = format!(
		"Sun,12,Jul,2026,{:02}:{:02}:{:02}:00009-VCID:0\r\nTEST PART,ADVMB,x,\r\n,,G004,F0,\r\n\r\n\
		 Marker,TIME,Loc. IDE00405,TIME,Loc. IDE00155,\r\n,STAMP,Engine speed,STAMP,Angle,\r\n,, /min,, deg,\r\n",
		day / 3600,
		day / 60 % 60,
		day % 60
	);
	for k in 0..40 {
		let t = k as f64 * 0.5;
		let value = (3200.0 + (t + 0.5) / 0.1 * 8.0) * 0.25;
		log.push_str(&format!(",{t:.2},{value},{t:.2},0,\r\n"));
	}
	let log_path = dir.join("log.CSV");
	std::fs::write(&log_path, log).unwrap();
	(capture.to_string_lossy().into_owned(), log_path.to_string_lossy().into_owned())
}

#[test]
fn every_dev_vcds_tool_keeps_data_on_stdout_at_every_level() {
	let here = tempfile::tempdir().unwrap();
	let home = here.path().join("home");
	let project = home.join(".vagcan/data/TEST");
	std::fs::create_dir_all(&project).unwrap();
	std::fs::write(home.join(".vagcan/config.toml"), "project = \"TEST\"\n").unwrap();
	// SAFETY: the one test of this binary, before any thread is started or `HOME` read.
	unsafe {
		std::env::set_var("HOME", &home);
		// The override a test build's data dir answers to; HOME alone it does not read.
		std::env::set_var(vag_cli_core::datadir::ROOT_ENV, home.join(".vagcan"));
	}

	let vcds_dir = here.path().join("vcds");
	let label_dir = vcds_dir.join("Labels");
	std::fs::create_dir_all(&label_dir).unwrap();
	std::fs::write(label_dir.join("022-906-032-C.lbl"), b"001,1,Engine Speed,,Range: 0...6500 RPM").unwrap();
	std::fs::write(label_dir.join("022-906-032-D.lbl"), b"001,1,Vehicle Speed,,Range: 0...300 km/h").unwrap();
	std::fs::write(vcds_dir.join("EV_TEST.rod"), b"no sections in here").unwrap();
	let keys = here.path().join("rod-keys.json");

	// labels: the inventory, a part, a block, an ODX name found and not found.
	let inventory = levels("labels", || labels(&vcds_dir, None, None, None, &keys));
	assert_eq!(
		inventory[0].stdout,
		"what  count\n.lbl      2\n.clb      0\n.rod      1\n\n3 files · 2 label files parsed\n"
	);
	assert_eq!(inventory[0].stderr, "");
	assert!(inventory[1].stderr.contains("counted and parsed in"), "{}", inventory[1].stderr);

	let part = levels("labels --part --block", || labels(&vcds_dir, Some("022-906-032-C"), Some(1), None, &keys));
	assert_eq!(
		part[0].stdout,
		"part  022-906-032-C\n\
		 file  022-906-032-C.lbl\n\
		 \n\
		 block  field  name          phys\n\
		 \x20   1      1  Engine Speed  RPM\n\
		 \n\
		 1 measurement\n\
		 \n\
		 file               block  field  name           phys\n\
		 022-906-032-C.lbl      1      1  Engine Speed   RPM\n\
		 022-906-032-D.lbl      1      1  Vehicle Speed  km/h\n\
		 \n\
		 2 definitions · block 1\n"
	);
	assert_eq!(part[0].stderr, "", "the cache built on the way is a note");
	assert!(part[1].stderr.contains("label cache"), "{}", part[1].stderr);
	// No project here, so no cache to check (`labels::look_into`): the files are parsed and
	// nothing is written, and -v says so.
	assert!(
		part[1].stderr.contains("label cache not used") && part[1].stderr.contains("nothing written"),
		"{}",
		part[1].stderr
	);

	let odx = levels("labels --odx", || labels(&vcds_dir, None, None, Some("EV_TEST"), &keys));
	assert_eq!(
		odx[0].stdout,
		"file         section  state        bytes\nEV_TEST.rod           no sections\n\n0/0 sections open · 1 file\n"
	);
	assert!(odx[1].stderr.contains("1 file named EV_TEST.rod"), "{}", odx[1].stderr);
	// Not an install: the fix is to pass one.
	let miss = levels("labels --odx, no such file", || labels(&vcds_dir, None, None, Some("EV_NOTHING"), &keys));
	assert_eq!(miss[0].stdout, "odx   EV_NOTHING\nfile  none\n");
	assert_eq!(
		miss[0].stderr,
		format!(
			"no EV_NOTHING.rod under {}\n  (pass the VCDS install root, the directory that holds UDS_EV/)\n",
			vcds_dir.display()
		)
	);
	// The install root, passed: telling its owner to pass it would be wrong.
	let install = here.path().join("install");
	std::fs::create_dir_all(install.join("UDS_EV")).unwrap();
	let miss = levels("labels --odx, an install without the file", || {
		labels(&install, None, None, Some("EV_NOTHING"), &keys)
	});
	assert_eq!(miss[0].stderr, format!("no EV_NOTHING.rod under {}\n", install.display()));

	// names: a hit, the limit, a miss.
	let names_file = here.path().join("names.json");
	std::fs::write(
		&names_file,
		r#"{"000017": "Invented manifold pressure", "000097": "Invented target distance", "012389": "Lid switch"}"#,
	)
	.unwrap();
	let names = |text: &str| {
		let text = text.to_string();
		let catalog = names_file.to_string_lossy().into_owned();
		move || Tool::Names {
			text: text.clone(),
			limit: 1,
			catalog: Some(catalog.clone()),
		}
	};
	let hits = levels("names", names("invented"));
	assert_eq!(
		hits[0].stdout,
		"id      name\n000017  Invented manifold pressure\n\n2 of 3 names match · first 1 shown (--limit)\n"
	);
	assert_eq!(hits[0].stderr, "");
	assert!(hits[1].stderr.contains("not by data identifier"), "{}", hits[1].stderr);
	assert!(hits[2].stderr.contains("2 of 3 match"), "{}", hits[2].stderr);
	assert_eq!(levels("names, no match", names("boost"))[0].stdout, "0 of 3 names match\n");

	// analyse: the one scaling proven, the constant not.
	let (capture, log) = session(here.path());
	let out = here.path().join("catalog.json");
	let analyse = levels("analyse", || Tool::Analyse {
		capture: capture.clone(),
		log: log.clone(),
		out: Some(out.to_string_lossy().into_owned()),
		min_r2: 0.995,
		min_points: 20,
	});
	let rows: Vec<&str> = analyse[0].stdout.lines().collect();
	assert_eq!(rows.len(), 5, "{}", analyse[0].stdout);
	assert!(rows[0].starts_with("ide       phys  range"), "{}", rows[0]);
	assert!(
		rows[1].starts_with("IDE00405  /min") && rows[1].contains("7E8/F40C  U16Be  0.250000") && rows[1].ends_with("proven"),
		"{}",
		rows[1]
	);
	assert!(rows[2].starts_with("IDE00155  deg") && rows[2].ends_with("never moved"), "{}", rows[2]);
	assert_eq!(rows[3], "", "one blank line between the table and its summary");
	assert_eq!(rows[4], "1 of 2 measurements proven");
	assert_eq!(analyse[0].stderr, format!("wrote 1 catalog row to {}\n", out.display()));
	assert!(
		analyse[1].stderr.contains("the log starts +0.5 s into the capture"),
		"{}",
		analyse[1].stderr
	);
	assert!(analyse[1].stderr.contains("bar: R² ≥ 0.995"), "{}", analyse[1].stderr);

	// rod: a file with no sections; the key cache it names is the temp one.
	let rod = levels("rod", || Tool::Rod {
		file: vcds_dir.join("EV_TEST.rod").to_string_lossy().into_owned(),
		no_crack: true,
		cache: Some(keys.to_string_lossy().into_owned()),
		dump: None,
	});
	assert!(
		rod[0]
			.stdout
			.ends_with("section  state  bytes  key  preview\n\n0/0 sections open · 0 keys recovered\n"),
		"{}",
		rod[0].stdout
	);
	assert!(
		rod[0].stdout.starts_with(&format!("file   {}", vcds_dir.join("EV_TEST.rod").display())),
		"{}",
		rod[0].stdout
	);
	assert_eq!(rod[0].stderr, "");
	assert!(rod[1].stderr.contains("0 keys in"), "{}", rod[1].stderr);

	// dump: the whole label directory, into a file outside it.
	let json = here.path().join("labels.json");
	let dump = levels("dump", || Tool::Dump {
		dir: label_dir.to_string_lossy().into_owned(),
		out: Some(json.to_string_lossy().into_owned()),
	});
	assert!(dump[0].stdout.starts_with("what   count\n.lbl       2\n"), "{}", dump[0].stdout);
	assert!(dump[0].stdout.contains("\nlong_coding      0\n"), "{}", dump[0].stdout);
	assert!(dump[0].stdout.ends_with("\n\n2 label files parsed\n"), "{}", dump[0].stdout);
	assert_eq!(dump[0].stderr, format!("wrote 2 label files to {}\n", json.display()));

	// tttext: one record that reads, one that does not.
	let txt = here.path().join("TXT.bin");
	let section: String = [(17u32, "Invented flap position,2,90001"), (5, "no separator")]
		.iter()
		.map(|(id, plain)| format!("{id:06},{}\r\n", vag_data_labels::glyphs::TableAlphabet::for_key(*id).encipher(plain)))
		.collect();
	std::fs::write(&txt, section).unwrap();
	let tttext = levels("tttext", || Tool::Tttext {
		file: txt.to_string_lossy().into_owned(),
		out: None,
		catalog: None,
	});
	assert_eq!(tttext[0].stdout, "000017\tInvented flap position\t2\t90001\n");
	assert_eq!(tttext[0].stderr, "1 record · 1 with an IDE or MAS id · 1 malformed, left out\n");
	assert!(tttext[1].stderr.contains("000005 read as"), "{}", tttext[1].stderr);

	// stats: the VCDS store's cache, built as `setup` builds it — `labels` reads a cache and
	// never writes one (#37).
	vag_cli_diag::labels::load_cached(&vcds_dir, &home.join(".vagcan/vcds/cache.sqlite"), false).unwrap();
	let stats = levels("stats", || Tool::Stats { catalog: None });
	assert_eq!(
		stats[0].stdout,
		"what            count\nlabel_file          2\nmeasurement         2\nredirect            0\nadaptation          0\nlong_coding         0\nregistry_row        0\nregistry_level      0\n"
	);
	assert_eq!(stats[0].stderr, "");
	assert!(
		stats[1].stderr.contains("label cache") && stats[1].stderr.contains("source vcds"),
		"{}",
		stats[1].stderr
	);
}
