//! Things not to do to a moving car.
//!
//! Reading identifiers is harmless. **Changing the diagnostic session is not.**
//! `0x10 0x03` puts a control unit into extended session, which on this
//! platform is workshop mode: units that assist the driver are entitled to
//! stop assisting while they are in it, and a driver-assistance unit that
//! stops assisting mid-drive is a hazard, not a diagnostic result.
//!
//! So the session change is not part of an ordinary read any more. It is
//! opt-in, and even then it is refused while the car is moving. So is a sweep
//! (`units --identify <unit>`, `sensors --unit <unit>` off 7E0–7E7, `adaptations`):
//! reading is not harmless to a diagnostic server asked what it may never have handled. The check
//! here says only whether the car stands; each caller says what it refuses.

use anyhow::Result;
use vag_uds_can::UnitLink;
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

/// Why the car does not count as standing still: the fact, and nothing about what was
/// refused because of it — each caller says that in its own words (an extended session,
/// a sweep).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotStationary {
	/// The engine reported this road speed, above zero.
	Moving(u8),
	/// The engine did not report road speed.
	Unknown,
}

impl std::fmt::Display for NotStationary {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			NotStationary::Moving(kmh) => write!(f, "the car is moving at {kmh} km/h"),
			NotStationary::Unknown => f.write_str("cannot tell whether the car is moving: the engine did not report road speed"),
		}
	}
}

/// Refuse what follows unless the car is standing still.
///
/// Takes the link, asks the engine for road speed, and hands it back. A car
/// that will not say how fast it is going is treated as moving: the failure
/// this guards against is one the driver feels through the controls.
pub async fn require_stationary<B: UnitLink>(backend: B) -> Result<B, (B, NotStationary)> {
	let mut uds = AsyncUdsClient::new(backend.to_unit(CanId::Standard(ENGINE_REQUEST), CanId::Standard(ENGINE_RESPONSE)));
	let speed = road_speed_kmh(&mut uds).await;
	let backend = B::release(uds.into_transport());
	match speed {
		Some(0) => Ok(backend),
		Some(kmh) => Err((backend, NotStationary::Moving(kmh))),
		None => Err((backend, NotStationary::Unknown)),
	}
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

	/// A link that is not a CAN backend, the way a BLE one will not be: the
	/// guard has to run over it, addressed to the engine.
	struct Scripted(MockAsyncTransport);

	impl UnitLink for Scripted {
		type Channel = MockAsyncTransport;

		fn to_unit(self, request: CanId, response: CanId) -> MockAsyncTransport {
			assert_eq!((request, response), (CanId::Standard(ENGINE_REQUEST), CanId::Standard(ENGINE_RESPONSE)));
			self.0
		}

		fn release(channel: MockAsyncTransport) -> Scripted {
			Scripted(channel)
		}
	}

	#[tokio::test]
	async fn the_guard_runs_over_any_link_and_hands_it_back() {
		let still = Scripted(MockAsyncTransport::new(vec![(req(SPEED_DID), resp(SPEED_DID, &[0]))]));
		let link = require_stationary(still).await.map_err(|(_, why)| why).expect("0 km/h is stationary");
		assert!(link.0.is_exhausted());

		let moving = Scripted(MockAsyncTransport::new(vec![(req(SPEED_DID), resp(SPEED_DID, &[30]))]));
		let (link, why) = require_stationary(moving).await.err().expect("30 km/h is moving");
		assert_eq!(why, NotStationary::Moving(30));
		assert!(link.0.is_exhausted(), "the link comes back on refusal too");

		let silent = Scripted(MockAsyncTransport::new(vec![(req(SPEED_DID), vec![0x7F, 0x22, 0x31])]));
		let (_, why) = require_stationary(silent).await.err().expect("no speed is moving");
		assert_eq!(why, NotStationary::Unknown);
	}

	/// The fact only: what was refused because of it is the caller's to say, so nothing
	/// here speaks of a diagnostic session to a command that never changes one.
	#[test]
	fn the_refusal_says_the_fact_and_not_its_consequence() {
		assert_eq!(NotStationary::Moving(30).to_string(), "the car is moving at 30 km/h");
		assert_eq!(
			NotStationary::Unknown.to_string(),
			"cannot tell whether the car is moving: the engine did not report road speed"
		);
		for why in [NotStationary::Moving(30), NotStationary::Unknown] {
			assert!(!why.to_string().contains("session"), "{why}");
		}
	}
}
