// Re-export TUI event types from seneschal-common so that callers in main.rs
// can continue to use `seneschal_tui::events::TuiEvent` etc.

pub use seneschal_common::tui_events::{
    InputSource, PipelineState, TuiEvent, TuiEventRx, TuiEventTx,
};
