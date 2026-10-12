//! Shared test scaffolding for the worker crate (compiled only for tests).

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use seneschal_common::tools::Tool;
use seneschal_core::llm::session::Message;
use seneschal_core::llm::{LlmProvider, RequestOptions, StreamToken};

/// One scripted `stream()` response.
pub enum MockResponse {
    /// Emit text content tokens, then end the stream.
    Text(String),
    /// Emit reasoning tokens (server reasoning channel), then end the stream.
    Reasoning(String),
    /// Emit a tool call.
    ToolCall { name: String, args: String },
    /// Emit several tool calls back-to-back (parallel calls in one turn).
    ToolCalls(Vec<(String, String)>),
    /// Never yield anything (for timeout/cancel tests).
    Hang,
}

impl MockResponse {
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text(s.into())
    }
    pub fn reasoning(s: impl Into<String>) -> Self {
        Self::Reasoning(s.into())
    }
}

/// Recorded metadata of one `stream()` call.
#[derive(Debug, Clone)]
pub struct RecordedCall {
    pub num_tools: usize,
    pub thinking: Option<bool>,
    pub thinking_with_tools: bool,
}

/// Scripted `LlmProvider` for engine tests: pops one `MockResponse` per
/// `stream()` call and records the request shape.
pub struct MockProvider {
    responses: Mutex<VecDeque<MockResponse>>,
    pub calls: Mutex<Vec<RecordedCall>>,
    /// Messages received by the most recent `stream()` call (for echo asserts).
    pub last_messages: Mutex<Vec<serde_json::Value>>,
}

impl MockProvider {
    pub fn new(responses: Vec<MockResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            calls: Mutex::new(Vec::new()),
            last_messages: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    fn provider_name(&self) -> &'static str {
        "mock"
    }

    async fn stream(
        &self,
        messages: &[serde_json::Value],
        tools: &[serde_json::Value],
        options: RequestOptions,
    ) -> anyhow::Result<(mpsc::Receiver<StreamToken>, tokio::task::JoinHandle<()>)> {
        self.calls.lock().unwrap().push(RecordedCall {
            num_tools: tools.len(),
            thinking: options.thinking,
            thinking_with_tools: options.thinking_with_tools,
        });
        *self.last_messages.lock().unwrap() = messages.to_vec();
        let response = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(MockResponse::Hang);

        let (tx, rx) = mpsc::channel::<StreamToken>(16);
        let handle = tokio::spawn(async move {
            match response {
                MockResponse::Text(s) => {
                    for chunk in s.split("  ").filter(|c| !c.is_empty()) {
                        if tx
                            .send(StreamToken::Content(chunk.to_string()))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                MockResponse::Reasoning(s) => {
                    for chunk in s.split("  ").filter(|c| !c.is_empty()) {
                        if tx
                            .send(StreamToken::Reasoning(chunk.to_string()))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                MockResponse::ToolCall { name, args } => {
                    let _ = tx.send(StreamToken::ToolCall { name, args }).await;
                }
                MockResponse::Hang => {
                    // Hold the channel open without yielding.
                    futures_util_hold().await;
                }
                MockResponse::ToolCalls(calls) => {
                    for (name, args) in calls {
                        if tx.send(StreamToken::ToolCall { name, args }).await.is_err() {
                            return;
                        }
                    }
                }
            }
        });
        Ok((rx, handle))
    }

    async fn complete(&self, _messages: &[Message]) -> anyhow::Result<String> {
        anyhow::bail!("not implemented in MockProvider")
    }

    async fn complete_short(&self, _messages: &[Message]) -> anyhow::Result<String> {
        anyhow::bail!("not implemented in MockProvider")
    }

    async fn complete_multimodal(
        &self,
        _image_data_url: &str,
        _text_prompt: &str,
    ) -> anyhow::Result<String> {
        anyhow::bail!("not implemented in MockProvider")
    }
}

/// Keep the Hang branch alive forever without pulling futures-util as a dep.
async fn futures_util_hold() {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
}

/// A trivial registered tool used to exercise the loop's tool execution.
pub struct NoopTool;

#[async_trait]
impl Tool for NoopTool {
    fn name(&self) -> &str {
        "noop"
    }

    fn description(&self) -> &str {
        "Does nothing, returns its args."
    }

    async fn run(&self, args: &str) -> String {
        format!("noop result: {args}")
    }
}

/// A tool that never finishes (for cancel tests).
pub struct HangingTool;

#[async_trait]
impl Tool for HangingTool {
    fn name(&self) -> &str {
        "hanging"
    }

    fn description(&self) -> &str {
        "Never returns."
    }

    async fn run(&self, _args: &str) -> String {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    }
}

/// A tool that sleeps briefly then returns (timeout tests).
pub struct SlowTool {
    pub secs: u64,
}

#[async_trait]
impl Tool for SlowTool {
    fn name(&self) -> &str {
        "slow"
    }

    fn description(&self) -> &str {
        "Sleeps then returns."
    }

    async fn run(&self, _args: &str) -> String {
        tokio::time::sleep(std::time::Duration::from_secs(self.secs)).await;
        "slow done".to_string()
    }
}

/// Recording `AgentDelegate` for nested-agent tests.
pub struct RecordingDelegate {
    pub calls: Mutex<Vec<(String, String)>>,
    hang_until_cancel: bool,
}

impl RecordingDelegate {
    pub fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            hang_until_cancel: false,
        }
    }

    pub fn hanging() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            hang_until_cancel: true,
        }
    }
}

#[async_trait]
impl crate::agent_delegate::AgentDelegate for RecordingDelegate {
    fn agents_section(&self) -> String {
        "## AGENTES EXTERNOS\n- hermes".to_string()
    }

    async fn delegate(&self, agent: &str, task: &str, cancel: CancellationToken) -> String {
        self.calls
            .lock()
            .unwrap()
            .push((agent.to_string(), task.to_string()));
        if self.hang_until_cancel {
            // Hang until cancelled so cancel-path tests are deterministic.
            tokio::select! {
                _ = cancel.cancelled() => "[Tarea cancelada.]".to_string(),
                _ = tokio::time::sleep(std::time::Duration::from_secs(3600)) => unreachable!(),
            }
        } else {
            format!("[Resultado delegado a {agent}: {task}]")
        }
    }
}
