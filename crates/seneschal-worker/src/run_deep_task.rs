//! `run_deep_task` — the single door from the voice model to the deep worker.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use seneschal_common::tools::Tool;

use crate::engine::DeepWorkerEngine;

/// Voice-facing tool that delegates heavy work to the deep worker.
///
/// `is_background() = true` so the pipeline's background-tool branch applies:
/// immediate ack ("estoy en ello"), filler sound, abbreviated persistence of
/// the delegation (anti-#221) and a single result announcement later
/// (anti-#218).
pub struct RunDeepTaskTool {
    engine: Arc<DeepWorkerEngine>,
}

impl RunDeepTaskTool {
    pub fn new(engine: Arc<DeepWorkerEngine>) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl Tool for RunDeepTaskTool {
    fn name(&self) -> &str {
        "run_deep_task"
    }

    fn description(&self) -> &str {
        "Delega trabajo pesado al deep worker, un subagente interno con su propio \
         modelo más potente y herramientas avanzadas (ficheros, pantalla, \
         búsqueda, agentes externos). ÚSALA para TODO lo que no sea \
         conversación ni una tool de voz directa: búsqueda o análisis \
         profundo, visión y pantalla, operaciones con ficheros, tareas \
         multi-paso, síntesis de información. El resultado llega después \
         por voz; mientras tanto di brevemente que lo estás procesando."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Descripción completa y autocontenida de la tarea a ejecutar"
                }
            },
            "required": ["task"],
            "additionalProperties": false
        })
    }

    fn is_background(&self) -> bool {
        true
    }

    async fn run(&self, args: &str) -> String {
        let task = serde_json::from_str::<serde_json::Value>(args)
            .ok()
            .and_then(|v| {
                v.get("task")
                    .and_then(|t| t.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| args.trim().to_string());

        if task.is_empty() {
            warn!(target: "deep_worker", "run_deep_task called with empty task");
            return "Error: run_deep_task requiere una descripción de la tarea.".to_string();
        }

        // Inline commands — mirror run_agent's cancel/status precedent.
        let lower = task.trim().to_lowercase();
        if lower == "cancel" || lower.starts_with("cancel ") {
            let n = self.engine.cancel_all();
            return if n == 0 {
                "[El deep worker no tiene ninguna tarea en curso para cancelar.]".to_string()
            } else {
                format!("[Canceladas {n} tarea(s) del deep worker.]")
            };
        }
        if lower == "status" || lower.starts_with("status ") {
            return self.engine.status();
        }

        let task_id = self.engine.spawn_task(task);
        format!("[Tarea delegada al deep worker ({task_id}). El resultado llegará en breve.]")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use tokio::sync::mpsc;

    use seneschal_common::events::ProactiveEvent;
    use seneschal_common::tools::ToolRegistry;
    use seneschal_core::llm::LlmProvider;

    use super::*;
    use crate::engine::DeepWorkerParams;
    use crate::tests_support::{MockProvider, MockResponse, NoopTool};

    fn engine_with(mock: MockProvider) -> Arc<DeepWorkerEngine> {
        let tools = Arc::new(Mutex::new(ToolRegistry::new()));
        let (proactive_tx, _rx) = mpsc::channel::<ProactiveEvent>(8);
        let params = DeepWorkerParams {
            system_prompt: "test".to_string(),
            thinking: false,
            thinking_mode: crate::ThinkingMode::External,
            max_iterations: 3,
            timeout: Duration::from_secs(5),
        };
        let provider: Arc<dyn LlmProvider> = Arc::new(mock);
        let (engine, _event_rx) = DeepWorkerEngine::new(provider, tools, proactive_tx, params);
        engine
    }

    #[tokio::test]
    async fn empty_task_returns_error() {
        let engine = engine_with(MockProvider::new(vec![]));
        let tool = RunDeepTaskTool::new(engine);
        let result = tool.run("").await;
        assert!(result.contains("Error"), "{result:?}");
    }

    #[tokio::test]
    async fn returns_ack_with_task_id() {
        let engine = engine_with(MockProvider::new(vec![MockResponse::text("ok")]));
        let tool = RunDeepTaskTool::new(engine);
        let result = tool.run(r#"{"task": "analiza el proyecto"}"#).await;
        assert!(result.contains("deep worker"), "{result:?}");
        // Extract the uuid between parentheses.
        let id = result.split('(').nth(1).unwrap().split(')').next().unwrap();
        assert!(uuid::Uuid::parse_str(id).is_ok(), "bad id: {id}");
    }

    #[tokio::test]
    async fn status_reports_active_tasks() {
        // A provider that never responds keeps the task alive.
        let engine = engine_with(MockProvider::new(vec![MockResponse::Hang]));
        let tool = RunDeepTaskTool::new(engine);
        tool.run(r#"{"task": "tarea larga"}"#).await;
        // Give the spawned task a moment to register.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let status = tool.run(r#"{"task": "status"}"#).await;
        assert!(status.contains("1 tarea"), "{status:?}");
    }

    #[tokio::test]
    async fn cancel_with_no_tasks() {
        let engine = engine_with(MockProvider::new(vec![]));
        let tool = RunDeepTaskTool::new(engine);
        let result = tool.run(r#"{"task": "cancel"}"#).await;
        assert!(result.contains("ninguna tarea"), "{result:?}");
    }

    #[tokio::test]
    async fn noop_tool_is_registered_name() {
        let t = NoopTool;
        assert_eq!(t.name(), "noop");
    }
}
