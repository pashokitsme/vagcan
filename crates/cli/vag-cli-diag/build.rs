//! The dash renderer as WebAssembly, for `vagcan dev dash edit` to serve beside its page.
//!
//! `vagcan` cannot compile WebAssembly when it runs — after `cargo install` there is no checkout
//! and no compiler to ask — so it is compiled here, while `vagcan` is, and embedded. Only when
//! the `wasm32-unknown-unknown` target is installed: without it this writes an empty
//! `dash.wasm` and the reason, and the command says what to install and that `vagcan` has to be
//! built again. A failed compile is the same — a warning here, the reason there, never a failed
//! `vagcan` build over its page editor — except that it is not kept: the next build tries again.
//!
//! [`REQUIRED_ENV`] set makes either a failed build. CI's `test` job sets it (`just test` does),
//! so a renderer that no longer compiles for wasm cannot pass CI.
//!
//! The crate is `crates/dash/vag-dash-wasm`, a workspace member; it is built by a second cargo,
//! for the wasm target, into a directory of its own beside this build's (`dash-wasm/`), so the
//! outer build's lock is not contended and every profile of the outer build shares one result.

use std::path::{Path, PathBuf};
use std::process::Command;

const TARGET: &str = "wasm32-unknown-unknown";

/// Set and not empty: no renderer is a failed build, not an empty `dash.wasm`.
const REQUIRED_ENV: &str = "VAGCAN_DASH_WASM_REQUIRED";

/// The first line of `dash-wasm-why.txt` when the target is not installed. `src/dash/edit.rs`
/// reads it to tell "install the target" from "build again from clean"; any other first line is
/// a failed compile.
const NOT_INSTALLED: &str = "not-installed";

/// The first line of `dash-wasm-why.txt` when the compile failed.
const FAILED: &str = "failed";

fn main() {
	let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
	let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
	let root = manifest.join("../../..");
	println!("cargo:rerun-if-changed=build.rs");
	println!("cargo:rerun-if-env-changed={REQUIRED_ENV}");
	// The workspace's Cargo.toml holds the `dash-wasm` profile; Cargo.lock, the versions the
	// renderer is compiled against.
	for watched in ["crates/dash/vag-dash-wasm", "crates/dash/vag-dash-render", "Cargo.toml", "Cargo.lock"] {
		println!("cargo:rerun-if-changed={}", root.join(watched).display());
	}
	let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
	// Installing the target adds a directory beside the host's: watching where it would appear
	// is what makes the next build of `vagcan` pick it up.
	if let Some(rustlib) = sysroot(&rustc).map(|s| s.join("lib/rustlib")) {
		println!("cargo:rerun-if-changed={}", rustlib.display());
	}
	let required = std::env::var_os(REQUIRED_ENV).is_some_and(|v| !v.is_empty());
	let (wasm, why) = match build(&rustc, &root, &target_dir(&out)) {
		Ok(bytes) => (bytes, String::new()),
		Err(Failure::NotInstalled) => {
			let why = format!("the {TARGET} target is not installed");
			if required {
				panic!("{REQUIRED_ENV} is set and {why}: rustup target add {TARGET}");
			}
			// Kept: the `rustlib` watch above runs this again when the target arrives.
			(Vec::new(), format!("{NOT_INSTALLED}\n{why}"))
		}
		Err(Failure::Failed(why)) => {
			if required {
				panic!("{REQUIRED_ENV} is set and the dash renderer did not build: {why}");
			}
			println!("cargo:warning=dash editor: {why}");
			// A file that never exists is always out of date, so the next build tries again: a
			// failure of one build (its RUSTFLAGS, a full disk) is not embedded for good.
			println!("cargo:rerun-if-changed={}", out.join("dash-wasm-retry").display());
			(Vec::new(), format!("{FAILED}\n{why}"))
		}
	};
	std::fs::write(out.join("dash.wasm"), wasm).expect("OUT_DIR is writable");
	std::fs::write(out.join("dash-wasm-why.txt"), why).expect("OUT_DIR is writable");
}

fn sysroot(rustc: &str) -> Option<PathBuf> {
	let out = Command::new(rustc).args(["--print", "sysroot"]).output().ok()?;
	out.status.success().then(|| PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

/// `<target dir>/dash-wasm`, from `OUT_DIR` = `<target dir>[/<triple>]/<profile>/build/<pkg>/out`;
/// `OUT_DIR` itself where it is not shaped like that.
fn target_dir(out: &Path) -> PathBuf {
	match out.ancestors().nth(2) {
		Some(build) if build.file_name().is_some_and(|n| n == "build") => build.parent().and_then(Path::parent).unwrap_or(out).join("dash-wasm"),
		_ => out.join("dash-wasm"),
	}
}

/// Why there is no renderer.
enum Failure {
	/// The wasm target's standard library is not installed.
	NotInstalled,
	/// Anything else, in words.
	Failed(String),
}

fn build(rustc: &str, root: &Path, target_dir: &Path) -> Result<Vec<u8>, Failure> {
	let libdir = Command::new(rustc)
		.args(["--print", "target-libdir", "--target", TARGET])
		.output()
		.map_err(|e| Failure::Failed(format!("cannot run {rustc}: {e}")))?;
	let libdir = PathBuf::from(String::from_utf8_lossy(&libdir.stdout).trim());
	let installed =
		std::fs::read_dir(&libdir).is_ok_and(|mut entries| entries.any(|e| e.is_ok_and(|e| e.file_name().to_string_lossy().starts_with("libcore"))));
	if !installed {
		return Err(Failure::NotInstalled);
	}
	let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
	let mut command = Command::new(cargo);
	command
		.current_dir(root)
		.args([
			"build",
			"-p",
			"vag-dash-wasm",
			"--profile",
			"dash-wasm",
			"--target",
			TARGET,
			"--target-dir",
		])
		.arg(target_dir)
		// The outer build's flags are for the host — a `-C target-cpu=native` would not build
		// for wasm — and clippy, when it is the outer build, lints the workspace it wraps.
		.env_remove("RUSTFLAGS")
		.env_remove("CARGO_ENCODED_RUSTFLAGS")
		.env_remove("RUSTC_WORKSPACE_WRAPPER")
		.env_remove("CLIPPY_ARGS")
		.env_remove("CARGO_TARGET_DIR");
	let run = command.output().map_err(|e| Failure::Failed(format!("cannot run cargo: {e}")))?;
	if !run.status.success() {
		let said = String::from_utf8_lossy(&run.stderr);
		let last: Vec<&str> = said.lines().rev().take(6).collect();
		return Err(Failure::Failed(format!(
			"compiling crates/dash/vag-dash-wasm for {TARGET} failed: {}",
			last.into_iter().rev().collect::<Vec<_>>().join(" / ")
		)));
	}
	let artifact = target_dir.join(TARGET).join("dash-wasm").join("vag_dash_wasm.wasm");
	std::fs::read(&artifact).map_err(|e| Failure::Failed(format!("{} was not written: {e}", artifact.display())))
}
