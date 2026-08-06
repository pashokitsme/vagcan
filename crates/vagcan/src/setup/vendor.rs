//! Getting hold of a VCDS installation when the machine has none.
//!
//! `vagcan setup` needs an installation to parse. Somebody who has VCDS points
//! at it; somebody who has not had, until now, nothing to do but read a link.
//! So the archives are hosted, and this fetches one — as a *step of* `setup`,
//! not a command of its own: the download is a means to an installation, and
//! the run continues into the same parse either way.
//!
//! ## What this deliberately does not do
//!
//! It adds no HTTP client and no zip decoder to the dependency tree. `curl` and
//! `unzip` are on every macOS and every ordinary Linux, they are what a person
//! would use by hand, and `curl` draws a better progress bar than anything that
//! would be written here. A missing one is reported by name with the manual
//! commands to run instead, which is a far better failure than ninety megabytes
//! of new dependencies for one download.
//!
//! ## What it checks
//!
//! The archive is around 90 MB and the interesting failure is not "the download
//! failed" but "the download stopped". A truncated zip unpacks into a partial
//! VCDS installation, and the symptom surfaces days later as label files that is
//! quietly missing files. So the length is asked for first and compared against
//! what arrived, the bytes are checked for a zip's own signature, and anything
//! short is deleted rather than unpacked.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Where the archives are served from.
///
/// One place to edit. They are Git LFS objects in this project's own
/// repository and GitHub serves the real bytes through `raw/` **for a public
/// repository** — so this resolves once the repository is public, and 404s
/// while it is private. That is the precondition, not a bug to work around: no
/// token belongs in a program anybody can run.
pub const ARCHIVE_BASE: &str = "https://github.com/pashokitsme/vagcan/raw/master/vendor";

/// The build this fetches.
///
/// Ross-Tech ship the label text translated, and for a while this offered the
/// Russian build alongside. It no longer does, because the two are not equal:
/// the Russian build's text table masks its key, so it yields fault text and
/// labels but **no measurement names**, and nothing offline recovers them
/// (`ARCHITECTURE.md`, "The file formats"). Offering it as a peer was offering a
/// quietly lesser install. Somebody who has that build can still point `setup`
/// straight at it — and then the tool says which part it cannot give them.
const BUILD: &str = "en";

/// The zip signature, so a truncated or error-page download is caught before it
/// is handed to `unzip` (PKWARE APPNOTE §4.3.7).
const ZIP_MAGIC: &[u8; 4] = b"PK\x03\x04";

/// Where downloaded installations are kept.
///
/// Under `~/.vagcan/vendor`, beside everything else this tool owns, and *not*
/// under `labels/`: that directory holds what was parsed out of an
/// installation, and this is the installation.
pub fn vendor_dir() -> Result<PathBuf> {
	Ok(crate::datadir::vagcan_dir()?.join("vendor"))
}

/// Ask whether to download at all.
///
/// The size is in the question, because ninety megabytes is a different
/// decision on a phone tether than on a desk, and a progress bar that starts
/// without being agreed to is how a tool loses trust.
pub fn confirm_download() -> Result<bool> {
	// No terminal, no download. This used to answer "yes" here, which was safe
	// only because the language question came next and refused without a
	// terminal; removing that question left a script fetching ninety megabytes
	// nobody asked for. A run with nowhere to ask is told the path form instead.
	if !std::io::stdin().is_terminal() {
		return Ok(false);
	}
	print!(
		"No VCDS installation was given.\n\
         Download one (about 90 MB) into {}? [y/N] ",
		vendor_dir()?.display()
	);
	std::io::stdout().flush().ok();
	let mut answer = String::new();
	std::io::stdin().read_line(&mut answer)?;
	Ok(matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

/// Fetch and unpack an installation, and return the directory it landed in.
///
/// An installation already unpacked is used as it stands: the archive does not
/// change, and re-downloading ninety megabytes because a later step failed
/// would be its own reason not to run this again.
pub fn fetch(base: &str) -> Result<PathBuf> {
	let dir = vendor_dir()?;
	let unpacked = dir.join(format!("vcds-{BUILD}"));
	if unpacked.is_dir() && std::fs::read_dir(&unpacked).is_ok_and(|d| d.count() > 0) {
		println!("Using the installation already at {}", unpacked.display());
		return Ok(unpacked);
	}
	std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

	let url = format!("{base}/vcds-{BUILD}.zip");
	let archive = dir.join(format!("vcds-{BUILD}.zip"));
	download(&url, &archive)?;
	unpack(&archive, &unpacked)?;
	Ok(unpacked)
}

/// Download `url` to `into`, with a progress bar, checking what arrived.
fn download(url: &str, into: &Path) -> Result<()> {
	need_tool("curl", url)?;
	let expected = content_length(url);
	// Into a part file: a `vcds-en.zip` left behind by an interrupted run is
	// indistinguishable from a complete one at a glance, and the next run would
	// unpack it.
	let part = into.with_extension("zip.part");
	let _ = std::fs::remove_file(&part);

	println!("Downloading {url}");
	let status = std::process::Command::new("curl")
		// `--fail` so an HTML error page is not saved as a zip; `--location`
		// because the hosting redirects to object storage.
		.args(["--fail", "--location", "--progress-bar", "--output"])
		.arg(&part)
		.arg(url)
		.status()
		.with_context(|| format!("running curl for {url}"))?;
	if !status.success() {
		let _ = std::fs::remove_file(&part);
		anyhow::bail!(
			"downloading {url} failed ({status}).\n\n\
             If the address is not reachable, fetch a VCDS installation yourself and \
             point at it:\n    \
             vagcan setup /path/to/VCDS\n\
             {}",
			crate::missing::VCDS_DOWNLOAD
		);
	}
	check_archive(&part, expected)?;
	std::fs::rename(&part, into).with_context(|| format!("moving the download into {}", into.display()))?;
	Ok(())
}

/// What the server says the archive is, when it will say.
///
/// Best effort. A server that declines to state a length is not a reason to
/// refuse the download — it is a reason to fall back to the signature and size
/// checks, which catch the same failure less precisely.
fn content_length(url: &str) -> Option<u64> {
	let out = std::process::Command::new("curl")
		.args(["--silent", "--location", "--head", url])
		.output()
		.ok()?;
	String::from_utf8_lossy(&out.stdout)
		.lines()
		.filter_map(|line| line.split_once(':'))
		.filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
		.filter_map(|(_, value)| value.trim().parse::<u64>().ok())
		.next_back()
}

/// Whether what arrived is a whole archive.
///
/// The failure being guarded is not "it did not download" — that is loud — but
/// "it stopped". A truncated zip unpacks into a VCDS installation missing an
/// arbitrary tail of its label files, and nothing downstream can tell that from
/// label files that genuinely lacks them.
pub fn check_archive(path: &Path, expected: Option<u64>) -> Result<()> {
	let got = std::fs::metadata(path)
		.with_context(|| format!("the download {} is not there", path.display()))?
		.len();
	if let Some(want) = expected {
		if got != want {
			let _ = std::fs::remove_file(path);
			anyhow::bail!(
				"the download stopped: {got} bytes of {want}. Nothing was unpacked — \
                 a partial archive becomes a VCDS installation missing files, and that \
                 failure surfaces days later as label files with holes in it. Run \
                 `vagcan setup` again."
			);
		}
	}
	let mut magic = [0u8; 4];
	let read = {
		use std::io::Read;
		std::fs::File::open(path)
			.with_context(|| format!("reading {}", path.display()))?
			.read(&mut magic)
			.unwrap_or(0)
	};
	if read < 4 || &magic != ZIP_MAGIC {
		let _ = std::fs::remove_file(path);
		anyhow::bail!(
			"what arrived is not a zip archive — the address served something else. \
             Nothing was unpacked."
		);
	}
	Ok(())
}

/// Unpack the archive into `into`.
fn unpack(archive: &Path, into: &Path) -> Result<()> {
	need_tool("unzip", "")?;
	println!("Unpacking into {}", into.display());
	std::fs::create_dir_all(into).with_context(|| format!("creating {}", into.display()))?;
	let status = std::process::Command::new("unzip")
		.args(["-q", "-o"])
		.arg(archive)
		.arg("-d")
		.arg(into)
		.status()
		.context("running unzip")?;
	anyhow::ensure!(
		status.success(),
		"unpacking {} failed ({status}) — the archive may be damaged; delete it and \
         run `vagcan setup` again",
		archive.display()
	);
	Ok(())
}

/// Refuse early, by name, when a tool this needs is not installed.
fn need_tool(tool: &str, url: &str) -> Result<()> {
	if std::process::Command::new(tool)
		.arg("--version")
		.stdout(std::process::Stdio::null())
		.stderr(std::process::Stdio::null())
		.status()
		.is_ok()
	{
		return Ok(());
	}
	anyhow::bail!(
		"`{tool}` is not installed, and this step is a download and an unzip rather \
         than a dependency of this program.\n\n\
         Do it by hand and then point at the result:\n    \
         curl -L -o vcds.zip {url}\n    \
         unzip vcds.zip -d vcds\n    \
         vagcan setup vcds"
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn temp(tag: &str) -> PathBuf {
		let dir = std::env::temp_dir().join(format!("vagcan-vendor-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
		let _ = std::fs::remove_dir_all(&dir);
		std::fs::create_dir_all(&dir).unwrap();
		dir
	}

	#[test]
	fn a_download_that_stopped_short_is_deleted_rather_than_unpacked() {
		// The failure this guard exists for. Ninety megabytes over a phone
		// tether ends early often enough, and a partial zip becomes a VCDS
		// installation with an arbitrary tail of its label files missing —
		// which reads, days later, as label files that never had them.
		let dir = temp("short");
		let file = dir.join("vcds-en.zip");
		std::fs::write(&file, b"PK\x03\x04partial").unwrap();
		let err = check_archive(&file, Some(90_000_000)).unwrap_err().to_string();
		assert!(err.contains("the download stopped"), "{err}");
		assert!(err.contains("Nothing was unpacked"), "{err}");
		assert!(!file.exists(), "a short download must not be left to be found later");
		let _ = std::fs::remove_dir_all(&dir);
	}

	#[test]
	fn something_that_is_not_an_archive_is_never_handed_to_unzip() {
		// An HTML error page is a 200 with a length, and it is exactly what a
		// private repository serves in place of the object.
		let dir = temp("html");
		let file = dir.join("vcds-en.zip");
		std::fs::write(&file, b"<!DOCTYPE html><title>404</title>").unwrap();
		let err = check_archive(&file, None).unwrap_err().to_string();
		assert!(err.contains("not a zip archive"), "{err}");
		assert!(!file.exists());
		let _ = std::fs::remove_dir_all(&dir);
	}

	#[test]
	fn a_whole_archive_passes_both_checks() {
		let dir = temp("whole");
		let file = dir.join("vcds-en.zip");
		let bytes = b"PK\x03\x04\x00\x00\x00\x00";
		std::fs::write(&file, bytes).unwrap();
		assert!(check_archive(&file, Some(bytes.len() as u64)).is_ok());
		assert!(file.exists());
		let _ = std::fs::remove_dir_all(&dir);
	}

	#[test]
	fn the_url_is_one_edit_and_names_the_language() {
		// The archives may be hosted elsewhere later, and a URL assembled at
		// three call sites is how that becomes a hunt.
		assert!(ARCHIVE_BASE.starts_with("https://"), "{ARCHIVE_BASE}");
		assert!(!ARCHIVE_BASE.ends_with('/'), "the language is joined with a slash");
	}
}
