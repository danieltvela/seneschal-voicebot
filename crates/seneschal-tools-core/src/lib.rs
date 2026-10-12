// seneschal-tools-core — Essential LLM-callable tools for Seneschal.
//
// These 10 tools form the "reasonable minimum" tool set.

pub mod apple_events;
pub mod clipboard;
pub mod current_time;
pub mod list_dir;
pub mod open_app;
pub mod quick_search;
pub mod read_file;
pub mod run_shell;
pub mod web_search;
pub mod write_file;

// Re-export Tool trait from common so `super::Tool` works in submodules.
pub use seneschal_common::tools::Tool;

pub use apple_events::AppleEventsTool;
pub use clipboard::{ReadClipboardTool, SetClipboardTool};
pub use current_time::CurrentTimeTool;
pub use list_dir::ListDirTool;
pub use open_app::OpenAppTool;
pub use quick_search::QuickSearchTool;
pub use read_file::ReadFileTool;
pub use run_shell::RunShellTool;
pub use web_search::WebSearchTool;
pub use write_file::WriteFileTool;
