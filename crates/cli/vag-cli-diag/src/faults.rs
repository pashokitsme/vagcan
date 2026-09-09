//! `vagcan faults` — what the car has stored against itself.
//!
//! `survey` reports fault *counts* as a by-product of walking the car; this
//! command is the fault reader proper: every unit, every confirmed code, and
//! on request the extended data the unit keeps beside it — which is where the
//! occurrence counter and the mileage stamp live on these control units.
//!
//! Two honesty rules run through it:
//!
//! * **Only confirmed codes are called faults.** Asking with status mask
//!   `0xFF` returns everything the unit knows about, including tests that have
//!   simply never run since the memory was last cleared. On the reference car
//!   the body control module answers 508 codes that way, of which three are
//!   actual stored faults.
//! * **A code is printed as a code until something names it.** The texts come
//!   from the ODIS project's own fault tables first ([`crate::odisfaults`])
//!   and from the VCDS label files where the project has none
//!   ([`crate::faultnames`]); where neither resolves one, it shows the raw
//!   bytes rather than a plausible-sounding invention. [`Namers`] is the
//!   order, in one place for both the live and the recorded path.
//!
//! Read-only: the service issued is `0x19`, which reads. Clearing faults is
//! `0x14`, which the client's allowlist rejects.

use anyhow::{Context, Result};
use vag_uds_can::{IsoTpCan, SlcanMode};
use vag_uds_client::address::UnitAddress;
use vag_uds_client::dtc::{CarTime, FaultContext, UnitStamp};
use vag_uds_client::{AsyncUdsClient, RawDtc, gateway};
use vag_uds_transport::CanId;

/// Status bit 3 — the unit confirmed this failure, as opposed to merely
/// listing the code.
pub const CONFIRMED: u8 = 0x08;

/// Status bit 0 — the test is failing at this moment, not historically.
pub const FAILED_NOW: u8 = 0x01;

/// What one unit reported.
#[derive(Debug, Clone, Default)]
pub struct UnitFaults {
	/// The unit's own component string, when it gave one.
	pub component: Option<String>,
	/// Every code the unit listed, confirmed or not.
	pub all: Vec<RawDtc>,
}

impl UnitFaults {
	pub fn confirmed(&self) -> Vec<&RawDtc> {
		self.all.iter().filter(|d| d.status & CONFIRMED != 0).collect()
	}
}

/// How a code is written: the three bytes, and the decimal fault number VW's
/// own tools print.
///
/// **All three bytes are one number**, big-endian. An earlier version of this
/// function split them into a two-byte number and a symptom byte, which was
/// wrong: `00 01 29` is fault 297, not fault 1 symptom 0x29. The refutation is
/// this car's own VCDS scan, which prints `0297` beside the brake unit's code
/// and `291104` beside the steering column's `04 71 20`
/// (`vendor/vcds-ru/Scans/`), matching the 24-bit reading in all four cases
/// checked and the 16-bit one in none.
pub fn format_code(code: [u8; 3]) -> String {
	let number = u32::from_be_bytes([0, code[0], code[1], code[2]]);
	format!("{:02X}{:02X}{:02X}  ({number})", code[0], code[1], code[2])
}

/// How long ago a fault happened, told against the unit's own counters.
///
/// Both halves are differences taken on the same control unit, so neither
/// depends on the car's clock being set right: the stamp is a packed date
/// (`vag_uds_client::dtc::CarTime`) and the difference between two of them is
/// real elapsed time even when the date itself is days out, as it is here.
/// The difference is taken through `seconds_between`, which unpacks first —
/// subtracting the raw stamps overshoots by 4 counts per minute boundary.
pub fn describe_age(context: &FaultContext, now: Option<&UnitStamp>) -> String {
	let mut parts = vec![format!("{} km", context.mileage_km)];
	match context.occurrences {
		0xFF => parts.push("255+ times".to_string()),
		n => parts.push(format!("{n}×")),
	}
	if let Some(now) = now {
		if let Some(seconds) = vag_uds_client::dtc::seconds_between(context.clock, now.clock) {
			let seconds = seconds as f64;
			parts.push(match seconds {
				s if s < 90.0 => format!("{s:.0} s ago"),
				s if s < 5_400.0 => format!("{:.0} min ago", s / 60.0),
				s if s < 172_800.0 => format!("{:.1} h ago", s / 3_600.0),
				s => format!("{:.1} days ago", s / 86_400.0),
			});
		}
		if now.mileage_km >= context.mileage_km {
			parts.push(format!("{} km ago", now.mileage_km - context.mileage_km));
		}
	}
	parts.join(", ")
}

/// When a fault was stored, by the car's own clock.
///
/// Exact, to the second: the stamp is a packed date and time, and two VCDS
/// printouts of this car reproduce field for field (`vag_uds_client::dtc::CarTime`).
/// It is the *car's* clock, which on this car runs some days behind real time,
/// so the wording says whose clock it is.
pub fn describe_when(context: &FaultContext) -> String {
	match CarTime::parse(context.clock) {
		Some(t) => format!(
			"{:04}-{:02}-{:02} {:02}:{:02}:{:02} by the car's own clock",
			t.year, t.month, t.day, t.hour, t.minute, t.second
		),
		// An unset or corrupt stamp is not a moment.
		None => format!("clock {:08X}, not a date", context.clock),
	}
}

/// Decode the status byte into the states it actually asserts.
///
/// Straight out of ISO 14229-1 table D.1 — every bit is defined there, so
/// nothing here is inferred from this car.
pub fn describe_status(status: u8) -> String {
	const BITS: [(u8, &str); 8] = [
		(0x01, "failed now"),
		(0x02, "failed this cycle"),
		(0x04, "pending"),
		(0x08, "confirmed"),
		(0x10, "not tested since clear"),
		(0x20, "failed since clear"),
		(0x40, "not tested this cycle"),
		(0x80, "warning lamp"),
	];
	let set: Vec<&str> = BITS.iter().filter(|(bit, _)| status & bit != 0).map(|(_, name)| *name).collect();
	if set.is_empty() { format!("{status:02X}") } else { set.join(", ") }
}

/// The identifiers by which a unit names its own description file: the ODX
/// name, and the coding index whose first three digits pick the variant
/// (`.archive/research/labels/fault-naming-hop.md` §10.4). Both come off the car.
const ODX_NAME: u16 = 0xF19E;
const ODX_VERSION: u16 = 0xF1A2;

/// Read one identifier value as the padded ASCII these units store.
fn ident_text(bytes: &[u8]) -> String {
	String::from_utf8_lossy(bytes).trim_end_matches(['\0', ' ']).to_string()
}

/// Both ways of naming a code, in the order they are asked.
///
/// **The project first, the label files second.** An ODIS project carries a
/// fault table per ECU variant with the text in the clear, and a VCDS
/// installation names a code through a registry, a per-unit catalogue and a
/// text store, each of which can be missing or sealed. The project is asked
/// first because it answers for the exact variant the unit named; the chain
/// is asked where the project has no table for the unit, or where
/// `[faults] language` names a language only the VCDS build declares.
///
/// Either half may be absent — a project set up from one source has one —
/// and a command with neither still prints the codes.
pub struct Namers {
	odis: Option<crate::odisfaults::OdisFaults>,
	vcds: Option<crate::faultnames::Namer>,
	/// Where the VCDS files were looked for, for the note when there are none.
	vcds_root: std::path::PathBuf,
}

/// What both halves know about one unit, looked up once.
pub struct UnitNamers {
	odis: Option<crate::odisfaults::UnitTexts>,
	vcds: Option<vag_data_labels::UnitLookup>,
}

impl Namers {
	/// Open whatever this machine has. Never fails for a shortage of data —
	/// see [`Namers::is_empty`] — only for a VCDS pool that is there and will
	/// not open.
	pub fn open(iv_cache: &str) -> Result<Namers> {
		let vcds_root = crate::project::rod_pool()?;
		let vcds = if crate::faultnames::has_fault_labels(&vcds_root) {
			Some(crate::faultnames::Namer::open(&vcds_root, &crate::datadir::resolve(iv_cache))?)
		} else {
			None
		};
		Ok(Namers {
			odis: crate::odisfaults::OdisFaults::open(),
			vcds,
			vcds_root,
		})
	}

	/// Whether there is nothing at all to name a code with.
	pub fn is_empty(&self) -> bool {
		self.odis.is_none() && self.vcds.is_none()
	}

	/// Where the VCDS files were looked for.
	pub fn vcds_root(&self) -> &std::path::Path {
		&self.vcds_root
	}

	/// The lines that say where the names come from, for above a listing.
	pub fn describe(&self) -> Vec<String> {
		let mut out = Vec::new();
		if let Some(odis) = &self.odis {
			out.push(odis.describe());
			if let Some(note) = odis.choice_note() {
				out.push(note);
			}
		}
		if let Some(vcds) = &self.vcds {
			out.push(format!(
				"{} rows of fault registry, {} texts, from {}{}",
				vcds.registry_rows(),
				vcds.codes_texts(),
				self.vcds_root.display(),
				if self.odis.is_some() { " — the fallback" } else { "" }
			));
		}
		out
	}

	/// Look a unit up in both halves by what it said about itself.
	pub fn unit(&mut self, odx_name: &str, version: &str) -> UnitNamers {
		UnitNamers {
			odis: self.odis.as_mut().map(|o| o.unit(odx_name, version)),
			vcds: self.vcds.as_mut().map(|n| n.unit(odx_name, version)),
		}
	}

	/// Why a unit's codes may come out as numbers, one line each.
	///
	/// The VCDS chain's own reasons are only worth saying when that chain is
	/// what the unit's codes will be named from — a sealed catalogue is no
	/// loss for a unit the project names in full.
	pub fn notes(&self, unit: &UnitNamers) -> Vec<String> {
		let mut out = Vec::new();
		let project_has_it = unit.odis.as_ref().is_some_and(|u| !u.is_empty());
		if self.odis.is_some() && !project_has_it {
			out.push(match &self.vcds {
				Some(_) => "the ODIS project has no fault table for this unit — VCDS text".to_string(),
				None => "the ODIS project has no fault table for this unit — codes only".to_string(),
			});
		}
		let vcds_matters = !project_has_it || self.odis.as_ref().is_some_and(|o| o.prefers_vcds());
		if vcds_matters && let Some(note) = unit.vcds.as_ref().and_then(crate::faultnames::unit_note) {
			out.push(note);
		}
		out
	}

	/// The VCDS catalogue a unit's codes would need opened, if it is sealed.
	pub fn sealed(unit: &UnitNamers) -> Option<std::path::PathBuf> {
		match &unit.vcds {
			Some(vag_data_labels::UnitLookup::Locked { file }) => Some(file.clone()),
			_ => None,
		}
	}

	/// One line for a code, and whether that line is a name rather than a
	/// reason. `None` when there is nothing to say under the number.
	pub fn name(&self, unit: &UnitNamers, code: [u8; 3]) -> Option<(String, bool)> {
		let odis = self.odis.as_ref().zip(unit.odis.as_ref()).and_then(|(o, u)| o.name(u, code));
		let vcds = match (&self.vcds, &unit.vcds) {
			(Some(namer), Some(vag_data_labels::UnitLookup::Found { catalogue, .. })) => Some(namer.name(catalogue, code)),
			_ => None,
		};
		let vcds_named = vcds.as_ref().filter(|n| matches!(n, crate::faultnames::Naming::Named { .. }));
		if self.odis.as_ref().is_some_and(|o| o.prefers_vcds())
			&& let Some(line) = vcds_named.and_then(|n| n.line())
		{
			return Some((line, true));
		}
		if let Some(naming) = odis {
			return Some((naming.line(), true));
		}
		if let Some(naming) = vcds {
			return naming
				.line()
				.map(|line| (line, matches!(naming, crate::faultnames::Naming::Named { .. })));
		}
		// The project has a table for the unit and this number is not in it,
		// and nothing else can be asked: said, rather than left blank.
		let project_has_it = unit.odis.as_ref().is_some_and(|u| !u.is_empty());
		project_has_it.then(|| ("not in this unit's fault table in the ODIS project".to_string(), false))
	}
}

/// Name the faults in a survey this tool recorded (`vagcan faults --from`).
///
/// The naming chain needs nothing from the car that a survey does not already
/// hold — the fault codes, and the two identifiers that pick each unit's
/// description file — so it runs offline against a recorded file. That is what
/// makes the whole chain testable without the adapter, and it is how the
/// figures in `.archive/research/labels/fault-naming-hop.md` §11.3 are reproduced.
pub fn run_named(survey_path: &str, iv_cache: &str, all_codes: bool) -> Result<()> {
	let text = std::fs::read_to_string(survey_path).with_context(|| format!("reading {survey_path:?}"))?;
	// Naming a recorded survey with nothing to name from is nothing this can do,
	// so a machine with neither source is a clear stop pointing at `vagcan
	// setup` rather than a bare "the registry did not decode".
	let mut namers = Namers::open(iv_cache)?;
	if namers.is_empty() {
		anyhow::bail!(crate::missing::cannot_name_faults(namers.vcds_root()));
	}
	for line in namers.describe() {
		println!("{line}");
	}
	println!();

	let (mut named, mut unnamed) = (0usize, 0usize);
	// Every sealed catalogue this car names, asked about once at the end rather
	// than printed as a command under each unit that hit one.
	let mut sealed: Vec<std::path::PathBuf> = Vec::new();
	for line in text.lines() {
		let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
			continue;
		};
		let Some(codes) = value["dtcs"].as_array() else { continue };
		let codes: Vec<(RawDtc, u8)> = codes
			.iter()
			.filter_map(|d| {
				let code = parse_code(d["code"].as_str()?)?;
				let status = u8::from_str_radix(d["status"].as_str()?, 16).ok()?;
				Some((RawDtc { code, status }, status))
			})
			.filter(|(dtc, _)| all_codes || dtc.status & CONFIRMED != 0)
			.collect();
		if codes.is_empty() {
			continue;
		}
		let unit = value["unit"].as_str().unwrap_or("--").to_string();
		let ident = |did: u16| -> String {
			value["ident"]
				.as_array()
				.into_iter()
				.flatten()
				.find(|f| f["did"].as_str() == Some(&format!("{did:04X}")))
				.and_then(|f| f["data"].as_str())
				.and_then(vag_cli_core::plan::hex_bytes)
				.map(|b| ident_text(&b))
				.unwrap_or_default()
		};
		let (odx, version) = (ident(ODX_NAME), ident(ODX_VERSION));
		println!("{unit}  {odx}");
		let lookup = namers.unit(&odx, &version);
		if let Some(file) = Namers::sealed(&lookup)
			&& !sealed.contains(&file)
		{
			sealed.push(file);
		}
		for note in namers.notes(&lookup) {
			println!("  ({note})");
		}
		for (dtc, _) in &codes {
			let naming = namers.name(&lookup, dtc.code);
			if naming.as_ref().is_some_and(|(_, is_name)| *is_name) {
				named += 1;
			} else {
				unnamed += 1;
			}
			println!("  {}   {}", format_code(dtc.code), describe_status(dtc.status));
			if let Some((line, _)) = naming {
				println!("      {line}");
			}
		}
		println!();
	}
	println!("{named} of {} {} named.", named + unnamed, crate::render::plural(named + unnamed, "code"));
	crate::faultnames::offer_to_unseal(&sealed, &crate::datadir::resolve(iv_cache))?;
	Ok(())
}

fn parse_code(text: &str) -> Option<[u8; 3]> {
	let bytes = vag_cli_core::plan::hex_bytes(text)?;
	(bytes.len() == 3).then(|| [bytes[0], bytes[1], bytes[2]])
}

/// Read faults from the car (see the module docs).
#[allow(clippy::too_many_arguments)]
pub async fn run(
	device_path: &str,
	baud: u32,
	only: Option<&str>,
	details: bool,
	all_codes: bool,
	supported: bool,
	extended: bool,
	iv_cache: &str,
) -> Result<()> {
	// Arguments first: the adapter is a single-user resource, so a typo in
	// --ecu must not cost the port before it is reported.
	// The fault texts are opened before the port for the same reason: a missing
	// `Codes.dat` is a mistake to report, not one to make after taking the
	// adapter and reading the whole car.
	//
	// The names come from what `vagcan setup` wrote — the project's cache and
	// the shared pool of VCDS files — and from nowhere else. Neither being
	// there is the ordinary "setup has not run yet" case: the codes are still
	// read and shown as numbers, with a note that names `vagcan setup`.
	let mut namers = Namers::open(iv_cache)?;
	let requested = only.map(|spec| crate::declared::unit_list("--ecu", spec)).transpose()?;

	let mut backend = crate::device::open(device_path, baud, SlcanMode::Normal).await?;

	if extended {
		// An extended session is workshop mode; see `crate::safety`.
		backend = match crate::safety::require_stationary(backend).await {
			Ok(backend) => backend,
			Err((_, why)) => anyhow::bail!("--extended refused: {why}"),
		};
	}

	let order = match requested {
		Some(ids) => ids,
		None => {
			let gw = UnitAddress::from_request(0x710).expect("the gateway is in VW's block");
			let mut uds = AsyncUdsClient::new(IsoTpCan::new(backend, CanId::Standard(gw.request), CanId::Standard(gw.response)));
			let listed = match uds.read_data_by_identifier(gateway::INSTALLATION_LIST).await {
				Ok(bitmap) => gateway::decode_installation_list(&bitmap),
				Err(e) => {
					println!("the gateway did not list the car's units ({e})");
					Vec::new()
				}
			};
			backend = uds.into_transport().into_backend();
			// The engine, the gearbox and the gateway itself are never in the
			// list — the first two live on the other id block, and the
			// gateway does not list itself.
			let mut ids = vec![0x7E0, 0x7E1, 0x710];
			for id in listed {
				if !ids.contains(&id) {
					ids.push(id);
				}
			}
			ids
		}
	};

	// The caveat belongs before the codes, not after them. A list of hex on a
	// screen headed "faults" is read as a verdict on the car; by the time a
	// disclaimer arrives at the bottom the reader has already had the fright.
	if !supported {
		println!(
			"Stored codes are a record that something happened once — not a diagnosis, and \n\
             not necessarily a fault present now. Only codes marked \"failed now\" are \n\
             currently failing.\n"
		);
		// Where something opened, each code names itself or says why it could
		// not, so only where the names come from is owed here. Where nothing
		// did, the data has not been set up yet — the codes read fine, and the
		// note says how to name them.
		if namers.is_empty() {
			println!("{}\n", crate::missing::no_fault_labels(namers.vcds_root()));
		} else {
			for line in namers.describe() {
				println!("{line}");
			}
			println!();
		}
	}

	let mut total = 0usize;
	let mut sealed: Vec<std::path::PathBuf> = Vec::new();
	let mut failing_now = 0usize;
	let count = order.len();
	let mut progress = crate::progress::Line::new();
	for (at, request) in order.into_iter().enumerate() {
		progress.update(&format!("reading faults — {request:03X}, unit {} of {count}", at + 1));
		let Some(address) = UnitAddress::from_request(request) else { continue };
		let mut uds = AsyncUdsClient::new(IsoTpCan::new(
			backend,
			CanId::Standard(address.request),
			CanId::Standard(address.response),
		));
		// See `crate::safety`: an extended session is workshop mode, and this
		// command reads faults perfectly well without one.
		if extended {
			let _ = uds.start_session(0x03).await;
		}

		// The unit names itself; nothing here maps an address to a name.
		let component = uds
			.read_data_by_identifier(0xF197)
			.await
			.ok()
			.map(|b| String::from_utf8_lossy(&b).trim_end_matches(['\0', ' ']).to_string())
			.filter(|s| !s.is_empty());
		let unit = UnitFaults {
			component,
			all: uds.read_dtcs_by_status_mask(0xFF).await.unwrap_or_default(),
		};
		// The unit's own "now": the same odometer and counter its faults are
		// stamped with, so ages are differences rather than dates.
		let now = uds
			.read_data_by_identifier(UnitStamp::DID)
			.await
			.ok()
			.and_then(|data| UnitStamp::parse(&data));

		if supported {
			// The unit's whole catalogue of codes, in its own order — which is
			// how the label files store fault names, so the two lists are
			// worth comparing.
			match uds.read_supported_dtcs().await {
				Ok(list) => {
					println!(
						"\n{}  {:03X}  {}  — {} {} supported",
						address.label(),
						request,
						unit.component.clone().unwrap_or_default(),
						list.len(),
						crate::render::plural(list.len(), "code")
					);
					for dtc in &list {
						println!("  {}", format_code(dtc.code));
					}
					total += list.len();
				}
				Err(e) => println!("\n{}  {:03X}  no supported list ({e})", address.label(), request),
			}
			backend = uds.into_transport().into_backend();
			continue;
		}

		let mut show: Vec<RawDtc> = if all_codes {
			unit.all.clone()
		} else {
			unit.confirmed().into_iter().cloned().collect()
		};
		// Something failing right now outranks a code stored months ago.
		show.sort_by_key(|d| (d.status & FAILED_NOW == 0, d.code));
		failing_now += show.iter().filter(|d| d.status & FAILED_NOW != 0).count();
		if !show.is_empty() {
			progress.finish();
			// A unit with no short number shows a dash rather than repeating
			// its id, which reads as a rendering fault.
			let number = match vag_uds_client::address::short_number(request) {
				Some(n) => format!("{n:02X}"),
				None => "--".to_string(),
			};
			println!("\n{number}  {request:03X}  {}", unit.component.clone().unwrap_or_default());
			// The unit names its own variant and description file, so they are
			// read only once something has to be named out of them.
			let lookup = match namers.is_empty() {
				true => None,
				false => {
					let read = |data: Option<Vec<u8>>| data.map(|b| ident_text(&b)).unwrap_or_default();
					let odx = read(uds.read_data_by_identifier(ODX_NAME).await.ok());
					let version = read(uds.read_data_by_identifier(ODX_VERSION).await.ok());
					Some(namers.unit(&odx, &version))
				}
			};
			if let Some(file) = lookup.as_ref().and_then(Namers::sealed)
				&& !sealed.contains(&file)
			{
				sealed.push(file);
			}
			for note in lookup.as_ref().map(|l| namers.notes(l)).unwrap_or_default() {
				println!("  ({note})");
			}
			for dtc in &show {
				println!("  {}   {}", format_code(dtc.code), describe_status(dtc.status));
				// The name goes under the code, never instead of it: the
				// number is what the car said and the name is this project's
				// reading of it.
				if let Some((line, _)) = lookup.as_ref().and_then(|l| namers.name(l, dtc.code)) {
					println!("      {line}");
				}
				// Extended data carries when it happened: the odometer at the
				// time and how often. Read for every fault, since that is the
				// question a stored code raises.
				let records = uds.read_dtc_extended(dtc.code).await.unwrap_or_default();
				for record in &records {
					match FaultContext::parse(&record.data) {
						Some(context) => {
							println!("      {}", describe_age(&context, now.as_ref()));
							println!("      {}", describe_when(&context));
						}
						// A record this project cannot read is shown, not
						// dropped — and not guessed at either.
						None if details => println!(
							"      record {:02X}: {}",
							record.record,
							record.data.iter().map(|b| format!("{b:02X}")).collect::<String>()
						),
						None => {}
					}
					if details {
						println!(
							"      raw {:02X}: {}",
							record.record,
							record.data.iter().map(|b| format!("{b:02X}")).collect::<String>()
						);
					}
				}
			}
			total += show.len();
		}
		backend = uds.into_transport().into_backend();
	}

	if supported {
		return Ok(());
	}
	if total == 0 {
		println!("No stored codes.");
		return Ok(());
	}
	println!(
		"\n{total} stored {}. {}",
		if total == 1 { "code" } else { "codes" },
		match failing_now {
			0 => "None is failing now — all of them are history.".to_string(),
			1 => "1 is failing now; the rest are history.".to_string(),
			n => format!("{n} are failing now; the rest are history."),
		}
	);
	crate::faultnames::offer_to_unseal(&sealed, &crate::datadir::resolve(iv_cache))?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn names_come_from_the_one_pool_setup_filled() {
		// The whole point of the copy: after `vagcan setup`, `faults` names codes
		// with no flag, because the raw files live under ~/.vagcan. The pool is
		// shared across projects — a fault registry is a property of a VCDS
		// build, not of one car — while the keys that open it are per project,
		// because a key is a property of one file's bytes (design §4.2).
		assert_eq!(crate::datadir::rod_pool_dir().unwrap(), crate::datadir::vagcan_dir().unwrap().join("rod"));
	}

	#[test]
	fn only_confirmed_codes_count_as_faults() {
		let unit = UnitFaults {
			all: vec![
				RawDtc {
					code: [0x00, 0x01, 0x07],
					status: 0x10,
				},
				RawDtc {
					code: [0x06, 0x09, 0x01],
					status: 0x08,
				},
			],
			..Default::default()
		};
		let confirmed = unit.confirmed();
		assert_eq!(confirmed.len(), 1);
		assert_eq!(confirmed[0].code, [0x06, 0x09, 0x01]);
	}

	#[test]
	fn the_status_byte_is_read_bit_by_bit_from_the_standard() {
		assert_eq!(describe_status(0x08), "confirmed");
		assert_eq!(describe_status(0x10), "not tested since clear");
		assert_eq!(describe_status(0x09), "failed now, confirmed");
		// Nothing asserted: report the byte rather than an empty line.
		assert_eq!(describe_status(0x00), "00");
	}

	#[test]
	fn a_fault_is_dated_by_the_cars_own_counters_not_by_a_calendar() {
		// Real record from the body control module, against the stamp read
		// from the same unit: 9 occurrences, 42 km and 17.9 hours ago.
		let context = FaultContext {
			priority: 6,
			occurrences: 9,
			cycle_counter: 0x02B8,
			mileage_km: 212_763,
			clock: 0x69F9_044B,
		};
		let now = UnitStamp {
			mileage_km: 212_805,
			clock: 0x69FA_005C,
		};
		let text = describe_age(&context, Some(&now));
		assert!(text.contains("212763 km"), "{text}");
		assert!(text.contains("9×"), "{text}");
		// The two stamps are 2026-07-28 00:18:19 and 2026-07-28 08:01:32 by
		// the car's clock — seven and three quarter hours apart. Reading the
		// stamp as a counter, as this once did, gave 17.9 h.
		assert!(text.contains("7.7 h ago"), "{text}");
		assert!(text.contains("42 km ago"), "{text}");

		// With nothing to compare against, only what the record itself says.
		let alone = describe_age(&context, None);
		assert!(alone.contains("212763 km"));
		assert!(!alone.contains("ago"), "{alone}");
	}

	#[test]
	fn a_fault_is_stamped_with_the_cars_own_date_and_time() {
		// Both anchors come from VCDS printouts of this car: one unit's fault
		// at 2026.07.27 00:00:03 and another's at 2026.07.28 23:50:02.
		// Six fields, two exact matches.
		let at = |clock| {
			describe_when(&FaultContext {
				priority: 1,
				occurrences: 1,
				cycle_counter: 0,
				mileage_km: 1,
				clock,
			})
		};
		assert_eq!(at(0x69F6_0003), "2026-07-27 00:00:03 by the car's own clock");
		assert_eq!(at(0x69F9_7C82), "2026-07-28 23:50:02 by the car's own clock");
		// An unset stamp is reported as one, not as the year 2000.
		assert!(at(0x0000_0000).contains("not a date"));
	}

	#[test]
	fn a_saturated_occurrence_count_is_not_reported_as_exactly_255() {
		let context = FaultContext {
			priority: 6,
			occurrences: 0xFF,
			cycle_counter: 0x02B9,
			mileage_km: 212_795,
			clock: 0x69F9_68D9,
		};
		assert!(describe_age(&context, None).contains("255+ times"));
	}

	#[test]
	fn all_three_bytes_are_one_fault_number() {
		// Checked against this car's own VCDS scan, which prints these decimal
		// numbers for these units: reading the first two bytes as the number
		// would give 1, 1137 and 260 instead.
		assert_eq!(format_code([0x00, 0x01, 0x29]), "000129  (297)");
		assert_eq!(format_code([0x04, 0x71, 0x20]), "047120  (291104)");
		assert_eq!(format_code([0x01, 0x04, 0x05]), "010405  (66565)");
	}
}
