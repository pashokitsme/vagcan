//! Static VAG diagnostic data: parsers and tables that turn raw ECU bytes and
//! Ross-Tech label files into human-meaningful names, units, and ranges.
//!
//! P2 scope: the plaintext `.lbl` label parser ([`label`]). The compiled `.clb`
//! format is not yet decoded (fixed-keystream XOR — a separate RE task).

pub mod label;

pub use label::{parse_label, LabelFile, Measurement, Record};
