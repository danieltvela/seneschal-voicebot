//! Deep worker engine: a background tool-calling loop against a secondary,
//! thinking-capable LLM endpoint.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use seneschal_common::config::Config;
use seneschal_common::events::ProactiveEvent;
use seneschal_common::tools::ToolRegistry;
use seneschal_core::llm::{LlmProvider, RequestOptions, StreamToken};

/// How the worker's thinking coexists with tool definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingMode {
    /// Two inferences: a thinking-only plan step (no tools), then a tool loop
    /// without thinking that follows the plan. Default — zero Jinja risk.
    External,
    /// Single loop sending `chat_template_kwargs` alongside tools. Opt-in:
    /// only valid on backends verified to support both (vLLM with
    /// `--enable-reasoning --reasoning-parser`).
    Inline,
}

impl ThinkingMode {
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        match s {
            "external" => Ok(Self::External),
            "inline" => Ok(Self::Inline),
            other => anyhow::bail!(
                "Invalid deep worker thinking mode '{other}' (expected 'external' or 'inline')"
            ),
        }
    }
}

/// Tunables for the engine, built from the `deep_worker_*` config block.
#[derive(Debug, Clone)]
pub struct DeepWorkerParams {
    pub system_prompt: String,
    pub thinking: bool,
    pub thinking_mode: ThinkingMode,
    pub max_iterations: usize,
    pub timeout: Duration,
}

impl DeepWorkerParams {
    pub fn from_config(config: &Config) -> Self {
        Self {
            system_prompt: config.deep_worker_system_prompt.clone(),
            thinking: config.deep_worker_thinking,
            thinking_mode: ThinkingMode::parse(&config.deep_worker_thinking_mode)
                .expect("config validation rejects invalid DEEP_WORKER_THINKING_MODE"),
            max_iterations: config.deep_worker_max_iterations.max(1) as usize,
            timeout: Duration::from_millis(config.deep_worker_timeout_ms.max(1)),
        }
    }
}

/// Progress events emitted by running worker tasks.
///
/// Forwarded by the binary into the ACP `SessionEvent` channel (agent name
/// `deep_worker`) so the existing TUI/Control bridge displays worker rows
/// without new variants.
#[derive(Debug, Clone)]
pub enum WorkerEvent {
    TaskStarted {
        task_id: String,
        task_text: String,
    },
    ToolCall {
        task_id: String,
        tool_name: String,
    },
    ToolResult {
        task_id: String,
        tool_name: String,
        result: String,
    },
    Thinking {
        task_id: String,
        text: String,
    },
    Done {
        task_id: String,
    },
    Failed {
        task_id: String,
        message: String,
    },
}

/// Bookkeeping for an in-flight worker task.
struct WorkerTask {
    cancel: CancellationToken,
    task_text: String,
    created_at: Instant,
}

/// The native deep worker (issue #236).
///
/// Owns a secondary `LlmProvider`, a dedicated `ToolRegistry` (the pool the
/// voice model never sees) and a task map. `spawn_task` returns immediately
/// with a task id; the result is delivered later through the standard
/// `ProactiveEvent::AgentResult` announcement flow (single narration,
/// voice history untouched by the worker's tool exchanges).
pub struct DeepWorkerEngine {
    provider: Arc<dyn LlmProvider>,
    tools: Arc<Mutex<ToolRegistry>>,
    proactive_tx: mpsc::Sender<ProactiveEvent>,
    event_tx: mpsc::Sender<WorkerEvent>,
    params: DeepWorkerParams,
    /// Extra prompt sections (external agents section, plugin sections) —
    /// set/updated by the binary after wiring, read at task start.
    extra_sections: Mutex<String>,
    tasks: Arc<DashMap<String, WorkerTask>>,
}

impl DeepWorkerEngine {
    /// Create the engine. Returns it behind an `Arc` plus the receiver end
    /// of the progress-event channel (consumed once by the binary).
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        tools: Arc<Mutex<ToolRegistry>>,
        proactive_tx: mpsc::Sender<ProactiveEvent>,
        params: DeepWorkerParams,
    ) -> (Arc<Self>, mpsc::Receiver<WorkerEvent>) {
        // Generous capacity: progress events are best-effort (try_send).
        let (event_tx, event_rx) = mpsc::channel::<WorkerEvent>(256);
        let engine = Arc::new(Self {
            provider,
            tools,
            proactive_tx,
            event_tx,
            params,
            extra_sections: Mutex::new(String::new()),
            tasks: Arc::new(DashMap::new()),
        });
        (engine, event_rx)
    }

    /// Append extra prompt sections (agents section, plugin sections).
    pub fn set_extra_sections(&self, sections: String) {
        *self.extra_sections.lock().unwrap() = sections;
    }

    /// Number of currently running tasks.
    pub fn active_count(&self) -> usize {
        self.tasks.len()
    }

    /// Cancel every running task. Returns how many were cancelled.
    pub fn cancel_all(&self) -> usize {
        let mut n = 0;
        for entry in self.tasks.iter() {
            entry.value().cancel.cancel();
            n += 1;
        }
        n
    }

    /// Human-readable status for the inline `status` command.
    pub fn status(&self) -> String {
        let entries: Vec<(String, String, u64)> = self
            .tasks
            .iter()
            .map(|e| {
                (
                    e.key().clone(),
                    e.value().task_text.clone(),
                    e.value().created_at.elapsed().as_secs(),
                )
            })
            .collect();
        if entries.is_empty() {
            "[El deep worker no tiene ninguna tarea activa.]".to_string()
        } else {
            let lines: Vec<String> = entries
                .iter()
                .map(|(id, text, secs)| {
                    let short: String = text.chars().take(60).collect();
                    format!("- {id} ({secs}s): {short}")
                })
                .collect();
            format!(
                "[El deep worker tiene {} tarea(s) activa(s):\n{}]",
                entries.len(),
                lines.join("\n")
            )
        }
    }

    /// Launch a task in the background. Returns the task id immediately.
    pub fn spawn_task(self: &Arc<Self>, task: String) -> String {
        let task_id = Uuid::new_v4().to_string();
        let engine = Arc::clone(self);
        let spawned_task_id = task_id.clone();
        tokio::spawn(async move {
            engine.run_task(spawned_task_id, task).await;
        });
        task_id
    }

    async fn run_task(&self, task_id: String, task: String) {
        let cancel = CancellationToken::new();
        self.tasks.insert(
            task_id.clone(),
            WorkerTask {
                cancel: cancel.clone(),
                task_text: task.clone(),
                created_at: Instant::now(),
            },
        );
        self.emit(WorkerEvent::TaskStarted {
            task_id: task_id.clone(),
            task_text: task.clone(),
        });
        info!(target: "deep_worker", "task started (id={task_id}): {task}");

        // Cancellation semantics (this worker replaces the external
        // sub-agents, so it must match them 1:1): a delegated task is
        // DETACHED from the conversation. Barge-in / new user speech does
        // NOT cancel it — RunAgentTool tasks keep running across chat
        // turns. The only cancellations are the explicit `cancel` inline
        // command (cancel_all), the task timeout below, and process exit.

        let loop_cancel = cancel.child_token();
        let result = match tokio::time::timeout(
            self.params.timeout,
            self.run_loop(&task_id, &task, loop_cancel),
        )
        .await
        {
            Ok(text) => text,
            Err(_) => {
                warn!(target: "deep_worker", "task timed out (id={task_id})");
                format!(
                    "Deep worker timeout: la tarea excedió el límite de {}s.",
                    self.params.timeout.as_secs()
                )
            }
        };

        self.tasks.remove(&task_id);
        let failed = result.starts_with("Deep worker");
        if failed {
            self.emit(WorkerEvent::Failed {
                task_id: task_id.clone(),
                message: result.clone(),
            });
        } else {
            self.emit(WorkerEvent::Done {
                task_id: task_id.clone(),
            });
        }

        info!(target: "deep_worker", "task complete (id={task_id}, {} chars)", result.len());
        // tool_call_id=None → the main loop queues a single announcement
        // (anti-#218) and never persists this into the voice history
        // (anti-#221: the bg_ route already stored the abbreviated exchange).
        let _ = self
            .proactive_tx
            .send(ProactiveEvent::AgentResult {
                task: task.clone(),
                result,
                tool_call_id: None,
                correlation_id: task_id.clone(),
            })
            .await;
    }

    fn emit(&self, event: WorkerEvent) {
        let _ = self.event_tx.try_send(event);
    }

    fn effective_prompt(&self) -> String {
        let extra = self.extra_sections.lock().unwrap().clone();
        if extra.is_empty() {
            self.params.system_prompt.clone()
        } else {
            format!("{}\n\n{}", self.params.system_prompt, extra)
        }
    }

    /// The tool-calling loop. Returns the final answer text, a
    /// "[Tarea cancelada.]" marker, or a "Deep worker …" error string.
    async fn run_loop(&self, task_id: &str, task: &str, cancel: CancellationToken) -> String {
        let system = serde_json::json!({"role": "system", "content": self.effective_prompt()});
        let mut messages = vec![
            system.clone(),
            serde_json::json!({"role": "user", "content": task}),
        ];

        // External thinking: plan first (one-shot, no tools, thinking on),
        // then run the tool loop without thinking following the plan. This
        // rides around the thinking/tools Jinja conflict entirely.
        if self.params.thinking
            && self.params.thinking_mode == ThinkingMode::External
            && let Some(plan) = self.plan_step(&system, task, &cancel).await
            && !plan.is_empty()
        {
            self.emit(WorkerEvent::Thinking {
                task_id: task_id.to_string(),
                text: plan.clone(),
            });
            messages.push(serde_json::json!({
                "role": "assistant",
                "content": format!("[Plan de trabajo]\n{plan}")
            }));
        }

        let tool_defs = self.tools.lock().unwrap().tool_definitions();
        let max = self.params.max_iterations;

        for iter in 0..max {
            if cancel.is_cancelled() {
                return "[Tarea cancelada.]".to_string();
            }

            // Last iteration: no tools so the model must produce a final answer.
            let is_last = iter + 1 >= max;
            let tools_this: Vec<serde_json::Value> = if is_last {
                Vec::new()
            } else {
                tool_defs.clone()
            };

            let mut options = RequestOptions::new().with_thinking(
                self.params.thinking && self.params.thinking_mode == ThinkingMode::Inline,
            );
            if self.params.thinking_mode == ThinkingMode::Inline {
                options = options.with_thinking_with_tools();
            }

            let (mut token_rx, stream_handle) =
                match self.provider.stream(&messages, &tools_this, options).await {
                    Ok(r) => r,
                    Err(e) => {
                        return format!(
                            "Deep worker error: no se pudo conectar con el LLM del worker: {e}"
                        );
                    }
                };

            let mut llm_text = String::new();
            // Parallel tool calls arrive as separate tokens; collect the
            // whole turn (the stream closes right after finish_reason) so
            // every requested call runs and is echoed. Echoing only part of
            // the model's call list — or call arguments mangled by the
            // parser — makes strict servers reject the replay with 400.
            let mut tool_calls: Vec<(String, String)> = Vec::new();
            let cancelled = loop {
                tokio::select! {
                    _ = cancel.cancelled() => break true,
                    tok = token_rx.recv() => match tok {
                        Some(StreamToken::Content(t)) => { llm_text.push_str(&t); }
                        Some(StreamToken::Reasoning(_)) => {
                            // Reasoning deltas are progress, not answer text.
                        }
                        Some(StreamToken::ToolCall { name, args }) => {
                            tool_calls.push((name, args));
                        }
                        None => break false,
                    }
                }
            };

            if cancelled {
                stream_handle.abort();
                return "[Tarea cancelada.]".to_string();
            }

            if tool_calls.is_empty() {
                let text = llm_text.trim().to_string();
                if !text.is_empty() {
                    return text;
                }
                // Empty answer without a tool call: keep looping (the model
                // returned nothing); the forced final iteration exits below.
                continue;
            }

            let mut tool_results: Vec<String> = Vec::new();

            // Execute every requested call sequentially (shared tool pool),
            // then echo one assistant message with the full call list plus
            // one tool result per call.
            for (name, args) in &tool_calls {
                self.emit(WorkerEvent::ToolCall {
                    task_id: task_id.to_string(),
                    tool_name: name.clone(),
                });
                let tool = self.tools.lock().unwrap().get_tool_arc(name);
                let result = match tool {
                    Some(t) => {
                        tokio::select! {
                            _ = cancel.cancelled() => return "[Tarea cancelada.]".to_string(),
                            r = t.run_with_cancel(args, cancel.clone()) => r,
                        }
                    }
                    None => format!("Unknown tool: {name}"),
                };
                self.emit(WorkerEvent::ToolResult {
                    task_id: task_id.to_string(),
                    tool_name: name.clone(),
                    result: result.clone(),
                });
                tool_results.push(result);
            }

            let content_value = if llm_text.trim().is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(llm_text.clone())
            };
            let mut assistant_msg = serde_json::json!({
                "role": "assistant",
                "content": content_value,
            });
            let echoed: Vec<serde_json::Value> = tool_calls
                .iter()
                .enumerate()
                .map(|(ci, (name, args))| {
                    serde_json::json!({
                        "id": format!("dw_{}_{}_{}", &task_id[..8.min(task_id.len())], iter, ci),
                        "type": "function",
                        "function": {"name": name, "arguments": args}
                    })
                })
                .collect();
            assistant_msg["tool_calls"] = serde_json::json!(echoed);
            messages.push(assistant_msg);
            for (ci, result) in tool_results.into_iter().enumerate() {
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": format!("dw_{}_{}_{}", &task_id[..8.min(task_id.len())], iter, ci),
                    "content": result
                }));
            }
        }

        format!("Deep worker error: el bucle terminó sin respuesta final tras {max} iteraciones.")
    }

    /// External-thinking plan step: one-shot, no tools, thinking on.
    /// Tolerant — on error the loop simply proceeds without a plan.
    async fn plan_step(
        &self,
        system: &serde_json::Value,
        task: &str,
        cancel: &CancellationToken,
    ) -> Option<String> {
        const PLAN_INSTRUCTION: &str = "\n\nAntes de ejecutar, elabora un plan de trabajo breve y operativo (máximo 10 líneas): pasos concretos, qué herramienta usar en cada uno, y el orden. Responde SOLO el plan.";
        let messages = vec![
            system.clone(),
            serde_json::json!({"role": "user", "content": format!("{task}{PLAN_INSTRUCTION}")}),
        ];
        let (mut token_rx, stream_handle) = self
            .provider
            .stream(&messages, &[], RequestOptions::new().with_thinking(true))
            .await
            .map_err(|e| {
                warn!(target: "deep_worker", "plan step failed, continuing without plan: {e}");
                e
            })
            .ok()?;

        let mut plan = String::new();
        let mut reasoning = String::new();
        let cancelled = loop {
            tokio::select! {
                _ = cancel.cancelled() => break true,
                tok = token_rx.recv() => match tok {
                    Some(StreamToken::Content(t)) => { plan.push_str(&t); }
                    Some(StreamToken::Reasoning(t)) => { reasoning.push_str(&t); }
                    Some(StreamToken::ToolCall { .. }) | None => break false,
                }
            }
        };
        if cancelled {
            stream_handle.abort();
            return None;
        }
        // Servers with a reasoning parser (SGLang/vLLM) deliver the plan on
        // `reasoning_content` with empty `content`: fall back to it.
        if plan.trim().is_empty() {
            Some(reasoning.trim().to_string())
        } else {
            Some(plan.trim().to_string())
        }
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
    use crate::tests_support::{HangingTool, MockProvider, MockResponse, NoopTool, SlowTool};

    fn params(thinking: bool, mode: ThinkingMode, max_iterations: usize) -> DeepWorkerParams {
        DeepWorkerParams {
            system_prompt: "worker test prompt".to_string(),
            thinking,
            thinking_mode: mode,
            max_iterations,
            timeout: Duration::from_secs(10),
        }
    }

    fn build(
        mock: MockProvider,
        registry: ToolRegistry,
        params: DeepWorkerParams,
    ) -> (
        Arc<DeepWorkerEngine>,
        mpsc::Receiver<ProactiveEvent>,
        Arc<MockProvider>,
    ) {
        let provider = Arc::new(mock);
        let provider_clone: Arc<MockProvider> = Arc::clone(&provider);
        let provider_dyn: Arc<dyn LlmProvider> = provider;
        let tools = Arc::new(Mutex::new(registry));
        let (proactive_tx, proactive_rx) = mpsc::channel::<ProactiveEvent>(8);
        let (engine, _event_rx) = DeepWorkerEngine::new(provider_dyn, tools, proactive_tx, params);
        (engine, proactive_rx, provider_clone)
    }

    /// Wait for the task's AgentResult on the proactive channel.
    async fn next_result(rx: &mut mpsc::Receiver<ProactiveEvent>) -> (String, String, String) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let ev = tokio::time::timeout_at(deadline, rx.recv())
                .await
                .expect("timed out waiting for AgentResult")
                .expect("channel closed");
            match ev {
                ProactiveEvent::AgentResult {
                    task,
                    result,
                    tool_call_id,
                    correlation_id,
                } => {
                    assert!(
                        tool_call_id.is_none(),
                        "worker results carry no tool_call_id"
                    );
                    return (task, result, correlation_id);
                }
                _ => continue,
            }
        }
    }

    #[tokio::test]
    async fn external_mode_two_step_then_final() {
        // plan (call 0: no tools, thinking on) → tool call (1) → final (2).
        let mock = MockProvider::new(vec![
            MockResponse::text("Plan: 1) leer fichero 2) resumir"),
            MockResponse::ToolCall {
                name: "noop".to_string(),
                args: r#"{"x": 1}"#.to_string(),
            },
            MockResponse::text("Resumen final"),
        ]);
        let mut registry = ToolRegistry::new();
        registry.register(NoopTool);
        let (engine, mut rx, provider) =
            build(mock, registry, params(true, ThinkingMode::External, 5));

        let id = engine.spawn_task("analiza el fichero".to_string());
        let (task, result, corr) = next_result(&mut rx).await;
        assert_eq!(task, "analiza el fichero");
        assert_eq!(result, "Resumen final");
        assert_eq!(corr, id);

        let calls = provider.calls.lock().unwrap();
        assert_eq!(calls.len(), 3, "plan + tool round + final: {calls:?}");
        // Plan step: no tools, thinking requested.
        assert_eq!(calls[0].num_tools, 0);
        assert_eq!(calls[0].thinking, Some(true));
        assert!(!calls[0].thinking_with_tools);
        // Tool loop: tools present, thinking OFF, no kwargs flag (external).
        assert!(calls[1].num_tools >= 1);
        assert_eq!(calls[1].thinking, Some(false));
        assert!(!calls[1].thinking_with_tools);
        // Final round after the tool result: still tools present (not last iter).
        assert!(calls[2].num_tools >= 1);
    }

    #[tokio::test]
    async fn parallel_tool_calls_all_executed_and_echoed() {
        // One turn, two calls: BOTH must run and BOTH must be echoed on the
        // next request — the assistant tool_calls + tool results must match
        // what the model asked for, or strict servers 400 the replay.
        let mock = MockProvider::new(vec![
            MockResponse::ToolCalls(vec![
                ("noop".to_string(), r#"{"a":1}"#.to_string()),
                ("noop".to_string(), r#"{"b":2}"#.to_string()),
            ]),
            MockResponse::text("final"),
        ]);
        let mut registry = ToolRegistry::new();
        registry.register(NoopTool);
        let (engine, mut rx, provider) =
            build(mock, registry, params(false, ThinkingMode::External, 5));

        engine.spawn_task("doble llamada".to_string());
        let (_, result, _) = next_result(&mut rx).await;
        assert_eq!(result, "final");

        let msgs = provider.last_messages.lock().unwrap();
        let assistant = msgs
            .iter()
            .find(|m| m["tool_calls"].is_array())
            .expect("assistant echo with tool_calls");
        assert_eq!(assistant["tool_calls"].as_array().unwrap().len(), 2);
        let tool_msgs: Vec<&serde_json::Value> =
            msgs.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 2);
        assert!(
            tool_msgs[0]["content"]
                .as_str()
                .unwrap()
                .contains(r#""a":1"#)
        );
        assert!(
            tool_msgs[1]["content"]
                .as_str()
                .unwrap()
                .contains(r#""b":2"#)
        );
        assert_ne!(
            tool_msgs[0]["tool_call_id"], tool_msgs[1]["tool_call_id"],
            "each call needs its own id"
        );
    }

    #[tokio::test]
    async fn inline_mode_sends_thinking_with_tools() {
        let mock = MockProvider::new(vec![MockResponse::text("respuesta")]);
        let (engine, mut rx, provider) = build(
            mock,
            ToolRegistry::new(),
            params(true, ThinkingMode::Inline, 3),
        );

        engine.spawn_task("tarea".to_string());
        let (_, result, _) = next_result(&mut rx).await;
        assert_eq!(result, "respuesta");

        let calls = provider.calls.lock().unwrap();
        // Inline: single loop, no plan step.
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert!(
            calls[0].thinking_with_tools,
            "inline must request kwargs with tools"
        );
        assert_eq!(calls[0].thinking, Some(true));
        assert_eq!(calls[0].num_tools, 0, "empty registry → no tool defs");
    }

    #[tokio::test]
    async fn last_iteration_forces_final_without_tools() {
        // max_iterations=2: iter 1 is last → tools stripped.
        let mock = MockProvider::new(vec![
            MockResponse::ToolCall {
                name: "noop".to_string(),
                args: "{}".to_string(),
            },
            MockResponse::text("final forzado"),
        ]);
        let mut registry = ToolRegistry::new();
        registry.register(NoopTool);
        let (engine, mut rx, provider) =
            build(mock, registry, params(false, ThinkingMode::External, 2));

        engine.spawn_task("t".to_string());
        let (_, result, _) = next_result(&mut rx).await;
        assert_eq!(result, "final forzado");

        let calls = provider.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].num_tools, 0, "last iteration must strip tools");
    }

    #[tokio::test]
    async fn exhausted_iterations_returns_error() {
        // Model keeps calling tools until the loop runs out.
        let mock = MockProvider::new(vec![
            MockResponse::ToolCall {
                name: "noop".into(),
                args: "{}".into(),
            },
            MockResponse::ToolCall {
                name: "noop".into(),
                args: "{}".into(),
            },
        ]);
        let mut registry = ToolRegistry::new();
        registry.register(NoopTool);
        let (engine, mut rx, _p) = build(mock, registry, params(false, ThinkingMode::External, 2));

        engine.spawn_task("t".to_string());
        let (_, result, _) = next_result(&mut rx).await;
        assert!(result.starts_with("Deep worker error"), "{result:?}");
    }

    #[tokio::test]
    async fn timeout_returns_marker() {
        // The stream never yields and the tool loop hangs: bounded by a tiny timeout.
        let mock = MockProvider::new(vec![MockResponse::Hang]);
        let mut p = params(false, ThinkingMode::External, 5);
        p.timeout = Duration::from_millis(150);
        let (engine, mut rx, _prov) = build(mock, ToolRegistry::new(), p);

        engine.spawn_task("tarea lenta".to_string());
        let (_, result, _) = next_result(&mut rx).await;
        assert!(result.starts_with("Deep worker timeout"), "{result:?}");
    }

    #[tokio::test]
    async fn cancel_all_aborts_mid_tool() {
        // The model calls a hanging tool; cancel_all must abort the loop.
        let mock = MockProvider::new(vec![MockResponse::ToolCall {
            name: "hanging".to_string(),
            args: "{}".to_string(),
        }]);
        let mut registry = ToolRegistry::new();
        registry.register(HangingTool);
        let (engine, mut rx, _p) = build(mock, registry, params(false, ThinkingMode::External, 5));

        let id = engine.spawn_task("bloqueante".to_string());
        // Wait until the tool is actually running (status shows 1 task).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while engine.active_count() == 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Give the loop a moment to enter the hanging tool, then cancel.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let n = engine.cancel_all();
        assert_eq!(n, 1);

        let (_, result, corr) = next_result(&mut rx).await;
        assert_eq!(result, "[Tarea cancelada.]");
        assert_eq!(corr, id);
        assert_eq!(engine.active_count(), 0, "task must be cleaned up");
    }

    #[tokio::test]
    async fn delegated_tasks_are_detached_from_conversation() {
        // External sub-agents are never cancelled by the conversation; the
        // deep worker must behave identically. Concurrent delegations
        // coexist and nothing in the conversation path aborts them — the
        // only cancel sources are cancel_all (tested separately) and the
        // task timeout.
        let mock = MockProvider::new(vec![MockResponse::Hang, MockResponse::Hang]);
        let mut p = params(false, ThinkingMode::External, 5);
        p.timeout = Duration::from_secs(60);
        let (engine, _rx, _prov) = build(mock, ToolRegistry::new(), p);

        engine.spawn_task("tarea A".to_string());
        engine.spawn_task("tarea B".to_string());
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            engine.active_count(),
            2,
            "concurrent delegations must coexist — no implicit cancellation"
        );
        engine.cancel_all();
    }

    #[tokio::test]
    async fn extra_sections_appended_to_prompt() {
        // The final answer echoes the prompt? Simpler: assert via messages is
        // not exposed — use the section text as the scripted answer carrier:
        // the MockProvider ignores messages, so instead verify the effective
        // prompt directly.
        let mock = MockProvider::new(vec![]);
        let (engine, _rx, _p) = build(
            mock,
            ToolRegistry::new(),
            params(false, ThinkingMode::External, 5),
        );
        engine.set_extra_sections("## AGENTES\n- hermes".to_string());
        let prompt = engine.effective_prompt();
        assert!(prompt.contains("worker test prompt"));
        assert!(prompt.contains("## AGENTES"));
    }

    #[tokio::test]
    async fn status_and_slow_tool_flow() {
        // Happy path with a slow-but-finite tool: result flows normally.
        let mock = MockProvider::new(vec![
            MockResponse::ToolCall {
                name: "slow".into(),
                args: "{}".into(),
            },
            MockResponse::text("resultado tras la tool"),
        ]);
        let mut registry = ToolRegistry::new();
        registry.register(SlowTool { secs: 0 });
        let (engine, mut rx, _p) = build(mock, registry, params(false, ThinkingMode::External, 5));

        let id = engine.spawn_task("tarea".to_string());
        let (_, result, corr) = next_result(&mut rx).await;
        assert_eq!(result, "resultado tras la tool");
        assert_eq!(corr, id);
        assert!(engine.status().contains("ninguna tarea activa"));
    }

    #[test]
    fn thinking_mode_parse() {
        assert_eq!(
            ThinkingMode::parse("external").unwrap(),
            ThinkingMode::External
        );
        assert_eq!(ThinkingMode::parse("inline").unwrap(), ThinkingMode::Inline);
        assert!(ThinkingMode::parse("bogus").is_err());
    }

    #[tokio::test]
    async fn plan_step_consumes_reasoning_channel() {
        // SGLang/vLLM reasoning parsers deliver the plan on `reasoning_content`
        // with empty content: the plan must reach the loop (Thinking event)
        // instead of being dropped by the SSE reader.
        let mock = MockProvider::new(vec![
            MockResponse::reasoning("1) listar  2) resumir"),
            MockResponse::text("final"),
        ]);
        let provider: Arc<dyn LlmProvider> = Arc::new(mock);
        let tools = Arc::new(Mutex::new(ToolRegistry::new()));
        let (proactive_tx, mut rx) = mpsc::channel::<ProactiveEvent>(8);
        let (engine, mut event_rx) = DeepWorkerEngine::new(
            provider,
            tools,
            proactive_tx,
            params(true, ThinkingMode::External, 3),
        );
        engine.spawn_task("tarea".to_string());

        let mut saw_plan = false;
        while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(5), event_rx.recv()).await
        {
            match ev {
                WorkerEvent::Thinking { text, .. } => {
                    assert!(text.contains("listar"), "{text}");
                    saw_plan = true;
                }
                WorkerEvent::Done { .. } => break,
                WorkerEvent::Failed { message, .. } => panic!("unexpected failure: {message}"),
                _ => continue,
            }
        }
        assert!(saw_plan, "reasoning-only plan must emit a Thinking event");
        let (_, result, _) = next_result(&mut rx).await;
        assert_eq!(result, "final");
    }
}
