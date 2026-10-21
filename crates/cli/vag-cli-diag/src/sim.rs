//! `vagcan dev sim`: be a car on a pseudo-terminal, or write the profile of one.
//!
//! Serving is `vag_uds_sim`'s; this is the command around it — reading the profile, saying
//! where the port is, and logging what the car hears at `-v` and `-vv`. Writing a profile
//! (`--from`) takes what this machine already holds about a car: a `watch --out` recording,
//! a `dev sniff --out` capture, a car's `units.json`. A recording needs the project's
//! catalogs to turn its columns back into identifiers and bytes, the same way
//! `watch --replay` does, so that half is here rather than in the sim's crate.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use vag_uds_sim::profile::{DidSpec, DidTable, Hex, Id, Point, Profile, UnitSpec};

use crate::watch::replay;

/// `n` and its noun: `1 unit`, `3 units`.
fn count(n: usize, noun: &str) -> String {
	format!("{n} {}", vag_cli_core::table::plural(n, noun))
}

/// A line of a multi-step report on stderr at `level` (CLAUDE.md, "Command output"): the
/// heading and the result line flush left, each step indented under them.
fn say(level: u8, line: &str) {
	vag_cli_core::verbosity::say(level, line);
}

/// A step of the report, past tense, indented under its heading.
fn step(level: u8, line: &str) {
	say(level, &format!("  {line}"));
}

/// Serve the car `profile_path` describes until interrupted. The port's path is the one
/// line on stdout; the rest is status on stderr.
///
/// Refused with `slcan`: the sim opens no adapter, so a flag about one means somebody
/// meant another command. Refused too when the path is a device rather than a file: the
/// sim is never pointed at a port, it makes its own.
pub async fn serve(profile_path: &str, slcan: bool) -> Result<()> {
	if slcan {
		bail!("`dev sim` opens no adapter, so --slcan has nothing to apply to\n\nhelp: vagcan dev sim {profile_path}");
	}
	let path = Path::new(profile_path);
	if is_device(path) {
		bail!(
			"{profile_path} is a device, not a profile: `dev sim` opens no port, it makes its own pseudo-terminal\n\n\
             help: vagcan dev sim <profile.toml>   (docs/virtual-car.md)"
		);
	}
	let text = std::fs::read_to_string(path).with_context(|| format!("reading the profile {profile_path}"))?;
	let car = Profile::parse(&text)
		.and_then(|p| p.resolve())
		.map_err(|e| anyhow::anyhow!("{profile_path}: {e}\n\nhelp: correct the profile (docs/virtual-car.md)"))?;
	let units = car.units.len();
	let unit_ids = car
		.units
		.iter()
		.map(|u| format!("{:03X}", u.address.request))
		.collect::<Vec<_>>()
		.join(" ");
	let sim = vag_uds_sim::Sim::start(car, log).context("opening a pseudo-terminal")?;
	let port = sim.path().display().to_string();
	vag_cli_core::outln!("{port}");
	say(0, &format!("Serve {} on {port}", file_name(profile_path)));
	step(0, &format!("started {}: {unit_ids}", count(units, "unit")));
	step(1, "opened a pseudo-terminal, which `vagcan devices` does not list: it lists USB devices");
	say(0, "Ready until Ctrl-C.");
	step(0, &format!("(use \"vagcan info --device {port}\" in another terminal)"));
	tokio::task::spawn_blocking(move || sim.wait())
		.await
		.context("the sim's thread")?
		.context("serving the pseudo-terminal")
}

/// Whether `path` is a character or block device: a port, not a profile.
fn is_device(path: &Path) -> bool {
	use std::os::unix::fs::FileTypeExt as _;
	std::fs::metadata(path).is_ok_and(|m| m.file_type().is_char_device() || m.file_type().is_block_device())
}

/// What the car hears, at the level it belongs to: a channel opening or closing and anything
/// that went wrong at `-v`, every request and answer at `-vv`, as the client's own `-vv`
/// writes the wire.
fn log(notice: vag_uds_sim::pty::Notice) {
	use vag_cli_core::verbosity::{NOTES, WIRE, hex};
	use vag_uds_sim::adapter::Note;
	use vag_uds_sim::bus::Event;
	use vag_uds_sim::pty::Notice;
	match notice {
		Notice::Adapter(Note::Opened { rate, listen_only }) => {
			let rate = match rate {
				Some(6) => "500 kbit/s".to_string(),
				Some(n) => format!("at S{n}, not diagnostic CAN's 500 kbit/s: no unit answers"),
				None => "with no bitrate set: no unit answers".to_string(),
			};
			let mode = if listen_only { ", listen-only" } else { "" };
			step(NOTES, &format!("opened the channel, {rate}{mode}"));
		}
		Notice::Adapter(Note::Closed) => step(NOTES, "closed the channel"),
		Notice::Adapter(Note::Refused(line)) => step(NOTES, &format!("refused the line {line:?}")),
		Notice::Bus(Event::Request { unit, pdu }) => vag_cli_core::verbosity::say(WIRE, &format!("→  {unit:03X}  {}", hex(&pdu))),
		Notice::Bus(Event::Answer { id, pdu, .. }) => vag_cli_core::verbosity::say(WIRE, &format!("←  {id:03X}  {}", hex(&pdu))),
		Notice::Bus(Event::Silent { unit, why }) => step(WIRE, &format!("{unit:03X} said nothing: {why:?}")),
		Notice::Bus(Event::Protocol { unit, what }) => step(NOTES, &format!("{unit:03X}: {what}")),
		Notice::Dropped(bytes) => step(NOTES, &format!("dropped {bytes} bytes the tester did not read within a second")),
	}
}

/// What one `--from` file is.
enum Source {
	/// `watch --out`: a CSV whose first column is `t_s`.
	Recording,
	/// `dev sniff --out`: JSON lines of capture records.
	Capture,
	/// A car's `units.json`.
	Units,
}

fn kind_of(path: &str, text: &str) -> Result<Source> {
	let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim_start();
	if first.starts_with("t_s") {
		return Ok(Source::Recording);
	}
	if first.starts_with('{') && serde_json::from_str::<vag_uds_capture::CaptureRecord>(first).is_ok() {
		return Ok(Source::Capture);
	}
	if serde_json::from_str::<serde_json::Value>(text).is_ok_and(|v| v.get("units").is_some() && v.get("vin").is_some()) {
		return Ok(Source::Units);
	}
	bail!("{path}: neither a `watch --out` recording, a `dev sniff --out` capture nor a car's units.json")
}

/// The profile `files` describe together, as TOML. A unit, an identifier or a fault one
/// file gives is not replaced by a later file's.
///
/// `vin`: the car a recording's columns belong to, whose record names its units; and the
/// VIN the engine answers when no file gives it. `catalogs`: the project's measurement
/// rows, for a recording.
pub fn profile_from(files: &[String], vin: Option<&str>, catalogs: impl Fn() -> Result<String>) -> Result<String> {
	let mut profile = Profile {
		about: Some(format!(
			"Written by `vagcan dev sim --from` from {}.",
			files.iter().map(|f| file_name(f)).collect::<Vec<_>>().join(", ")
		)),
		..Profile::default()
	};
	say(
		0,
		&format!(
			"Write a profile from {}",
			files.iter().map(|f| file_name(f)).collect::<Vec<_>>().join(", ")
		),
	);
	let mut vin = vin.map(str::to_string);
	for path in files {
		let text = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
		let from = match kind_of(path, &text)? {
			Source::Capture => {
				let records = vag_uds_capture::read_records(text.as_bytes()).with_context(|| format!("reading the capture {path}"))?;
				let (from, taken) = vag_uds_sim::capture::profile(&records, "");
				step(
					0,
					&format!(
						"read {}: a capture of {}, {}, {} ({} changing), {}",
						file_name(path),
						count(taken.frames, "frame"),
						count(taken.units, "unit"),
						count(taken.identifiers, "identifier"),
						taken.series,
						count(taken.faults, "fault")
					),
				);
				if taken.multi_reads > 0 {
					step(
						1,
						&format!(
							"left out {} answers to reads of several identifiers: they do not say where one ends",
							taken.multi_reads
						),
					);
				}
				from
			}
			Source::Units => {
				let record: serde_json::Value = serde_json::from_str(&text)?;
				if vin.is_none() {
					vin = record.get("vin").and_then(|v| v.as_str()).map(str::to_string);
				}
				let units = vag_cli_core::units::read_record(Path::new(path))?.unwrap_or_default();
				step(
					0,
					&format!("read {}: {}, as each said what it is", file_name(path), count(units.len(), "unit")),
				);
				identities(&units)
			}
			Source::Recording => {
				let Some(vin) = vin.as_deref() else {
					bail!(
						"{path} is a recording, and a recording does not say which car\n\nhelp: name it with --vin <VIN>, or give the car's units.json first"
					);
				};
				recording(path, &text, vin, &catalogs()?)?
			}
		};
		merge(&mut profile, from);
	}
	if let Some(vin) = vin {
		let engine = unit(&mut profile, crate::plan::ENGINE);
		if engine.vin.is_none() && !engine.did.contains_key("F190") {
			engine.vin = Some(vin);
		}
	}
	profile.units.sort_by_key(|u| u.request);
	// What is written must be a car the sim runs.
	profile
		.resolve()
		.map_err(|e| anyhow::anyhow!("the profile these files make does not run: {e}"))?;
	let units = profile.units.len();
	say(0, &format!("Ready: a profile of {}, on stdout.", count(units, "unit")));
	step(0, "(use \"vagcan dev sim <profile.toml>\" to serve it)");
	Ok(profile.to_toml())
}

fn file_name(path: &str) -> String {
	Path::new(path)
		.file_name()
		.map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

fn unit(profile: &mut Profile, request: u16) -> &mut UnitSpec {
	match profile.units.iter().position(|u| u.request.0 == request) {
		Some(at) => &mut profile.units[at],
		None => {
			profile.units.push(UnitSpec {
				request: Id(request),
				..UnitSpec::default()
			});
			profile.units.last_mut().expect("just pushed")
		}
	}
}

/// What each unit of a car's record said it is, and the gateway's list of the rest.
fn identities(units: &[crate::plan::UnitIdentity]) -> Profile {
	let mut profile = Profile::default();
	for identity in units {
		let spec = unit(&mut profile, identity.request);
		spec.part = identity.part_number.clone();
		spec.odx = identity.odx_name.clone();
		spec.odx_version = identity.odx_version.clone();
		spec.component = identity.component.clone();
	}
	// The gateway lists every unit of VW's block the record holds but itself — the units the
	// record has because the gateway listed them.
	let listed: Vec<Id> = units
		.iter()
		.map(|u| u.request)
		.filter(|r| !vag_uds_client::gateway::NOT_LISTED.contains(r) && (0x700..=vag_uds_client::address::VW_LAST).contains(r))
		.map(Id)
		.collect();
	if !listed.is_empty() {
		unit(&mut profile, vag_uds_client::gateway::GATEWAY).installed = listed;
	}
	profile
}

/// One identifier's answers over a recording: milliseconds from its start, and the bytes.
type Series = Vec<(u64, Vec<u8>)>;

/// A recording's columns as identifiers that read what was recorded, when it was.
fn recording(path: &str, csv: &str, vin: &str, catalogs: &str) -> Result<Profile> {
	let recording = replay::Recording::parse(csv).map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
	let record = crate::datadir::units_record(vin)?;
	let identities = vag_cli_core::units::read_record(&record)?.ok_or_else(|| {
		anyhow::anyhow!(
			"no record of {vin}'s control units at {}: a recording's columns are found through the units that read them",
			record.display()
		)
	})?;
	let store = vag_data_labels::catalog::CatalogStore::open(catalogs);
	let extracted = crate::extracted::current().for_car(Some(vin));
	let mut channels = crate::plan::available(&store, &extracted, &identities);
	let mut resolved = replay::resolve(&recording.columns, &mut channels, crate::plan::ENGINE);
	replay::settle_formats(&recording, &mut resolved, &channels);
	// Per identifier, the answer at each sample: every field's column laid over the last
	// answer, as the replay rebuilds it.
	let start = recording.samples.first().map_or(0.0, |(t, _)| *t);
	let mut answers: BTreeMap<(u16, u16), Series> = BTreeMap::new();
	let mut held: BTreeMap<(u16, u16), Vec<u8>> = BTreeMap::new();
	for (t, cells) in &recording.samples {
		let at_ms = ((t - start).max(0.0) * 1000.0).round() as u64;
		let mut touched: Vec<(u16, u16)> = Vec::new();
		for (column, hit) in &resolved {
			let Some(cell) = cells.get(*column).and_then(|c| c.as_deref()) else {
				continue;
			};
			let channel = &channels[hit.channel];
			let key = (channel.request, channel.did);
			if let Some(bytes) = replay::answer_from_cell(cell, channel, hit.raw, held.get(&key).map(Vec::as_slice)) {
				held.insert(key, bytes);
				if !touched.contains(&key) {
					touched.push(key);
				}
			}
		}
		for key in touched {
			let bytes = held[&key].clone();
			let series = answers.entry(key).or_default();
			if series.last().is_none_or(|(_, last)| *last != bytes) {
				series.push((at_ms, bytes));
			}
		}
	}
	let dropped = recording.columns.len() - resolved.len();
	let unmatched = match dropped {
		0 => String::new(),
		n => format!(", {n} columns no channel matched"),
	};
	step(
		0,
		&format!(
			"read {}: a recording of {:.0} s, {}, {}{unmatched}",
			file_name(path),
			recording.duration(),
			count(recording.columns.len(), "column"),
			count(answers.len(), "identifier")
		),
	);
	let mut profile = Profile::default();
	for ((request, did), series) in answers {
		let spec = match series.as_slice() {
			[(_, only)] => DidSpec::Value(Hex(only.clone())),
			_ => DidSpec::Full(DidTable {
				series: Some(series.into_iter().map(|(at_ms, value)| Point { at_ms, value: Hex(value) }).collect()),
				..DidTable::default()
			}),
		};
		unit(&mut profile, request).did.insert(format!("{did:04X}"), spec);
	}
	Ok(profile)
}

/// `from` laid under `into`: what `into` has stays, what it lacks is taken.
fn merge(into: &mut Profile, from: Profile) {
	for spec in from.units {
		let unit = unit(into, spec.request.0);
		fill(&mut unit.name, spec.name);
		fill(&mut unit.part, spec.part);
		fill(&mut unit.component, spec.component);
		fill(&mut unit.odx, spec.odx);
		fill(&mut unit.odx_version, spec.odx_version);
		fill(&mut unit.vin, spec.vin);
		for id in spec.installed {
			if !unit.installed.contains(&id) {
				unit.installed.push(id);
			}
		}
		for (did, value) in spec.did {
			unit.did.entry(did).or_insert(value);
		}
		for fault in spec.faults {
			if !unit.faults.iter().any(|f| f.code == fault.code) {
				unit.faults.push(fault);
			}
		}
		for code in spec.supported {
			if !unit.supported.contains(&code) {
				unit.supported.push(code);
			}
		}
	}
}

fn fill<T>(slot: &mut Option<T>, value: Option<T>) {
	if slot.is_none() {
		*slot = value;
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_units_record_becomes_units_and_the_gateways_list() {
		let units = vec![
			crate::plan::UnitIdentity {
				request: 0x7E0,
				part_number: Some("PART-E".into()),
				odx_name: Some("EV_E".into()),
				odx_version: Some("001001".into()),
				component: Some("ENGINE".into()),
			},
			crate::plan::UnitIdentity {
				request: 0x713,
				part_number: Some("PART-B".into()),
				..Default::default()
			},
			crate::plan::UnitIdentity {
				request: 0x710,
				..Default::default()
			},
		];
		let profile = identities(&units);
		let car = profile.resolve().unwrap();
		let gateway = car.units.iter().find(|u| u.address.request == 0x710).unwrap();
		let vag_uds_sim::profile::Reads::Value(bitmap) = &gateway.dids[&vag_uds_client::gateway::INSTALLATION_LIST].reads else {
			panic!()
		};
		assert_eq!(
			vag_uds_client::gateway::decode_installation_list(bitmap),
			vec![0x713],
			"the gateway does not list itself"
		);
		let engine = car.units.iter().find(|u| u.address.request == 0x7E0).unwrap();
		assert_eq!(engine.dids[&0xF187].reads, vag_uds_sim::profile::Reads::Value(b"PART-E".to_vec()));
	}

	#[test]
	fn an_earlier_file_wins_and_a_later_one_fills_in() {
		let mut a = Profile::default();
		unit(&mut a, 0x7E0).part = Some("A".into());
		unit(&mut a, 0x7E0).did.insert("F40D".into(), DidSpec::Value(Hex(vec![0])));
		let mut b = Profile::default();
		unit(&mut b, 0x7E0).part = Some("B".into());
		unit(&mut b, 0x7E0).component = Some("C".into());
		unit(&mut b, 0x7E0).did.insert("F40D".into(), DidSpec::Value(Hex(vec![1])));
		merge(&mut a, b);
		let engine = &a.units[0];
		assert_eq!((engine.part.as_deref(), engine.component.as_deref()), (Some("A"), Some("C")));
		assert_eq!(engine.did["F40D"], DidSpec::Value(Hex(vec![0])));
	}

	#[test]
	fn a_device_is_not_a_profile() {
		assert!(is_device(Path::new("/dev/null")));
		assert!(!is_device(Path::new("Cargo.toml")));
	}
}
