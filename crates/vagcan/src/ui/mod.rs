//! What the commands draw with.
//!
//! One module so far, and it is here rather than beside its first caller
//! because it has four of them: the terminal guard that every full-screen
//! command and the picker enter through.

pub mod term;
