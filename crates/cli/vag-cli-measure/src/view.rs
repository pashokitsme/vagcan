//! `measure view` — the chart page.
//!
//! Almost nothing happens here on purpose. The page is [`view.html`], a real
//! file with the CSS, the JavaScript and the SVG scaffold in it, and this
//! module's whole job is to substitute the session's JSON at one marked point,
//! write the result next to the input and try to open it.
//!
//! Written the other way — `format!` in Rust — the page becomes a thousand
//! lines of string soup no editor understands, and the "no external URL" test
//! passes just as happily on a page that renders nothing at all.
//!
//! The session arrives as a [`serde_json::Value`] rather than a typed struct:
//! the page consumes the document described in the design's §5, and reading it
//! as the document keeps this module from being rewritten every time a field
//! is added to the writer.
//!
//! [`view.html`]: ./view.html

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The page, with a hole in it where the session goes.
const PAGE: &str = include_str!("view.html");

/// The hole. A JavaScript comment, so `view.html` stays valid on its own.
const MARKER: &str = "/*{{SESSION}}*/";

/// The page with this session in it.
pub fn render(session: &serde_json::Value) -> String {
	// The session rides inside a `<script type="application/json">` block, so a
	// `</script>` inside any string would close it early and take the rest of
	// the page with it. `\/` is a JSON escape for `/` and parses back to the
	// same string, so the round-trip is unharmed.
	let json = serde_json::to_string(session)
		.unwrap_or_else(|_| "null".to_string())
		.replace("</", "<\\/");
	PAGE.replace(MARKER, &json)
}

/// Write the page beside the session file and hand back its path.
///
/// `runs/2026-08-03.json` becomes `runs/2026-08-03.html`. An input that is
/// already an `.html` keeps its own name and the page gains a second
/// extension, because overwriting the file we were asked to read would be a
/// poor way to render it.
pub fn write_beside(input: &Path, session: &serde_json::Value) -> Result<PathBuf> {
	let mut out = input.with_extension("html");
	if out == input {
		let mut name = input.as_os_str().to_os_string();
		name.push(".html");
		out = PathBuf::from(name);
	}
	std::fs::write(&out, render(session)).with_context(|| format!("writing the chart page to {}", out.display()))?;
	Ok(out)
}

/// Write the page, say where it is, then try to open it.
///
/// The path is printed **first**: opening the file is a convenience, and a
/// platform without a usable opener is not a failure of the command.
pub fn write_and_open(input: &Path, session: &serde_json::Value) -> Result<PathBuf> {
	let out = write_beside(input, session)?;
	println!("{}", out.display());
	open_in_browser(&out);
	Ok(out)
}

/// Ask the desktop to open a file. Every failure is ignored, including the
/// absence of anything to ask.
pub fn open_in_browser(path: &Path) {
	let opener = if cfg!(target_os = "macos") {
		"open"
	} else if cfg!(target_os = "windows") {
		"explorer"
	} else {
		"xdg-open"
	};
	let _ = std::process::Command::new(opener)
		.arg(path)
		.stdout(std::process::Stdio::null())
		.stderr(std::process::Stdio::null())
		.spawn();
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A session with three runs on it — a launch, three upshifts, marks that
	/// closed and one run aborted at 82 km/h. It is a fixture rather than a
	/// recording: no car was driven for it, and the numbers come from a road-
	/// load simulation written to the shape the design's §5 describes.
	const SAMPLE: &str = include_str!("view_sample.json");

	fn sample() -> serde_json::Value {
		serde_json::from_str(SAMPLE).expect("the sample session is valid JSON")
	}

	/// **Nothing here verifies that the chart draws.** These tests can assert
	/// that the session reached the page and that the page asks the network
	/// for nothing; they cannot see a line, an axis or a tooltip. That part
	/// was checked by opening the rendered page in a browser and looking at
	/// it, and it has to be checked that way again after any change to
	/// `view.html`.
	#[test]
	fn the_placeholder_is_substituted() {
		let html = render(&sample());
		assert!(!html.contains(MARKER), "the marker survived into the output");
		assert!(PAGE.contains(MARKER), "the template lost its marker");
		assert!(html.contains("TMBJJ7NE1J0000000"), "the session did not reach the page");
	}

	#[test]
	fn the_embedded_json_round_trips() {
		let session = sample();
		let html = render(&session);
		let back: serde_json::Value = serde_json::from_str(&embedded(&html)).expect("the embedded block is valid JSON");
		assert_eq!(back, session);
	}

	/// The block the page parses at load: everything between the data script's
	/// tags. The same text the browser's `JSON.parse` is handed.
	fn embedded(html: &str) -> String {
		let open = html
			.find(r#"<script type="application/json" id="session-data">"#)
			.expect("the data block is in the page");
		let start = open + html[open..].find('>').expect("the tag closes") + 1;
		let end = start + html[start..].find("</script>").expect("the data block closes");
		html[start..end].to_string()
	}

	/// No CDN, no web font, no remote image, no fetch — the page is one file
	/// and it works on a laptop with the network off.
	#[test]
	fn nothing_reaches_outside_the_page() {
		for (what, text) in [("the template", PAGE), ("the output", &render(&sample())[..])] {
			for scheme in ["http://", "https://"] {
				assert!(!text.contains(scheme), "{what} carries an external URL: {scheme}");
			}
		}
	}

	#[test]
	fn the_page_lands_beside_the_session() {
		let dir = std::env::temp_dir().join(format!("vagcan-measure-view-{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();
		let input = dir.join("drive.json");
		std::fs::write(&input, SAMPLE).unwrap();

		let out = write_beside(&input, &sample()).unwrap();
		assert_eq!(out, dir.join("drive.html"));
		let written = std::fs::read_to_string(&out).unwrap();
		assert!(written.contains("<title>"), "the page is not a page");
		assert!(!written.contains(MARKER));

		// An input that is already HTML is not the file we write over.
		let html_input = dir.join("drive.html");
		let out2 = write_beside(&html_input, &sample()).unwrap();
		assert_eq!(out2, dir.join("drive.html.html"));

		std::fs::remove_dir_all(&dir).ok();
	}

	/// A `</script>` inside a string would end the data block early. It is
	/// escaped on the way in, and the escape is invisible to the parser.
	#[test]
	fn a_closing_tag_inside_the_session_cannot_end_the_block() {
		let session = serde_json::json!({
				"schema": 1,
				"runs": [],
				"car": { "vin": "</script><b>x" }
		});
		let html = render(&session);
		assert!(!html.contains("</script><b>x"));
		let back: serde_json::Value = serde_json::from_str(&embedded(&html)).unwrap();
		assert_eq!(back, session);
	}
}
