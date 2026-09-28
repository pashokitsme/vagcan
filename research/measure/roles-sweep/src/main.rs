//! Resolve `measure`'s roles over every engine and gearbox variant of an ODIS project.
//!
//! FOR: seeing what a change to `vag_cli_measure::channels` does to cars other than the
//! reference one, before it lands. On 2026-09-28 it showed the word pass reading an air-flow
//! ceiling constant as air mass on 13 engine variants, one-row stub engine variants losing their
//! speed, engine speed and pedal, and that no configuration which resolved came to refuse
//! (PR #17). Written by a review subagent; kept per the cleanup skill's 2026-09-22 and
//! 2026-09-27 rules.
//!
//! IN: the project's cache and two lists of variant names, one per line (see README.md for the
//! queries that write them):
//!     cargo run --release -- ~/.vagcan/data/<project>/cache.sqlite engines.txt gearboxes.txt
//!
//! OUT: stdout, one line per configuration — each engine with each gearbox and with none, with
//! and without the reference car's other thirteen units — the roles and the identifiers they
//! resolved to, or `MISSING [...]` with the required roles that found nothing. Run it on two
//! checkouts and `diff` the outputs.

use std::collections::BTreeMap;
use std::path::PathBuf;

use vag_cli_measure::channels;
use vag_cli_measure::extracted::Extracted;
use vag_cli_measure::plan::UnitIdentity;
use vag_data_labels::catalog::CatalogStore;

fn id(request: u16, odx: &str, version: &str) -> UnitIdentity {
	UnitIdentity {
		request,
		part_number: None,
		odx_name: Some(odx.into()),
		odx_version: Some(version.into()),
		component: None,
	}
}

/// A variant name's ODX file and a coding index whose first three digits pick it.
fn split(variant: &str) -> (String, String) {
	let (odx, version) = variant.rsplit_once('_').expect("a variant name ends in _<version>");
	(odx.to_string(), format!("{version}001"))
}

fn main() {
	let args: Vec<String> = std::env::args().collect();
	let [_, cache, engines, gearboxes] = args.as_slice() else {
		eprintln!("usage: roles-sweep <cache.sqlite> <engines.txt> <gearboxes.txt>");
		std::process::exit(2);
	};
	let lines = |path: &str| -> Vec<String> {
		std::fs::read_to_string(path)
			.unwrap_or_else(|e| panic!("{path}: {e}"))
			.lines()
			.filter(|l| !l.trim().is_empty())
			.map(str::to_string)
			.collect()
	};
	let (engines, gearboxes) = (lines(engines), lines(gearboxes));
	// No proven rows: what the project alone gives. An empty directory is an empty store.
	let empty = std::env::temp_dir().join("roles-sweep-empty-store");
	std::fs::create_dir_all(&empty).expect("a temporary directory");
	let store = CatalogStore::open(empty);
	let odis = Extracted::synthetic(PathBuf::from(cache), BTreeMap::new());
	// The reference car's other units, as its parked survey identified them — research data
	// about one car, here so each configuration also meets the copies those units hold.
	let others: Vec<UnitIdentity> = [
		(0x710, "EV_GatewNF", "013020"),
		(0x70A, "EV_EPHVA14AU3700000", "009029"),
		(0x70C, "EV_SMLSVALEOMQBLRH", "001007"),
		(0x70E, "EV_BCMMQB", "017001"),
		(0x712, "EV_SteerAssisMQB", "013144"),
		(0x713, "EV_Brake1UDSContiMK100ESP", "036010"),
		(0x714, "EV_DashBoardVDDMQBAB", "009051"),
		(0x715, "EV_AirbaVW21TS6VW48X", "001014"),
		(0x746, "EV_ACClimaBHBVW37X", "006145"),
		(0x74A, "EV_DCUDriveSideEWMAXCONT", "006001"),
		(0x74B, "EV_DCUPasseSideEWMAXCONT", "006001"),
		(0x767, "EV_OCULowMQBLGE", "003042"),
		(0x773, "EV_MUEnt4CGen2LGE", "001039"),
	]
	.iter()
	.map(|(request, odx, version)| id(*request, odx, version))
	.collect();
	let mut boxes: Vec<Option<&String>> = gearboxes.iter().map(Some).collect();
	boxes.push(None);
	for engine in &engines {
		let (odx, version) = split(engine);
		for gearbox in &boxes {
			for with_others in [true, false] {
				let mut units = vec![id(0x7E0, &odx, &version)];
				if let Some(g) = gearbox {
					let (godx, gversion) = split(g);
					units.push(id(0x7E1, &godx, &gversion));
				}
				if with_others {
					units.extend(others.iter().cloned());
				}
				let body = match channels::resolve(&store, &odis, &units, true) {
					Ok(set) => set
						.all()
						.map(|c| format!("{}={}", c.key.replace(' ', "_"), c.source()))
						.collect::<Vec<_>>()
						.join(" "),
					Err(missing) => format!("MISSING {:?}", missing.iter().map(|m| m.key).collect::<Vec<_>>()),
				};
				println!("E {engine} gb={}/others={with_others} {body}", gearbox.map_or("none", |g| g.as_str()));
			}
		}
	}
}
