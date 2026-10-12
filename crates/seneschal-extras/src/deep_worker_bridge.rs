//! Bridge between the deep worker (seneschal-worker) and the existing agent
//! delegation infrastructure (RunAgentTool cores).
//!
//! The delegate holds the `Arc<RunAgentTool>` handles the binary creates, so
//! the worker's nested `run_agent` tool reuses the SAME sessions, transports
//! and task maps as the voice path — one Hermes session per agent, shared.

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use seneschal_common::tools::ToolRegistry;

use crate::run_agent::RunAgentTool;
use seneschal_worker::AgentDelegate;

/// `AgentDelegate` implementation over the registered `RunAgentTool`s.
///
/// Populated by the binary's wiring (base agents from the registry plus
/// plugin agents as they activate).
#[derive(Default)]
pub struct RunAgentToolDelegate {
    tools: std::sync::Mutex<Vec<Arc<RunAgentTool>>>,
}

impl RunAgentToolDelegate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handle for delegation (the same instance the registry
    /// serves, when also registered there).
    pub fn add(&self, tool: Arc<RunAgentTool>) {
        self.tools.lock().unwrap().push(tool);
    }

    pub fn is_empty(&self) -> bool {
        self.tools.lock().unwrap().is_empty()
    }

    fn find(&self, agent: &str) -> Option<Arc<RunAgentTool>> {
        let tools = self.tools.lock().unwrap();
        tools
            .iter()
            .find(|t| t.agent_name() == agent || t.agent_name_with_prefix() == agent)
            .cloned()
    }
}

#[async_trait]
impl AgentDelegate for RunAgentToolDelegate {
    fn agents_section(&self) -> String {
        let tools = self.tools.lock().unwrap();
        if tools.is_empty() {
            return String::new();
        }
        let mut section = String::from(
            "\n\n## AGENTES EXTERNOS DISPONIBLES\n\n\
             Puedes delegar subtareas a los siguientes agentes externos con la \
             herramienta run_agent(agent, task). Cada agente tiene herramientas \
             propias y especialización. La llamada es bloqueante: devuelve el \
             resultado final del agente.\n",
        );
        for tool in tools.iter() {
            section.push_str(&format!(
                "\n- {}: {}\n",
                tool.agent_name(),
                tool.when_to_use()
            ));
        }
        section
    }

    async fn delegate(&self, agent: &str, task: &str, cancel: CancellationToken) -> String {
        match self.find(agent) {
            Some(tool) => tool.run_await(task.to_string(), cancel).await,
            None => format!(
                "Agent error: no hay ningún agente registrado llamado '{agent}'. \
                 Usa solo los agentes listados en tu system prompt."
            ),
        }
    }
}

/// Register the worker-facing nested `run_agent` tool into the worker pool.
pub fn register_nested_agent_tool(
    registry: &mut ToolRegistry,
    delegate: Arc<RunAgentToolDelegate>,
) {
    registry.register(seneschal_worker::RunAgentNestedTool::new(delegate));
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use dashmap::DashMap;
    use tokio::sync::mpsc;

    use super::*;
    use crate::run_agent::ActiveTask;
    use seneschal_agents::AgentConfig;
    use seneschal_common::events::ProactiveEvent;

    fn cli_agent_tool(name: &str, command: &str) -> Arc<RunAgentTool> {
        let config = AgentConfig {
            name: name.to_string(),
            mode: "cli".to_string(),
            command: Some(command.to_string()),
            acp_command: String::new(),
            remote_url: String::new(),
            remote_dir: String::new(),
            remote_session_path: String::new(),
            remote_message_path: String::new(),
            remote_event_path: String::new(),
            remote_api_key: String::new(),
            when_to_use: "Testing".to_string(),
            prompt: String::new(),
            description: String::new(),
            task_description: String::new(),
        };
        let task_map: Arc<DashMap<String, ActiveTask>> = Arc::new(DashMap::new());
        let (tx, _rx) = mpsc::channel::<ProactiveEvent>(8);
        Arc::new(RunAgentTool::new(config, task_map, tx))
    }

    #[tokio::test]
    async fn delegates_to_registered_agent() {
        let delegate = RunAgentToolDelegate::new();
        delegate.add(cli_agent_tool("hermes", "echo hermes-dice-hola"));
        let result = delegate
            .delegate("hermes", "test", CancellationToken::new())
            .await;
        assert!(result.contains("hermes-dice-hola"), "{result:?}");
    }

    #[tokio::test]
    async fn unknown_agent_returns_error() {
        let delegate = RunAgentToolDelegate::new();
        let result = delegate
            .delegate("nope", "test", CancellationToken::new())
            .await;
        assert!(result.contains("Agent error"), "{result:?}");
    }

    #[test]
    fn agents_section_lists_agents() {
        let delegate = RunAgentToolDelegate::new();
        assert!(delegate.agents_section().is_empty());
        delegate.add(cli_agent_tool("hermes", "echo hi"));
        let section = delegate.agents_section();
        assert!(section.contains("hermes"), "{section:?}");
        assert!(section.contains("run_agent"), "{section:?}");
    }

    #[test]
    fn find_matches_with_and_without_prefix() {
        let delegate = RunAgentToolDelegate::new();
        delegate.add(cli_agent_tool("hermes", "echo hi"));
        assert!(delegate.find("hermes").is_some());
        assert!(delegate.find("run_hermes").is_some());
        assert!(delegate.find("other").is_none());
    }
}
