//! Static VAG diagnostic data: parsers and tables that turn raw ECU bytes and
//! Ross-Tech label files into human-meaningful names, units, and ranges.
//!
//! Handles both the plaintext `.lbl` label format ([`label`]) and the
//! encrypted compiled `.clb` format ([`clb`]), which decrypts to the same
//! textual format `label::parse_label` understands.

pub mod catalog;
pub mod clb;
pub mod codes;
pub mod corpus;
pub mod db;
pub mod glyphs;
pub mod label;
pub mod measure;
pub mod obd;
pub mod mwb;
pub mod rod;
pub mod tttext;
mod tea;

pub use catalog::{ignition_angle, MeasurementCatalog, MeasurementDef, ReadId, Scaling};
pub use clb::decrypt_clb;
pub use codes::{CodesDb, ISO_BAND_START};
pub use corpus::{
    find_rod_by_odx_name, find_rod_by_odx_variant, load_corpus, scan_corpus, CorpusLoad,
    CorpusScan, OdxMatch,
};
pub use db::LabelDb;
pub use label::{parse_label, LabelFile, Measurement, Record};
pub use measure::{LinearScale, RawForm, IGNITION_ANGLE_ZERO_DIDS, IGNITION_ANGLE_ZERO_RAW};
pub use mwb::{parse_mwb, MwbEntry};
pub use rod::{decode_rod, RodSection, RodStatus};
