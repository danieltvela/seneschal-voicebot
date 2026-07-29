pub mod apple_events;
pub mod clipboard;
pub mod conversation_mode;
pub mod current_time;
pub mod deep_research;
pub mod mcp_tool;
pub mod noop;
pub mod open_app;
pub mod open_terminal;
pub mod prompt_build;
pub mod quick_search;
pub mod read_file;
pub mod recover_historical_context;
pub mod run_agent;
pub mod run_shell;
pub mod subtask;
pub mod switch_plugin;
pub mod take_screenshot;
pub mod web_search;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tracing::info;

pub use apple_events::AppleEventsTool;
pub use clipboard::{ReadClipboardTool, SetClipboardTool};
pub use conversation_mode::{ConversationMode, SetConversationModeTool};
pub use current_time::CurrentTimeTool;
pub use deep_research::DeepResearchTool;
pub use mcp_tool::McpToolProxy;
pub use noop::NoopTool;
pub use open_app::OpenAppTool;
#[cfg(target_os = "macos")]
pub use open_terminal::OpenTerminalTool;
pub use prompt_build::SetPromptBuildTool;
pub use quick_search::QuickSearchTool;
pub use read_file::ReadFileTool;
#[allow(unused_imports)]
pub use recover_historical_context::RecoverHistoricalContextTool;
#[allow(unused_imports)]
pub use run_agent::{
    AcpWriter, ActiveTask, JsonRpcMessage, PendingInteractionEntry, RunAgentTool, format_history,
};
pub use run_shell::RunShellTool;
pub use subtask::{ListTasksTool, SubtaskTracker};
pub use switch_plugin::SwitchPluginTool;
pub use take_screenshot::TakeScreenshotTool;
pub use web_search::WebSearchTool;

// Re-exported from seneschal-common to avoid type conflicts.
pub use seneschal_common::tools::{Tool, ToolRegistry};

#[cfg(test)]
mod tests {
    use super::*;

    fn registry_with_current_time() -> ToolRegistry {
        let mut r = ToolRegistry::new();
        r.register(CurrentTimeTool);
        r
    }

    // ── parse_tool_call ───────────────────────────────────────────────────────

    #[test]
    fn parse_detects_current_time_call() {
        let r = registry_with_current_time();
        let llm_output = "<tool_call>current_time</tool_call>";
        assert_eq!(
            r.parse_tool_call(llm_output),
            Some(("current_time".to_string(), String::new()))
        );
    }

    #[test]
    fn parse_detects_tool_call_with_args() {
        let r = registry_with_current_time();
        // The parser splits on ':' so any args after the colon are captured.
        let llm_output = "<tool_call>current_time: some args</tool_call>";
        assert_eq!(
            r.parse_tool_call(llm_output),
            Some(("current_time".to_string(), "some args".to_string()))
        );
    }

    #[test]
    fn parse_detects_tool_call_embedded_in_text() {
        let r = registry_with_current_time();
        let llm_output = "  <tool_call>current_time</tool_call>  ";
        assert_eq!(
            r.parse_tool_call(llm_output),
            Some(("current_time".to_string(), String::new()))
        );
    }

    #[test]
    fn parse_returns_none_for_unregistered_tool() {
        let r = registry_with_current_time();
        let llm_output = "<tool_call>nonexistent_tool</tool_call>";
        assert_eq!(r.parse_tool_call(llm_output), None);
    }

    #[test]
    fn parse_returns_none_for_missing_closing_tag() {
        let r = registry_with_current_time();
        assert_eq!(r.parse_tool_call("<tool_call>current_time"), None);
    }

    #[test]
    fn parse_returns_none_for_missing_opening_tag() {
        let r = registry_with_current_time();
        assert_eq!(r.parse_tool_call("current_time</tool_call>"), None);
    }

    #[test]
    fn parse_returns_none_for_empty_registry() {
        let r = ToolRegistry::new();
        assert_eq!(
            r.parse_tool_call("<tool_call>current_time</tool_call>"),
            None
        );
    }

    #[test]
    fn parse_returns_none_for_plain_text() {
        let r = registry_with_current_time();
        assert_eq!(r.parse_tool_call("What time is it?"), None);
    }

    // ── execute ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn execute_current_time_returns_non_empty() {
        let r = registry_with_current_time();
        let result = r.execute("current_time", "").await;
        assert!(!result.is_empty());
    }

    #[tokio::test]
    async fn execute_current_time_contains_colon_separator() {
        // Output is "HH:MM:SS, Weekday DD Month YYYY" — always has ':'
        let r = registry_with_current_time();
        let result = r.execute("current_time", "").await;
        assert!(
            result.contains(':'),
            "expected time separator ':' in {result:?}"
        );
    }

    #[tokio::test]
    async fn execute_unknown_tool_returns_error_message() {
        let r = registry_with_current_time();
        let result = r.execute("nonexistent", "").await;
        assert!(
            result.contains("nonexistent"),
            "error message should mention the tool name"
        );
    }

    // ── system_prompt_section ─────────────────────────────────────────────────

    #[test]
    fn system_prompt_section_empty_for_empty_registry() {
        let r = ToolRegistry::new();
        assert!(r.system_prompt_section().is_empty());
    }

    #[test]
    fn system_prompt_section_non_empty_when_tools_registered() {
        let r = registry_with_current_time();
        let section = r.system_prompt_section();
        assert!(!section.is_empty());
        assert!(section.contains("REGLA DE NARRACIÓN ANTES DE HERRAMIENTAS"));
        assert!(section.contains("NUNCA simules ni finjas el resultado"));
        assert!(!section.contains("REGLA CRÍTICA ABSOLUTA"));
        assert!(!section.contains("REGLA DE FUERZA DE HERRAMIENTAS"));
    }

    #[test]
    fn tool_definitions_contains_tool_name_and_description() {
        let r = registry_with_current_time();
        let defs = r.tool_definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0]["function"]["name"], "current_time");
        assert!(
            !defs[0]["function"]["description"]
                .as_str()
                .unwrap_or("")
                .is_empty()
        );
    }

    #[test]
    fn tool_definitions_empty_for_empty_registry() {
        let r = ToolRegistry::new();
        assert!(r.tool_definitions().is_empty());
    }

    // ── forced_tool_for_query ─────────────────────────────────────────────────

    #[test]
    fn forced_tool_for_query_returns_current_time_for_time_request() {
        let r = registry_with_current_time();
        assert_eq!(
            r.forced_tool_for_query("¿Qué hora es?"),
            Some("current_time")
        );
    }

    #[test]
    fn forced_tool_for_query_returns_none_for_unrelated_request() {
        let r = registry_with_current_time();
        assert_eq!(r.forced_tool_for_query("Cuéntame un chiste"), None);
    }

    #[test]
    fn forced_tool_for_query_returns_first_matching_tool() {
        let mut r = ToolRegistry::new();
        r.register(CurrentTimeTool);
        r.register(WebSearchTool::new("http://localhost".into(), "".into()));
        assert_eq!(
            r.forced_tool_for_query("Busca noticias"),
            Some("web_search")
        );
        assert_eq!(
            r.forced_tool_for_query("¿Qué hora es?"),
            Some("current_time")
        );
    }

    // ── is_background ─────────────────────────────────────────────────────────

    #[test]
    fn current_time_is_not_background() {
        let mut r = ToolRegistry::new();
        r.register(CurrentTimeTool);
        assert!(!r.is_background("current_time"));
    }

    #[test]
    fn is_background_unknown_tool_returns_false() {
        let r = ToolRegistry::new();
        assert!(!r.is_background("nonexistent"));
    }

    // ── get_tool ──────────────────────────────────────────────────────────────

    #[test]
    fn get_tool_returns_some_for_registered_tool() {
        let mut r = ToolRegistry::new();
        r.register(CurrentTimeTool);
        assert!(r.get_tool("current_time").is_some());
    }

    #[test]
    fn get_tool_returns_none_for_unregistered_tool() {
        let r = ToolRegistry::new();
        assert!(r.get_tool("nonexistent").is_none());
    }

    #[test]
    fn get_tool_returns_correct_name() {
        let mut r = ToolRegistry::new();
        r.register(CurrentTimeTool);
        let tool = r.get_tool("current_time").expect("tool should exist");
        assert_eq!(tool.name(), "current_time");
    }

    // ── parse → execute round-trip ────────────────────────────────────────────

    #[tokio::test]
    async fn parse_and_execute_current_time_round_trip() {
        let r = registry_with_current_time();
        let llm_output = "<tool_call>current_time</tool_call>";

        let (name, args) = r
            .parse_tool_call(llm_output)
            .expect("should parse current_time");
        let result = r.execute(&name, &args).await;

        assert_eq!(name, "current_time");
        assert!(!result.is_empty());
        // Result should look like a time (contains ':')
        assert!(result.contains(':'));
    }

    // ── unregister ────────────────────────────────────────────────────────────

    #[test]
    fn unregister_removes_tool_and_returns_true() {
        let mut r = ToolRegistry::new();
        r.register(CurrentTimeTool);
        assert!(r.get_tool("current_time").is_some());

        let removed = r.unregister("current_time");
        assert!(removed);
        assert!(r.get_tool("current_time").is_none());
    }

    #[test]
    fn unregister_returns_false_for_unknown_tool() {
        let mut r = ToolRegistry::new();
        let removed = r.unregister("nonexistent");
        assert!(!removed);
    }

    #[test]
    fn unregister_removes_from_list_registered() {
        let mut r = ToolRegistry::new();
        r.register(CurrentTimeTool);
        assert!(r.list_registered().contains(&"current_time".to_string()));

        r.unregister("current_time");
        assert!(!r.list_registered().contains(&"current_time".to_string()));
    }

    #[test]
    fn unregister_clears_cached_tool_definitions() {
        let mut r = ToolRegistry::new();
        r.register(CurrentTimeTool);
        let _defs = r.tool_definitions();

        r.unregister("current_time");
        let defs_after = r.tool_definitions();
        assert!(defs_after.is_empty());
    }

    #[test]
    fn unregister_does_not_affect_other_tools() {
        let mut r = ToolRegistry::new();
        r.register(CurrentTimeTool);
        r.register(NoopTool::new("test".to_string()));

        r.unregister("current_time");
        assert!(r.get_tool("current_time").is_none());
        assert!(r.get_tool("noop").is_some());
    }

    #[test]
    fn unregister_twice_returns_false_second_time() {
        let mut r = ToolRegistry::new();
        r.register(CurrentTimeTool);

        assert!(r.unregister("current_time"));
        assert!(!r.unregister("current_time"));
    }
}
