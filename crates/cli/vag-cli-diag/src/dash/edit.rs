//! `vagcan dev dash edit <VIN>` — the dash's pages laid out in a browser, and saved.
//!
//! The page (`editor.html`) draws the panel with the board's own renderer compiled to
//! WebAssembly (`crates/dash/vag-dash-wasm`, embedded by this crate's build script), offers the
//! car's channels as [`super::catalog`] resolves them, and opens on the pages of the car's
//! `dash.toml`. Save writes them back into that file ([`super::save`]): the server builds the
//! plan from the new text first, exactly as `dev dash build` does ([`CarBuild`]), and writes
//! nothing when the build refuses; when it builds, `dash.toml` is replaced in one step and
//! `plan.json` and `plan.rs` beside it as `dev dash build` writes them.
//!
//! A save carries the fingerprint of the file the page opened on ([`file_fingerprint`]); a
//! file changed on disk since — by hand, or by another page — is refused, never overwritten.
//!
//! What the page loads is written first into `~/.vagcan/dash/<VIN>/editor/`
//! ([`vag_cli_core::datadir::dash_editor_dir`]) and served from there by [`serve`], on
//! `127.0.0.1` only, for `GET` and `HEAD` of a file directly in that directory; `layout.json`
//! is answered from the file as it is at that moment. The one request that writes is
//! `POST /save`, taken only from this server's own page: its `Host`, its `Origin`, and the
//! token this run made, which only `layout.json` carries ([`Editor::respond`]). The page asks
//! for nothing but these: its Content-Security-Policy says so to the browser too.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use vag_cli_core::config::Language;
use vag_cli_core::dash::{self, Input, PageInput};
use vag_cli_core::marks::facts;
use vag_cli_core::plan::UnitIdentity;
use vag_data_labels::catalog::CatalogStore;

use super::{catalog, save};

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
	let vin = vin.trim();
	let record = vag_cli_core::datadir::units_record(vin)?;
	let units = vag_cli_core::units::read_record(&record)?.ok_or_else(|| dash::no_record(vin, &record))?;
	// The car's own pages, when it has a `dash.toml`. One that does not parse is refused here
	// with the build's own words: the editor would open on something other than the file.
	let input_path = vag_cli_core::datadir::dash_dir(vin)?.join("dash.toml");
	let (_, input) = read_input(vin, &input_path)?;
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
	// `layout.json` is answered from `dash.toml` as it is when the page asks; an older run
	// wrote it here, and a file left beside the page would be a second answer.
	let _ = std::fs::remove_file(dir.join("layout.json"));
	write(&dir.join("index.html"), PAGE.as_bytes())?;
	write(&dir.join("dash.wasm"), WASM)?;
	match made {
		Made::Kept(n) => vag_cli_core::note!("channels  {n}, kept from the last run: {}", dir.join("channels.json").display()),
		Made::Built(n, numeric) => vag_cli_core::note!("channels  {n}, {numeric} of them numbers: {}", dir.join("channels.json").display()),
	}
	let editor = Arc::new(Editor::new(
		dir.clone(),
		vin,
		input_path.clone(),
		token()?,
		names(&dir.join("channels.json")),
		CarBuild {
			vin: vin.to_string(),
			input: input_path.clone(),
		},
	));

	let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
		.await
		.context("opening a port on 127.0.0.1")?;
	let port = listener.local_addr()?.port();
	let url = format!("http://127.0.0.1:{port}/");
	// The address is what the command makes: data. How to use it is a note.
	vag_cli_core::outln!("{url}");
	vag_cli_core::note!("serving   {} on 127.0.0.1 only", dir.display());
	vag_cli_core::say!(
		"Save on the page writes {} once the build accepts it",
		vag_cli_core::datadir::tilde(&input_path)
	);
	vag_cli_core::say!("Ctrl-C ends it");
	vag_cli_core::desktop::open(&url);
	serve(listener, editor).await
}

/// `dash.toml` at `path` as it is now — its bytes, or `None` when there is none — and parsed,
/// for `vin`. A file that does not parse, or is another car's, is refused with the build's
/// words.
fn read_input(vin: &str, path: &Path) -> Result<(Option<Vec<u8>>, Option<Input>)> {
	let bytes = match std::fs::read(path) {
		Ok(bytes) => bytes,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((None, None)),
		Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
	};
	let text = std::str::from_utf8(&bytes).with_context(|| format!("{} is not UTF-8 text", path.display()))?;
	let input = dash::parse_input(text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
	if !input.vin.eq_ignore_ascii_case(vin.trim()) {
		bail!("{} is for VIN {} but the editor was asked for {vin}", path.display(), input.vin);
	}
	Ok((Some(bytes), Some(input)))
}

/// What a save checks a page against: the file's bytes, or that there is none. The length and
/// a 64-bit FNV-1a of the bytes — a change by hand, not an attack, is what it has to see.
pub fn file_fingerprint(bytes: Option<&[u8]>) -> String {
	let Some(bytes) = bytes else { return "absent".to_string() };
	let hash = bytes
		.iter()
		.fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3));
	format!("{}-{hash:016x}", bytes.len())
}

/// The token this run's page sends with a save: 128 bits from the system's random source, as
/// hex. Never printed: only `layout.json`, which only this server's own page can read, holds it.
fn token() -> Result<String> {
	let mut bytes = [0u8; 16];
	getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no random bytes from the system for the editor's save token: {e}"))?;
	Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Each channel of `channels.json` by every spelling of it, to its name and unit of measure:
/// the comment on a `[[channel]]` a save adds. Nothing when the file does not read.
fn names(path: &Path) -> BTreeMap<String, String> {
	let rows: Vec<Value> = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
	let mut out = BTreeMap::new();
	for row in rows {
		let name = row["name"].as_str().unwrap_or_default();
		let uom = row["uom"].as_str().unwrap_or_default();
		let said = if uom.is_empty() { name.to_string() } else { format!("{name}, {uom}") };
		let spellings = row["ref"]
			.as_str()
			.into_iter()
			.chain(row["aliases"].as_array().into_iter().flatten().filter_map(Value::as_str));
		for spelling in spellings {
			out.entry(spelling.to_string()).or_insert_with(|| said.clone());
		}
	}
	out
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
	let stores = vag_cli_core::project::stores()?;
	// The run's one source, and only its files: the catalog is that source's channels.
	let run = vag_cli_core::source::run(&stores)?;
	let default_language = vag_cli_core::config::language(&vag_cli_core::config::load());
	let mut sources = vec![vag_cli_core::datadir::units_record(vin)?];
	sources.extend(run.iter().flat_map(vag_cli_core::source::Run::files));
	sources.extend([stores.measurements_dir(), vag_cli_core::glossary::path()?]);
	// Not `config.toml` itself: `watch` writes its marks there, and the one thing in it the
	// catalog reads — the language — is in the key below.
	// This build of the tool: a new one may resolve differently.
	if let Ok(me) = std::env::current_exe() {
		sources.push(me);
	}
	let key = format!(
		"format {CATALOG_FORMAT}\nproject {}\nsource {}\nlanguage {}\n{}",
		stores.id(),
		run.as_ref().map_or("none", |r| r.source.word()),
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
	let store = CatalogStore::open(stores.measurements_dir());
	let extracted = match &run {
		Some(run) => vag_cli_core::extracted::open_run(run),
		None => vag_cli_core::extracted::Extracted::none(),
	}
	.for_car(Some(vin));
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
/// board's limits. With no `dash.toml`, no pages. Each page says which of the file's it is
/// (`from`), so a save keeps that table's comments.
pub fn layout(vin: &str, input: Option<&Input>, from_file: bool) -> Value {
	let pages: Vec<Value> = input
		.map(|i| i.pages.as_slice())
		.unwrap_or_default()
		.iter()
		.enumerate()
		.map(|(from, p)| match p {
			PageInput::Values { title, cells, volts } => json!({
				"kind": "values",
				"title": title,
				"cells": cells.iter().map(ToString::to_string).collect::<Vec<_>>(),
				"volts": volts.unwrap_or(true),
				"from": from,
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
				"from": from,
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

/// What a save's build made, for the line that says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Saved {
	pub plan: u64,
	pub pages: usize,
	pub channels: usize,
	pub units: usize,
}

impl Saved {
	pub fn of(plan: &dash::Plan) -> Saved {
		Saved {
			plan: dash::plan_id(plan),
			pages: plan.pages.len(),
			channels: plan.channels.len(),
			units: plan.units.len(),
		}
	}

	/// `plan 0123456789abcdef · 6 pages · 26 channels on 5 units`.
	fn facts(&self) -> String {
		let count = |n: usize, what: &str| format!("{n} {}", crate::render::plural(n, what));
		facts([
			format!("plan {:016x}", self.plan),
			count(self.pages, "page"),
			format!("{} on {}", count(self.channels, "channel"), count(self.units, "unit")),
		])
	}
}

/// The build a save goes through: the text checked before anything is written, the plan
/// written after `dash.toml` is.
pub trait Build: Send + Sync + 'static {
	type Checked;
	/// Build the plan from `text` and write nothing; the build's refusal as the error.
	fn check(&self, text: &str) -> Result<Self::Checked>;
	/// Write the plan [`Build::check`] made.
	fn write(&self, checked: Self::Checked) -> Result<Saved>;
}

/// `dev dash build`'s own build, through the same functions: the input read from the text,
/// the car's record and the run's one source ([`dash::resolve`]), the plan written where that
/// command writes it ([`dash::write_plan`]).
pub struct CarBuild {
	pub vin: String,
	/// `~/.vagcan/dash/<VIN>/dash.toml`, which the refusals name.
	pub input: PathBuf,
}

impl Build for CarBuild {
	type Checked = dash::Resolved;

	fn check(&self, text: &str) -> Result<dash::Resolved> {
		dash::resolve(dash::inputs_from_text(&self.vin, text, self.input.clone())?)
	}

	fn write(&self, checked: dash::Resolved) -> Result<Saved> {
		Ok(Saved::of(&dash::write_plan(checked)?.built.plan))
	}
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
	/// The methods this server takes at that path, said with a 405.
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

	fn json(status: u16, reason: &'static str, value: &Value) -> Response {
		Response {
			status,
			reason,
			content_type: "application/json",
			body: serde_json::to_vec(value).unwrap_or_default(),
			head_only: false,
			allow: None,
		}
	}

	/// A save or a layout that did not happen, said to the page in cargo's words.
	fn said(status: u16, reason: &'static str, said: &str) -> Response {
		Response::json(status, reason, &json!({ "error": said }))
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

/// The largest save request taken. The page's state for every page the board holds is a few
/// kilobytes.
pub const MAX_BODY: usize = 1024 * 1024;

/// One request's head, split.
struct Request<'a> {
	method: &'a str,
	target: &'a str,
	/// The target without its query.
	path: &'a str,
	headers: Vec<(String, &'a str)>,
}

/// A header sent twice where once is the rule: a request that does it is refused, not read
/// one way here and another way by whatever sent it.
struct Twice;

impl<'a> Request<'a> {
	fn parse(head: &'a str) -> Request<'a> {
		let mut lines = head.split("\r\n");
		let mut parts = lines.next().unwrap_or_default().split(' ');
		let (method, target) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
		let headers = lines
			.filter_map(|l| l.split_once(':'))
			.map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim()))
			.collect();
		Request {
			method,
			target,
			path: target.split(['?', '#']).next().unwrap_or_default(),
			headers,
		}
	}

	fn header(&self, name: &str) -> Result<Option<&'a str>, Twice> {
		let mut found = self.headers.iter().filter(|(k, _)| k == name).map(|(_, v)| *v);
		match (found.next(), found.next()) {
			(_, Some(_)) => Err(Twice),
			(one, None) => Ok(one),
		}
	}

	/// Whether `Host` names this server: a page on another site that has a name resolve to
	/// 127.0.0.1 is refused rather than answered (DNS rebinding).
	fn to_us(&self, port: u16) -> bool {
		let ours = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
		matches!(self.header("host"), Ok(Some(h)) if ours.iter().any(|o| o.eq_ignore_ascii_case(h)))
	}

	/// The body length a save request declares, when it is one this server reads.
	fn save_length(&self) -> Option<usize> {
		if self.method != "POST" || self.path != "/save" {
			return None;
		}
		let length = self.header("content-length").ok()??;
		length
			.bytes()
			.all(|b| b.is_ascii_digit())
			.then(|| length.parse().ok())
			.flatten()
			.filter(|&n| n <= MAX_BODY)
	}
}

/// The body length `head` declares for a request whose body [`Editor::respond`] reads: a save
/// within [`MAX_BODY`]. 0 for anything else, whose body is not read.
pub fn body_length(head: &str) -> usize {
	Request::parse(head).save_length().unwrap_or(0)
}

/// Two secrets compared in a time that does not depend on where they first differ.
fn same_secret(a: &[u8], b: &[u8]) -> bool {
	a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// The server's state: where its files are, the car's `dash.toml`, the run's save token and
/// the build a save goes through.
pub struct Editor<B> {
	dir: PathBuf,
	vin: String,
	input: PathBuf,
	token: String,
	/// A channel's name and unit of measure, by each spelling of it ([`names`]).
	names: BTreeMap<String, String>,
	build: B,
	/// One save at a time: two pages saving at once would each check the file the other is
	/// replacing.
	saving: std::sync::Mutex<()>,
}

impl<B: Build> Editor<B> {
	pub fn new(dir: PathBuf, vin: &str, input: PathBuf, token: String, names: BTreeMap<String, String>, build: B) -> Editor<B> {
		Editor {
			dir,
			vin: vin.trim().to_string(),
			input,
			token,
			names,
			build,
			saving: std::sync::Mutex::new(()),
		}
	}

	/// What this server answers to one request — its head (the request line and headers) and
	/// the body [`body_length`] said to read — reached at `port`.
	///
	/// `GET` and `HEAD`, with a `Host` of this server, of a plain file name directly in the
	/// directory — no separator, no `..`, no leading dot, no escapes, no link — and of
	/// `layout.json`, made from `dash.toml` now. A `HEAD` gets no body, whatever the answer.
	///
	/// `POST /save`, and only when every one of these holds: the `Host` is this server; the
	/// `Origin` is exactly `http://127.0.0.1:<port>` or `http://localhost:<port>`; the
	/// `X-Vagcan-Token` is this run's; the body is `application/json` of a `Content-Length`
	/// within [`MAX_BODY`]. Any other method is refused, `OPTIONS` too, so a browser's
	/// cross-origin preflight fails before a save is sent.
	pub fn respond(&self, port: u16, head: &str, body: &[u8]) -> Response {
		let request = Request::parse(head);
		if request.path == "/save" {
			return match request.method {
				"POST" => self.post_save(port, &request, body),
				method => Response {
					allow: Some("POST"),
					head_only: method == "HEAD",
					..Response::refuse(405, "Method Not Allowed")
				},
			};
		}
		let head_only = match request.method {
			"GET" => false,
			"HEAD" => true,
			_ => {
				return Response {
					allow: Some("GET, HEAD"),
					..Response::refuse(405, "Method Not Allowed")
				};
			}
		};
		let answer = if !request.to_us(port) {
			Response::refuse(403, "Forbidden")
		} else if request.path == "/layout.json" {
			self.layout_now(&request)
		} else {
			file(&self.dir, request.target)
		};
		Response { head_only, ..answer }
	}

	/// `layout.json`: [`layout`] of the file as it is now, with its fingerprint and the token a
	/// save sends back. Only to this server's own pages: a browser that says the request came
	/// from another site is refused, though no other site could read the answer anyway.
	fn layout_now(&self, request: &Request<'_>) -> Response {
		if !matches!(request.header("sec-fetch-site"), Ok(None | Some("same-origin" | "none"))) {
			return Response::refuse(403, "Forbidden");
		}
		match read_input(&self.vin, &self.input) {
			Ok((bytes, input)) => {
				let mut layout = layout(&self.vin, input.as_ref(), bytes.is_some());
				layout["fingerprint"] = json!(file_fingerprint(bytes.as_deref()));
				layout["token"] = json!(self.token);
				Response::json(200, "OK", &layout)
			}
			Err(e) => Response::said(409, "Conflict", &vag_cli_core::report::render(&e, 0, vag_cli_core::report::Colour::Never)),
		}
	}

	/// The checks of `POST /save` that are about who sent it and how, then [`Editor::save`].
	fn post_save(&self, port: u16, request: &Request<'_>, body: &[u8]) -> Response {
		let (Ok(origin), Ok(token), Ok(site), Ok(kind), Ok(length), Ok(chunked)) = (
			request.header("origin"),
			request.header("x-vagcan-token"),
			request.header("sec-fetch-site"),
			request.header("content-type"),
			request.header("content-length"),
			request.header("transfer-encoding"),
		) else {
			return Response::refuse(400, "Bad Request");
		};
		let origins = [format!("http://127.0.0.1:{port}"), format!("http://localhost:{port}")];
		let from_us = request.to_us(port)
			&& origin.is_some_and(|o| origins.iter().any(|ours| o == ours))
			&& matches!(site, None | Some("same-origin"))
			&& token.is_some_and(|t| same_secret(t.as_bytes(), self.token.as_bytes()));
		if !from_us {
			vag_cli_core::say!("error: a save request that did not come from this editor's page was refused");
			return Response::refuse(403, "Forbidden");
		}
		let json = kind
			.and_then(|k| k.split(';').next())
			.is_some_and(|k| k.trim().eq_ignore_ascii_case("application/json"));
		if !json {
			return Response::refuse(415, "Unsupported Media Type");
		}
		if chunked.is_some() {
			return Response::refuse(400, "Bad Request");
		}
		let Some(length) = length else {
			return Response::refuse(411, "Length Required");
		};
		let Some(length) = length.bytes().all(|b| b.is_ascii_digit()).then(|| length.parse::<usize>().ok()).flatten() else {
			return Response::refuse(400, "Bad Request");
		};
		if length > MAX_BODY {
			return Response::refuse(413, "Content Too Large");
		}
		if length != body.len() {
			return Response::refuse(400, "Bad Request");
		}
		self.save(body)
	}

	/// A save: the page's state checked, made into the file, built, and only then written.
	fn save(&self, body: &[u8]) -> Response {
		let _one = self.saving.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
		let shown = vag_cli_core::datadir::tilde(&self.input);
		let refused = |status: u16, reason: &'static str, err: anyhow::Error| {
			vag_cli_core::say!("error: {shown} not saved: {err}");
			Response::said(
				status,
				reason,
				&vag_cli_core::report::render(&err, 0, vag_cli_core::report::Colour::Never),
			)
		};
		let body: save::Body = match serde_json::from_slice(body) {
			Ok(body) => body,
			Err(e) => {
				return refused(
					400,
					"Bad Request",
					anyhow::anyhow!("the save request is not one this editor's page sends: {e}"),
				);
			}
		};
		let current = match std::fs::read(&self.input) {
			Ok(bytes) => Some(bytes),
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
			Err(e) => return refused(500, "Internal Server Error", anyhow::Error::new(e).context(format!("reading {shown}"))),
		};
		if body.fingerprint != file_fingerprint(current.as_deref()) {
			use vag_cli_core::report::Advice as _;
			let err = anyhow::anyhow!("the file changed on disk since the page read it").help("reload the page: it opens on the file as it is now");
			return refused(409, "Conflict", err);
		}
		let current = match current.map(String::from_utf8).transpose() {
			Ok(text) => text,
			Err(_) => return refused(500, "Internal Server Error", anyhow::anyhow!("{shown} is not UTF-8 text")),
		};
		let wanted = match body.check(current.as_deref().map_or(0, save::pages_in)) {
			Ok(wanted) => wanted,
			Err(why) => return refused(400, "Bad Request", anyhow::anyhow!(why)),
		};
		let text = match save::edit(current.as_deref(), &self.vin, &wanted, |r| self.names.get(&r.to_string()).cloned()) {
			Ok(text) => text,
			Err(why) => return refused(500, "Internal Server Error", anyhow::anyhow!(why)),
		};
		let checked = match self.build.check(&text) {
			Ok(checked) => checked,
			Err(err) => return refused(422, "Unprocessable Content", err),
		};
		if let Err(err) = vag_cli_core::datadir::replace_file(&self.input, text.as_bytes()) {
			return refused(500, "Internal Server Error", err);
		}
		let saved = match self.build.write(checked) {
			Ok(saved) => saved,
			Err(err) => {
				use vag_cli_core::report::Advice as _;
				let err = err
					.context(format!("{shown} is saved, and its plan is not"))
					.help(format!("run `vagcan dev dash build {}`", self.vin));
				vag_cli_core::say!("error: {err}");
				return Response::said(
					500,
					"Internal Server Error",
					&vag_cli_core::report::render(&err, 0, vag_cli_core::report::Colour::Never),
				);
			}
		};
		vag_cli_core::say!("{}", facts([format!("saved {shown}"), format!("plan {:016x}", saved.plan)]));
		let layout = match read_input(&self.vin, &self.input) {
			Ok((bytes, input)) => {
				let mut layout = layout(&self.vin, input.as_ref(), bytes.is_some());
				layout["fingerprint"] = json!(file_fingerprint(bytes.as_deref()));
				layout["token"] = json!(self.token);
				layout
			}
			Err(_) => Value::Null,
		};
		Response::json(
			200,
			"OK",
			&json!({ "saved": facts(["saved".to_string(), saved.facts()]), "layout": layout }),
		)
	}
}

/// What `GET` of `target` answers: the file it names in `dir`, or the refusal.
fn file(dir: &Path, target: &str) -> Response {
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

/// How long one connection may take to send its request, head and body.
const READ_FOR: std::time::Duration = std::time::Duration::from_secs(10);

/// One request off `stream`: its head, and the body [`body_length`] says to read. `None` for
/// a head longer than [`MAX_HEAD`]; an error for a connection that closed or went quiet first.
async fn read_request(stream: &mut tokio::net::TcpStream) -> std::result::Result<Option<(String, Vec<u8>)>, ()> {
	let mut got = Vec::new();
	let mut buf = [0u8; 4096];
	let end = loop {
		if let Some(at) = got.windows(4).position(|w| w == b"\r\n\r\n") {
			break at;
		}
		if got.len() >= MAX_HEAD {
			return Ok(None);
		}
		match stream.read(&mut buf).await {
			Ok(0) | Err(_) => return Err(()),
			Ok(n) => got.extend_from_slice(&buf[..n]),
		}
	};
	if end >= MAX_HEAD {
		return Ok(None);
	}
	let head = String::from_utf8_lossy(&got[..end + 4]).into_owned();
	let wanted = body_length(&head);
	let mut body = got.split_off(end + 4);
	while body.len() < wanted {
		match stream.read(&mut buf).await {
			Ok(0) | Err(_) => break,
			Ok(n) => body.extend_from_slice(&buf[..n]),
		}
	}
	body.truncate(wanted);
	Ok(Some((head, body)))
}

/// Serve `editor` on `listener` until the process ends.
pub async fn serve<B: Build>(listener: tokio::net::TcpListener, editor: Arc<Editor<B>>) -> Result<()> {
	let port = listener.local_addr()?.port();
	loop {
		let Ok((mut stream, _)) = listener.accept().await else {
			continue;
		};
		let editor = editor.clone();
		tokio::spawn(async move {
			let (line, response) = match tokio::time::timeout(READ_FOR, read_request(&mut stream)).await {
				Ok(Ok(Some((head, body)))) => {
					let line = head.lines().next().unwrap_or_default().to_string();
					// Off the runtime's threads: a save builds a plan, which reads the project.
					let answered = tokio::task::spawn_blocking(move || editor.respond(port, &head, &body)).await;
					(line, answered.unwrap_or_else(|_| Response::refuse(500, "Internal Server Error")))
				}
				Ok(Ok(None)) => (String::new(), Response::refuse(431, "Request Header Fields Too Large")),
				_ => return,
			};
			vag_cli_core::wire!("{line} → {} {}", response.status, response.reason);
			let _ = stream.write_all(&response.bytes()).await;
			let _ = stream.shutdown().await;
		});
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const PORT: u16 = 41234;
	const VIN: &str = "TESTVIN0000000001";
	const TOKEN: &str = "0123456789abcdef0123456789abcdef";

	/// A build that checks what `dash.toml` parses to and nothing about a car: the server's
	/// part of a save, without a project. Refuses when told to; counts what it wrote.
	#[derive(Default)]
	struct Stub {
		refuse: Option<&'static str>,
		wrote: std::sync::Mutex<Vec<String>>,
	}

	impl Build for Stub {
		type Checked = String;

		fn check(&self, text: &str) -> Result<String> {
			use vag_cli_core::report::Advice as _;
			dash::parse_input(text)?;
			match self.refuse {
				Some(why) => Err(anyhow::anyhow!(why).help("write it another way")),
				None => Ok(text.to_string()),
			}
		}

		fn write(&self, text: String) -> Result<Saved> {
			let input = dash::parse_input(&text)?;
			self.wrote.lock().unwrap().push(text);
			Ok(Saved {
				plan: 0xabc,
				pages: input.pages.len(),
				channels: input.channels.len(),
				units: 1,
			})
		}
	}

	const FILE: &str = "vin = \"TESTVIN0000000001\"\n\n# the only one\n[[channel]]\nref = \"01:IDE00001\"\nalarm_max = 110\n\n[[page]]\nkind = \"values\"\ncells = [\"01:IDE00001\"]\n";

	struct Served {
		_root: tempfile::TempDir,
		dir: PathBuf,
		input: PathBuf,
		editor: Editor<Stub>,
	}

	fn served_with(build: Stub, file: Option<&str>) -> Served {
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
		std::fs::write(root.path().join("secret.toml"), "vin = \"SECRET\"").unwrap();
		let input = root.path().join("dash.toml");
		if let Some(text) = file {
			std::fs::write(&input, text).unwrap();
		}
		let names = BTreeMap::from([("01:IDE00002".to_string(), "Boost, kPa".to_string())]);
		let editor = Editor::new(dir.clone(), VIN, input.clone(), TOKEN.to_string(), names, build);
		Served {
			_root: root,
			dir,
			input,
			editor,
		}
	}

	fn served() -> Served {
		served_with(Stub::default(), Some(FILE))
	}

	fn get(s: &Served, target: &str) -> Response {
		s.editor
			.respond(PORT, &format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\n\r\n"), &[])
	}

	/// A save request the page sends, with its headers; `headers` replaces or drops one by
	/// name (`None` drops it).
	fn post(s: &Served, body: &str, change: &[(&str, Option<&str>)]) -> Response {
		let length = body.len().to_string();
		let mut headers: Vec<(String, String)> = [
			("Host", format!("127.0.0.1:{PORT}")),
			("Origin", format!("http://127.0.0.1:{PORT}")),
			("X-Vagcan-Token", TOKEN.to_string()),
			("Content-Type", "application/json".to_string()),
			("Content-Length", length),
		]
		.into_iter()
		.map(|(k, v)| (k.to_string(), v))
		.collect();
		for (name, value) in change {
			headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
			if let Some(value) = value {
				headers.push((name.to_string(), value.to_string()));
			}
		}
		let head: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
		s.editor.respond(PORT, &format!("POST /save HTTP/1.1\r\n{head}\r\n"), body.as_bytes())
	}

	fn layout_of(s: &Served) -> Value {
		let r = get(s, "/layout.json");
		assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
		serde_json::from_slice(&r.body).unwrap()
	}

	/// The page's state as it opened, with one page changed: what Save sends.
	fn body(layout: &Value) -> Value {
		json!({
			"fingerprint": layout["fingerprint"],
			"pages": [
				{"kind": "values", "title": "MAIN", "cells": ["01:IDE00001", "01:IDE00002"], "volts": true, "from": 0},
			],
			"labels": {"01:IDE00002": "BOOST"},
		})
	}

	/// The file's bytes and modification time: what "nothing was written" means.
	fn on_disk(path: &Path) -> Option<(Vec<u8>, std::time::SystemTime)> {
		let bytes = std::fs::read(path).ok()?;
		Some((bytes, std::fs::metadata(path).unwrap().modified().unwrap()))
	}

	#[test]
	fn the_files_in_the_directory_are_served_with_their_types() {
		let s = served();
		let page = get(&s, "/");
		assert_eq!(
			(page.status, page.content_type, page.body.as_slice()),
			(200, "text/html; charset=utf-8", b"<p>page</p>".as_slice())
		);
		assert_eq!(get(&s, "/index.html"), page);
		assert_eq!(
			get(&s, "/channels.json?v=2").content_type,
			"application/json",
			"a query is not part of the name"
		);
		assert_eq!(get(&s, "/dash.wasm").content_type, "application/wasm");
		let head = s
			.editor
			.respond(PORT, &format!("HEAD /channels.json HTTP/1.1\r\nHost: localhost:{PORT}\r\n\r\n"), &[]);
		assert!(head.head_only && head.status == 200);
		assert!(
			String::from_utf8(head.bytes()).unwrap().ends_with("\r\n\r\n"),
			"no body after HEAD's headers"
		);
	}

	#[test]
	fn nothing_outside_the_directory_or_under_it_is_served() {
		let s = served();
		for target in [
			"/../secret.toml",
			"/..%2Fsecret.toml",
			"/%2e%2e/secret.toml",
			"/..",
			"/.hidden",
			"/sub/inner.json",
			"/sub",
			"//etc/passwd",
			"/%2Fetc%2Fpasswd",
			"/..\\secret.toml",
			"/nope.html",
			"secret.toml",
			"http://127.0.0.1/secret.toml",
			"/../dash.toml",
		] {
			let r = get(&s, target);
			assert!(r.status == 404 || r.status == 400, "{target}: {}", r.status);
			assert!(!String::from_utf8_lossy(&r.body).contains("SECRET"), "{target}");
		}
	}

	#[cfg(unix)]
	#[test]
	fn a_link_in_the_directory_is_not_followed() {
		let s = served();
		std::os::unix::fs::symlink(s.dir.parent().unwrap().join("secret.toml"), s.dir.join("linked.json")).unwrap();
		assert_eq!(get(&s, "/linked.json").status, 404);
	}

	#[test]
	fn only_get_and_head_and_only_this_server_s_host() {
		let s = served();
		for method in ["POST", "PUT", "DELETE", "OPTIONS"] {
			let r = s
				.editor
				.respond(PORT, &format!("{method} / HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\n\r\n"), &[]);
			assert_eq!(r.status, 405, "{method}");
			let said = String::from_utf8(r.bytes()).unwrap();
			assert!(said.contains("\r\nAllow: GET, HEAD\r\n"), "{method}: {said}");
		}
		let said = String::from_utf8(get(&s, "/").bytes()).unwrap();
		assert!(!said.contains("Allow:"), "only a 405 says it: {said}");
		// A refused HEAD is a HEAD: headers, no body.
		for (target, host, status) in [
			("/nope.html", "127.0.0.1", 404),
			("/", "evil.example", 403),
			("/../dash.toml", "127.0.0.1", 404),
			("/save", "127.0.0.1", 405),
		] {
			let host = if host == "127.0.0.1" {
				format!("{host}:{PORT}")
			} else {
				host.to_string()
			};
			let r = s.editor.respond(PORT, &format!("HEAD {target} HTTP/1.1\r\nHost: {host}\r\n\r\n"), &[]);
			assert_eq!(r.status, status, "{target}");
			let said = String::from_utf8(r.bytes()).unwrap();
			assert!(said.ends_with("\r\n\r\n"), "no body after a refused HEAD: {said:?}");
		}
		for host in ["", "Host: evil.example\r\n", "Host: 127.0.0.1:1\r\n", "Host: 127.0.0.1\r\n"] {
			for target in ["/", "/layout.json"] {
				let r = s.editor.respond(PORT, &format!("GET {target} HTTP/1.1\r\n{host}\r\n"), &[]);
				assert_eq!(r.status, 403, "{target} {host:?}");
			}
		}
	}

	#[test]
	fn every_answer_tells_the_browser_to_load_nothing_from_elsewhere() {
		let s = served();
		for r in [get(&s, "/"), get(&s, "/layout.json"), post(&s, "{}", &[("X-Vagcan-Token", None)])] {
			let said = String::from_utf8(r.bytes()).unwrap();
			assert!(said.contains("Content-Security-Policy: default-src 'self';"), "{said}");
		}
		assert!(String::from_utf8(get(&s, "/").bytes()).unwrap().starts_with("HTTP/1.1 200 OK\r\n"));
	}

	#[tokio::test]
	async fn the_server_answers_on_127_0_0_1() {
		let s = served();
		let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
		let port = listener.local_addr().unwrap().port();
		let input = s.input.clone();
		let editor = Arc::new(Editor::new(
			s.dir.clone(),
			VIN,
			input.clone(),
			TOKEN.to_string(),
			BTreeMap::new(),
			Stub::default(),
		));
		tokio::spawn(serve(listener, editor));
		let ask = |request: String| async move {
			let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
			stream.write_all(request.as_bytes()).await.unwrap();
			let mut said = String::new();
			stream.read_to_string(&mut said).await.unwrap();
			said
		};
		let said = ask(format!("GET /channels.json HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")).await;
		assert!(said.starts_with("HTTP/1.1 200 OK\r\n") && said.ends_with("\r\n\r\n[]"), "{said}");

		// A save, through the socket, its body after the head.
		let said = ask(format!("GET /layout.json HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n")).await;
		let layout: Value = serde_json::from_str(said.split_once("\r\n\r\n").unwrap().1).unwrap();
		let body = body(&layout).to_string();
		let said = ask(format!(
			"POST /save HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://localhost:{port}\r\nX-Vagcan-Token: {TOKEN}\r\n\
			 Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
			body.len()
		))
		.await;
		assert!(said.starts_with("HTTP/1.1 200 OK\r\n"), "{said}");
		let text = std::fs::read_to_string(&input).unwrap();
		assert!(text.contains("ref = \"01:IDE00002\"\nlabel = \"BOOST\"\n"), "{text}");
		assert!(
			text.starts_with("vin = \"TESTVIN0000000001\"\n\n# the only one\n[[channel]]\nref = \"01:IDE00001\"\nalarm_max = 110\n"),
			"{text}"
		);
	}

	#[test]
	fn the_layout_is_the_file_as_it_is_now_with_its_fingerprint_and_the_token() {
		let s = served();
		let layout = layout_of(&s);
		assert_eq!(layout["fingerprint"], file_fingerprint(Some(FILE.as_bytes())));
		assert_eq!(layout["token"], TOKEN);
		assert_eq!(layout["pages"][0]["from"], 0);
		// Changed by hand: the next read says so.
		std::fs::write(&s.input, FILE.replace("110", "115")).unwrap();
		assert_ne!(layout_of(&s)["fingerprint"], layout["fingerprint"]);
		// Broken by hand: the build's words, not a page on something else.
		std::fs::write(&s.input, "vin = \"TESTVIN0000000001\"\n").unwrap();
		let r = get(&s, "/layout.json");
		assert_eq!(r.status, 409);
		let said: Value = serde_json::from_slice(&r.body).unwrap();
		assert!(said["error"].as_str().unwrap().starts_with("error: "), "{said}");
		// A browser that says another site asked is refused the token.
		std::fs::write(&s.input, FILE).unwrap();
		let r = s.editor.respond(
			PORT,
			&format!("GET /layout.json HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\nSec-Fetch-Site: cross-site\r\n\r\n"),
			&[],
		);
		assert_eq!(r.status, 403);
		assert!(!String::from_utf8_lossy(&r.body).contains(TOKEN));
		// No file yet.
		let none = served_with(Stub::default(), None);
		let layout = layout_of(&none);
		assert_eq!(
			(layout["fingerprint"].clone(), layout["fromFile"].clone()),
			(json!("absent"), json!(false))
		);
	}

	#[test]
	fn a_save_writes_the_file_and_the_plan_and_answers_with_the_new_layout() {
		let s = served();
		let r = post(&s, &body(&layout_of(&s)).to_string(), &[]);
		assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
		let said: Value = serde_json::from_slice(&r.body).unwrap();
		assert_eq!(said["saved"], "saved · plan 0000000000000abc · 1 page · 2 channels on 1 unit");
		let text = std::fs::read_to_string(&s.input).unwrap();
		assert_eq!(
			text,
			FILE
				.replace(
					"alarm_max = 110\n",
					"alarm_max = 110\n\n[[channel]]\nref = \"01:IDE00002\"   # Boost, kPa\nlabel = \"BOOST\"\n"
				)
				.replace("kind = \"values\"\n", "kind = \"values\"\ntitle = \"MAIN\"\n")
				.replace("[\"01:IDE00001\"]", "[\"01:IDE00001\", \"01:IDE00002\"]")
		);
		assert_eq!(
			*s.editor.build.wrote.lock().unwrap(),
			std::slice::from_ref(&text),
			"the plan of the file written"
		);
		assert_eq!(said["layout"]["fingerprint"], file_fingerprint(Some(text.as_bytes())));
		assert_eq!(said["layout"]["token"], TOKEN);
		assert_eq!(said["layout"]["pages"][0]["title"], "MAIN");
		// The page saves again from the layout it was given.
		let again = post(&s, &body(&said["layout"]).to_string(), &[]);
		assert_eq!(again.status, 200);
		assert_eq!(std::fs::read_to_string(&s.input).unwrap(), text, "the same state, the same file");
	}

	#[test]
	fn a_car_with_no_file_gets_one() {
		let s = served_with(Stub::default(), None);
		let r = post(&s, &body(&layout_of(&s)).to_string().replace(",\"from\":0", ""), &[]);
		assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
		let text = std::fs::read_to_string(&s.input).unwrap();
		assert!(
			text.starts_with("vin = \"TESTVIN0000000001\"\n\n[[channel]]\nref = \"01:IDE00001\"\n"),
			"{text}"
		);
		dash::parse_input(&text).unwrap();
	}

	#[test]
	fn a_refused_build_writes_nothing_and_says_why() {
		let s = served_with(
			Stub {
				refuse: Some("01:IDE00002: unit 01 has no channel with the id IDE00002"),
				..Stub::default()
			},
			Some(FILE),
		);
		let before = on_disk(&s.input);
		let r = post(&s, &body(&layout_of(&s)).to_string(), &[]);
		assert_eq!(r.status, 422);
		let said: Value = serde_json::from_slice(&r.body).unwrap();
		assert_eq!(
			said["error"],
			"error: 01:IDE00002: unit 01 has no channel with the id IDE00002\n\nhelp: write it another way\n"
		);
		assert_eq!(on_disk(&s.input), before);
		assert!(s.editor.build.wrote.lock().unwrap().is_empty());
	}

	#[test]
	fn a_file_changed_on_disk_is_never_overwritten() {
		let s = served();
		let body = body(&layout_of(&s)).to_string();
		std::fs::write(&s.input, FILE.replace("110", "115")).unwrap();
		let before = on_disk(&s.input);
		let r = post(&s, &body, &[]);
		assert_eq!(r.status, 409);
		let said: Value = serde_json::from_slice(&r.body).unwrap();
		assert!(said["error"].as_str().unwrap().contains("changed on disk"), "{said}");
		assert!(said["error"].as_str().unwrap().contains("help: reload the page"), "{said}");
		assert_eq!(on_disk(&s.input), before);
		// Made since the page read that there was none: the same.
		let none = served_with(Stub::default(), None);
		let body = super::tests::body(&layout_of(&none)).to_string();
		std::fs::write(&none.input, FILE).unwrap();
		assert_eq!(post(&none, &body, &[]).status, 409);
		assert_eq!(std::fs::read_to_string(&none.input).unwrap(), FILE);
	}

	#[test]
	fn a_save_from_anywhere_but_this_server_s_page_is_refused() {
		let s = served();
		let good = body(&layout_of(&s)).to_string();
		let before = on_disk(&s.input);
		let other = format!("http://127.0.0.1:{}", PORT + 1);
		type Change<'a> = Vec<(&'a str, Option<&'a str>)>;
		let cases: Vec<(&str, Change, u16)> = vec![
			("no token", vec![("X-Vagcan-Token", None)], 403),
			("a wrong token", vec![("X-Vagcan-Token", Some("0123456789abcdef0123456789abcdee"))], 403),
			("a short token", vec![("X-Vagcan-Token", Some("0123"))], 403),
			("no Origin", vec![("Origin", None)], 403),
			("another site", vec![("Origin", Some("http://evil.example"))], 403),
			("another port", vec![("Origin", Some(other.as_str()))], 403),
			("https", vec![("Origin", Some("https://127.0.0.1:41234"))], 403),
			("the null origin", vec![("Origin", Some("null"))], 403),
			("a cross-site fetch", vec![("Sec-Fetch-Site", Some("cross-site"))], 403),
			("another Host", vec![("Host", Some("evil.example"))], 403),
			("no Host", vec![("Host", None)], 403),
			("a form", vec![("Content-Type", Some("application/x-www-form-urlencoded"))], 415),
			("plain text", vec![("Content-Type", Some("text/plain"))], 415),
			("no type", vec![("Content-Type", None)], 415),
			("no length", vec![("Content-Length", None)], 411),
			("a length too large", vec![("Content-Length", Some("2000000"))], 413),
			("a length that is not one", vec![("Content-Length", Some("12x"))], 400),
			("a signed length", vec![("Content-Length", Some("+12"))], 400),
			("a length that is not the body's", vec![("Content-Length", Some("3"))], 400),
			("chunked", vec![("Transfer-Encoding", Some("chunked"))], 400),
		];
		for (what, change, status) in cases {
			let r = post(&s, &good, &change);
			assert_eq!(r.status, status, "{what}");
			assert!(!String::from_utf8_lossy(&r.body).contains(TOKEN), "{what}");
		}
		// A header twice is not read two ways.
		let head = format!(
			"POST /save HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\nOrigin: http://127.0.0.1:{PORT}\r\nOrigin: http://evil.example\r\n\
			 X-Vagcan-Token: {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
			good.len()
		);
		assert_eq!(s.editor.respond(PORT, &head, good.as_bytes()).status, 400);
		for bad in ["not json", "{\"fingerprint\": \"x\"}", "{\"fingerprint\": 1, \"pages\": []}", "[]"] {
			assert_eq!(post(&s, bad, &[]).status, 400, "{bad}");
		}
		for method in ["OPTIONS", "PUT", "GET", "DELETE", "PATCH"] {
			let r = s.editor.respond(
				PORT,
				&format!("{method} /save HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\nOrigin: http://127.0.0.1:{PORT}\r\nX-Vagcan-Token: {TOKEN}\r\n\r\n"),
				&[],
			);
			assert_eq!((r.status, r.allow), (405, Some("POST")), "{method}");
		}
		assert_eq!(on_disk(&s.input), before, "nothing refused wrote anything");
		// And the very request with every header right is taken.
		assert_eq!(post(&s, &good, &[]).status, 200);
	}

	#[test]
	fn the_body_read_is_a_save_s_within_the_limit_and_nothing_else_s() {
		let head = |method: &str, path: &str, length: &str| format!("{method} {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {length}\r\n\r\n");
		assert_eq!(body_length(&head("POST", "/save", "12")), 12);
		assert_eq!(body_length(&head("POST", "/save", &MAX_BODY.to_string())), MAX_BODY);
		assert_eq!(body_length(&head("POST", "/save", &(MAX_BODY + 1).to_string())), 0);
		assert_eq!(body_length(&head("POST", "/", "12")), 0);
		assert_eq!(body_length(&head("PUT", "/save", "12")), 0);
		assert_eq!(body_length(&head("POST", "/save", "-1")), 0);
		assert_eq!(body_length("POST /save HTTP/1.1\r\nHost: x\r\n\r\n"), 0);
	}

	#[test]
	fn a_token_is_128_bits_of_hex_and_never_the_same_twice() {
		let (a, b) = (token().unwrap(), token().unwrap());
		assert_eq!(a.len(), 32);
		assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
		assert_ne!(a, b);
	}

	/// The build itself — [`dash::build`] over a made-up project's rows, as [`CarBuild`] runs it
	/// over the run's one source — not a stub.
	struct Real {
		_here: tempfile::TempDir,
		store: CatalogStore,
		extracted: vag_cli_core::extracted::Extracted,
		units: Vec<UnitIdentity>,
	}

	impl Real {
		fn new() -> Real {
			use vag_data_labels::catalog::Scaling;
			use vag_data_labels::measure::LinearScale;
			use vag_data_labels::odis::Reading;
			let here = tempfile::tempdir().unwrap();
			let cache = here.path().join("cache.sqlite");
			let reading = |did: u16, name: &str, text_id: &str| Reading {
				did,
				name: name.to_string(),
				unit: Some("°C".to_string()),
				bit_offset: 0,
				bit_length: 8,
				signed: false,
				big_endian: true,
				text_id: Some(text_id.to_string()),
				scaling: Scaling::Linear(LinearScale { factor: 1.0, offset: -40.0 }),
			};
			let readings = [reading(0x1001, "Coolant", "IDE00001"), reading(0x1002, "Intake", "IDE00002")];
			vag_data_db::put_readings(&cache, "/nowhere/TEST", "EV_Test_001", &readings).unwrap();
			Real {
				store: CatalogStore::open(here.path().join("proven")),
				extracted: vag_cli_core::extracted::Extracted::synthetic(cache, BTreeMap::new()),
				units: vec![UnitIdentity {
					request: vag_cli_core::plan::ENGINE,
					part_number: Some("PART1".to_string()),
					odx_name: Some("EV_Test".to_string()),
					odx_version: Some("001004".to_string()),
					component: Some("ENGINE".to_string()),
				}],
				_here: here,
			}
		}
	}

	impl Build for Real {
		type Checked = dash::Built;

		fn check(&self, text: &str) -> Result<dash::Built> {
			let input = dash::parse_input(text)?;
			Ok(dash::build(&input, &self.store, &self.extracted, &self.units, Language::En)?)
		}

		fn write(&self, built: dash::Built) -> Result<Saved> {
			Ok(Saved::of(&built.plan))
		}
	}

	#[test]
	fn through_the_build_a_channel_the_car_lacks_is_refused_and_one_it_has_is_saved() {
		let root = tempfile::tempdir().unwrap();
		let input = root.path().join("dash.toml");
		std::fs::write(&input, FILE).unwrap();
		let editor = Editor::new(
			root.path().to_path_buf(),
			VIN,
			input.clone(),
			TOKEN.to_string(),
			BTreeMap::new(),
			Real::new(),
		);
		let send = |body: &Value| {
			let body = body.to_string();
			let head = format!(
				"POST /save HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\nOrigin: http://127.0.0.1:{PORT}\r\nX-Vagcan-Token: {TOKEN}\r\n\
				 Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
				body.len()
			);
			let r = editor.respond(PORT, &head, body.as_bytes());
			(r.status, serde_json::from_slice::<Value>(&r.body).unwrap())
		};
		let fingerprint = file_fingerprint(Some(FILE.as_bytes()));
		let page = |cells: &[&str]| json!({"fingerprint": fingerprint, "pages": [{"kind": "values", "cells": cells, "from": 0}]});
		let before = on_disk(&input);
		let (status, said) = send(&page(&["01:IDE00001", "01:IDE00009"]));
		assert_eq!(status, 422, "{said}");
		assert!(
			said["error"].as_str().unwrap().starts_with("error: 01:IDE00009: "),
			"the build's words: {said}"
		);
		assert_eq!(on_disk(&input), before, "nothing written");

		let (status, said) = send(&page(&["01:IDE00001", "01:IDE00002"]));
		assert_eq!(status, 200, "{said}");
		let text = std::fs::read_to_string(&input).unwrap();
		let built = editor.build.check(&text).unwrap();
		assert_eq!(
			said["saved"],
			format!("saved{}{}", vag_cli_core::marks::SEP, Saved::of(&built.plan).facts())
		);
		assert_eq!(built.plan.channels.len(), 2);
	}

	#[test]
	fn the_names_for_a_new_channel_s_comment_come_from_the_catalog() {
		let root = tempfile::tempdir().unwrap();
		let path = root.path().join("channels.json");
		let rows = json!([
			{"ref": "01:IDE00001", "aliases": ["7E0:IDE00001", "01:1001"], "name": "Coolant", "uom": "°C"},
			{"ref": "01:1002", "aliases": [], "name": "Gear", "uom": ""},
		]);
		std::fs::write(&path, rows.to_string()).unwrap();
		let names = names(&path);
		assert_eq!(names["01:IDE00001"], "Coolant, °C");
		assert_eq!(names["01:1001"], "Coolant, °C");
		assert_eq!(names["01:1002"], "Gear");
		assert!(super::names(&root.path().join("nope.json")).is_empty());
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
		// The one request that writes: to this server's `save`, with the token, as JSON.
		assert_eq!(PAGE.matches("fetch(").count(), 4, "three files and the save");
		assert!(PAGE.contains("fetch(\"save\", {"), "it saves to save");
		assert!(PAGE.contains("\"X-Vagcan-Token\": LAYOUT.token"));
		assert!(PAGE.contains("\"Content-Type\": \"application/json\""));
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
				{"kind": "values", "title": "MAIN", "cells": ["01:IDE00025"], "volts": true, "from": 0},
				{"kind": "chart", "title": "", "cell": "01:IDE00025", "min": 70.0, "max": 110.0, "volts": false, "from": 1},
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
