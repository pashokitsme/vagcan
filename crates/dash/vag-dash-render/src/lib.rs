//! The panel, as pixels. 256×32, and nothing about where they land.
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
//! clock.
//!
//! Sizes are in pixels and the panel is 32 of them tall, which is the single
//! fact that shapes every decision here. Four tiers of text — the label over two
//! lines, the number, the unit under it — do not fit. Two do.

#![no_std]

pub mod alarm;
pub mod frame;
pub mod plan;
pub mod render;
pub mod theme;

pub use frame::{Cell, Frame};
pub use render::draw;
pub use theme::{Numerals, Theme};

use embedded_graphics::geometry::Size;

/// The panel this crate is laid out for.
///
/// Carried as a constant rather than read from the target's bounding box so a
/// test can state the panel it means. The renderer honours whatever size the
/// target actually reports; this is what the layout was *designed* against, and
/// the two differing is worth knowing about.
pub const PANEL: Size = Size::new(256, 32);
