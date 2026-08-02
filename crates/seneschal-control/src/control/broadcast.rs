//! Re-export control broadcast types from `seneschal-common`.
//!
//! The event enum and bus live in common so `seneschal-core` pipeline tasks can
//! publish without depending on this crate (or a broken `#[cfg(feature = "control")]`).

pub use seneschal_common::{ControlBroadcast, ControlEvent};
