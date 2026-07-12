//! High-level read operations on the UDS client, as an extension trait.
//!
//! These are the "do a read (or a set of reads) and fold the answer into a
//! meaningful value" operations. They live as methods on [`AsyncUdsClient`] via
//! [`UdsReadExt`] rather than as free functions, so every high-level read is
//! discoverable off the client itself — `uds.read_identity()`,
//! `uds.read_measurement(def)`, `uds.read_catalog(cat)`.
//!
//! The trait lives here (in `vag-protocol`, which depends on `vag-data`) rather
//! than in `vag-data`, so the pure data model (`MeasurementDef` and its
//! `interpret`) stays free of any transport/I-O dependency: `vag-data` remains a
//! leaf crate, and the I/O layer reaches down to it.

use vag_data::catalog::{MeasurementCatalog, MeasurementDef, ReadId};
use vag_transport::AsyncIsoTpTransport;

use crate::identity::EcuIdentity;
use crate::uds::UdsError;
use crate::AsyncUdsClient;

/// One measurement read back from the ECU: the human name and unit (copied from
/// the [`MeasurementDef`]), the engineering `value`, and the raw response bytes.
///
/// `value` is `None` when the definition could not convert the raw bytes — a
/// short response, or a [`vag_data::catalog::Scaling::Anchor`] row observed away
/// from its single proven point (an unknown slope, so no honest value). The
/// `raw` bytes are always returned, so an uninterpretable reading is still
/// inspectable (and usable as fresh crib data).
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub name: String,
    pub unit: String,
    pub value: Option<f64>,
    pub raw: Vec<u8>,
}

/// High-level reads on the UDS client (see the module docs).
///
/// The methods are only ever called on the concrete `AsyncUdsClient<T>`, never
/// through a trait object, so the "async fn in public trait" lint (about
/// unspecifiable auto-trait bounds on the returned futures) does not apply.
#[allow(async_fn_in_trait)]
pub trait UdsReadExt {
    /// Read the ECU identification block (VIN + part/HW/SW/component/serial +
    /// coding). Per-DID tolerant: an unsupported identifier stays `None`.
    async fn read_identity(&mut self) -> EcuIdentity;

    /// Read one measurement per its [`MeasurementDef`]: issue the addressed read,
    /// then interpret the response into a [`Reading`]. Propagates a transport /
    /// negative-response failure as `Err`; a successful read whose bytes cannot
    /// be scaled yields `Ok` with `value: None`.
    async fn read_measurement(&mut self, def: &MeasurementDef) -> Result<Reading, UdsError>;

    /// Read every definition in a catalog, tolerantly: a measurement whose read
    /// fails is skipped, not fatal. Order follows the catalog.
    async fn read_catalog(&mut self, cat: &MeasurementCatalog) -> Vec<Reading>;
}

impl<T: AsyncIsoTpTransport> UdsReadExt for AsyncUdsClient<T> {
    async fn read_identity(&mut self) -> EcuIdentity {
        crate::identity::read_identity(self).await
    }

    async fn read_measurement(&mut self, def: &MeasurementDef) -> Result<Reading, UdsError> {
        let raw = match def.address {
            ReadId::Uds(did) => self.read_data_by_identifier(did).await?,
        };
        Ok(Reading {
            name: def.name.to_string(),
            unit: def.unit.to_string(),
            value: def.interpret(&raw),
            raw,
        })
    }

    async fn read_catalog(&mut self, cat: &MeasurementCatalog) -> Vec<Reading> {
        let mut out = Vec::with_capacity(cat.defs.len());
        for def in &cat.defs {
            if let Ok(reading) = self.read_measurement(def).await {
                out.push(reading);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;
    use vag_data::catalog::{ReadId, Scaling};
    use vag_data::measure::{LinearScale, RawForm};
    use vag_transport::MockAsyncTransport;

    /// RDBI request PDU `0x22 <hi> <lo>`.
    fn req(did: u16) -> Vec<u8> {
        vec![0x22, (did >> 8) as u8, (did & 0xFF) as u8]
    }
    /// Positive RDBI response PDU `0x62 <hi> <lo> <data…>`.
    fn resp(did: u16, data: &[u8]) -> Vec<u8> {
        let mut v = vec![0x62, (did >> 8) as u8, (did & 0xFF) as u8];
        v.extend_from_slice(data);
        v
    }

    fn linear(name: &'static str, did: u16, form: RawForm, factor: f64, offset: f64) -> MeasurementDef {
        MeasurementDef {
            name: Cow::Borrowed(name),
            unit: Cow::Borrowed("/min"),
            address: ReadId::Uds(did),
            raw_form: form,
            scaling: Scaling::Linear(LinearScale { factor, offset }),
        }
    }

    #[tokio::test]
    async fn read_measurement_reads_and_scales() {
        // RPM def: DID 0xF40C, u16be * 0.25. Raw 0x0B34 = 2868 → 717 rpm.
        let def = linear("Engine RPM", 0xF40C, RawForm::U16Be, 0.25, 0.0);
        let script = vec![(req(0xF40C), resp(0xF40C, &[0x0B, 0x34]))];
        let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(script));

        let r = uds.read_measurement(&def).await.expect("reads");
        assert_eq!(r.name, "Engine RPM");
        assert_eq!(r.unit, "/min");
        assert_eq!(r.value, Some(717.0));
        assert_eq!(r.raw, vec![0x0B, 0x34]);
    }

    #[tokio::test]
    async fn anchor_off_point_yields_value_none_but_keeps_raw() {
        // An ignition anchor read AWAY from its proven 0x5555 point: value is
        // None (unknown slope) but the raw bytes still come back as crib data.
        let def = MeasurementDef {
            name: Cow::Borrowed("Ignition angle"),
            unit: Cow::Borrowed("°"),
            address: ReadId::Uds(0xA058),
            raw_form: RawForm::U16Be,
            scaling: Scaling::Anchor { raw: 0x5555, value: 0.0 },
        };
        let script = vec![(req(0xA058), resp(0xA058, &[0x57, 0xE9]))];
        let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(script));

        let r = uds.read_measurement(&def).await.expect("reads");
        assert_eq!(r.value, None, "off-anchor: no invented value");
        assert_eq!(r.raw, vec![0x57, 0xE9], "raw preserved for crib use");
    }

    #[tokio::test]
    async fn read_catalog_is_tolerant_of_a_failing_read() {
        // Two defs; the first DID answers, the second gives a negative response.
        // read_catalog returns the one good reading, skipping the failure.
        let ok = linear("A", 0x1000, RawForm::U8First, 1.0, 0.0);
        let bad = linear("B", 0x2000, RawForm::U8First, 1.0, 0.0);
        let cat = MeasurementCatalog::new(vec![ok, bad]);
        let script = vec![
            (req(0x1000), resp(0x1000, &[0x2A])),        // 42
            (req(0x2000), vec![0x7F, 0x22, 0x31]),       // requestOutOfRange
        ];
        let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(script));

        let readings = uds.read_catalog(&cat).await;
        assert_eq!(readings.len(), 1, "failing read skipped");
        assert_eq!(readings[0].name, "A");
        assert_eq!(readings[0].value, Some(42.0));
    }
}
