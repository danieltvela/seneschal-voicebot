//! Nivel 3: the worker's door to external agents (Hermes, fleet, …).
//!
//! The `AgentDelegate` trait is implemented by the binary's wiring (over the
//! existing `RunAgentTool` cores in seneschal-extras) so this crate stays
//! independent of the agent backends. The nested delegation is synchronous
//! from the worker loop's point of view: it awaits the agent's final result,
//! which then feeds the next loop iteration.

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use seneschal_common::tools::Tool;

/// Awaitable delegation into a registered external agent.
#[async_trait]
pub trait AgentDelegate: Send + Sync {
    /// System-prompt section describing the delegatable agents for the
    /// worker (names + when to use). Empty when no agents are registered.
    fn agents_section(&self) -> String;

    /// Delegate `task` to `agent`, awaiting the final result.
    /// Must honor `cancel` (return promptly with a cancellation marker).
    async fn delegate(&self, agent: &str, task: &str, cancel: CancellationToken) -> String;
}

/// The worker-facing `run_agent` tool: nested delegation (issue #236, Nivel 3).
///
/// Unlike the voice-facing `run_{agent}` tools this one AWAITS the result —
/// the worker's loop needs the agent's answer to continue reasoning.
pub struct RunAgentNestedTool {
    delegate: Arc<dyn AgentDelegate>,
}

impl RunAgentNestedTool {
    pub fn new(delegate: Arc<dyn AgentDelegate>) -> Self {
        Self { delegate }
    }
}

#[async_trait]
impl Tool for RunAgentNestedTool {
    fn name(&self) -> &str {
        "run_agent"
    }

    fn description(&self) -> &str {
        "Delega una subtarea a un agente externo registrado (p. ej. Hermes \
         orquestando una flota de subagentes). Úsala cuando la tarea requiera \
         capacidades que no tienes directamente: ejecución de proyectos \
         largos, herramientas externas especializadas, orquestación de flota. \
         Devuelve el resultado final del agente (bloqueante: espera a que termine)."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "agent": {
                    "type": "string",
                    "description": "Nombre del agente registrado"
                },
                "task": {
                    "type": "string",
                    "description": "Descripción completa y autocontenida de la subtarea"
                }
            },
            "required": ["agent", "task"],
            "additionalProperties": false
        })
    }

    async fn run(&self, args: &str) -> String {
        self.run_with_cancel(args, CancellationToken::new()).await
    }

    async fn run_with_cancel(&self, args: &str, cancel: CancellationToken) -> String {
        let v = match serde_json::from_str::<serde_json::Value>(args) {
            Ok(v) => v,
            Err(_) => {
                return "Error: run_agent espera JSON args {\"agent\", \"task\"}.".to_string();
            }
        };
        let agent = match v["agent"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
            Some(a) => a.to_string(),
            None => return "Error: falta el nombre del agente.".to_string(),
        };
        let task = match v["task"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
            Some(t) => t.to_string(),
            None => return "Error: falta la tarea a delegar.".to_string(),
        };

        // Spawn + join: if the engine drops this future (its own cancel won
        // the race), the spawned delegation still runs to completion so the
        // agent-side cleanup (session state, dangling permissions) happens.
        let delegate = Arc::clone(&self.delegate);
        let handle = tokio::spawn(async move { delegate.delegate(&agent, &task, cancel).await });
        match handle.await {
            Ok(result) => result,
            Err(e) => format!("Agent error: la delegación anidada falló: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::tests_support::RecordingDelegate;

    fn tool(delegate: Arc<RecordingDelegate>) -> RunAgentNestedTool {
        RunAgentNestedTool::new(delegate)
    }

    #[tokio::test]
    async fn delegates_agent_and_task() {
        let delegate = Arc::new(RecordingDelegate::new());
        let result = tool(delegate.clone())
            .run(r#"{"agent": "hermes", "task": "busca esto"}"#)
            .await;
        assert!(result.contains("hermes"), "{result:?}");
        let calls = delegate.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "hermes");
        assert_eq!(calls[0].1, "busca esto");
    }

    #[tokio::test]
    async fn missing_fields_return_error() {
        let delegate = Arc::new(RecordingDelegate::new());
        assert!(
            tool(delegate.clone())
                .run(r#"{"agent": "hermes"}"#)
                .await
                .contains("falta"),
        );
        assert!(
            tool(delegate)
                .run(r#"{"task": "x"}"#)
                .await
                .contains("falta"),
        );
    }

    #[tokio::test]
    async fn cancel_reaches_delegate() {
        // A delegate that hangs until cancelled.
        let delegate = Arc::new(RecordingDelegate::hanging());
        let t = tool(delegate);
        let cancel = CancellationToken::new();
        let c = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            c.cancel();
        });
        let result = t
            .run_with_cancel(r#"{"agent": "hermes", "task": "x"}"#, cancel)
            .await;
        assert!(result.contains("cancelada"), "{result:?}");
    }
}
