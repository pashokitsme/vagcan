//! UDS measurement scaling: turn a `ReadDataByIdentifier` response's raw data
//! bytes into an engineering value + unit.
//!
//! ## Provenance — an engine-running capture, empirically
//! The scaling constants here are derived **empirically**, by pairing decoded
//! raw UDS with VCDS's own displayed values, from ENGINE-RUNNING captures of the
//! owner's Škoda Octavia 1.8 TSI (`research/dumps/`, all gitignored — USB traces
//! `capture-w-logs.pcapng` and `coolant-rpm-speed.pcapng` plus their VCDS ADVMB
//! logs `logs-engine.CSV` / `logs-dsg.CSV` / `coolant-rpm-speed.CSV`). The link
//! cipher is decoded per channel; each measurement DID's raw time-series is
//! aligned to a logged measurement by curve shape (cross-correlation) and fitted
//! by least squares. Tooling: `research/clb-crack/measure_{series,ttp,final}.py`
//! (first capture) and `measure_{coolant,fit,overlay,channels,probe}.py` (the
//! second, wide-rev capture).
//!
//! ## What is PROVEN (and shipped)
//! The **ignition-angle zero point**: DIDs [`IGNITION_ANGLE_ZERO_DIDS`] each
//! return raw `0x5555` (big-endian `u16`) for a displayed value of **0.00°**.
//! This is cross-validated four independent ways — the four DIDs read a constant
//! `0x5555` for the entire capture while the four constant ignition-angle
//! channels VCDS logged (`IDE00155/156/157/158`) read a constant `0.00°` over the
//! same window. It fixes the COMPU **zero point** of the ignition-angle method.
//!
//! ## What is NOT proven (deliberately not shipped — no forced fits)
//! - **The ignition-angle SLOPE.** The four proven DIDs are constant at `0x5555`
//!   for the whole session, so they pin the offset but carry no gradient. The one
//!   varying ignition-angle DID (`0xA051` ↔ `IDE00149`) shape-matches only loosely
//!   (best `|r| ≈ 0.86`, non-monotonic raw→° relation, `R² ≈ 0.73`) — not a clean
//!   linear fit, so no `(factor, offset)` is asserted for it.
//! - **RPM and vehicle speed.** No decodable DID in either engine-running capture
//!   tracks either with a proof-grade fit. This was re-tested with the exact
//!   capture the first pass prescribed — a **single ECU (Engine 01, `8V0 906 264 H`)
//!   polled through a wide, sustained rev (`IDE00405` = 784 → 3807 /min) with a
//!   tight ~1.4 s ADVMB log** (`research/dumps/coolant-rpm-speed.{pcapng,CSV}`,
//!   gitignored). The wide rev is present in the log, yet **no polled DID carries
//!   it**: at the single true capture→log lag (≈ 52 s, pinned by the drive-away
//!   window), RPM correlates with *nothing* (`|r| < 0.5` for every DID×form). The
//!   only decodable RDBI DIDs on the two TP-crib channels are
//!   `{7410,7419,7444,7450,7458,82D4,A03B,A0EF}` — the 2-byte ones (`A03B`≈`0x56xx`,
//!   `A0EF`≈`0x55xx`, `7458` idles at `0x55`) sit in the ignition-angle 0x5555 band
//!   and are near-constant or bidirectional, i.e. engine-internal angle/throttle
//!   signals, **not** the logged RPM/speed/coolant. High per-pair `|r|` shows up
//!   only at *inconsistent, per-measurement* lags (RPM's best fits scatter across
//!   lags 34–90 s) — the signature of spurious window-matching, not tracking. So
//!   the ADVMB display values are computed from raw the decodable channels do not
//!   expose; a further capture cannot settle this by rev range alone. See
//!   `research/rod-labels.md §4` for the full negative.
//! - **Coolant temp.** Same capture: `IDE00025` rises 99 → 104 °C (slow, monotonic);
//!   the only slowly-drifting DID (`7450`) *falls* `0xDE → 0xC5` and anti-correlates
//!   (`r ≈ −0.66`), and the standard `raw·0.75 − 48` maps it to 118 → 99 °C (wrong
//!   direction and magnitude) — so `7450` is a different, cooling temperature, not
//!   the logged coolant. No clean fit.
//!
//! [`LinearScale`] + [`RawForm`] are the reusable runtime machinery (mirroring
//! the `MeasurementDef`/`Compu::Linear` model sketched in `research/rod-labels.md
//! §5`); car-specific `(factor, offset)` rows drop in here as they are proven.

/// How to read an integer out of an RDBI response's data bytes (the bytes after
/// the `62 <DID hi> <DID lo>` echo). VAG measurements are 1- or 2-byte; both byte
/// orders occur, so the interpretation is part of each measurement's definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RawForm {
    /// First data byte as unsigned 8-bit.
    U8First,
    /// Second data byte as unsigned 8-bit.
    U8Second,
    /// Two data bytes, unsigned 16-bit big-endian (`data[0] << 8 | data[1]`).
    U16Be,
    /// Two data bytes, unsigned 16-bit little-endian (`data[1] << 8 | data[0]`).
    U16Le,
    /// Two data bytes, signed 16-bit big-endian.
    I16Be,
}

impl RawForm {
    /// Extract the raw integer from `data` (the response bytes after the DID
    /// echo). Returns `None` if `data` is too short for this form.
    pub fn read(self, data: &[u8]) -> Option<i32> {
        match self {
            RawForm::U8First => data.first().map(|&b| b as i32),
            RawForm::U8Second => data.get(1).map(|&b| b as i32),
            RawForm::U16Be => match data {
                [hi, lo, ..] => Some(((*hi as i32) << 8) | *lo as i32),
                _ => None,
            },
            RawForm::U16Le => match data {
                [lo, hi, ..] => Some(((*hi as i32) << 8) | *lo as i32),
                _ => None,
            },
            RawForm::I16Be => match data {
                [hi, lo, ..] => Some((((*hi as u16) << 8 | *lo as u16) as i16) as i32),
                _ => None,
            },
        }
    }
}

/// A linear COMPU-METHOD: `engineering = raw * factor + offset`. This is the VAG
/// default (RPM ≈ raw·0.25, speed ≈ raw·0.01, temp ≈ raw·0.75 − 48, …). Non-linear
/// / table methods are not modelled yet (none is proven for this car).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LinearScale {
    /// Multiplier applied to the raw integer.
    pub factor: f64,
    /// Additive offset.
    pub offset: f64,
}

impl LinearScale {
    /// Apply the scaling to a raw integer.
    pub fn apply(self, raw: i32) -> f64 {
        raw as f64 * self.factor + self.offset
    }

    /// Read `data` per `form` and apply the scaling. `None` if `data` is too
    /// short for `form`.
    pub fn apply_bytes(self, form: RawForm, data: &[u8]) -> Option<f64> {
        form.read(data).map(|r| self.apply(r))
    }
}

/// Unit string of the ignition-angle measurements (degrees crank).
pub const IGNITION_ANGLE_UNIT: &str = "°";

/// The raw value (read as [`RawForm::U16Be`]) that the ignition-angle DIDs return
/// for a displayed **0.00°**. Cross-validated against VCDS's ADVMB log.
pub const IGNITION_ANGLE_ZERO_RAW: u16 = 0x5555;

/// Engine-ECU RDBI DIDs proven to belong to the ignition-angle family (unit
/// `°`), each observed returning [`IGNITION_ANGLE_ZERO_RAW`] = **0.00°** for the
/// whole engine-running capture. They match the four constant ignition-angle
/// channels VCDS logged (`IDE00155/156/157/158`); the exact one-to-one DID↔IDE
/// pairing is not individually determined (all four are constant `0.00°`), so
/// only set membership + the zero point are asserted.
pub const IGNITION_ANGLE_ZERO_DIDS: &[u16] = &[0xA058, 0xA059, 0xA05E, 0xA05F];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_form_reads_each_interpretation() {
        let d = [0x57u8, 0xE9];
        assert_eq!(RawForm::U8First.read(&d), Some(0x57));
        assert_eq!(RawForm::U8Second.read(&d), Some(0xE9));
        assert_eq!(RawForm::U16Be.read(&d), Some(0x57E9));
        assert_eq!(RawForm::U16Le.read(&d), Some(0xE957));
        assert_eq!(RawForm::I16Be.read(&[0xFF, 0xFE]), Some(-2));
        assert_eq!(RawForm::U16Be.read(&[0x01]), None);
        assert_eq!(RawForm::U8Second.read(&[0x01]), None);
    }

    #[test]
    fn linear_scale_arithmetic() {
        // The machinery itself: a textbook VAG coolant-temp style scale.
        let temp = LinearScale { factor: 0.75, offset: -48.0 };
        assert!((temp.apply(0x80) - 48.0).abs() < 1e-9); // 128*0.75-48 = 48.0
        // And through raw bytes (single-byte form).
        assert!((temp.apply_bytes(RawForm::U8First, &[0x80]).unwrap() - 48.0).abs() < 1e-9);
    }

    #[test]
    fn ignition_zero_point_matches_capture_and_log() {
        // Captured raw data bytes (after the `62 A0 xx` echo) for every ignition-
        // angle-family DID, for the whole engine-running session, are these two
        // literal bytes; VCDS displayed 0.00° for the matching logged channels.
        // (Bytes/values are the crib, not the gitignored capture itself.)
        let captured_raw: [u8; 2] = [0x55, 0x55];
        let vcds_displayed_deg = 0.00_f64;

        let raw = RawForm::U16Be.read(&captured_raw).unwrap();
        assert_eq!(raw as u16, IGNITION_ANGLE_ZERO_RAW);

        // The proven COMPU zero point: this raw maps to 0.00° for each DID. The
        // slope is unproven, but ANY linear scale through this zero point (i.e.
        // offset = -factor*0x5555) reproduces the displayed value here.
        for &did in IGNITION_ANGLE_ZERO_DIDS {
            assert!((0xA058..=0xA05F).contains(&did));
            let factor = 0.01; // arbitrary; the zero point is what is asserted
            let scale = LinearScale { factor, offset: -factor * IGNITION_ANGLE_ZERO_RAW as f64 };
            let got = scale.apply(raw);
            assert!(
                (got - vcds_displayed_deg).abs() < 1e-6,
                "DID {did:#06X}: raw {raw:#06X} -> {got}, expected {vcds_displayed_deg}"
            );
        }
    }
}
