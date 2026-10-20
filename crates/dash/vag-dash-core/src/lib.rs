//! The board's own logic, for the board and the laptop alike.
//!
//! `vag-dash-fw` is `no_std` for the chip with its own build-std configuration, so it is not a
//! workspace member and none of its code is built or tested on the host. What it decides by
//! lives here instead, `no_std` and allocation-free, and the firmware links it: the workspace's
//! tests are then tests of what the board runs, and the laptop's replay of the board
//! (`vag-cli-diag`'s `dashreplay`) runs the same code rather than a copy of it.
//!
//! * [`values`]: the value store, and how long a reading may stay on the glass.
//! * [`frames`]: one frame of the panel — the screen's answer, the charts' samples, the page
//!   composed — as the board's panel task runs it and the replay runs it over a recording.
//! * [`panel`]: the glass's framebuffer, one bit a pixel, and the `FRAME` line that carries it
//!   over USB. Its size is the renderer's [`PANEL`](vag_dash_render::PANEL).
//! * [`ssd1322`]: the glass's driver, over `embedded-hal` traits.
//! * [`schema`], [`saving`]: the settings record as flash holds it, and when it is written.
//! * [`faults`], [`exchange`]: the fault count's shell round the planner, and one exchange's
//!   waits.
//! * [`input`]: the queue every input's commands go through.
//! * [`bus_rate`]: the board's count of its own exchanges, for the console and `state`.
//! * [`unit_check`]: one plan unit's part check — when it is polled, asked its part number
//!   again, or taken for silent.
//!
//! What is the chip's — the HAL, the tasks, the radio, flash — stays in `vag-dash-fw`.

#![no_std]

pub mod bus_rate;
pub mod exchange;
pub mod faults;
pub mod frames;
pub mod input;
pub mod panel;
pub mod saving;
pub mod schema;
pub mod ssd1322;
pub mod unit_check;
pub mod values;
