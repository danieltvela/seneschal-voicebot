//! # seneschal-worker — Native Deep Worker (issue #236)
//!
//! Heavy subagent running inside the Seneschal process with its own LLM
//! endpoint (thinking-capable), its own tool-calling loop, and its own tool
//! pool (MCP, agents, local management tools). The voice model delegates via
//! the `run_deep_task` tool and receives the result asynchronously through
//! the existing `ProactiveEvent::AgentResult` → announcement flow.
//!
//! Design contracts (see `secondary-agent.md`, issues #218/#221):
//! - The voice model never sees the worker's tool-calling: its history only
//!   keeps the abbreviated delegation exchange.
//! - The worker never speaks: results are narrated by the voice model.
//! - A single result announcement per task.

pub mod agent_delegate;
pub mod engine;
pub mod run_deep_task;

#[cfg(test)]
mod tests_support;

pub use agent_delegate::{AgentDelegate, RunAgentNestedTool};
pub use engine::{DeepWorkerEngine, DeepWorkerParams, ThinkingMode, WorkerEvent};
pub use run_deep_task::RunDeepTaskTool;

/// Agent name under which the worker surfaces in session events / TUI rows.
pub const WORKER_AGENT_NAME: &str = "deep_worker";
