//! Shared pieces for the recon firmware. The binaries in `src/bin` are thin;
//! anything worth testing or reusing lives here.

#![no_std]

extern crate alloc;

pub mod can;
pub mod config;
pub mod panel;
pub mod store;
pub mod ui;
