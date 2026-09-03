fn main() {
	// First, because when the linker calls this binary back as its
	// error-handling script it must answer and exit — not build a plan.
	linker_be_nice();
	plan();
	// make sure linkall.x is the last linker script (otherwise might cause problems with flip-link)
	println!("cargo:rustc-link-arg=-Tlinkall.x");
}

/// Builds the plan this image is for, and tells `src/plan.rs` where it is.
///
/// The firmware is built for **one car**, named by `VAGCAN_DASH_VIN` — or,
/// when that is unset, by being the only car under `~/.vagcan/dash/` (see
/// [`only_car`]). The plan is resolved here, on the host, from
/// `~/.vagcan/dash/<VIN>/dash.toml`, the car's survey and the project's
/// catalog cache — the same generator `vagcan dev dash build` runs — and
/// written back under `~/.vagcan/`. Nothing it produces lands in the
/// checkout: it is derived from VW's data and describes one owner's car, and
/// `CLAUDE.md` says why that may not be committed.
///
/// Failing loud is the point. A `dash.toml` that is not there, a channel the
/// car's variant does not declare — each stops the build with the generator's
/// own reason, because an image with a guessed plan would show a plausible
/// number for the wrong thing, which is the failure this project exists to
/// avoid.
fn plan() {
	// Once a build script names any `rerun-if-*`, cargo stops watching the
	// package's own files, so the script names itself too.
	println!("cargo:rerun-if-changed=build.rs");
	println!("cargo:rerun-if-env-changed=VAGCAN_DASH_VIN");
	// The project override. A different project is a different cache, and a
	// different cache is a different plan.
	println!("cargo:rerun-if-env-changed={}", vag_cli_core::project::PROJECT_ENV);

	let vin = match std::env::var("VAGCAN_DASH_VIN") {
		Ok(vin) if !vin.trim().is_empty() => vin,
		_ => only_car(),
	};

	let written = match vag_cli_core::dash::build_for_car(&vin, None) {
		Ok(written) => written,
		// `{:#}` prints the whole chain — "no build input at …", "channel X
		// is not declared by variant Y" — which is what the person needs.
		Err(e) => panic!("dash plan for VIN {vin}: {e:#}"),
	};

	// What was resolved and how, so a builder sees a proven scaling beating a
	// declared one, or a label taken from the glossary, without opening the
	// output. One line each: a cargo warning is one line.
	for note in &written.built.notes {
		let note = note.replace(['\r', '\n'], " ");
		println!("cargo:warning=plan: {note}");
	}

	// Everything the plan was resolved from — the input, the survey, the
	// project's cache and proven rows, the name table, the settings and the
	// glossary. A change to any of them is a different plan, and an image
	// built from a stale one is exactly as wrong as one built from none.
	// Cargo watches a directory recursively, which is what `measurements/`
	// needs.
	for input in &written.inputs {
		println!("cargo:rerun-if-changed={}", input.display());
	}

	// What rustc `include!`s is a copy in this build's own `OUT_DIR`, not the
	// file under `~/.vagcan/`. Two builds can run at once — an editor's and a
	// terminal's — and each rewrites that file; a copy per build is what keeps
	// one build's generator from rewriting the file another build's rustc is
	// in the middle of reading. The one under `~/.vagcan/` stays for people.
	let out_dir = std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR for a build script");
	let out = std::path::Path::new(&out_dir).join("plan.rs");
	if let Err(e) = std::fs::copy(&written.rust, &out) {
		panic!("copying {} to {}: {e}", written.rust.display(), out.display());
	}
	println!("cargo:rustc-env=VAG_DASH_PLAN={}", out.display());
}

/// The VIN when none was named: the one car under `~/.vagcan/dash/`.
///
/// This exists for rust-analyzer, which runs the build script with whatever
/// environment the editor has, and that environment names no car. Without
/// a fallback the script panics, and every use of `PLAN` in the editor is an
/// error. With exactly one car in the owner's `~/.vagcan/dash/` there is
/// nothing to choose, so it is chosen — and said, so nobody flashes an image
/// without noticing which car it is for. With none or several, the choice is
/// a person's, and the build stops and says so.
///
/// A car is a directory holding a `dash.toml` — the generator's own
/// definition of one it can build for. The directory also holds things that
/// are not cars (`preview/` is the renderer's pictures) and they do not
/// count, or the fallback would refuse on the very machine it exists for.
///
/// It reads the owner's home directory, never the checkout, so no car
/// reaches the tree by this route either.
fn only_car() -> String {
	let dash = match vag_cli_core::datadir::vagcan_dir() {
		Ok(dir) => dir.join("dash"),
		Err(e) => panic!("VAGCAN_DASH_VIN is unset and there is no ~/.vagcan to look in: {e:#}"),
	};
	// Adding a second car changes the answer, so the directory is watched.
	println!("cargo:rerun-if-changed={}", dash.display());

	let mut cars: Vec<String> = std::fs::read_dir(&dash)
		.map(|entries| {
			entries
				.filter_map(Result::ok)
				.filter(|entry| entry.path().join("dash.toml").is_file())
				.filter_map(|entry| entry.file_name().into_string().ok())
				.collect()
		})
		.unwrap_or_default();
	cars.sort();

	match cars.as_slice() {
		[vin] => {
			println!("cargo:warning=plan: VAGCAN_DASH_VIN unset — building for the one car under ~/.vagcan/dash: {vin}");
			vin.clone()
		}
		[] => panic!(
			"this firmware is built for one car and none was named.\n\
			 VAGCAN_DASH_VIN is unset and {} holds no car (a directory with a dash.toml) to fall back to.\n\
			 Set VAGCAN_DASH_VIN to its VIN, e.g. `VAGCAN_DASH_VIN=<VIN> cargo build --release`,\n\
			 with `~/.vagcan/dash/<VIN>/dash.toml` describing what the panel shows\n\
			 (see `vagcan dev dash build`).",
			dash.display()
		),
		many => panic!(
			"this firmware is built for one car and none was named.\n\
			 VAGCAN_DASH_VIN is unset and {} holds {} cars — {} — so there is nothing to fall back to.\n\
			 Set VAGCAN_DASH_VIN to the VIN of the one this image is for, e.g. `VAGCAN_DASH_VIN=<VIN> cargo build --release`.",
			dash.display(),
			many.len(),
			many.join(", ")
		),
	}
}

fn linker_be_nice() {
	let args: Vec<String> = std::env::args().collect();
	if args.len() > 1 {
		let kind = &args[1];
		let what = &args[2];

		match kind.as_str() {
			"undefined-symbol" => match what.as_str() {
				"_defmt_timestamp" => {
					eprintln!();
					eprintln!("💡 `defmt` not found - make sure `defmt.x` is added as a linker script and you have included `use defmt_rtt as _;`");
					eprintln!();
				}
				"_stack_start" => {
					eprintln!();
					eprintln!("💡 Is the linker script `linkall.x` missing?");
					eprintln!();
				}
				"esp_wifi_preempt_enable" | "esp_wifi_preempt_yield_task" | "esp_wifi_preempt_task_create" => {
					eprintln!();
					eprintln!(
						"💡 `esp-wifi` has no scheduler enabled. Make sure you have the `builtin-scheduler` feature enabled, or that you provide an external scheduler."
					);
					eprintln!();
				}
				"embedded_test_linker_file_not_added_to_rustflags" => {
					eprintln!();
					eprintln!("💡 `embedded-test` not found - make sure `embedded-test.x` is added as a linker script for tests");
					eprintln!();
				}
				_ => (),
			},
			// we don't have anything helpful for "missing-lib" yet
			_ => {
				std::process::exit(1);
			}
		}

		std::process::exit(0);
	}

	println!(
		"cargo:rustc-link-arg=--error-handling-script={}",
		std::env::current_exe().unwrap().display()
	);
}
