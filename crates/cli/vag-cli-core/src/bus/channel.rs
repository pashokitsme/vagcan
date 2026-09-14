//! A [`Bus`] is a [`UnitLink`]: every command written against the link runs through
//! the scheduler without knowing it.

use std::time::Duration;

use vag_uds_can::UnitLink;
use vag_uds_transport::{AsyncIsoTpTransport, CanId, TransportError};

use super::{Bus, Class, ExchangeError, Unit};

/// The bus while a command has it addressed to one unit.
///
/// `send` only holds the request; `recv` queues it as a [`Class::Foreground`] raw
/// exchange and waits for the answer, so the UDS client above sees exactly what a CAN
/// link gave it: the answer PDU (a negative one included, for the client to decode),
/// or a [`TransportError`].
pub struct BusChannel {
	bus: Bus,
	/// `None` for an extended id, which the planner has no unit for.
	unit: Option<Unit>,
	request: Option<Vec<u8>>,
}

impl UnitLink for Bus {
	type Channel = BusChannel;

	fn to_unit(self, request: CanId, response: CanId) -> BusChannel {
		let unit = match (request, response) {
			(CanId::Standard(request), CanId::Standard(response)) => Some(Unit { request, response }),
			_ => None,
		};
		BusChannel {
			bus: self,
			unit,
			request: None,
		}
	}

	fn release(channel: BusChannel) -> Bus {
		channel.bus
	}
}

impl AsyncIsoTpTransport for BusChannel {
	async fn send(&mut self, pdu: &[u8]) -> Result<(), TransportError> {
		if self.unit.is_none() {
			return Err(TransportError::Unsupported("extended CAN ids on the bus scheduler"));
		}
		self.request = Some(pdu.to_vec());
		Ok(())
	}

	async fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
		// Nothing asked, nothing coming: the bus has already waited out any `78`.
		let (Some(unit), Some(pdu)) = (self.unit, self.request.take()) else {
			return Err(TransportError::Timeout);
		};
		match self.bus.exchange_within(Class::Foreground, unit, pdu, timeout).await {
			Ok((answer, _)) => Ok(answer),
			Err(ExchangeError::NoAnswer) => Err(TransportError::Timeout),
			Err(ExchangeError::Link(why)) => Err(why),
			Err(ExchangeError::Closed) => Err(TransportError::Disconnected),
			Err(ExchangeError::Forbidden(why)) => Err(TransportError::Protocol(why.to_string())),
			// The board's own words, so the reason reaches whoever reads the error.
			Err(refused @ ExchangeError::Refused(_)) => Err(TransportError::Protocol(refused.to_string())),
		}
	}
}
