pub mod apple_events;
pub mod clipboard;
pub mod conversation_mode;
pub mod current_time;
pub mod deep_research;
pub mod mcp_tool;
pub mod noop;
pub mod open_app;
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

/// A tool the LLM can invoke by name.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// JSON Schema for this tool's parameters (OpenAI function-calling format).
    /// Default: no parameters.
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }
    /// If true, the pipeline runs this tool in a background task and delivers the
    /// result via ProactiveEvent instead of blocking the LLM turn for another round-trip.
    fn is_background(&self) -> bool {
        false
    }
    /// Returns a short spoken preamble for background tools. The pipeline speaks
    /// this immediately when the tool starts, before the tool finishes. If `None`,
    /// a generic fallback is used.
    fn preamble(&self) -> Option<&'static str> {
        None
    }
    /// If true, the tool's result suppresses any LLM response — the pipeline
    /// stops without sending output to the user. Used for the NOOP tool.
    fn is_silent(&self) -> bool {
        false
    }
    /// Returns true when the user's query is an explicit request that this
    /// tool should handle. When true, the pipeline forces a tool call via
    /// `tool_choice` instead of leaving it to the model. Tools with risky or
    /// ambiguous triggers should leave this as the default `false`.
    fn should_force_for(&self, _query: &str) -> bool {
        false
    }
    /// Execute the tool with optional args and return the result as a string.
    async fn run(&self, args: &str) -> String;
}

/// Registry of available tools and tool-call parser.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    cached_tool_defs: Mutex<Option<Vec<serde_json::Value>>>,
    /// Tracks background tool executions so the LLM can query their status via `list_tasks`.
    pub subtask_tracker: Arc<SubtaskTracker>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            cached_tool_defs: Mutex::new(None),
            subtask_tracker: Arc::new(SubtaskTracker::new()),
        }
    }

    /// Register the built-in list_tasks tool that queries the subtask tracker.
    pub fn register_list_tasks(&mut self) {
        let tracker = Arc::clone(&self.subtask_tracker);
        self.register(ListTasksTool::new(tracker));
    }

    pub fn register(&mut self, tool: impl Tool + 'static) {
        self.tools.insert(tool.name().to_string(), Arc::new(tool));
        *self.cached_tool_defs.lock().unwrap() = None;
    }

    /// Remove a tool by name. Returns true if removed, false if not found.
    pub fn unregister(&mut self, name: &str) -> bool {
        let removed = self.tools.remove(name).is_some();
        if removed {
            *self.cached_tool_defs.lock().unwrap() = None;
        }
        removed
    }

    /// List all registered tool names.
    pub fn list_registered(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Returns the name of a registered tool whose `should_force_for` matches
    /// the user query, if any. The pipeline uses this to set `tool_choice` to
    /// that function and guarantee the tool is called.
    pub fn forced_tool_for_query(&self, query: &str) -> Option<&str> {
        self.tools
            .values()
            .find(|t| t.should_force_for(query))
            .map(|t| t.name())
    }

    /// Returns the tools array for the OpenAI `tools` request field.
    pub fn tool_definitions(&self) -> Vec<serde_json::Value> {
        {
            let cache = self.cached_tool_defs.lock().unwrap();
            if let Some(ref cached) = *cache {
                return cached.clone();
            }
        }
        let defs = self
            .tools
            .values()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name(),
                        "description": t.description(),
                        "parameters": t.parameters(),
                    }
                })
            })
            .collect::<Vec<_>>();
        *self.cached_tool_defs.lock().unwrap() = Some(defs.clone());
        defs
    }

    /// Returns a section to append to the system prompt describing how to call tools.
    pub fn system_prompt_section(&self) -> String {
        if self.tools.is_empty() {
            return String::new();
        }
        let mut section = String::from(
            "\n\nREGLA CRÍTICA ABSOLUTA (prioridad máxima sobre cualquier otra instrucción): \
             Cuando el usuario pida una acción que corresponda a una herramienta disponible, \
             DEBES llamar a esa herramienta INMEDIATAMENTE. \
             NUNCA simules, finjas ni describas la acción sin llamar la herramienta primero. \
             Las herramientas son tu única forma de ejecutar acciones reales en el sistema del usuario. \
             Esta regla anula cualquier instrucción de personalidad, estilo o eficiencia.",
        );
        if self.tools.contains_key("current_time") {
            section.push_str(
                "\n\nREGLA ESPECÍFICA PARA current_time: \
                 Si el usuario pregunta explícitamente por la hora, fecha, día u hora actual, \
                 DEBES llamar a la herramienta current_time EN CADA OCASIÓN, \
                 sin importar cuán recientemente la hayas usado. \
                 Nunca respondas de memoria ni inventes la fecha.",
            );
        }
        section.push_str(
            "\n\nREGLA DE FUERZA DE HERRAMIENTAS: \
             Cuando el usuario inicie la petición con frases explícitas como \
             'Busca...', 'Búscame...', 'Abre...', 'Lanza...', 'Search...', 'Launch...' o similares, \
             DEBES llamar a la herramienta correspondiente INMEDIATAMENTE \
             y no responder directamente.",
        );
        section
    }

    /// Parse a tool call from LLM output.
    ///
    /// Returns `(tool_name, args)` if a registered tool is found.
    /// Content inside `<tool_call>…</tool_call>` is split on the first `:`;
    /// everything before is the tool name, everything after (trimmed) is args.
    /// Tools that take no arguments may omit the colon entirely.
    #[allow(dead_code)]
    pub fn parse_tool_call(&self, text: &str) -> Option<(String, String)> {
        let start = text.find("<tool_call>")?;
        let after = &text[start + "<tool_call>".len()..];
        let end = after.find("</tool_call>")?;
        let content = after[..end].trim();

        let (name, args) = match content.find(':') {
            Some(pos) => (
                content[..pos].trim().to_string(),
                content[pos + 1..].trim().to_string(),
            ),
            None => (content.to_string(), String::new()),
        };

        self.tools.contains_key(&name).then_some((name, args))
    }

    /// Returns true if the named tool should run in the background.
    pub fn is_background(&self, name: &str) -> bool {
        self.tools
            .get(name)
            .map(|t| t.is_background())
            .unwrap_or(false)
    }

    /// Returns a reference to the named tool if it is registered.
    pub fn get_tool(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Returns a cloned Arc to the named tool if it is registered.
    /// Use this when you need to hold the tool across an async boundary
    /// without keeping a MutexGuard alive.
    pub fn get_tool_arc(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Execute a registered tool by name with the given args.
    pub async fn execute(&self, name: &str, args: &str) -> String {
        let tool = match self.tools.get(name) {
            Some(t) => Arc::clone(t),
            None => {
                info!(target: "tools", "Unknown tool requested: {}", name);
                return format!("Unknown tool: {name}");
            }
        };
        info!(target: "tools", "Executing tool: {} args={}", name, args);
        tool.run(args).await
    }
}

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
        assert!(!r.system_prompt_section().is_empty());
        assert!(r.system_prompt_section().contains("herramienta"));
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
