// Tool trait and ToolRegistry — shared types extracted from src/tools/mod.rs.
// Individual tool implementations stay in their respective crates.
pub mod subtask;

use std::collections::HashMap;

/// Whether prompt-build mode is active and what the current prompt text is.
#[derive(Debug, Clone, PartialEq)]
pub enum PromptBuildState {
    Inactive,
    Active { prompt: String },
}

impl PromptBuildState {
    pub fn is_active(&self) -> bool {
        matches!(self, PromptBuildState::Active { .. })
    }

    pub fn prompt_text(&self) -> Option<&str> {
        match self {
            PromptBuildState::Active { prompt } => Some(prompt),
            _ => None,
        }
    }
}

/// Whether Seneschal is actively listening or only responding to its wake word.
#[derive(Debug, Clone, PartialEq)]
pub enum ConversationMode {
    /// Default — responds to the enrolled user's voice normally.
    Active,
    /// Quiet mode activated automatically (silence timer or non-user streak).
    Ambient,
    /// Quiet mode activated explicitly by the user via the tool.
    AmbientLocked,
}

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tracing::info;

use subtask::SubtaskTracker;

/// A tool the LLM can invoke by name.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// JSON Schema for this tool's parameters (OpenAI function-calling format).
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }
    /// If true, runs in background and delivers result via ProactiveEvent.
    fn is_background(&self) -> bool {
        false
    }
    /// If true, suppresses any LLM response after tool execution.
    fn is_silent(&self) -> bool {
        false
    }
    /// Returns true when this tool should be force-called for a query.
    fn should_force_for(&self, _query: &str) -> bool {
        false
    }
    /// Execute the tool with optional args.
    async fn run(&self, args: &str) -> String;
}

/// Registry of available tools and tool-call parser.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    cached_tool_defs: Mutex<Option<Vec<serde_json::Value>>>,
    /// Tracks background tool executions.
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
        self.register(subtask::ListTasksTool::new(tracker));
    }

    pub fn register(&mut self, tool: impl Tool + 'static) {
        self.tools.insert(tool.name().to_string(), Arc::new(tool));
        *self.cached_tool_defs.lock().unwrap() = None;
    }

    pub fn unregister(&mut self, name: &str) -> bool {
        let removed = self.tools.remove(name).is_some();
        if removed {
            *self.cached_tool_defs.lock().unwrap() = None;
        }
        removed
    }

    pub fn list_registered(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn forced_tool_for_query(&self, query: &str) -> Option<&str> {
        self.tools
            .values()
            .find(|t| t.should_force_for(query))
            .map(|t| t.name())
    }

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

    pub fn system_prompt_section(&self) -> String {
        if self.tools.is_empty() {
            return String::new();
        }
        let mut section = String::from(
            "\n\nREGLA DE NARRACIÓN ANTES DE HERRAMIENTAS: \
             Antes de llamar a cualquier herramienta, DEBES escribir primero una frase corta \
             en texto natural que describa lo que vas a hacer (por ejemplo: \"Buscando en internet...\", \
             \"Déjame buscar eso\", \"Abriendo la aplicación\"). \
             Esta frase se leerá en voz alta mientras la herramienta se ejecuta. \
             Después de escribir la frase, llama a la herramienta inmediatamente. \
             NUNCA simules ni finjas el resultado de una acción — siempre llama a la herramienta real.",
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
        section
    }

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

    pub fn is_background(&self, name: &str) -> bool {
        self.tools
            .get(name)
            .map(|t| t.is_background())
            .unwrap_or(false)
    }

    pub fn get_tool(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    pub fn get_tool_arc(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

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
