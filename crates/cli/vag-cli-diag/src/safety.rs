//! Things not to do to a moving car.
//!
//! Reading identifiers is harmless. **Changing the diagnostic session is not.**
//! `0x10 0x03` puts a control unit into extended session, which on this
//! platform is workshop mode: units that assist the driver are entitled to
//! stop assisting while they are in it, and a driver-assistance unit that
//! stops assisting mid-drive is a hazard, not a diagnostic result.
//!
//! So the session change is not part of an ordinary read any more. It is
//! opt-in, and even then it is refused while the car is moving.

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

/// Refuse an extended diagnostic session unless the car is standing still.
///
/// Takes the link, asks the engine for road speed, and hands it back. A car
/// that will not say how fast it is going is treated as moving: the failure
/// this guards against is one the driver feels through the controls.
pub async fn require_stationary<B: UnitLink>(backend: B) -> Result<B, (B, String)> {
	let mut uds = AsyncUdsClient::new(backend.to_unit(CanId::Standard(ENGINE_REQUEST), CanId::Standard(ENGINE_RESPONSE)));
	let speed = road_speed_kmh(&mut uds).await;
	let backend = B::release(uds.into_transport());
	match speed {
		Some(0) => Ok(backend),
		Some(kmh) => Err((
			backend,
			format!(
				"the car is moving at {kmh} km/h. An extended diagnostic session can make a \
                 control unit stop assisting the driver. Stop the car first."
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
		assert!(why.contains("30 km/h"), "{why}");
		assert!(link.0.is_exhausted(), "the link comes back on refusal too");

		let silent = Scripted(MockAsyncTransport::new(vec![(req(SPEED_DID), vec![0x7F, 0x22, 0x31])]));
		let (_, why) = require_stationary(silent).await.err().expect("no speed is moving");
		assert!(why.contains("cannot tell"), "{why}");
	}
}
