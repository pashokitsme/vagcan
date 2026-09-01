//! Things not to do to a moving car.
//!
//! Reading identifiers is harmless. **Changing the diagnostic session is not.**
//! `0x10 0x03` puts a control unit into extended session, which on this
//! platform is workshop mode: units that assist the driver are entitled to
//! stop assisting while they are in it, and the steering assist on the
//! reference car did exactly that — it dropped out mid-drive, about a third of
//! the way through a survey, which is where `0x712` falls in the walk order.
//!
//! So the session change is not part of an ordinary read any more. It is
//! opt-in, and even then it is refused while the car is moving.

use anyhow::Result;
use vag_uds_can::IsoTpCan;
use vag_uds_client::AsyncUdsClient;
use vag_uds_transport::{AsyncIsoTpTransport, CanId};

/// The engine's request id, where the legislated OBD-II parameters live.
const ENGINE_REQUEST: u16 = 0x7E0;
const ENGINE_RESPONSE: u16 = 0x7E8;

/// Road speed, from the standard parameter set mirrored at `0xF400 + PID`.
/// One byte of km/h, defined by SAE J1979 rather than by this car.
const SPEED_DID: u16 = 0xF40D;

/// Is the car standing still?
///
/// `None` when the engine did not answer — which is not "stationary", and
/// callers must not treat it as such.
pub async fn road_speed_kmh<T: AsyncIsoTpTransport>(uds: &mut AsyncUdsClient<T>) -> Option<u8> {
	uds.read_data_by_identifier(SPEED_DID).await.ok()?.first().copied()
}

/// Refuse an extended diagnostic session unless the car is standing still.
///
/// Takes the backend, asks the engine for road speed, and hands it back. A car
/// that will not say how fast it is going is treated as moving: the failure
/// this guards against is one the driver feels through the steering wheel.
pub async fn require_stationary<B: vag_uds_can::CanBackend>(backend: B) -> Result<B, (B, String)> {
	let mut uds = AsyncUdsClient::new(IsoTpCan::new(backend, CanId::Standard(ENGINE_REQUEST), CanId::Standard(ENGINE_RESPONSE)));
	let speed = road_speed_kmh(&mut uds).await;
	let backend = uds.into_transport().into_backend();
	match speed {
		Some(0) => Ok(backend),
		Some(kmh) => Err((
			backend,
			format!(
				"the car is moving at {kmh} km/h. An extended diagnostic session can make a \
                 control unit stop assisting the driver — the steering assist on the \
                 reference car dropped out mid-drive that way. Stop the car first."
			),
		)),
		None => Err((
			backend,
			"cannot tell whether the car is moving — the engine did not report road speed. \
             Refusing to change the diagnostic session on that basis."
				.to_string(),
		)),
	}
}

/// Control units a whole-car sweep leaves alone, and why.
///
/// **This is a safety rule, not a table of car data.** `0x712` is VW's
/// diagnostic address for steering assistance across the platform, not a fact
/// about one vehicle, and what it is doing here is recorded in `SAFETY.md`: the
/// steering assist on the reference car dropped out mid-drive about a third of
/// the way through a survey — which is exactly where `0x712` falls in the walk
/// order — and a second run cost it permanently. It has been reading its own
/// fault memory ever since, and every sweep since has made it complain again.
///
/// Skipping it costs a whole-car survey one unit of fifteen. Not skipping it
/// costs the owner a steering rack, twice already.
///
/// **Only the whole-car walk.** `--only 712` still reaches it, because naming a
/// unit by hand is a person deciding to read that unit, the same way `--blind`
/// is. What this removes is the sweep nobody aimed.
pub const SPARED: [u16; 1] = [0x712];

/// The walk order minus the units a sweep spares, and what was left out.
///
/// Returns the pair rather than filtering silently: a sweep that quietly reads
/// fourteen units while saying fifteen is the kind of gap that gets discovered
/// as a missing unit in `watch` months later.
pub fn spare(order: Vec<u16>) -> (Vec<u16>, Vec<u16>) {
	let (spared, walk): (Vec<u16>, Vec<u16>) = order.into_iter().partition(|id| SPARED.contains(id));
	(walk, spared)
}

/// What to say about the units a sweep spared, or nothing when it spared none.
pub fn spared_notice(spared: &[u16]) -> Option<String> {
	if spared.is_empty() {
		return None;
	}
	let list: Vec<String> = spared.iter().map(|id| format!("{id:03X}")).collect();
	Some(format!(
		"  {} not swept — it is the unit this project has twice damaged, and it \n  \
         reports a fault every time it is read (SAFETY.md). Name it to read it: \n    \
         vagcan survey --only {}",
		list.join(", "),
		list.join(",")
	))
}

#[cfg(test)]
mod tests {
	use super::*;
	use vag_uds_transport::MockAsyncTransport;

	fn req(did: u16) -> Vec<u8> {
		vec![0x22, (did >> 8) as u8, (did & 0xFF) as u8]
	}
	fn resp(did: u16, data: &[u8]) -> Vec<u8> {
		let mut v = vec![0x62, (did >> 8) as u8, (did & 0xFF) as u8];
		v.extend_from_slice(data);
		v
	}

	#[tokio::test]
	async fn road_speed_comes_from_the_standard_parameter() {
		let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(vec![(req(SPEED_DID), resp(SPEED_DID, &[57]))]));
		assert_eq!(road_speed_kmh(&mut uds).await, Some(57));
	}

	#[tokio::test]
	async fn a_unit_that_does_not_answer_is_not_reported_as_stationary() {
		// The distinction this module exists for: "no answer" and "0 km/h"
		// must never collapse into the same value.
		let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(vec![(req(SPEED_DID), vec![0x7F, 0x22, 0x31])]));
		assert_eq!(road_speed_kmh(&mut uds).await, None);
	}

	#[test]
	fn a_whole_car_sweep_spares_the_unit_this_project_has_damaged() {
		// `SAFETY.md`, twice over. The steering assist dropped out a third of
		// the way through a survey — which is where `0x712` falls in the walk
		// order — and a second run cost it permanently. It has reported a fault
		// on every read since.
		let (walk, spared) = spare(vec![0x7E0, 0x7E1, 0x710, 0x712, 0x713]);
		assert_eq!(walk, vec![0x7E0, 0x7E1, 0x710, 0x713]);
		assert_eq!(spared, vec![0x712]);
		// The order of what is left is the order it was given in: a sweep that
		// reshuffled itself would make two runs hard to compare, and comparing
		// two runs is what a survey is for.
		assert_eq!(spare(vec![0x713, 0x7E0]).0, vec![0x713, 0x7E0]);
	}

	#[test]
	fn what_was_spared_is_said_out_loud_and_says_how_to_read_it_anyway() {
		// A sweep that quietly reads fourteen units while saying fifteen is a
		// gap somebody finds months later as a control unit missing from
		// `watch`. And the unit is not forbidden — it is not *swept*, which is
		// a different sentence, so the notice carries the command that reads it.
		let notice = spared_notice(&[0x712]).expect("something was spared");
		assert!(notice.contains("712"), "{notice}");
		assert!(notice.contains("SAFETY.md"), "{notice}");
		assert!(notice.contains("--only 712"), "{notice}");
		// And nothing is said when nothing was held back.
		assert_eq!(spared_notice(&[]), None);
	}
}
