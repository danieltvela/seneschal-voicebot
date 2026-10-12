pub mod control;

// Re-export permission types for Control consumers / handlers.
pub use seneschal_common::{
    HttpPermissionResult, PermissionGate, PermissionOptionWire, PermissionPhase,
    PermissionSlotView, ResolveOutcome,
};
