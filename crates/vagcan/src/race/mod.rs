//! `vagcan race` — time an acceleration run from the car's own speed signal.
//!
//! The design is `docs/superpowers/specs/2026-08-03-race-design.md`; the two
//! rules that shape every module here are worth repeating at the door.
//!
//! **Two kinds of number, and only two.** Everything shown is either *read* — a
//! value that was on the bus and whose meaning is proven by a catalog row or by
//! SAE J1979 — or *derived*, computed here from read values. The `(raw)` class
//! the rest of this crate deals with does not exist in `race`: an unproven byte
//! cannot be timed, integrated or differentiated, and a channel that will not
//! resolve is a channel this command does without.
//!
//! **The file holds raw samples; derivatives are recomputed.** Nothing shown
//! live is ever saved. That is what lets a method be corrected afterwards
//! without re-driving the car, and every correction in the design's history so
//! far has needed it.

// The module is being built task by task; items land before their callers do.
#![allow(dead_code)]

pub mod types;
