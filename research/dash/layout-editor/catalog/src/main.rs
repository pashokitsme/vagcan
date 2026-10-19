//! Every channel a car's dash build can resolve, as a JSON catalog for a page-layout editor.
//!
//! Read-only: everything goes through vag-cli-core's own read paths (`project::current`,
//! `extracted::open`, `plan::available`, `dash::read_inputs`, `dash::build`), and nothing in
//! that path writes. Run it with HOME pointing at a copy of ~/.vagcan to be sure of that.
//!
//! usage: dash-catalog <VIN> <out-dir> <sweep-ranges> [survey.jsonl ...]
//!   sweep-ranges: the identifier pages the surveys asked, e.g. 0200-02FF,2000-22FF

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use vag_cli_core::dash::{self, ChannelInput, Error as BuildError, Input, Reference};
use vag_cli_core::plan::{self, UnitIdentity};
use vag_data_labels::catalog::{CatalogStore, ReadId, Scaling};
use vag_data_labels::measure::RawForm;
use vag_uds_client::address::UnitAddress;

/// Bits of a raw form, as `dash::bits_of` (private there) gives them: (offset, length).
fn bits_of(form: RawForm) -> (u32, u32) {
	match form {
		RawForm::U8First => (0, 8),
		RawForm::U8Second => (8, 8),
		RawForm::U16Be | RawForm::U16Le | RawForm::I16Be => (0, 16),
		RawForm::U24Be => (0, 24),
		RawForm::U32Be => (0, 32),
		RawForm::Int {
			byte_offset, byte_length, ..
		} => (u32::from(byte_offset) * 8, u32::from(byte_length) * 8),
		RawForm::Bits { bit_offset, bit_length, .. } => (bit_offset, u32::from(bit_length)),
	}
}

fn unit_label(request: u16) -> String {
	UnitAddress::from_request(request)
		.map(|a| a.label())
		.unwrap_or_else(|| format!("{request:03X}"))
}

fn probe(reference: Reference) -> ChannelInput {
	ChannelInput {
		reference,
		label: None,
		decimals: None,
		hz: None,
		setpoint: None,
		zero_at_rest: false,
	}
}

fn input_of(vin: &str, language: Option<vag_cli_core::config::Language>, refs: &[Reference]) -> Input {
	Input {
		vin: vin.to_string(),
		language,
		channels: refs.iter().cloned().map(probe).collect(),
		pages: Vec::new(),
		alarms: Vec::new(),
		stalk: None,
		stopwatch: None,
		buttons: Vec::new(),
		strip: None,
	}
}

/// The reference a build error is about, when it is about one channel.
fn culprit(e: &BuildError) -> Option<Reference> {
	match e {
		BuildError::Undeclared(r) | BuildError::Ambiguous(r, _) | BuildError::NotLinear(r, _) | BuildError::Duplicate(r) | BuildError::NotFinite(r) => {
			Some(r.clone())
		}
		BuildError::SameRow { second, .. } => Some(second.clone()),
		_ => None,
	}
}

fn parse_ranges(text: &str) -> Result<Vec<(u16, u16)>> {
	text
		.split(',')
		.filter(|s| !s.trim().is_empty())
		.map(|r| {
			let (a, b) = r.trim().split_once('-').context("range is A-B")?;
			Ok((u16::from_str_radix(a, 16)?, u16::from_str_radix(b, 16)?))
		})
		.collect()
}

struct Survey {
	/// request → F187 as the survey read it
	part: BTreeMap<u16, String>,
	/// request → identifiers that answered
	answered: BTreeMap<u16, BTreeSet<u16>>,
}

fn read_surveys(paths: &[String]) -> Result<Survey> {
	let mut s = Survey {
		part: BTreeMap::new(),
		answered: BTreeMap::new(),
	};
	for path in paths {
		let text = std::fs::read_to_string(path).with_context(|| path.clone())?;
		for line in text.lines().filter(|l| !l.trim().is_empty()) {
			let o: Value = serde_json::from_str(line)?;
			let request = u16::from_str_radix(o["request"].as_str().context("request")?, 16)?;
			for i in o["ident"].as_array().into_iter().flatten() {
				if i["did"].as_str() == Some("F187")
					&& let Some(hex) = i["data"].as_str()
				{
					let bytes = plan::hex_bytes(hex).unwrap_or_default();
					let part = String::from_utf8_lossy(&bytes).trim().to_string();
					if let Some(prev) = s.part.get(&request)
						&& *prev != part
					{
						bail!("{path}: {request:03X} answered F187 {part} where another survey read {prev}");
					}
					s.part.insert(request, part);
				}
			}
			let set = s.answered.entry(request).or_default();
			for d in o["dids"].as_array().into_iter().flatten() {
				if let Some(did) = d["did"].as_str() {
					set.insert(u16::from_str_radix(did, 16)?);
				}
			}
		}
	}
	Ok(s)
}

/// Split a CSV line, honouring quotes (what `watch --out` writes since headings are quoted).
fn split_quoted(line: &str) -> Vec<String> {
	let mut out = Vec::new();
	let mut field = String::new();
	let mut quoted = false;
	let mut chars = line.chars().peekable();
	while let Some(c) = chars.next() {
		match (c, quoted) {
			('"', true) if chars.peek() == Some(&'"') => {
				chars.next();
				field.push('"');
			}
			('"', true) => quoted = false,
			('"', false) if field.is_empty() => quoted = true,
			(',', false) => out.push(std::mem::take(&mut field)),
			_ => field.push(c),
		}
	}
	out.push(field);
	out
}

/// The value-column names of a `watch --out` header, `t_s` then `X_t_s,X` pairs. An older
/// recording wrote a heading with a comma unquoted, so the pairs are found by the rule that
/// `X_t_s,` is followed by `X` itself, which no plain split can see.
fn recording_columns(header: &str) -> Option<Vec<String>> {
	if header.contains('"') {
		let f = split_quoted(header);
		if f.first().map(String::as_str) != Some("t_s") || f.len() % 2 == 0 {
			return None;
		}
		let mut out = Vec::new();
		for pair in f[1..].chunks(2) {
			if pair[0].strip_suffix("_t_s") != Some(pair[1].as_str()) {
				return None;
			}
			out.push(pair[1].clone());
		}
		return Some(out);
	}
	let mut rest = header.strip_prefix("t_s,")?;
	let mut out = Vec::new();
	while !rest.is_empty() {
		let at = rest.find("_t_s,")?;
		let name = &rest[..at];
		let after = &rest[at + 5..];
		let tail = after.strip_prefix(name)?;
		out.push(name.to_string());
		rest = match tail.strip_prefix(',') {
			Some(t) => t,
			None if tail.is_empty() => "",
			None => return None,
		};
	}
	Some(out)
}

/// The value columns of a recording that hold at least one value, and the rows skipped as
/// not splitting into the header's columns.
fn recording_with_values(path: &str) -> Result<(BTreeSet<String>, usize, usize)> {
	let text = std::fs::read_to_string(path).with_context(|| path.to_string())?;
	let mut lines = text.lines().filter(|l| !l.trim().is_empty());
	let header = lines.next().context("empty")?;
	let columns = recording_columns(header).with_context(|| format!("{path}: not a watch --out header"))?;
	let mut with_values = BTreeSet::new();
	let (mut rows, mut skipped) = (0, 0);
	for line in lines {
		let cells = split_quoted(line);
		if cells.len() != 1 + 2 * columns.len() {
			skipped += 1;
			continue;
		}
		rows += 1;
		for (k, name) in columns.iter().enumerate() {
			if !cells[2 + 2 * k].trim().is_empty() {
				with_values.insert(name.clone());
			}
		}
	}
	Ok((with_values, rows, skipped))
}

fn main() -> Result<()> {
	let args: Vec<String> = std::env::args().collect();
	if args.len() < 4 {
		bail!("usage: dash-catalog <VIN> <out-dir> <sweep-ranges> [survey.jsonl ...] [recording.csv ...]");
	}
	let vin = &args[1];
	let out_dir = std::path::PathBuf::from(&args[2]);
	let ranges = parse_ranges(&args[3])?;
	let survey_paths: Vec<String> = args[4..].iter().filter(|p| p.ends_with(".jsonl")).cloned().collect();
	let recording_paths: Vec<String> = args[4..].iter().filter(|p| p.ends_with(".csv")).cloned().collect();
	let survey = read_surveys(&survey_paths)?;
	eprintln!("HOME = {}", std::env::var("HOME").unwrap_or_default());

	// The car's own build input, read the way `dev dash build` reads it — for its language
	// and to prove the VIN has a record of units.
	let inputs = dash::read_inputs(vin, None)?;
	let language = inputs.parsed.language;
	let units: Vec<UnitIdentity> = inputs.units.clone();

	let project = vag_cli_core::project::current()?;
	let store = CatalogStore::open(project.measurements_dir());
	let extracted = vag_cli_core::extracted::open(&project);
	let default_language = vag_cli_core::config::language(&vag_cli_core::config::load());

	let t = Instant::now();
	let offered = plan::available(&store, &extracted, &units);
	eprintln!("plan::available: {} rows in {:?}", offered.len(), t.elapsed());

	// The car's own dash.toml must still build: the same code path, unchanged.
	let t = Instant::now();
	let own = dash::build(&inputs.parsed, &store, &extracted, &units, default_language).map_err(|e| anyhow::anyhow!("own dash.toml: {e}"))?;
	eprintln!("own dash.toml builds: {} channels in {:?}", own.plan.channels.len(), t.elapsed());

	// Which source declares each field, per unit — the order `extracted` ranks them in.
	let mut declared_by: BTreeMap<(u16, u16, u32), (&'static str, String)> = BTreeMap::new();
	for u in &units {
		for kind in [vag_data_db::ODIS, vag_data_db::VCDS] {
			for def in extracted.for_unit_from(kind, u.odx_name.as_deref(), u.odx_version.as_deref()) {
				let ReadId::Uds(did) = def.address;
				declared_by
					.entry((u.request, did, def.raw_form.bit_offset()))
					.or_insert((kind, def.name.to_string()));
			}
		}
	}

	// Rows with a definition, per unit, and how many rows on that unit carry each text id.
	let rows: Vec<&plan::Channel> = offered.iter().filter(|c| c.def.is_some()).collect();
	let mut by_id: BTreeMap<(u16, String), (usize, usize)> = BTreeMap::new(); // (all, linear)
	for c in &rows {
		if let Some(id) = &c.text_id {
			let e = by_id.entry((c.request, id.clone())).or_default();
			e.0 += 1;
			if matches!(c.def.as_ref().map(|d| &d.scaling), Some(Scaling::Linear(_))) {
				e.1 += 1;
			}
		}
	}

	// One reference per row: its text id where that names this row and no other — the rule
	// `dash::resolve_channel` applies (a single linear match wins over enum rows sharing the
	// id; otherwise the id must be the only match) — and `<unit>:<DID>[@bit]` otherwise.
	let mut refs: Vec<Reference> = Vec::with_capacity(rows.len());
	for c in &rows {
		let def = c.def.as_ref().unwrap();
		let linear = matches!(def.scaling, Scaling::Linear(_));
		let by_text = c.text_id.as_ref().filter(|id| {
			let (all, lin) = by_id[&(c.request, (*id).clone())];
			if linear { lin == 1 } else { all == 1 }
		});
		let r = match by_text {
			Some(id) => Reference::TextId {
				request: c.request,
				text_id: id.clone(),
			},
			None => Reference::Field {
				request: c.request,
				did: c.did,
				bit_offset: def.raw_form.bit_offset(),
			},
		};
		// What a person writes must parse back to the same thing.
		let spelled = r.to_string();
		let back = Reference::parse(&spelled).map_err(|e| anyhow::anyhow!("{spelled}: {e}"))?;
		if back != r {
			bail!("{spelled} parses back as {back:?}, not {r:?}");
		}
		refs.push(r);
	}
	let distinct: BTreeSet<String> = refs.iter().map(|r| r.to_string()).collect();
	if distinct.len() != refs.len() {
		bail!("{} refs for {} rows: two rows got one spelling", distinct.len(), refs.len());
	}

	// Validate through `dash::build`: every row the catalog calls numeric must build as a
	// [[channel]] and resolve to exactly its own field. One build of all of them; a refusal
	// names its channel, which is taken out and the rest built again.
	let mut verdict: BTreeMap<usize, Result<dash::Channel, String>> = BTreeMap::new();
	let mut pending: Vec<usize> = (0..rows.len())
		.filter(|&i| matches!(rows[i].def.as_ref().unwrap().scaling, Scaling::Linear(_)))
		.collect();
	let t = Instant::now();
	let mut builds = 0;
	loop {
		let these: Vec<Reference> = pending.iter().map(|&i| refs[i].clone()).collect();
		builds += 1;
		match dash::build(&input_of(vin, language, &these), &store, &extracted, &units, default_language) {
			Ok(built) => {
				if built.plan.channels.len() != pending.len() {
					bail!("{} channels built for {} asked", built.plan.channels.len(), pending.len());
				}
				for (&i, ch) in pending.iter().zip(built.plan.channels.iter()) {
					verdict.insert(i, Ok(ch.clone()));
				}
				break;
			}
			Err(e) => {
				let Some(r) = culprit(&e) else {
					bail!("build refused the whole input: {e}")
				};
				let at = pending
					.iter()
					.position(|&i| refs[i] == r)
					.with_context(|| format!("{e}: names a ref not asked"))?;
				let i = pending.remove(at);
				verdict.insert(i, Err(e.to_string()));
			}
		}
	}
	eprintln!("linear rows: {builds} builds in {:?}", t.elapsed());

	// Every other row, one build each: the refusal says what it is.
	let t = Instant::now();
	for i in 0..rows.len() {
		if verdict.contains_key(&i) {
			continue;
		}
		let r = dash::build(
			&input_of(vin, language, std::slice::from_ref(&refs[i])),
			&store,
			&extracted,
			&units,
			default_language,
		);
		verdict.insert(i, r.map(|b| b.plan.channels[0].clone()).map_err(|e| e.to_string()));
	}
	eprintln!("other rows: one build each in {:?}", t.elapsed());

	let ru = vag_cli_core::glossary::parse(
		&std::fs::read_to_string(vag_cli_core::glossary::path()?).unwrap_or_default(),
		vag_cli_core::config::Language::Ru,
	);
	let component: BTreeMap<u16, String> = units.iter().map(|u| (u.request, u.component.clone().unwrap_or_default())).collect();
	let part_of: BTreeMap<u16, String> = units.iter().map(|u| (u.request, u.part_number.clone().unwrap_or_default())).collect();
	let swept = |did: u16| ranges.iter().any(|&(a, b)| (a..=b).contains(&did));

	// Recordings name a column by the channel's label in the language `watch` ran in (the
	// label `plan::available` gives here, config.toml's): a heading answers for an identifier
	// only when every row of this car carrying that label is on one unit and identifier.
	let mut by_label: BTreeMap<String, BTreeSet<(u16, u16)>> = BTreeMap::new();
	for c in &rows {
		by_label.entry(c.label()).or_default().insert((c.request, c.did));
	}
	let mut recorded: BTreeMap<(u16, u16), String> = BTreeMap::new();
	let mut recording_notes = Vec::new();
	for path in &recording_paths {
		let (names, n, skipped) = recording_with_values(path)?;
		let (mut unique, mut ambiguous, mut unknown) = (0, Vec::new(), Vec::new());
		for name in &names {
			match by_label.get(name) {
				Some(set) if set.len() == 1 => {
					let key = *set.iter().next().unwrap();
					recorded.entry(key).or_insert_with(|| path.rsplit('/').next().unwrap_or(path).to_string());
					unique += 1;
				}
				Some(set) => ambiguous.push(format!("{name} ({} identifiers)", set.len())),
				None => unknown.push(name.clone()),
			}
		}
		recording_notes.push(format!(
			"  {path}: {n} rows ({skipped} skipped), {} columns with values: {unique} pinned to one identifier, {} ambiguous {:?}, {} matching no label {:?}",
			names.len(),
			ambiguous.len(),
			ambiguous,
			unknown.len(),
			unknown
		));
	}
	// Every other spelling the build reads as the same row: the identifier form of a row
	// named by text id, and the request id for a unit that has a short number.
	let aliases_of = |r: &Reference, c: &plan::Channel| -> Vec<String> {
		let def = c.def.as_ref().unwrap();
		let field = Reference::Field {
			request: c.request,
			did: c.did,
			bit_offset: def.raw_form.bit_offset(),
		};
		let mut forms = vec![r.to_string()];
		if *r != field {
			forms.push(field.to_string());
		}
		let short = unit_label(c.request);
		let long = format!("{:03X}", c.request);
		let mut out: Vec<String> = Vec::new();
		for f in &forms {
			if f != &r.to_string() {
				out.push(f.clone());
			}
			if short != long {
				out.push(format!("{long}{}", &f[short.len()..]));
			}
		}
		out
	};

	let mut out: Vec<Value> = Vec::new();
	let mut refusals: BTreeMap<String, usize> = BTreeMap::new();
	for (i, c) in rows.iter().enumerate() {
		let def = c.def.as_ref().unwrap();
		let (bit_offset, bit_length) = bits_of(def.raw_form);
		let key = (c.request, c.did, def.raw_form.bit_offset());
		let built = &verdict[&i];
		if let Ok(ch) = built
			&& (ch.unit, ch.did, ch.bit_offset, ch.bit_length) != (c.request, c.did, bit_offset, bit_length)
		{
			bail!("{} built as {:04X}@{}/{}, not this row", refs[i], ch.did, ch.bit_offset, ch.bit_length);
		}
		if let Err(why) = built {
			let kind = why.strip_prefix(&format!("{}: ", refs[i])).unwrap_or(why).to_string();
			*refusals.entry(kind).or_default() += 1;
		}
		let (source, raw_name) = match declared_by.get(&key) {
			Some((kind, name)) => (kind.to_string(), name.clone()),
			None if c.is_standard() => ("obd".to_string(), def.name.to_string()),
			None => ("proven".to_string(), def.name.to_string()),
		};
		// Survey evidence only for a unit whose part number is the one on record now.
		let surveyed = survey.part.get(&c.request).is_some_and(|p| Some(p) == part_of.get(&c.request));
		let in_recording = recorded.get(&(c.request, c.did));
		let (answered, evidence) = match survey.answered.get(&c.request) {
			Some(set) if surveyed && set.contains(&c.did) => (Some(true), Some("survey".to_string())),
			_ if in_recording.is_some() => (Some(true), in_recording.cloned()),
			Some(_) if surveyed && swept(c.did) => (Some(false), Some("survey".to_string())),
			_ => (None, None),
		};
		let kind = match &def.scaling {
			Scaling::Linear(_) => "linear",
			Scaling::Enum { .. } => "enum",
			Scaling::Anchor { .. } => "anchor",
		};
		out.push(json!({
				"ref": refs[i].to_string(),
				"unit": unit_label(c.request),
				"unitName": component[&c.request],
				"name": raw_name,
				"ru": c.text_id.as_ref().and_then(|id| ru.get(id)),
				"did": format!("{:04X}", c.did),
				"uom": def.unit.to_string(),
				"numeric": built.is_ok(),
				"decimals": built.as_ref().ok().map(|ch| ch.decimals),
				"min": Value::Null,
				"max": Value::Null,
				"answered": answered,
				"proven": c.proven,
				"source": source,
				// beyond the asked schema: what an editor may want, and why a row is not numeric
				"textId": c.text_id,
				"request": format!("{:03X}", c.request),
				"bitOffset": bit_offset,
				"bitLength": bit_length,
				"scaling": kind,
				"label": built.as_ref().ok().map(|ch| ch.label.clone()),
				"refused": built.as_ref().err().map(|why| why.strip_prefix(&format!("{}: ", refs[i])).unwrap_or(why).to_string()),
				"answeredBy": evidence,
				"aliases": aliases_of(&refs[i], c),
		}));
	}
	// Every alias of a numeric row builds, alone in its input, to the very read its ref built
	// to: one build per spelling family (the request-id forms, the identifier forms), since
	// two spellings of one row in one input are refused as the same row.
	let t = Instant::now();
	for family in 0..2 {
		let mut asked: Vec<(usize, Reference)> = Vec::new();
		for (i, c) in rows.iter().enumerate() {
			if verdict[&i].is_err() {
				continue;
			}
			let all = aliases_of(&refs[i], c);
			let pick = all.iter().find(|a| {
				let long = a.starts_with(&format!("{:03X}:", c.request)) && unit_label(c.request) != format!("{:03X}", c.request);
				(family == 0) == long
			});
			if let Some(a) = pick {
				asked.push((i, Reference::parse(a).map_err(|e| anyhow::anyhow!("{a}: {e}"))?));
			}
		}
		let these: Vec<Reference> = asked.iter().map(|(_, r)| r.clone()).collect();
		let built = dash::build(&input_of(vin, language, &these), &store, &extracted, &units, default_language)
			.map_err(|e| anyhow::anyhow!("aliases (family {family}): {e}"))?;
		for ((i, r), ch) in asked.iter().zip(built.plan.channels.iter()) {
			let own = verdict[i].as_ref().unwrap();
			if (ch.unit, ch.did, ch.bit_offset, ch.bit_length) != (own.unit, own.did, own.bit_offset, own.bit_length) {
				bail!("alias {r} builds another read than {}", refs[*i]);
			}
		}
		eprintln!("aliases family {family}: {} built to their rows", asked.len());
	}
	eprintln!("aliases checked in {:?}", t.elapsed());

	out.sort_by(|a, b| {
		let k = |v: &Value| {
			(
				v["unit"].as_str().unwrap_or("").to_string(),
				v["name"].as_str().unwrap_or("").to_string(),
				v["ref"].as_str().unwrap_or("").to_string(),
			)
		};
		k(a).cmp(&k(b))
	});

	std::fs::create_dir_all(&out_dir)?;
	std::fs::write(out_dir.join("catalog.json"), serde_json::to_string_pretty(&out)?)?;

	// Summary.
	let mut s = String::new();
	use std::fmt::Write as _;
	let odis_variants = vag_data_db::reading_variants_of(&project.cache(), vag_data_db::ODIS).unwrap_or_default();
	let vcds_variants = vag_data_db::reading_variants_of(&project.cache(), vag_data_db::VCDS).unwrap_or_default();
	writeln!(s, "dash channel catalog for {vin}")?;
	writeln!(s, "project: {} ({})", project.id, project.dir.display())?;
	writeln!(
		s,
		"sources in the cache: ODIS {} variants, VCDS {} variants",
		odis_variants.len(),
		vcds_variants.len()
	)?;
	for src in extracted.odis_sources() {
		writeln!(s, "  ODIS project read from: {src}")?;
	}
	writeln!(s, "units: {} (car record {})", units.len(), inputs.record.display())?;
	writeln!(s, "build language: {:?} (dash.toml), default {:?}", language, default_language)?;
	writeln!(
		s,
		"rows: {} ({} numeric = accepted as [[channel]] by dash::build)",
		out.len(),
		out.iter().filter(|v| v["numeric"] == true).count()
	)?;
	writeln!(s, "refusals by kind (non-numeric rows):")?;
	for (k, n) in &refusals {
		writeln!(s, "  {n:6}  {k}")?;
	}
	writeln!(s)?;
	writeln!(
		s,
		"{:<5} {:<4} {:<16} {:<28} {:>6} {:>7} {:>8} {:>8} {:>6}  source",
		"unit", "req", "component", "ODIS variant(s)", "total", "numeric", "answered", "ans/num", "proven"
	)?;
	let mut total = [0usize; 5];
	let mut sorted_units = units.clone();
	sorted_units.sort_by_key(|u| unit_label(u.request));
	let mut empty = Vec::new();
	for u in &sorted_units {
		let label = unit_label(u.request);
		let here: Vec<&Value> = out.iter().filter(|v| v["request"] == format!("{:03X}", u.request)).collect();
		let n = here.len();
		let num = here.iter().filter(|v| v["numeric"] == true).count();
		let ans = here.iter().filter(|v| v["answered"] == true).count();
		let ans_num = here.iter().filter(|v| v["answered"] == true && v["numeric"] == true).count();
		let prov = here.iter().filter(|v| v["proven"] == true).count();
		let mut srcs: BTreeMap<String, usize> = BTreeMap::new();
		for v in &here {
			*srcs.entry(v["source"].as_str().unwrap_or("").to_string()).or_default() += 1;
		}
		let variants = vag_cli_core::extracted::best_variants(&odis_variants, u.odx_name.as_deref(), u.odx_version.as_deref());
		let vtext = if variants.is_empty() {
			"-".to_string()
		} else {
			variants.iter().map(|v| v.as_str()).collect::<Vec<_>>().join(" ")
		};
		writeln!(
			s,
			"{label:<5} {:03X}  {:<16} {:<28} {n:>6} {num:>7} {ans:>8} {ans_num:>8} {prov:>6}  {}",
			u.request,
			u.component.clone().unwrap_or_default(),
			vtext,
			srcs.iter().map(|(k, n)| format!("{k} {n}")).collect::<Vec<_>>().join(", ")
		)?;
		for (t, x) in total.iter_mut().zip([n, num, ans, ans_num, prov]) {
			*t += x;
		}
		if n == 0 {
			empty.push(u.clone());
		}
	}
	writeln!(
		s,
		"{:<55} {:>6} {:>7} {:>8} {:>8} {:>6}",
		"all", total[0], total[1], total[2], total[3], total[4]
	)?;
	writeln!(s)?;
	writeln!(s, "units with no channels:")?;
	if empty.is_empty() {
		writeln!(s, "  none")?;
	}
	for u in &empty {
		writeln!(
			s,
			"  {} {:03X} {} — F19E {:?} F1A2 {:?}: no ODIS variant matches and no VCDS rows are cached for it",
			unit_label(u.request),
			u.request,
			u.component.clone().unwrap_or_default(),
			u.odx_name,
			u.odx_version
		)?;
	}
	let described = vag_cli_core::registry::described_by(&extracted);
	match vag_cli_core::registry::unread(&project, &units, &described)? {
		vag_cli_core::registry::Unread::Nothing => writeln!(s, "VCDS registry: nothing left to read for this car's units")?,
		vag_cli_core::registry::Unread::Readable { units, root } => writeln!(
			s,
			"VCDS registry: {} unit(s) not yet read from {} — {:?} (not read here: that read writes the cache)",
			units.len(),
			root.display(),
			units
		)?,
		vag_cli_core::registry::Unread::Unreadable { units, why } => {
			writeln!(s, "VCDS registry: {} unit(s) not read, and cannot be: {why} — {:?}", units.len(), units)?
		}
	}
	writeln!(s)?;
	writeln!(s, "answered: from the surveys {:?}", survey_paths)?;
	writeln!(s, "  and the recordings {:?}", recording_paths)?;
	writeln!(
		s,
		"  true  = the identifier answered in a survey of that unit (identifier level, not field level),"
	)?;
	writeln!(
		s,
		"          or a recording's column with values whose heading is the label of rows on that identifier alone (answeredBy names which)"
	)?;
	writeln!(s, "  false = inside the swept pages {} and never answered", &args[3])?;
	writeln!(
		s,
		"  null  = outside the swept pages, or the unit's F187 in the survey differs from the record"
	)?;
	for u in &sorted_units {
		let p = survey.part.get(&u.request);
		if p != u.part_number.as_ref() {
			writeln!(
				s,
				"  {:03X}: survey F187 {:?}, record {:?} — answered left null",
				u.request, p, u.part_number
			)?;
		}
	}
	for n in &recording_notes {
		writeln!(s, "{n}")?;
	}
	std::fs::write(out_dir.join("catalog-summary.txt"), &s)?;
	print!("{s}");
	Ok(())
}
