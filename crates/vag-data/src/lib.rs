//! Static VAG diagnostic data: parsers and tables that turn raw ECU bytes and
//! Ross-Tech label files into human-meaningful names, units, and ranges.
//!
//! Handles both the plaintext `.lbl` label format ([`label`]) and the
//! encrypted compiled `.clb` format ([`clb`]), which decrypts to the same
//! textual format `label::parse_label` understands.

pub mod clb;
pub mod corpus;
pub mod db;
pub mod label;
pub mod measure;
pub mod mwb;
pub mod rod;
pub mod struc;
mod tea;

pub use clb::decrypt_clb;
pub use corpus::{load_corpus, scan_corpus, CorpusLoad, CorpusScan};
pub use db::LabelDb;
pub use label::{parse_label, LabelFile, Measurement, Record};
pub use measure::{LinearScale, RawForm, IGNITION_ANGLE_ZERO_DIDS, IGNITION_ANGLE_ZERO_RAW};
pub use mwb::{parse_mwb, MwbEntry};
pub use rod::{decode_rod, RodSection, RodStatus};
pub use struc::{StrucRecord, StrucTable};
