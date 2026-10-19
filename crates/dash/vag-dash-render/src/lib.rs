//! The panel, as pixels. 256×64, and nothing about where they land.
//!
//! This crate draws a [`Frame`] onto any `embedded_graphics` `DrawTarget` and
//! knows nothing else — not CAN, not UDS, not what a control unit is. On the
//! board the target is the OLED driver; on a laptop it is
//! `embedded-graphics-simulator`, writing PNG. That is the whole point of the
//! seam: "the simulator" and "the firmware" are one body of drawing code with
//! two sets of dependencies, so the layout can be finished before the hardware
//! arrives, and finished by looking rather than by arguing.
//!
//! What this crate is **not** is the plan. A [`Frame`] is a description of one
//! picture — labels already in the right language, values already scaled. Who
//! read the car and who did the arithmetic is not this crate's business, and
//! keeping it that way is what lets a page be rendered from a recording, from a
//! live cable or from a fixture with the same code.
//!
//! One thing here is not pixels: [`alarm`], the rule that decides *which* page
//! is on the glass when a channel nobody is watching goes wrong. It is here and
//! not in the firmware for the same reason the renderer is — the firmware is not
//! a workspace member and cannot be built for the host, so anything that lives
//! there is untested by CI. The alarm machine is `no_std`, allocation-free, has
//! no dependency on this crate's drawing code, and is the only thing that ever
//! sets [`Cell::alarm`]; it costs a laptop nothing and it earns a synthetic
//! clock. [`screen`] puts it together with the page cursor and what the driver asks for, so
//! the firmware only feeds it a clock, the store's values and a [`control::Command`]. More
//! machines are here for the same reason: [`button`] and [`control`], buttons on the
//! board's pins and `dashsim`'s presses as commands; [`stalk`], the cruise lever as buttons while cruise is off; and
//! [`stopwatch`], the stopwatch page's arming, launch and marks (`todo/dash/19`).
//!
//! Sizes are in pixels, and the panel's height is the fact that shapes the layout: it is
//! read from the target, not assumed ([`render`] says what 64 rows hold and what 32 do).

#![no_std]

#[cfg(test)]
extern crate std;

pub mod alarm;
pub mod button;
pub mod control;
pub mod frame;
pub mod history;
pub mod pages;
pub mod plan;
pub mod render;
pub mod screen;
pub mod stalk;
pub mod stopwatch;
pub mod theme;

pub use frame::{Board, Cell, Deviation, Faults, Frame, Links, Rates, Strip, Volts};
pub use render::{draw, draw_page, draw_with};
pub use theme::{Numerals, Theme};

use embedded_graphics::geometry::Size;

/// The glass: the board's SSD1322, 256×64 (`vag-dash-core`'s framebuffer and the glass's driver
/// are sized by it, and so is the laptop's replay of the board).
///
/// The renderer honours whatever size its target reports, so the layout is decided by
/// measuring; this is the size the tests lay the pages out on, because it is the one a board
/// shows. A test of the 32-row fallback ([`render`]) names its own size.
pub const PANEL: Size = Size::new(256, 64);
