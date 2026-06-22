use std::collections::HashSet;
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::mpsc;
use tracing::warn;

use crate::agents::{AcpSessionManager, AgentConfig, ProactiveEvent};
use crate::config::HermesSessionViewerMode;
use crate::llm::LlmProvider;
use crate::tools::{ActiveTask, RunAgentTool, Tool, ToolRegistry};

use super::manifest::PluginAgentConfig;

/// Convert plugin agent configs to AgentConfig, skipping duplicates.
///
/// `existing_names` are agent names already registered (from base config).
/// Returns the list of converted configs and a set of skipped duplicate names.
pub fn resolve_plugin_agents(
    plugin_agents: &[PluginAgentConfig],
    existing_names: &HashSet<String>,
) -> (Vec<AgentConfig>, Vec<String>) {
    let mut agents = Vec::new();
    let mut skipped = Vec::new();

    for pa in plugin_agents {
        if existing_names.contains(&pa.name) {
            warn!(
                agent = %pa.name,
                "Plugin agent name conflicts with existing agent, skipping"
            );
            skipped.push(pa.name.clone());
            continue;
        }
        agents.push(pa.clone().into());
    }

    (agents, skipped)
}

/// Create and register `run_{name}` tools for plugin agents in the tool registry.
///
/// Returns the list of registered tool names for later cleanup.
pub fn register_plugin_agent_tools(
    agents: &[AgentConfig],
    tool_registry: &mut ToolRegistry,
    shared_history: Arc<std::sync::RwLock<String>>,
    proactive_tx: mpsc::Sender<ProactiveEvent>,
    session_manager: Option<Arc<AcpSessionManager>>,
    secondary_llm: Option<Arc<dyn LlmProvider>>,
    hermes_viewer_mode: HermesSessionViewerMode,
) -> Vec<String> {
    let mut registered_names = Vec::new();

    for agent in agents {
        let task_map: Arc<DashMap<String, ActiveTask>> = Arc::new(DashMap::new());
        let mut run_agent_tool = RunAgentTool::new(
            agent.clone(),
            task_map,
            shared_history.clone(),
            proactive_tx.clone(),
        );

        if let Some(ref client) = secondary_llm {
            run_agent_tool = run_agent_tool.with_synthesis(Arc::clone(client));
        }

        if agent.mode == "acp" {
            if let Some(ref mgr) = session_manager {
                run_agent_tool = run_agent_tool.with_session_manager(Arc::clone(mgr));
            }
            run_agent_tool = run_agent_tool.with_hermes_viewer(hermes_viewer_mode);
        }

        let tool_name = run_agent_tool.name().to_string();
        tool_registry.register(run_agent_tool);
        registered_names.push(tool_name);
    }

    registered_names
}

/// Unregister agent tools by name and return the list of successfully removed names.
pub fn unregister_plugin_agent_tools(
    tool_registry: &mut ToolRegistry,
    tool_names: &[String],
) -> Vec<String> {
    let mut removed = Vec::new();
    for name in tool_names {
        if tool_registry.unregister(name) {
            removed.push(name.clone());
        }
    }
    removed
}

/// Build a system prompt section for plugin agents.
///
/// Returns an empty string if no agents are configured.
/// Uses the same format as `AgentRegistry::system_prompt_section()`.
pub fn build_plugin_agent_prompt_section(agents: &[AgentConfig]) -> String {
    if agents.is_empty() {
        return String::new();
    }

    let mut section = String::from(
        "\n\n## AGENTES EXTERNOS DISPONIBLES\n\n\
         Puedes delegar tareas complejas a los siguientes agentes externos.\n\
         Cada agente tiene herramientas propias y especialización.\n\
         Para delegar, llama a la herramienta correspondiente (run_<nombre>) \n\
         con task=\"descripción de la tarea\". El resultado llega de forma proactiva.\n",
    );

    for agent in agents {
        section.push_str(&format!(
            "\n### {display_name} (run_{name})\n\
             Cuándo usar: {when}\n\
             Instrucciones para el agente: {instructions}\n",
            display_name = capitalize(&agent.name),
            name = agent.name,
            when = agent.when_to_use,
            instructions = agent.instructions,
        ));
    }

    section
}

/// Build a combined system prompt section merging base and plugin agent sections.
///
/// If both are non-empty, merges them into a single section.
/// If only one is non-empty, returns that one.
pub fn merge_agent_prompt_sections(base_section: &str, plugin_section: &str) -> String {
    match (base_section.is_empty(), plugin_section.is_empty()) {
        (true, true) => String::new(),
        (false, true) => base_section.to_string(),
        (true, false) => plugin_section.to_string(),
        (false, false) => {
            // Both have content — merge by combining agent entries.
            // The header is shared, so we strip the header from the plugin section
            // and append its agent entries to the base section.
            if let Some(first_header) = plugin_section.find("### ") {
                format!("{}{}", base_section, &plugin_section[first_header..])
            } else {
                format!("{}\n{}", base_section, plugin_section)
            }
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}
