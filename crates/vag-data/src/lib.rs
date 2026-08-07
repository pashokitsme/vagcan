//! Static VAG diagnostic data: parsers and tables that turn raw ECU bytes and
//! Ross-Tech label files into human-meaningful names, units, and ranges.
//!
//! Handles both the plaintext `.lbl` label format ([`label`]) and the
//! encrypted compiled `.clb` format ([`clb`]), which decrypts to the same
//! textual format `label::parse_label` understands.

pub mod catalog;
pub mod clb;
pub mod codes;
pub mod db;
pub mod dtc;
pub mod glyphs;
pub mod label;
pub mod label_files;
pub mod measure;
pub mod mwb;
pub mod obd;
pub mod odis;
pub mod rod;
mod tea;
pub mod tttext;

pub use catalog::{MeasurementCatalog, MeasurementDef, ReadId, Scaling, ignition_angle};
pub use clb::decrypt_clb;
pub use codes::{CodesDb, ISO_BAND_START};
pub use db::LabelDb;
pub use dtc::{DtcRegistry, DtcRow, FaultName, UnitCatalogue, UnitLookup};
pub use label::{LabelFile, Measurement, Record, parse_label};
pub use label_files::{LabelFileLoad, LabelScan, OdxMatch, find_rod_by_odx_name, find_rod_by_odx_variant, load_label_files, scan_label_files};
pub use measure::{IGNITION_ANGLE_ZERO_DIDS, IGNITION_ANGLE_ZERO_RAW, LinearScale, RawForm};
pub use mwb::{MwbEntry, parse_mwb};
pub use rod::{RodSection, RodStatus, decode_rod};
