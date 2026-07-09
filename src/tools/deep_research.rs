use std::sync::Arc;
use std::sync::RwLock;

use async_trait::async_trait;
use serde_json::json;
use tracing::info;

use super::Tool;
use crate::agents::AgentConfig;
use crate::tools::run_agent;

/// Deep research tool — delegates complex research to an autonomous agent.
///
/// This is the "deep path": it runs asynchronously (background) via
/// the existing agent delegation infrastructure.  Use it for comparative
/// analysis, synthesis across multiple documents, multi-step reasoning,
/// or any task requiring more than a simple factual lookup.
///
/// The result is delivered asynchronously through the pipeline's proactive
/// event system (see [`ProactiveEvent`]).
pub struct DeepResearchTool {
    agent_config: AgentConfig,
    shared_history: Arc<RwLock<String>>,
}

impl DeepResearchTool {
    pub fn new(agent_config: AgentConfig, shared_history: Arc<RwLock<String>>) -> Self {
        Self {
            agent_config,
            shared_history,
        }
    }
}

#[async_trait]
impl Tool for DeepResearchTool {
    fn name(&self) -> &str {
        "deep_research"
    }

    fn description(&self) -> &str {
        "Investigación profunda y síntesis compleja de información. \
         Úsala cuando el usuario pida: análisis comparativo, \
         investigación exhaustiva, resumen de múltiples fuentes, \
         informes detallados, o tareas que requieran razonamiento \
         extendido con acceso a herramientas. \
         NO la uses para consultas factuales simples (usa quick_search). \
         Esta herramienta tarda más pero da resultados más completos."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The research query or task description"
                }
            },
            "required": ["query"]
        })
    }

    fn is_background(&self) -> bool {
        true
    }

    fn preamble(&self) -> Option<&'static str> {
        Some("Investigando en profundidad.")
    }

    async fn run(&self, args: &str) -> String {
        let query = match serde_json::from_str::<serde_json::Value>(args) {
            Ok(v) => v["query"].as_str().unwrap_or("").trim().to_string(),
            Err(_) => args.trim().to_string(),
        };

        if query.is_empty() {
            return "Error: no research query provided.".to_string();
        }

        info!(target: "tools", "deep_research: agent='{}' query={:?}", self.agent_config.name, query);

        let history = self
            .shared_history
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();
        let full_query = if history.is_empty() {
            query
        } else {
            format!("Historial de la conversación:\n{history}\n\nTarea de investigación: {query}")
        };

        // Delegate to the agent as a one-shot CLI call.
        let command = match &self.agent_config.command {
            Some(cmd) => cmd.clone(),
            None => {
                return format!(
                    "Error: agent '{}' has no CLI command configured",
                    self.agent_config.name
                );
            }
        };
        run_agent::call_agent(command, full_query).await
    }
}
