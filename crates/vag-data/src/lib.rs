//! Static VAG diagnostic data: parsers and tables that turn raw ECU bytes and
//! Ross-Tech label files into human-meaningful names, units, and ranges.
//!
//! Handles both the plaintext `.lbl` label format ([`label`]) and the
//! encrypted compiled `.clb` format ([`clb`]), which decrypts to the same
//! textual format `label::parse_label` understands.

pub mod clb;
pub mod label;

pub use clb::decrypt_clb;
pub use label::{parse_label, LabelFile, Measurement, Record};
