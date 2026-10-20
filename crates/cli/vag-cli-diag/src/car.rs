//! The car commands that read one or a few identifiers and print them: `info`, `units`
//! (the listing, `--identify`, and `--identify <unit>`), `sensors`, and the F19E read
//! behind `dev vcds labels --from-car`.
//!
//! Each is generic over [`UnitLink`] and takes the link through `open`, called only once
//! every argument has been checked — so a command runs through the bus unchanged, and
//! against a scripted link in a test. Data goes to stdout and the rest to stderr at its
//! level ([`vag_cli_core::verbosity`]).

use anyhow::{Context as _, Result};
use vag_cli_core::{note, out, say};
use vag_uds_can::UnitLink;
use vag_uds_client::address::UnitAddress;
use vag_uds_client::{AsyncUdsClient, UdsReadExt};

use crate::{anomaly, labels, plan, progress, project, props, render, safety, scan};

/// Parse how the user named a control unit — `01`, `17`, or a request id like
/// `70E`. Which id block it lives on, and therefore which response rule
/// applies, is decided by `vag_uds_client::address`.
///
/// `flag` is the flag the unit was typed on (`--unit`, `--identify`), and what an
/// error is said under.
pub fn parse_unit(flag: &str, text: &str) -> Result<UnitAddress> {
	vag_uds_client::address::parse(text).map_err(|e| anyhow::anyhow!("{flag}: {e}"))
}

/// A unit's short number as every table writes it — `01` — and `--` for one with none,
/// rather than a blank that reads as a column out of place.
pub fn unit_number(number: Option<u8>) -> String {
	number.map(|n| format!("{n:02X}")).unwrap_or_else(|| "--".to_string())
}

/// Address one control unit over UDS.
fn address_unit<L: UnitLink>(link: L, unit: UnitAddress) -> AsyncUdsClient<L::Channel> {
	AsyncUdsClient::new(link.to_unit(
		vag_uds_transport::CanId::Standard(unit.request),
		vag_uds_transport::CanId::Standard(unit.response),
	))
}

/// Identify the car (see the `Info` subcommand docs): the VIN, and the engine's and the
/// gearbox's identification.
pub async fn info<L: UnitLink>(open: impl AsyncFnOnce() -> Result<L>) -> Result<()> {
	// One link, two control units: read the engine, then release the link and
	// address the gearbox rather than re-opening it.
	let engine_unit = vag_uds_client::address::parse("01").expect("01 is a unit number");
	let mut engine_uds = address_unit(open().await?, engine_unit);
	let engine = engine_uds.read_identity().await;

	let gearbox_unit = vag_uds_client::address::parse("02").expect("02 is a unit number");
	let mut gearbox_uds = address_unit(L::release(engine_uds.into_transport()), gearbox_unit);
	let gearbox = gearbox_uds.read_identity().await;

	if engine.is_empty() && gearbox.is_empty() {
		render::say_nothing_answered();
		return Ok(());
	}
	out!("{}", render::render_info(engine.vin.as_deref(), &engine, &gearbox));
	note!("next: `vagcan units` (the other units) · `faults` · `sensors` · `watch`");
	Ok(())
}

/// Which link a command may be opened on. The command decides, once its arguments are
/// read; whoever opens the device carries it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
	/// Whatever `--device` names: a cable adapter, or the dash board on its cable or over BLE.
	AnyLink,
	/// A cable adapter only. Never through the dash board, unless `--slcan` makes the board
	/// a plain adapter — the rule for a sweep (`CLAUDE.md`, Safety).
	CableOnly,
}

/// Read the standard OBD-II sensors (see the `Sensors` subcommand docs).
///
/// The table in `vag_data_labels::obd` is SAE J1979's, and J1979 is only binding on
/// the emissions-related units ISO 15765-4 addresses. Every identifier that
/// answers is still shown; whether its bytes become a number is decided by
/// `obd::conversion_for`, per parameter, from the unit's block and the width of
/// what it actually answered.
///
/// **Where it is a sweep, and where it is not.** `7E0`–`7E7` is the block ISO 15765-4
/// gives the legislated OBD servers, the emissions-related units. `F400`–`F4FF` is where
/// ISO 14229-1 (Annex C) and SAE J1979-2 put the J1979 parameters under `0x22`: the same
/// parameters a generic scan tool asks with service `01` of a running, moving car. Read off
/// a unit in that block they are what the regulation has that unit serve, and the
/// moving-car check is itself one of them (`F40D` off `7E0`,
/// [`safety::require_stationary`]). Plain
/// `sensors` is the engine (`--unit 01`), so it is not a sweep: it runs on any link and at
/// any speed, and over BLE the board's own guards apply. Outside the block nothing binds a
/// unit to these identifiers — the climate unit answers `F405` and means something else
/// ([`UnitAddress::is_emissions_related`]) — so there the reads are guesses at one
/// diagnostic server, which is a sweep, and it is guarded as `units --identify <unit>` is:
/// one named unit; the fixed list in `obd::PIDS`, never a range; refused on a moving car,
/// with "no answer" counted as moving; through a cable adapter only ([`Reach::CableOnly`]).
/// On every unit the read stops the moment the unit falls silent under it
/// ([`anomaly::Monitor`]).
pub async fn sensors<L: UnitLink>(open: impl AsyncFnOnce(Reach) -> Result<L>, unit_text: &str) -> Result<()> {
	use render::SensorLine;
	use vag_data_labels::obd::{self, PIDS};

	labels::install_recorded_units();
	let unit = parse_unit("--unit", unit_text)?;
	let established = unit.is_emissions_related();
	let mut backend = open(if established { Reach::AnyLink } else { Reach::CableOnly }).await?;
	if !established {
		backend = match safety::require_stationary(backend).await {
			Ok(backend) => backend,
			Err((_, why)) => anyhow::bail!(
				"{why}.\n\
                 Read it parked: {} standard identifiers asked of a unit outside the emissions block \
                 (7E0–7E7) are a sweep, and a sweep can stop a unit doing its job while the car moves.",
				PIDS.len()
			),
		};
	}
	let mut uds = address_unit(backend, unit);

	// Ask for every standard parameter; the unit refuses the ones it does not
	// implement, and those are skipped rather than failing the run. Each identifier is a
	// span of one: the list is fixed, and nothing between two of them is asked. No
	// witness and no keepalive: each identifier is asked once, and what the monitor can
	// see here is a unit that had been answering going quiet.
	let ranges: Vec<_> = PIDS
		.iter()
		.map(|p| {
			let did = obd::did_for_pid(p.pid);
			did..=did
		})
		.collect();
	let mut monitor = anomaly::Monitor::new(unit.request);
	let mut guard = scan::Guard {
		witness: None,
		monitor: &mut monitor,
	};
	let mut answered = Vec::new();
	let stats = scan::scan_dids(&mut uds, &ranges, std::time::Duration::ZERO, 0, &mut guard, |hit| {
		answered.push(hit.clone());
		Ok(())
	})
	.await?;
	if let Some(halt) = monitor.halted() {
		let mut progress = progress::Line::new();
		progress.notice(&halt.report());
		anyhow::bail!("the read was stopped: control unit {} changed while it was being read", halt.unit());
	}

	let mut lines = Vec::new();
	for scan::DidHit { did, data: bytes } in answered {
		let p = PIDS
			.iter()
			.find(|p| obd::did_for_pid(p.pid) == did)
			.expect("only the listed identifiers are asked");
		lines.push(match obd::conversion_for(p, established, &bytes) {
			Ok(def) => SensorLine::Converted {
				did,
				reading: vag_uds_client::Reading {
					name: def.name.to_string(),
					unit: def.unit.to_string(),
					value: def.interpret(&bytes),
					raw: bytes,
				},
			},
			Err(why) => SensorLine::Unconverted { did, bytes, why },
		});
	}

	if lines.is_empty() {
		render::say_nothing_answered();
		return Ok(());
	}
	let (data, notes) = render::render_sensors(&unit.label(), &lines);
	out!("{data}");
	note!(
		"{} standard parameters asked, {} not answered or refused",
		PIDS.len(),
		stats.refused + stats.failed
	);
	for line in notes {
		note!("{line}");
	}
	Ok(())
}

/// Read the ODX label-file name a control unit reports for itself (F19E).
pub async fn odx_name_from_car<L: UnitLink>(open: impl AsyncFnOnce() -> Result<L>, unit_text: &str) -> Result<String> {
	const ODX_FILE_NAME: u16 = 0xF19E;

	labels::install_recorded_units();
	let unit = parse_unit("--unit", unit_text)?;
	let mut uds = address_unit(open().await?, unit);
	let data = uds
		.read_data_by_identifier(ODX_FILE_NAME)
		.await
		.context("reading the ODX file name (F19E) from the control unit")?;
	let name = String::from_utf8_lossy(&data).trim_end_matches(['\0', ' ']).to_string();
	if name.is_empty() {
		anyhow::bail!("the control unit returned an empty ODX file name");
	}
	Ok(name)
}

/// List the car's control units (see the `Units` subcommand docs).
pub async fn units<L: UnitLink>(open: impl AsyncFnOnce() -> Result<L>, identify: bool) -> Result<()> {
	use vag_cli_core::units as walk;

	// The label files turn a part number the car reports into the unit's diagnostic
	// address and name, for any VAG car rather than for a list written here.
	// They come from what `vagcan setup` extracted, so `units --identify`
	// resolves names with no flag, the plain listing names what it can from them,
	// and a machine that has not run setup simply goes without.
	//
	// A machine with no project set up simply identifies without names — this is
	// the ordinary "setup has not run yet" case, not an error.
	let label_files_dir = project::current().ok().filter(labels::has_project_labels);
	let label_files = match &label_files_dir {
		Some(project) => match labels::load_project(project) {
			Ok(db) => {
				// The label files' numbering, in force for the rest of the run: what
				// each number *is*. Which id answers it is learned below, from the
				// car.
				labels::install_unit_numbers(&db);
				say!("names    {} label files", project.id);
				Some(db)
			}
			Err(e) if identify => return Err(e),
			// The plain listing only borrows names from them, and a list of ids is
			// still the answer without.
			Err(e) => {
				say!("names    none — the label files of {} would not open: {e:#}", project.id);
				None
			}
		},
		None => {
			say!("names    none — `vagcan setup` names the units");
			None
		}
	};

	// The same read `watch` and `measure` start from, so the three cannot come to
	// different lists.
	let mut spinner = progress::Line::new();
	let (backend, listed) = walk::installed(open().await?, &mut spinner).await;
	spinner.finish();
	// A gateway that does not answer its list ends the plain listing, which has
	// nothing else to show. With `--identify` it does not: `watch` in the same
	// state still identifies the powertrain and the gateway, and so does this,
	// with the cause kept in the one line that says so.
	let (ids, unlisted) = match listed {
		Ok(ids) => (ids, None),
		Err(e) if identify => (Vec::new(), Some(e)),
		Err(e) => return Err(anyhow::Error::new(e).context("reading the car's list of control units from the gateway")),
	};

	if !identify {
		if ids.is_empty() {
			say!("the gateway listed no control units");
			return Ok(());
		}
		// Names from this machine only — nothing more is asked of the car. A record
		// that does not read is no names, not a failed listing.
		let cars = walk::recorded_cars().unwrap_or_default();
		let record = record_of_these(&ids, &cars);
		let by_part = |part: &str| label_files.as_ref()?.unit_for_part(part).map(|u| (u.address, u.name.clone()));
		let by_number = |number: u8| label_files.as_ref()?.unit_name(number).map(str::to_owned);
		out!(
			"{}",
			render_listing(
				&ids,
				record.map(|car| car.units.as_slice()),
				by_part,
				by_number,
				vag_uds_client::address::short_number
			)
		);
		// The record is the one source here that may not be this car's: say whose it is.
		if let Some(car) = record {
			note!("names    and what {}'s units said to `vagcan units --identify`", car.vin);
		}
		return Ok(());
	}

	// The walk `watch` and `measure` make, and no other request: the gateway's
	// list, the gateway itself and the powertrain, four identifiers each, then
	// the VIN off the engine — so that what is recorded here is what they would
	// record.
	let wanted = walk::to_identify(&ids, &[plan::ENGINE]);
	let (backend, found) = walk::identify_listed(backend, &wanted, &mut spinner).await;
	spinner.update("reading the vehicle identification number");
	let (_, vin) = walk::read_vin(backend).await;
	spinner.finish();
	// Written down for the commands that need this car's units without the car:
	// `setup`'s registry step, and the dash build — this is the command a dash
	// owner runs once. A write that fails is one line, and so is a car that gave
	// no VIN to file it under.
	walk::record_for(vin.as_deref(), &found);
	// And the channels of the units nothing has read yet, out of the VCDS
	// installation the project was set up from — as `watch` and `measure` do,
	// so that a dash owner who ran this once has channels to build from.
	vag_cli_core::registry::ensure_async(found.clone()).await;

	let mut asked: Vec<u16> = wanted;
	asked.sort_unstable();
	asked.dedup();
	// An empty list is not a stop either: the gateway and the powertrain are
	// asked, recorded and printed, as `watch` does in the same state.
	match (&unlisted, ids.is_empty()) {
		(Some(cause), _) => say!("the gateway gave no list of control units ({cause}): only the gateway and the powertrain were asked"),
		(None, true) => say!("the gateway listed no control units: only the gateway and the powertrain were asked"),
		(None, false) => {}
	}
	let mut table = vag_cli_core::table::Table::new(&["unit", "id", "part", "component", "name"]);
	let mut answered = 0usize;
	let mut resolved = 0usize;
	for id in &asked {
		let id = *id;
		if UnitAddress::from_request(id).is_none() {
			note!("{id:03X}  skipped: no diagnostic address (700-795 or 7E0-7E7)");
			continue;
		}
		let unit = found.iter().find(|u| u.request == id);
		let part = unit.and_then(|u| u.part_number.clone()).unwrap_or_default();
		let component = unit.and_then(|u| u.component.clone()).unwrap_or_default();
		if part.is_empty() && component.is_empty() {
			table.row(vec![
				unit_number(vag_uds_client::address::short_number(id)),
				format!("{id:03X}"),
				String::new(),
				String::new(),
				"(no answer)".to_string(),
			]);
			continue;
		}
		// Two names, both from data: the unit's own component string, and
		// what the label files call the part number — the latter also
		// supplying the diagnostic address people use.
		answered += 1;
		let name = label_files
			.as_ref()
			.and_then(|db| db.unit_for_part(&part))
			.map(|u| {
				resolved += 1;
				// This is the pairing: the label files say the part number is
				// unit 44, the car says 0x712 answered with it. Neither
				// half is in this program's source, and one read of the
				// car is what joins them.
				vag_uds_client::address::install([vag_uds_client::address::UnitNumber {
					number: u.address,
					request: Some(id),
					name: Some(u.name.clone()),
				}]);
				u.name.clone()
			})
			.unwrap_or_default();
		if !part.is_empty() && name.is_empty() && label_files.is_some() {
			note!("{id:03X}  {part}: not in the label files");
		}
		// The number in force — the label files' and the car's, then the
		// built-in fallback's — or nothing when nothing has paired one with it.
		table.row(vec![
			unit_number(vag_uds_client::address::short_number(id)),
			format!("{id:03X}"),
			part,
			component,
			name,
		]);
	}
	out!("{}", table.render(vag_cli_core::table::terminal_width()));
	out!(
		"\n{} asked · {answered} answered · {} on the gateway's list{}\n",
		asked.len(),
		ids.len(),
		match &label_files_dir {
			Some(_) => format!(" · {resolved} named"),
			None => String::new(),
		}
	);
	if let Some(vin) = &vin {
		note!("vin    {vin}");
	}
	Ok(())
}

/// The one recorded car whose record holds every unit the gateway listed, if there is exactly
/// one.
///
/// The plain listing reads no VIN, so it cannot ask which car this is. A record that covers
/// the list is this car's, or one built like it; with two or more there is no telling which,
/// so none is used.
pub fn record_of_these<'a>(listed: &[u16], cars: &'a [vag_cli_core::units::Recorded]) -> Option<&'a vag_cli_core::units::Recorded> {
	let mut covering = cars
		.iter()
		.filter(|car| listed.iter().all(|id| car.units.iter().any(|u| u.request == *id)));
	match (covering.next(), covering.next()) {
		(Some(one), None) => Some(one),
		_ => None,
	}
}

/// The plain `units` listing: a table of each request id, the short number it goes by, the
/// part number recorded for it and a name, and a count.
///
/// The name is, in order: what the label files call the part number the unit reported (with
/// the number they give it), what they call its short number, and what the unit called itself
/// (`F197`). The part number and that last name come from `record`, what an earlier
/// `--identify` wrote down. With none of them, just the id.
pub fn render_listing(
	ids: &[u16],
	record: Option<&[plan::UnitIdentity]>,
	by_part: impl Fn(&str) -> Option<(u8, String)>,
	by_number: impl Fn(u8) -> Option<String>,
	short: impl Fn(u16) -> Option<u8>,
) -> String {
	let mut table = vag_cli_core::table::Table::new(&["unit", "id", "part", "name"]);
	for &id in ids {
		let said = record.and_then(|units| units.iter().find(|u| u.request == id));
		let part = said.and_then(|u| u.part_number.clone());
		let from_part = part.as_deref().and_then(&by_part);
		let number = from_part.as_ref().map(|(n, _)| *n).or_else(|| short(id));
		let name = from_part
			.map(|(_, name)| name)
			.or_else(|| number.and_then(&by_number))
			.or_else(|| said.and_then(|u| u.component.clone()));
		table.row(vec![
			unit_number(number),
			format!("{id:03X}"),
			part.unwrap_or_default(),
			name.unwrap_or_default(),
		]);
	}
	let mut out = table.render(vag_cli_core::table::terminal_width());
	out.push_str(&format!("\n{} {}\n", ids.len(), render::plural(ids.len(), "unit")));
	out
}

/// Read one unit's whole identification block (`vagcan units --identify <unit>`).
///
/// The deeper of the two depths `units` has: the shallow one asks every unit
/// the four identifiers that name it, this asks one unit the whole
/// 256-identifier block and names what answers.
pub async fn identification<L: UnitLink>(open: impl AsyncFnOnce() -> Result<L>, unit_text: &str, while_driving: bool) -> Result<()> {
	let unit = parse_unit("--identify", unit_text)?;
	let ranges = scan::parse_ranges(props::IDENT_RANGE).expect("the built-in range parses");

	let mut backend = open().await?;
	// 256 reads aimed at one control unit is a sweep, whatever the block they
	// are in is called. This was the one sweep-shaped path in the tool with no
	// road-speed check on it — `vagcan units --identify`, which anybody could run at
	// speed while the whole-car sweep refused to. Guarded now like the rest.
	if !while_driving {
		backend = match safety::require_stationary(backend).await {
			Ok(backend) => backend,
			Err((_, why)) => anyhow::bail!(
				"{why}.\n\
                 Read it parked: 256 reads of one unit can stop it doing its job while the car moves \
                 (--while-driving lifts this)."
			),
		};
	}
	let mut uds = address_unit(backend, unit);

	let mut found = Vec::new();
	// The read is bounded and the block is standardised, but the rule "stop
	// when something changes" is not about how big the read was. No witness:
	// there is nothing established as known-good before this runs, so the guard
	// watches only for the unit going quiet after it had been answering.
	let mut monitor = anomaly::Monitor::new(unit.request);
	let mut guard = scan::Guard {
		witness: None,
		monitor: &mut monitor,
	};
	note!("reading {} of unit {}", props::IDENT_RANGE, unit.label());
	scan::scan_dids(&mut uds, &ranges, std::time::Duration::from_millis(2), 400, &mut guard, |hit| {
		found.push(props::Property {
			did: hit.did,
			data: hit.data.clone(),
		});
		Ok(())
	})
	.await?;
	if let Some(halt) = monitor.halted() {
		let mut progress = progress::Line::new();
		progress.notice(&halt.report());
		anyhow::bail!("the read was stopped: control unit {} changed while it was being read", halt.unit());
	}

	if found.is_empty() {
		render::say_nothing_answered();
		return Ok(());
	}

	// Mode 09 lives outside the identification block and carries what a part
	// number cannot: which emissions calibration this unit is actually
	// running.
	let mut info = Vec::new();
	for (pid, name) in vag_data_labels::obd::VEHICLE_INFO {
		let did = vag_data_labels::obd::did_for_info_pid(*pid);
		let Ok(data) = uds.read_data_by_identifier(did).await else {
			continue;
		};
		if let Some(items) = vag_data_labels::obd::decode_info_text(&data) {
			info.push((did, format!("OBD-II 09: {name}"), items.join(", ")));
		}
	}
	out!("{}", props::render(&unit.label(), &found, &info));
	note!("{} identifiers answered of {}", found.len(), props::IDENT_RANGE);
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn identity(request: u16, part: &str, component: &str) -> plan::UnitIdentity {
		plan::UnitIdentity {
			request,
			part_number: Some(part.to_string()),
			component: Some(component.to_string()),
			..plan::UnitIdentity::default()
		}
	}

	#[test]
	fn the_plain_listing_names_each_unit_from_what_is_on_this_machine() {
		// Invented parts and names; the rule is what is tested.
		let record = [identity(0x7E0, "PART-E", "ENGINE SELF"), identity(0x713, "PART-B", "BRAKES SELF")];
		let by_part = |part: &str| (part == "PART-B").then(|| (0x03, "Brakes by part".to_string()));
		let by_number = |n: u8| (n == 0x01).then(|| "Engine by number".to_string());
		let short = |request: u16| (request == 0x7E0).then_some(0x01);
		let listing = render_listing(&[0x7E0, 0x713, 0x70A], Some(&record), by_part, by_number, short);
		assert_eq!(
			listing,
			"unit  id   part    name\n\
			 01    7E0  PART-E  Engine by number\n\
			 03    713  PART-B  Brakes by part\n\
			 --    70A\n\
			 \n\
			 3 units\n"
		);
		// No label files: what the units called themselves.
		let listing = render_listing(&[0x7E0, 0x713], Some(&record), |_| None, |_| None, short);
		assert_eq!(
			listing,
			"unit  id   part    name\n\
			 01    7E0  PART-E  ENGINE SELF\n\
			 --    713  PART-B  BRAKES SELF\n\
			 \n\
			 2 units\n"
		);
		// Nothing on this machine: the ids, as before.
		let listing = render_listing(&[0x7E0, 0x713], None, |_| None, |_| None, |_| None);
		assert_eq!(listing, "unit  id   part  name\n--    7E0\n--    713\n\n2 units\n");
	}

	#[test]
	fn the_plain_listing_uses_a_record_only_when_one_car_covers_the_list() {
		let car = |vin: &str, requests: &[u16]| vag_cli_core::units::Recorded {
			vin: vin.to_string(),
			units: requests.iter().map(|r| identity(*r, "P", "C")).collect(),
		};
		let cars = [
			car("TESTVIN0000000001", &[0x7E0, 0x713]),
			car("TESTVIN0000000002", &[0x7E0, 0x713, 0x70A]),
		];
		assert_eq!(record_of_these(&[0x7E0, 0x70A], &cars).map(|c| c.vin.as_str()), Some("TESTVIN0000000002"));
		// Both cover it: no telling which this is.
		assert_eq!(record_of_these(&[0x7E0, 0x713], &cars), None);
		// A unit no record has: not a car recorded here.
		assert_eq!(record_of_these(&[0x7E0, 0x746], &cars), None);
	}
}
