//! The seam every backend implements: raw CAN frames, whole ISO-TP PDUs, and
//! the async variant the UDS client rides.
//!
//! **Runs on the board.** `--no-default-features` builds this crate `no_std`
//! (`alloc` only) for `riscv32imc-unknown-none-elf`; the host build is
//! unchanged, because `std` does not *define* `Duration`, `Vec` or `VecDeque` —
//! it re-exports them. Same types, same call sites.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod error;
pub mod frame;
pub mod mock;
pub mod traits;

pub use error::TransportError;
pub use frame::{CanFrame, CanId};
#[cfg(any(test, feature = "test-util"))]
pub use mock::MockAsyncTransport;
pub use mock::{ScriptStep, ScriptedCan};
pub use traits::{AsyncIsoTpTransport, IsoTpTransport, MaybeSend, RawCanTransport};
