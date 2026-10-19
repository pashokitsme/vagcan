//! Shared pieces for the firmware's images. The binaries in `src/bin` are thin; what is the
//! chip's lives here, and what the host can test as well lives in `vag-dash-core`.

#![no_std]

extern crate alloc;

pub mod can;
pub mod config;
pub mod health;
pub mod plan;
pub mod slcan;
pub mod store;
pub mod ui;
pub mod usb;
