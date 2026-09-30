//! Shared pieces for the recon firmware. The binaries in `src/bin` are thin;
//! anything worth testing or reusing lives here.

#![no_std]

extern crate alloc;

pub mod can;
pub mod config;
pub mod exchange;
pub mod faults;
pub mod health;
pub mod input;
pub mod panel;
pub mod plan;
pub mod saving;
pub mod schema;
pub mod slcan;
pub mod ssd1322;
pub mod store;
pub mod ui;
pub mod usb;
