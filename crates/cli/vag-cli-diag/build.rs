//! The dash renderer as WebAssembly, for `vagcan dev dash edit` to serve beside its page.
//!
//! `vagcan` cannot compile WebAssembly when it runs — after `cargo install` there is no checkout
//! and no compiler to ask — so it is compiled here, while `vagcan` is, and embedded. Only when
//! the `wasm32-unknown-unknown` target is installed: without it this writes an empty
//! `dash.wasm` and the reason, and the command says what to install and that `vagcan` has to be
//! built again. A failed compile is the same: a warning here, the reason there — never a failed
//! `vagcan` build over its page editor.
//!
//! The crate is `crates/dash/vag-dash-wasm`, a workspace member; it is built by a second cargo,
//! for the wasm target, into a directory of its own beside this build's (`dash-wasm/`), so the
//! outer build's lock is not contended and every profile of the outer build shares one result.

use std::path::{Path, PathBuf};
use std::process::Command;

const TARGET: &str = "wasm32-unknown-unknown";

fn main() {
	let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
	let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
	let root = manifest.join("../../..");
	println!("cargo:rerun-if-changed=build.rs");
	// The workspace's Cargo.toml holds the `dash-wasm` profile.
	for watched in ["crates/dash/vag-dash-wasm", "crates/dash/vag-dash-render", "Cargo.toml"] {
		println!("cargo:rerun-if-changed={}", root.join(watched).display());
	}
	let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
	// Installing the target adds a directory beside the host's: watching where it would appear
	// is what makes the next build of `vagcan` pick it up.
	if let Some(rustlib) = sysroot(&rustc).map(|s| s.join("lib/rustlib")) {
		println!("cargo:rerun-if-changed={}", rustlib.display());
	}
	let (wasm, why) = match build(&rustc, &root, &target_dir(&out)) {
		Ok(bytes) => (bytes, String::new()),
		Err(why) => {
			if !why.starts_with("the wasm32-unknown-unknown target is not installed") {
				println!("cargo:warning=dash editor: {why}");
			}
			(Vec::new(), why)
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

fn build(rustc: &str, root: &Path, target_dir: &Path) -> Result<Vec<u8>, String> {
	let libdir = Command::new(rustc)
		.args(["--print", "target-libdir", "--target", TARGET])
		.output()
		.map_err(|e| format!("cannot run {rustc}: {e}"))?;
	let libdir = PathBuf::from(String::from_utf8_lossy(&libdir.stdout).trim());
	let installed =
		std::fs::read_dir(&libdir).is_ok_and(|mut entries| entries.any(|e| e.is_ok_and(|e| e.file_name().to_string_lossy().starts_with("libcore"))));
	if !installed {
		return Err(format!("the {TARGET} target is not installed"));
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
	let run = command.output().map_err(|e| format!("cannot run cargo: {e}"))?;
	if !run.status.success() {
		let said = String::from_utf8_lossy(&run.stderr);
		let last: Vec<&str> = said.lines().rev().take(6).collect();
		return Err(format!(
			"compiling crates/dash/vag-dash-wasm for {TARGET} failed: {}",
			last.into_iter().rev().collect::<Vec<_>>().join(" / ")
		));
	}
	let artifact = target_dir.join(TARGET).join("dash-wasm").join("vag_dash_wasm.wasm");
	std::fs::read(&artifact).map_err(|e| format!("{} was not written: {e}", artifact.display()))
}
