//! `vagcan dev dash edit <VIN>` — the dash's pages laid out in a browser.
//!
//! The page (`editor.html`) draws the panel with the board's own renderer compiled to
//! WebAssembly (`crates/dash/vag-dash-wasm`, embedded by this crate's build script), offers the
//! car's channels as [`super::catalog`] resolves them, and opens on the pages of the car's
//! `dash.toml`. It writes nothing: what it makes is a block of `dash.toml` to copy.
//!
//! Everything it serves is written first into `~/.vagcan/dash/<VIN>/editor/`
//! ([`vag_cli_core::datadir::dash_editor_dir`]) and served from there by [`serve`], on
//! `127.0.0.1` only, for `GET` and `HEAD` of a file directly in that directory and nothing
//! else. The page asks for nothing but those files: its Content-Security-Policy says so to the
//! browser too.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use vag_cli_core::config::Language;
use vag_cli_core::dash::{self, Input, PageInput};
use vag_cli_core::plan::UnitIdentity;
use vag_data_labels::catalog::CatalogStore;

use super::catalog;

/// The page.
const PAGE: &str = include_str!("editor.html");

/// The renderer, as `build.rs` compiled it; empty when it could not.
const WASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/dash.wasm"));

/// Why [`WASM`] is empty: `build.rs`'s kind of failure on the first line (`not-installed` or
/// `failed`), its words after it.
const WASM_WHY: &str = include_str!(concat!(env!("OUT_DIR"), "/dash-wasm-why.txt"));

/// Bumped when the shape of `channels.json` or the catalog's rules change, so a kept one is
/// not served to a page that reads another.
const CATALOG_FORMAT: &str = "2";

/// The refusal when this `vagcan` was built without the renderer, from `build.rs`'s
/// [`WASM_WHY`]. A missing target is installed; any other failure is built again from clean.
fn no_renderer(why: &str) -> String {
	let (kind, said) = why.trim().split_once('\n').unwrap_or((why.trim(), ""));
	if kind == "not-installed" {
		return format!(
			"this vagcan was built without the dash renderer for the browser: {}. Install the target, then build vagcan again — `just install` in a clone, or the `cargo install` it came from:\n\n  rustup target add wasm32-unknown-unknown\n  just install",
			said.trim()
		);
	}
	format!(
		"this vagcan was built without the dash renderer for the browser: its build failed — {}\n\nBuild vagcan again from clean — in a clone:\n\n  cargo clean -p vag-cli-diag\n  just install\n\nor run the `cargo install` it came from again.",
		said.trim()
	)
}

/// Prepare the editor's directory for `vin`, serve it, and open the page. Runs until Ctrl-C.
pub async fn run(vin: &str) -> Result<()> {
	if WASM.is_empty() {
		bail!(no_renderer(WASM_WHY));
	}
	let record = vag_cli_core::datadir::units_record(vin)?;
	let units = vag_cli_core::units::read_record(&record)?.ok_or_else(|| anyhow::anyhow!("{}", dash::no_record(vin, &record)))?;
	// The car's own pages, when it has a `dash.toml`. One that does not parse is refused here
	// with the build's own words: the editor would open on something other than the file.
	let input_path = vag_cli_core::datadir::dash_dir(vin)?.join("dash.toml");
	let input = match std::fs::read_to_string(&input_path) {
		Ok(text) => {
			let input = dash::parse_input(&text).map_err(|e| anyhow::anyhow!("{}: {e}", input_path.display()))?;
			if !input.vin.eq_ignore_ascii_case(vin.trim()) {
				bail!("{} is for VIN {} but the editor was asked for {vin}", input_path.display(), input.vin);
			}
			Some(input)
		}
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
		Err(e) => return Err(e).with_context(|| format!("reading {}", input_path.display())),
	};
	// The channels of units nothing has read yet, as `dev dash build` reads them first.
	vag_cli_core::registry::ensure_async(units.clone()).await;

	let dir = vag_cli_core::datadir::dash_editor_dir(vin)?;
	std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
	let language = input.as_ref().and_then(|i| i.language);
	let made = tokio::task::spawn_blocking({
		let (vin, dir, units) = (vin.to_string(), dir.clone(), units.clone());
		move || channels(&vin, &dir, language, &units)
	})
	.await??;
	let layout = layout(vin, input.as_ref(), input_path.is_file());
	write(&dir.join("layout.json"), serde_json::to_vec(&layout)?.as_slice())?;
	write(&dir.join("index.html"), PAGE.as_bytes())?;
	write(&dir.join("dash.wasm"), WASM)?;
	match made {
		Made::Kept(n) => vag_cli_core::note!("channels  {n}, kept from the last run: {}", dir.join("channels.json").display()),
		Made::Built(n, numeric) => vag_cli_core::note!("channels  {n}, {numeric} of them numbers: {}", dir.join("channels.json").display()),
	}

	let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
		.await
		.context("opening a port on 127.0.0.1")?;
	let port = listener.local_addr()?.port();
	let url = format!("http://127.0.0.1:{port}/");
	// The address is what the command makes: data. How to use it is a note.
	vag_cli_core::outln!("{url}");
	vag_cli_core::note!("serving   {} on 127.0.0.1 only", dir.display());
	vag_cli_core::note!("it changes no file of yours: copy the block it shows into {}", input_path.display());
	vag_cli_core::say!("Ctrl-C ends it");
	vag_cli_core::desktop::open(&url);
	serve(listener, dir).await
}

/// What [`channels`] did.
enum Made {
	/// Kept from the last run: nothing it was made from has changed. Its row count.
	Kept(usize),
	/// Made now: rows, and how many are numeric.
	Built(usize, usize),
}

/// `channels.json` in `dir`: the catalog, made again only when what it is made from changed.
fn channels(vin: &str, dir: &Path, language: Option<Language>, units: &[UnitIdentity]) -> Result<Made> {
	let project = vag_cli_core::project::current()?;
	let default_language = vag_cli_core::config::language(&vag_cli_core::config::load());
	let mut sources = vec![
		vag_cli_core::datadir::units_record(vin)?,
		project.cache(),
		project.measurements_dir(),
		project.names(),
		vag_cli_core::glossary::path()?,
	];
	// Not `config.toml` itself: `watch` writes its marks there, and the one thing in it the
	// catalog reads — the language — is in the key below.
	// This build of the tool: a new one may resolve differently.
	if let Ok(me) = std::env::current_exe() {
		sources.push(me);
	}
	let key = format!(
		"format {CATALOG_FORMAT}\nproject {}\nlanguage {}\n{}",
		project.id,
		language.unwrap_or(default_language).code(),
		fingerprint(&sources)
	);
	let (json_path, key_path) = (dir.join("channels.json"), dir.join("channels.key"));
	if std::fs::read_to_string(&key_path).is_ok_and(|kept| kept == key)
		&& let Ok(Value::Array(rows)) = std::fs::read(&json_path)
			.map_err(anyhow::Error::from)
			.and_then(|b| Ok(serde_json::from_slice(&b)?))
	{
		return Ok(Made::Kept(rows.len()));
	}
	let store = CatalogStore::open(project.measurements_dir());
	let extracted = vag_cli_core::extracted::open(&project).for_car(Some(vin));
	let rows = catalog::rows(&catalog::Sources {
		vin,
		language,
		default_language,
		store: &store,
		extracted: &extracted,
		units,
	});
	write(&json_path, serde_json::to_vec(&catalog::to_json(&rows))?.as_slice())?;
	write(&key_path, key.as_bytes())?;
	Ok(Made::Built(rows.len(), rows.iter().filter(|r| r.numeric).count()))
}

/// Each path's size and modification time, a directory's entries' with it: what tells a kept
/// catalog from a stale one.
fn fingerprint(paths: &[PathBuf]) -> String {
	fn stamp(path: &Path, out: &mut String) {
		let Ok(meta) = std::fs::metadata(path) else {
			out.push_str(&format!("{}\tabsent\n", path.display()));
			return;
		};
		let modified = meta
			.modified()
			.ok()
			.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
			.map_or(0, |d| d.as_nanos());
		out.push_str(&format!("{}\t{}\t{modified}\n", path.display(), meta.len()));
		if meta.is_dir() {
			let mut entries: Vec<PathBuf> = std::fs::read_dir(path).into_iter().flatten().flatten().map(|e| e.path()).collect();
			entries.sort();
			for entry in entries {
				stamp(&entry, out);
			}
		}
	}
	let mut out = String::new();
	for path in paths {
		stamp(path, &mut out);
	}
	out
}

/// Write a file the editor serves, saying which when it cannot.
fn write(path: &Path, bytes: &[u8]) -> Result<()> {
	std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// `layout.json`: the pages and channels of the car's `dash.toml` the page opens on, and the
/// board's limits. With no `dash.toml`, no pages.
pub fn layout(vin: &str, input: Option<&Input>, from_file: bool) -> Value {
	let pages: Vec<Value> = input
		.map(|i| i.pages.as_slice())
		.unwrap_or_default()
		.iter()
		.map(|p| match p {
			PageInput::Values { title, cells, volts } => json!({
				"kind": "values",
				"title": title,
				"cells": cells.iter().map(ToString::to_string).collect::<Vec<_>>(),
				"volts": volts.unwrap_or(true),
			}),
			PageInput::Chart {
				title,
				cell,
				min,
				max,
				volts,
			} => json!({
				"kind": "chart",
				"title": title,
				"cell": cell.to_string(),
				"min": min,
				"max": max,
				"volts": volts.unwrap_or(true),
			}),
		})
		.collect();
	let channels: Vec<Value> = input
		.map(|i| i.channels.as_slice())
		.unwrap_or_default()
		.iter()
		.map(|c| {
			json!({
				"ref": c.reference.to_string(),
				"label": c.label,
				"decimals": c.decimals,
				"alarm": c.alarm_max.is_some() || c.alarm_min.is_some() || c.drift.is_some(),
			})
		})
		.collect();
	json!({
		"vin": vin,
		"fromFile": from_file && input.is_some(),
		"language": input.and_then(|i| i.language).map(Language::code),
		"strip": input.and_then(|i| i.strip.as_ref()).map(|s| s.volts.to_string()),
		"pages": pages,
		"channels": channels,
		"maxPages": vag_dash_render::pages::MAX_PAGES,
		"maxCells": vag_dash_render::plan::MAX_VALUE_CELLS,
	})
}

/// The answer to one request.
#[derive(Debug, PartialEq)]
pub struct Response {
	pub status: u16,
	pub reason: &'static str,
	pub content_type: &'static str,
	pub body: Vec<u8>,
	/// `HEAD`: the headers of the `GET`, and no body — a refusal's too.
	pub head_only: bool,
	/// The methods this server takes, said with a 405.
	pub allow: Option<&'static str>,
}

impl Response {
	fn refuse(status: u16, reason: &'static str) -> Response {
		Response {
			status,
			reason,
			content_type: "text/plain; charset=utf-8",
			body: format!("{status} {reason}\n").into_bytes(),
			head_only: false,
			allow: None,
		}
	}

	fn bytes(&self) -> Vec<u8> {
		let allow = self.allow.map(|a| format!("Allow: {a}\r\n")).unwrap_or_default();
		let mut out = format!(
			"HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{allow}Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
			 Content-Security-Policy: default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; \
			 img-src 'self' data:; form-action 'none'; frame-ancestors 'none'\r\nConnection: close\r\n\r\n",
			self.status,
			self.reason,
			self.content_type,
			self.body.len()
		)
		.into_bytes();
		if !self.head_only {
			out.extend_from_slice(&self.body);
		}
		out
	}
}

/// What `dir` answers to one request's head (the request line and headers), reached at `port`.
///
/// Only `GET` and `HEAD`; only a `Host` of this server — a page on another site that has a
/// name resolve to 127.0.0.1 is refused rather than answered (DNS rebinding); and only a plain
/// file name directly in `dir`: no separator, no `..`, no leading dot, no escapes, no link.
/// A `HEAD` gets no body, whatever the answer.
pub fn respond(dir: &Path, port: u16, head: &str) -> Response {
	let mut lines = head.split("\r\n");
	let mut parts = lines.next().unwrap_or_default().split(' ');
	let (method, target) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
	let head_only = match method {
		"GET" => false,
		"HEAD" => true,
		_ => {
			return Response {
				allow: Some("GET, HEAD"),
				..Response::refuse(405, "Method Not Allowed")
			};
		}
	};
	let headers: BTreeMap<String, &str> = lines
		.filter_map(|l| l.split_once(':'))
		.map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim()))
		.collect();
	Response {
		head_only,
		..file(dir, port, target, &headers)
	}
}

/// What `GET` of `target` answers: the file it names in `dir`, or the refusal. [`respond`]
/// drops the body for `HEAD`.
fn file(dir: &Path, port: u16, target: &str, headers: &BTreeMap<String, &str>) -> Response {
	let ours = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
	if !headers.get("host").is_some_and(|h| ours.iter().any(|o| o.eq_ignore_ascii_case(h))) {
		return Response::refuse(403, "Forbidden");
	}
	let path = target.split(['?', '#']).next().unwrap_or_default();
	let Some(name) = path.strip_prefix('/') else {
		return Response::refuse(400, "Bad Request");
	};
	let name = if name.is_empty() { "index.html" } else { name };
	let plain = !name.starts_with('.') && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
	if !plain {
		return Response::refuse(404, "Not Found");
	}
	let file = dir.join(name);
	// Not followed: a link in the directory could point anywhere.
	let is_file = std::fs::symlink_metadata(&file).is_ok_and(|m| m.file_type().is_file());
	let Some(body) = is_file.then(|| std::fs::read(&file).ok()).flatten() else {
		return Response::refuse(404, "Not Found");
	};
	let content_type = match name.rsplit_once('.').map(|(_, ext)| ext) {
		Some("html") => "text/html; charset=utf-8",
		Some("json") => "application/json",
		Some("wasm") => "application/wasm",
		_ => "application/octet-stream",
	};
	Response {
		status: 200,
		reason: "OK",
		content_type,
		body,
		head_only: false,
		allow: None,
	}
}

/// The longest request head read: the page's own requests are a few hundred bytes.
const MAX_HEAD: usize = 16 * 1024;

/// Serve `dir` on `listener` until the process ends.
pub async fn serve(listener: tokio::net::TcpListener, dir: PathBuf) -> Result<()> {
	let port = listener.local_addr()?.port();
	loop {
		let Ok((mut stream, _)) = listener.accept().await else {
			continue;
		};
		let dir = dir.clone();
		tokio::spawn(async move {
			let mut head = Vec::new();
			let mut buf = [0u8; 2048];
			let read = tokio::time::timeout(std::time::Duration::from_secs(10), async {
				while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < MAX_HEAD {
					match stream.read(&mut buf).await {
						Ok(0) | Err(_) => return false,
						Ok(n) => head.extend_from_slice(&buf[..n]),
					}
				}
				true
			})
			.await;
			let response = match read {
				Ok(true) if head.len() < MAX_HEAD => respond(&dir, port, &String::from_utf8_lossy(&head)),
				Ok(true) => Response::refuse(431, "Request Header Fields Too Large"),
				_ => return,
			};
			vag_cli_core::wire!(
				"{} → {} {}",
				String::from_utf8_lossy(&head).lines().next().unwrap_or_default(),
				response.status,
				response.reason
			);
			let _ = stream.write_all(&response.bytes()).await;
			let _ = stream.shutdown().await;
		});
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const PORT: u16 = 41234;

	fn get(dir: &Path, target: &str) -> Response {
		respond(dir, PORT, &format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\n\r\n"))
	}

	fn served() -> (tempfile::TempDir, PathBuf) {
		let root = tempfile::tempdir().unwrap();
		let dir = root.path().join("editor");
		std::fs::create_dir(&dir).unwrap();
		std::fs::write(dir.join("index.html"), "<p>page</p>").unwrap();
		std::fs::write(dir.join("channels.json"), "[]").unwrap();
		std::fs::write(dir.join("dash.wasm"), [0u8, 0x61, 0x73, 0x6D]).unwrap();
		std::fs::write(dir.join(".hidden"), "no").unwrap();
		std::fs::create_dir(dir.join("sub")).unwrap();
		std::fs::write(dir.join("sub").join("inner.json"), "{}").unwrap();
		// Beside the directory: what a traversal would be after.
		std::fs::write(root.path().join("dash.toml"), "vin = \"SECRET\"").unwrap();
		(root, dir)
	}

	#[test]
	fn the_files_in_the_directory_are_served_with_their_types() {
		let (_root, dir) = served();
		let page = get(&dir, "/");
		assert_eq!(
			(page.status, page.content_type, page.body.as_slice()),
			(200, "text/html; charset=utf-8", b"<p>page</p>".as_slice())
		);
		assert_eq!(get(&dir, "/index.html"), page);
		assert_eq!(
			get(&dir, "/channels.json?v=2").content_type,
			"application/json",
			"a query is not part of the name"
		);
		assert_eq!(get(&dir, "/dash.wasm").content_type, "application/wasm");
		let head = respond(&dir, PORT, &format!("HEAD /channels.json HTTP/1.1\r\nHost: localhost:{PORT}\r\n\r\n"));
		assert!(head.head_only && head.status == 200);
		assert!(
			String::from_utf8(head.bytes()).unwrap().ends_with("\r\n\r\n"),
			"no body after HEAD's headers"
		);
	}

	#[test]
	fn nothing_outside_the_directory_or_under_it_is_served() {
		let (_root, dir) = served();
		for target in [
			"/../dash.toml",
			"/..%2Fdash.toml",
			"/%2e%2e/dash.toml",
			"/..",
			"/.hidden",
			"/sub/inner.json",
			"/sub",
			"//etc/passwd",
			"/%2Fetc%2Fpasswd",
			"/..\\dash.toml",
			"/nope.html",
			"dash.toml",
			"http://127.0.0.1/dash.toml",
		] {
			let r = get(&dir, target);
			assert!(r.status == 404 || r.status == 400, "{target}: {}", r.status);
			assert!(!String::from_utf8_lossy(&r.body).contains("SECRET"), "{target}");
		}
	}

	#[cfg(unix)]
	#[test]
	fn a_link_in_the_directory_is_not_followed() {
		let (root, dir) = served();
		std::os::unix::fs::symlink(root.path().join("dash.toml"), dir.join("linked.json")).unwrap();
		assert_eq!(get(&dir, "/linked.json").status, 404);
	}

	#[test]
	fn only_get_and_head_and_only_this_server_s_host() {
		let (_root, dir) = served();
		for method in ["POST", "PUT", "DELETE", "OPTIONS"] {
			let r = respond(&dir, PORT, &format!("{method} / HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\n\r\n"));
			assert_eq!(r.status, 405, "{method}");
			let said = String::from_utf8(r.bytes()).unwrap();
			assert!(said.contains("\r\nAllow: GET, HEAD\r\n"), "{method}: {said}");
		}
		let said = String::from_utf8(get(&dir, "/").bytes()).unwrap();
		assert!(!said.contains("Allow:"), "only a 405 says it: {said}");
		// A refused HEAD is a HEAD: headers, no body.
		for (target, host, status) in [
			("/nope.html", "127.0.0.1", 404),
			("/", "evil.example", 403),
			("/../dash.toml", "127.0.0.1", 404),
		] {
			let host = if host == "127.0.0.1" {
				format!("{host}:{PORT}")
			} else {
				host.to_string()
			};
			let r = respond(&dir, PORT, &format!("HEAD {target} HTTP/1.1\r\nHost: {host}\r\n\r\n"));
			assert_eq!(r.status, status, "{target}");
			let said = String::from_utf8(r.bytes()).unwrap();
			assert!(said.ends_with("\r\n\r\n"), "no body after a refused HEAD: {said:?}");
		}
		for host in ["", "Host: evil.example\r\n", "Host: 127.0.0.1:1\r\n", "Host: 127.0.0.1\r\n"] {
			let r = respond(&dir, PORT, &format!("GET / HTTP/1.1\r\n{host}\r\n"));
			assert_eq!(r.status, 403, "{host:?}");
		}
	}

	#[test]
	fn every_answer_tells_the_browser_to_load_nothing_from_elsewhere() {
		let (_root, dir) = served();
		let said = String::from_utf8(get(&dir, "/").bytes()).unwrap();
		assert!(said.contains("Content-Security-Policy: default-src 'self';"), "{said}");
		assert!(said.starts_with("HTTP/1.1 200 OK\r\n"), "{said}");
	}

	#[tokio::test]
	async fn the_server_answers_on_127_0_0_1() {
		let (_root, dir) = served();
		let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
		let port = listener.local_addr().unwrap().port();
		tokio::spawn(serve(listener, dir));
		let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
		stream
			.write_all(format!("GET /channels.json HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes())
			.await
			.unwrap();
		let mut said = String::new();
		stream.read_to_string(&mut said).await.unwrap();
		assert!(said.starts_with("HTTP/1.1 200 OK\r\n") && said.ends_with("\r\n\r\n[]"), "{said}");
	}

	#[test]
	fn the_page_asks_for_nothing_but_its_own_files() {
		// The editor must not send anything anywhere but this server: no fonts, no scripts, no
		// requests to another host, no store of an artifact host.
		for needle in ["http://", "https://", "//fonts", "window.claude", "@import"] {
			assert!(!PAGE.contains(needle), "editor.html holds {needle:?}");
		}
		for file in ["dash.wasm", "channels.json", "layout.json"] {
			assert!(PAGE.contains(&format!("fetch(\"{file}\")")), "it loads {file}");
		}
		// Not even a `/favicon.ico` the server has no file for.
		assert!(PAGE.contains("<link rel=\"icon\" href=\"data:,\">"));
	}

	#[test]
	fn the_layout_is_the_file_s_pages_and_channels() {
		let text = "vin = \"TESTVIN0000000001\"\nlanguage = \"ru\"\n[[channel]]\nref = \"01:IDE00025\"\nlabel = \"ОЖ\"\nalarm_max = 110\n[[channel]]\nref = \"01:IDE00018\"\ndecimals = 1\n[strip]\nvolts = \"01:IDE00018\"\n[[page]]\nkind = \"values\"\ntitle = \"MAIN\"\ncells = [\"01:IDE00025\"]\n[[page]]\nkind = \"chart\"\ncell = \"01:IDE00025\"\nmin = 70\nmax = 110\nvolts = false\n";
		let input = dash::parse_input(text).unwrap();
		let layout = layout("TESTVIN0000000001", Some(&input), true);
		assert_eq!(layout["fromFile"], true);
		assert_eq!(layout["language"], "ru");
		assert_eq!(layout["strip"], "01:IDE00018");
		assert_eq!(
			layout["pages"],
			json!([
				{"kind": "values", "title": "MAIN", "cells": ["01:IDE00025"], "volts": true},
				{"kind": "chart", "title": "", "cell": "01:IDE00025", "min": 70.0, "max": 110.0, "volts": false},
			])
		);
		assert_eq!(
			layout["channels"][0],
			json!({"ref": "01:IDE00025", "label": "ОЖ", "decimals": null, "alarm": true})
		);
		assert_eq!(layout["channels"][1]["alarm"], false);
		assert_eq!(layout["maxPages"], vag_dash_render::pages::MAX_PAGES);
		let none = super::layout("TESTVIN0000000001", None, false);
		assert_eq!((none["fromFile"].clone(), none["pages"].clone()), (json!(false), json!([])));
	}

	#[test]
	fn a_build_without_the_target_says_what_to_install() {
		let said = no_renderer("not-installed\nthe wasm32-unknown-unknown target is not installed");
		assert!(said.contains(": the wasm32-unknown-unknown target is not installed."), "{said}");
		assert!(said.contains("rustup target add wasm32-unknown-unknown"), "{said}");
		assert!(said.contains("build vagcan again"), "{said}");
		assert!(!said.contains("not-installed"), "the kind is not words: {said}");
	}

	#[test]
	fn a_failed_renderer_build_is_said_and_built_again_from_clean() {
		// With the target installed, "install the target" is no answer: the build said why.
		let said = no_renderer("failed\ncompiling crates/dash/vag-dash-wasm for wasm32-unknown-unknown failed: error[E0425] / x");
		assert!(said.contains("its build failed — compiling crates/dash/vag-dash-wasm"), "{said}");
		assert!(said.contains("error[E0425]"), "{said}");
		assert!(said.contains("cargo clean -p vag-cli-diag"), "{said}");
		assert!(!said.contains("rustup target add"), "{said}");
	}

	#[test]
	fn the_reason_build_rs_writes_is_the_one_read_here() {
		// The kinds are two strings in two files; this holds them together.
		let script = include_str!("../../build.rs");
		assert!(script.contains("const NOT_INSTALLED: &str = \"not-installed\";"));
		assert!(script.contains("const FAILED: &str = \"failed\";"));
		if WASM.is_empty() {
			let kind = WASM_WHY.lines().next().unwrap_or_default();
			assert!(kind == "not-installed" || kind == "failed", "{WASM_WHY:?}");
		} else {
			assert!(WASM_WHY.is_empty(), "{WASM_WHY:?}");
		}
	}
}
