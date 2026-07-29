// seneschal-tools-core — Essential LLM-callable tools for Seneschal.
//
// These 10 tools form the "reasonable minimum" tool set.

pub mod apple_events;
pub mod clipboard;
pub mod current_time;
pub mod noop;
pub mod open_app;
pub mod open_terminal;
pub mod quick_search;
pub mod read_file;
pub mod run_shell;
pub mod web_search;

// Re-export Tool trait from common so `super::Tool` works in submodules.
pub use seneschal_common::tools::Tool;

pub use apple_events::AppleEventsTool;
pub use clipboard::{ReadClipboardTool, SetClipboardTool};
pub use current_time::CurrentTimeTool;
pub use noop::NoopTool;
pub use open_app::OpenAppTool;
#[cfg(target_os = "macos")]
pub use open_terminal::OpenTerminalTool;
pub use quick_search::QuickSearchTool;
pub use read_file::ReadFileTool;
pub use run_shell::RunShellTool;
pub use web_search::WebSearchTool;
