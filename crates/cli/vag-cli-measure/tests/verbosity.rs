//! What `measure view` and `measure setup` write, at each `-v` level, through the
//! process's own stdout and stderr.
//!
//! Its own binary: a [`Capture`] takes every write in the process while it runs, so a
//! test beside it that printed anything at the same moment would land in its output.
//! Every test here captures, one at a time, and nothing else runs in this binary.

use std::io::Cursor;

use vag_cli_core::verbosity::{Capture, Captured, assert_split};
use vag_cli_measure::carfile::{CarFile, Source};
use vag_cli_measure::coastdown::Fit;
use vag_cli_measure::setup::{self, Console, KeptPass};

/// A session with three runs on it — the crate's own fixture, no car driven for it.
const SAMPLE: &str = include_str!("../src/view_sample.json");

/// `run` at levels 0, 1 and 2, each under its own capture.
fn at_each_level<T>(mut run: impl FnMut() -> T) -> (Vec<T>, Vec<Captured>) {
	let mut results = Vec::new();
	let mut captured = Vec::new();
	for level in 0..=2 {
		let capture = Capture::start(level);
		results.push(run());
		captured.push(capture.finish());
	}
	(results, captured)
}

#[test]
fn view_writes_the_session_and_the_page_on_stdout_and_the_rest_at_v() {
	let dir = tempfile::tempdir().unwrap();
	let input = dir.path().join("drive.json");
	std::fs::write(&input, SAMPLE).unwrap();
	let path = input.to_string_lossy().to_string();

	let (pages, runs) = at_each_level(|| vag_cli_measure::write_view(&path).expect("the sample is a session"));
	assert_split("measure view", &runs);
	let page = dir.path().join("drive.html");
	assert!(pages.iter().all(|written| *written == page));
	assert_eq!(runs[0].stdout, format!("session  {path}\npage     {}\n", page.display()));
	assert_eq!(runs[0].stderr, "", "nothing but the data at level 0");
	assert_eq!(
		runs[1].stderr,
		"3 runs · 3 re-derived from the samples by this build · 0 kept as recorded\n"
	);
	assert_eq!(
		runs[2].stderr,
		"3 runs · 3 re-derived from the samples by this build · 0 kept as recorded\n\
		 run 1  re-derived\n\
		 run 2  re-derived\n\
		 run 3  re-derived\n"
	);
}

#[test]
fn view_refuses_another_schema_in_two_lines_with_the_reason_at_v() {
	let dir = tempfile::tempdir().unwrap();
	let input = dir.path().join("future.json");
	std::fs::write(&input, r#"{"schema": 7, "runs": []}"#).unwrap();
	let path = input.to_string_lossy().to_string();

	let (errors, runs) = at_each_level(|| {
		let refused = vag_cli_measure::write_view(&path).expect_err("schema 7 is refused");
		vag_cli_core::report::render(&refused, 0, vag_cli_core::report::Colour::Never)
	});
	assert_split("measure view, another schema", &runs);
	assert_eq!(
		errors[0],
		format!("error: {path} is schema 7, and this build reads schema 1\n\nhelp: open it with a build that reads schema 7\n")
	);
	assert_eq!(runs[0], Captured::default(), "the error is the caller's to print");
	assert!(runs[1].stderr.contains("cannot be regenerated"), "{}", runs[1].stderr);
	assert!(!dir.path().join("future.html").exists(), "a page was written for a refused session");
}

#[test]
fn the_interview_asks_on_stderr_and_explains_itself_at_v() {
	let (cars, runs) = at_each_level(|| {
		let mut io = Console::reading(Cursor::new("1395\ny\n80\n205/55R16\n"));
		let mut car = CarFile::new("XW8AD4NE9JH008917");
		setup::interview(&mut io, &mut car, "2026-10-20").expect("every question answered");
		car
	});
	assert_split("measure setup, the interview", &runs);
	assert!(cars.iter().all(|car| car.mass_total_kg() == Some(1475.0)));
	assert_eq!(runs[0].stdout, "", "an interview has no data of its own");
	// The questions, as typed at: the answer and its Enter end each line on a terminal.
	let asked = [
		"  mass in running order — EU field G, \"mass in service\" on a V5C, in kg: a plain number like 1450, between 300 and 5000? ",
		"  does that figure already include a driver — an EU field G does (yes or no; Enter alone means yes) [yes] ",
		"  anything else aboard — passengers, luggage, and your own difference from 75 kg, in kg: a plain number like 80, \
         between -200 and 2000, Enter alone means 0 kg? [0] ",
		"    1395 kg stated + 80 kg aboard  =  1475 kg   (the document's 75 kg driver is already in it)\n",
		"  tyre size, exactly as written on the sidewall — width/aspect then the rim, e.g. 205/55R16? ",
	];
	assert_eq!(runs[0].stderr, asked.concat());
	assert!(runs[1].stderr.contains("counts you twice"), "{}", runs[1].stderr);
	assert!(runs[1].stderr.contains("moulded into the sidewall"), "{}", runs[1].stderr);
}

/// A car the interview described, and two passes with these rolling resistances: what
/// `conclude` fits.
fn described_and_driven(dir: &std::path::Path, crr: (f64, f64)) -> (CarFile, std::path::PathBuf, Vec<KeptPass>, f64) {
	let mut io = Console::reading(Cursor::new("1395\ny\n80\n205/55R16\n"));
	let mut car = CarFile::new("XW8AD4NE9JH008917");
	let capture = Capture::start(0);
	setup::interview(&mut io, &mut car, "2026-10-20").unwrap();
	drop(capture);
	let pass = |crr: f64| KeptPass {
		fit: Fit {
			cda: 0.63,
			crr,
			rms_kmh: 0.21,
			mean_speed_ms: 22.3,
		},
		at: "2026-10-20".into(),
		seconds: 38.2,
		rho: 1.2,
		rho_source: Source::Measured,
		mass_kg: 1475.0,
	};
	let delta1 = setup::delta1(&car).unwrap();
	(car, dir.join("car.json"), vec![pass(crr.0), pass(crr.1)], delta1)
}

#[test]
fn a_fit_that_holds_is_the_car_file_on_stdout_and_how_it_was_judged_at_v() {
	let dir = tempfile::tempdir().unwrap();
	let (car, path, kept, delta1) = described_and_driven(dir.path(), (0.0114, 0.0114));
	let (_, runs) = at_each_level(|| {
		let mut car = car.clone();
		setup::conclude(&mut Console::reading(Cursor::new("")), &mut car, &path, &kept, delta1, "2026-10-20").unwrap();
	});
	assert_split("measure setup, the fit", &runs);
	assert_eq!(
		runs[0].stdout,
		format!(
			"car file {}\n\
			 fitted at 1.200 kg/m³ measured · 1475 kg\n\
			 \n\
			 FIGURES\n\
			 \x20 mass  1475 kg      you, 2026-10-20\n\
			 \x20 tyre  205/55R16    you, 2026-10-20\n\
			 \x20 CdA   0.63 m²      measured on this car, 2 passes\n\
			 \x20 Crr   0.0114       measured on this car\n",
			vag_cli_core::datadir::tilde(&path)
		)
	);
	assert_eq!(runs[0].stderr, "power is available now: vagcan measure --full\n");
	assert!(runs[1].stderr.contains("rms off its curve"), "{}", runs[1].stderr);
	assert!(CarFile::load(&path).unwrap().road_load().is_ok(), "the road load was not written");
}

#[test]
fn a_fit_that_does_not_hold_writes_nothing_and_says_why_in_two_lines() {
	let dir = tempfile::tempdir().unwrap();
	let (car, path, kept, delta1) = described_and_driven(dir.path(), (0.001, 0.08));
	let (_, runs) = at_each_level(|| {
		let mut car = car.clone();
		setup::conclude(&mut Console::reading(Cursor::new("")), &mut car, &path, &kept, delta1, "2026-10-20").unwrap();
	});
	assert_split("measure setup, a rejected fit", &runs);
	assert_eq!(
		runs[0].stdout,
		"2 passes disagree · implied slope -4.0 % · implied wind 0.0 m/s\n\
		 \n\
		 PASSES\n\
		 \x20 1  CdA 0.63 m² · Crr 0.0010\n\
		 \x20 2  CdA 0.63 m² · Crr 0.0800\n"
	);
	assert_eq!(
		runs[0].stderr,
		"no road load written: the two passes disagree by 98 % on Crr (limit 95 %)\n\
         re-run `vagcan measure setup` on a flatter stretch or a calmer day; it asks only for the passes\n"
	);
	assert!(runs[1].stderr.contains("a flatter stretch"), "{}", runs[1].stderr);
	assert!(!path.exists(), "a car file was written for a fit that did not hold");
}
