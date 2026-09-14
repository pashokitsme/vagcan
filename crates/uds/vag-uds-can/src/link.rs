//! [`UnitLink`]: the seam a command talks to the car through.
//!
//! A command never needs frames. It addresses one control unit, exchanges
//! whole UDS PDUs with it, and lets go before addressing the next. Over CAN
//! that is an [`IsoTpCan`] wrapped around the backend and unwrapped again, and
//! the blanket impl below is exactly that. A link that carries whole PDUs on
//! its own — BLE to the dash board — implements this trait directly, and every
//! command generic over it runs over both.
//!
//! The value is owned, not shared: `to_unit` consumes the link and `release`
//! hands it back, so two exchanges in flight cannot be written down.

use vag_uds_transport::{AsyncIsoTpTransport, CanId, MaybeSend};

use crate::{CanBackend, IsoTpCan};

/// One conversation with one control unit at a time: address a unit, exchange PDUs, let go.
pub trait UnitLink: MaybeSend + Sized {
	/// The link while it is addressed to one unit.
	type Channel: AsyncIsoTpTransport;
	/// Address the unit that listens on `request` and answers on `response`.
	fn to_unit(self, request: CanId, response: CanId) -> Self::Channel;
	/// Let go of the unit, getting the link back for the next one.
	fn release(channel: Self::Channel) -> Self;
	/// Throw away whatever the link already holds from earlier exchanges, before the next
	/// request goes out; how many it held. A CAN backend can hold a late answer
	/// ([`CanBackend::discard_queued`]). A link that matches each answer to its request by
	/// a sequence number cannot, and keeps this default.
	#[allow(async_fn_in_trait)] // static-dispatch seam, as `CanBackend`
	async fn discard_stale(&mut self) -> usize {
		0
	}
}

/// A CAN backend is a link: ISO-TP on the two ids, per unit.
impl<B: CanBackend> UnitLink for B {
	type Channel = IsoTpCan<B>;

	async fn discard_stale(&mut self) -> usize {
		self.discard_queued().await
	}

	fn to_unit(self, request: CanId, response: CanId) -> IsoTpCan<B> {
		IsoTpCan::new(self, request, response)
	}

	fn release(channel: IsoTpCan<B>) -> B {
		channel.into_backend()
	}
}
