// Imports used only in test code (AcpWriter was extracted to common).
// Suppress warnings in non-test builds.
#![cfg_attr(not(test), allow(unused_imports))]
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use dashmap::DashMap;
use serde_json::Value;
use tokio::process::Command;
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{debug, info, warn};
use uuid::Uuid;

use seneschal_agents::agent_session::VisibleSessionManager;
use seneschal_agents::session_manager::PREWARM_DRAIN_TIMEOUT_SECS;
use seneschal_agents::{
    AcpSessionManager, AgentConfig, HttpAgentTransport, OpenCodeHttpTransport, SessionEvent,
    SessionEventTx,
};
use seneschal_common::events::ProactiveEvent;
use seneschal_common::permission::PermissionGate;
use seneschal_common::tools::Tool;

// Re-imported for test code (AcpWriter was extracted to common)
use seneschal_common::acp_writer::{jsonrpc_notification, jsonrpc_request, parse_jsonrpc};

// ── Subprocess helper ─────────────────────────────────────────────────────────

/// Spawns the agent CLI passing `query` via the `-q` flag.
/// Reads the complete stdout as the response.
///
/// Command construction: `{command_parts...} -q {query}`
/// e.g. AGENT_COMMAND=`hermes chat` → `hermes chat -Q -q "..."`
pub(crate) async fn call_agent(command: String, query: String) -> String {
    let parts: Vec<String> = command.split_whitespace().map(String::from).collect();
    let program = match parts.first() {
        Some(p) => p.clone(),
        None => return "Agent error: AGENT_COMMAND is empty.".to_string(),
    };
    let mut args: Vec<String> = parts[1..].to_vec();
    args.push("-Q".to_string()); // quiet: suppress banner, spinner, tool previews
    args.push("-q".to_string());
    args.push(query);

    let child = match Command::new(&program)
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to spawn agent '{}': {}", program, e);
            return format!("Agent error: failed to launch '{}': {}", program, e);
        }
    };

    match child.wait_with_output().await {
        Ok(output) => {
            let raw = String::from_utf8_lossy(&output.stdout).to_string();
            let text = strip_hermes_cli_noise(&raw);
            if text.is_empty() {
                "Agent completed with no output.".to_string()
            } else {
                text
            }
        }
        Err(e) => {
            warn!("Agent process error: {}", e);
            format!("Agent error: {}", e)
        }
    }
}

/// Strip structural lines Hermes emits even in quiet mode:
///   - Box borders: lines whose trimmed content starts with ╭, ╰, or │
///   - Session trailer: lines starting with "session_id:"
///
/// Everything else is kept; leading/trailing whitespace is removed.
fn strip_hermes_cli_noise(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();

    let start = lines
        .iter()
        .position(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('╭') && !t.starts_with('╰') && !t.starts_with('│')
        })
        .unwrap_or(0);

    let end = lines
        .iter()
        .rposition(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with("session_id:")
        })
        .map(|i| i + 1)
        .unwrap_or(lines.len());

    if start >= end {
        return String::new();
    }
    lines[start..end].join("\n").trim().to_string()
}

// ── JSON-RPC 2.0 helpers (now in seneschal-common) ──────────────────────────

// ── RunAgentTool ──────────────────────────────────────────────────────────────

/// Unified agent delegation tool.
///
/// Supports four modes (selected by the `config.mode` field):
/// - `"cli"` — spawns the agent as a one-shot CLI subprocess (fire-and-forget).
/// - `"acp"` — maintains a persistent ACP subprocess via JSON-RPC 2.0 over stdio.
/// - `"remote"` — connects to an OpenCode HTTP server for prompt submission.
/// - `"visible"` — spawns the agent in a PTY with a visible Terminal window.
///
/// Additionally handles two inline commands that require no subprocess:
/// - `run_<name>: cancel` — cancels the currently running ACP task.
/// - `run_<name>: status` — reports whether the ACP agent is busy.
pub struct RunAgentTool {
    config: AgentConfig,
    task_map: Arc<DashMap<String, ActiveTask>>,
    proactive_tx: mpsc::Sender<ProactiveEvent>,
    session_manager: Option<Arc<AcpSessionManager>>,
    opencode_transport: Option<Arc<OpenCodeHttpTransport>>,
    /// Shared permission gate. When set, any pending permission slots for the
    /// task's scope are cancelled when the ACP prompt finishes (success,
    /// cancel, or error). Issue #167.
    permission_gate: Option<Arc<PermissionGate>>,
    /// Manager for visible (PTY-based) agent sessions.
    visible_manager: Option<Arc<VisibleSessionManager>>,
    /// Directory for visible session log files.
    session_dir: String,
    tool_name: OnceLock<&'static str>,
    /// Resolved tool description (config.description or built-in fallback).
    tool_description: String,
    /// Resolved task parameter description (config.task_description or built-in fallback).
    task_description: String,
}

impl RunAgentTool {
    pub fn new(
        config: AgentConfig,
        task_map: Arc<DashMap<String, ActiveTask>>,
        proactive_tx: mpsc::Sender<ProactiveEvent>,
    ) -> Self {
        let tool_description = if config.description.is_empty() {
            "Delegates a task to an external agent for execution. \
             Use when the user asks for web search, complex reasoning, \
             or file system actions. The result arrives asynchronously."
                .to_string()
        } else {
            config.description.clone()
        };
        let task_description = if config.task_description.is_empty() {
            "The task to delegate".to_string()
        } else {
            config.task_description.clone()
        };
        Self {
            config,
            task_map,
            proactive_tx,
            session_manager: None,
            opencode_transport: None,
            permission_gate: None,
            visible_manager: None,
            session_dir: "/tmp/seneschal_sessions".to_string(),
            tool_name: OnceLock::new(),
            tool_description,
            task_description,
        }
    }

    /// Attach the shared permission gate. When set, the gate cancels any
    /// pending permission slots for the task's scope at the end of every
    /// ACP prompt (so a finishing turn never leaves a permission hanging).
    /// Issue #167.
    pub fn with_permission_gate(mut self, gate: Arc<PermissionGate>) -> Self {
        self.permission_gate = Some(gate);
        self
    }

    /// Attach an optional session manager for persistent ACP sessions.
    pub fn with_session_manager(mut self, mgr: Arc<AcpSessionManager>) -> Self {
        self.session_manager = Some(mgr);
        self
    }

    /// Attach an OpenCode HTTP transport for remote mode.
    pub fn with_opencode_transport(mut self, transport: Arc<OpenCodeHttpTransport>) -> Self {
        self.opencode_transport = Some(transport);
        self
    }

    /// Attach a visible session manager for PTY-based (visible) agent mode.
    pub fn with_visible_manager(mut self, mgr: Arc<VisibleSessionManager>) -> Self {
        self.visible_manager = Some(mgr);
        self
    }

    /// Set the directory for visible session log files.
    pub fn with_session_dir(mut self, dir: String) -> Self {
        self.session_dir = dir;
        self
    }

    /// Cancel the in-flight ACP task, if any.
    async fn cancel(&self) -> String {
        // Clone keys first to avoid holding read lock during remove (deadlock)
        let keys: Vec<String> = self.task_map.iter().map(|e| e.key().clone()).collect();
        if keys.len() == 1 {
            let task_id = keys[0].clone();
            if let Some((_k, active)) = self.task_map.remove(&task_id) {
                let _ = active.cancel_handle.send(());
            }
            info!(target: "agent", "RunAgentTool(acp): task cancelled: {}", task_id);
            "[Tarea cancelada.]".to_string()
        } else if keys.is_empty() {
            "[No hay ninguna tarea en curso para cancelar.]".to_string()
        } else {
            format!(
                "[Hay {} tareas activas: {}. Cancela con el ID específico si es necesario.]",
                keys.len(),
                keys.join(", ")
            )
        }
    }

    /// Report whether the ACP agent is currently busy.
    async fn status(&self) -> String {
        // Clone data to avoid holding read lock
        let entries: Vec<(String, std::time::Instant)> = self
            .task_map
            .iter()
            .map(|e| (e.key().clone(), e.value().created_at))
            .collect();
        if entries.is_empty() {
            "[El agente no tiene ninguna tarea activa.]".to_string()
        } else {
            let tasks: Vec<String> = entries
                .iter()
                .map(|(id, created)| format!("- {} ({}s)", id, created.elapsed().as_secs()))
                .collect();
            format!(
                "[El agente tiene {} tarea(s) activa(s):\n{}]",
                entries.len(),
                tasks.join("\n")
            )
        }
    }

    /// Remote mode: submit prompt to OpenCode or Hermes HTTP server, deliver result proactively.
    async fn run_remote(&self, task: String) -> String {
        let transport = match &self.opencode_transport {
            Some(t) => Arc::clone(t),
            None => return "Error: Remote transport not configured.".to_string(),
        };

        if transport.is_hermes() {
            return self.run_remote_hermes(transport, task).await;
        }

        // ── OpenCode mode ─────────────────────────────────────────────────
        let query = build_agent_query(&task, &self.config.prompt);
        let proactive_tx = self.proactive_tx.clone();
        let agent_name = self.config.name.clone();

        tokio::spawn(async move {
            info!(target: "opencode", "RunAgentTool(remote): task started: {:?}", task);

            // Get or create session
            let session = match transport.get_or_create_session().await {
                Ok(s) => s,
                Err(e) => {
                    warn!(target: "opencode", "Session creation failed: {e}");
                    let _ = proactive_tx
                        .send(ProactiveEvent::AgentResult {
                            task,
                            result: format!("OpenCode session error: {e}"),
                            tool_call_id: None,
                            correlation_id: String::new(),
                        })
                        .await;
                    return;
                }
            };

            // ── Subscribe to SSE events for milestone narration ────────────────
            let (mut milestone_rx, sse_cancel) = transport.subscribe_events(&session.session_id);

            // ── Submit prompt (runs in parallel with SSE subscriber) ──────────
            let cancel = transport.cancellation_token();
            let submit_handle = tokio::spawn({
                let transport = Arc::clone(&transport);
                let session_id = session.session_id.clone();
                let query = query.clone();
                async move { transport.submit_prompt(&session_id, &query, cancel).await }
            });

            // ── Forward milestones while prompt runs ──────────────────────────
            let milestone_tx = proactive_tx.clone();
            let milestone_task = tokio::spawn(async move {
                while let Some(ms) = milestone_rx.recv().await {
                    let _ = milestone_tx
                        .send(ProactiveEvent::AgentMilestone {
                            agent_name: agent_name.clone(),
                            milestone: ms.milestone,
                            correlation_id: ms.correlation_id,
                        })
                        .await;
                }
            });

            // ── Wait for prompt submission result ─────────────────────────────
            let result = match submit_handle.await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    warn!(target: "opencode", "Prompt submission failed: {e}");
                    format!("OpenCode error: {e}")
                }
                Err(e) => {
                    warn!(target: "opencode", "Prompt task panicked: {e}");
                    format!("OpenCode internal error: {e}")
                }
            };

            // Cancel SSE subscriber and abort milestone forwarding
            sse_cancel.cancel();
            milestone_task.abort();

            info!(target: "opencode", "RunAgentTool(remote): task complete ({} chars)", result.len());

            if proactive_tx
                .send(ProactiveEvent::AgentResult {
                    task,
                    result,
                    tool_call_id: None,
                    correlation_id: String::new(),
                })
                .await
                .is_err()
            {
                warn!(
                    "RunAgentTool(remote): failed to deliver agent result: main loop channel closed"
                );
            }
        });

        "[Tarea delegada al agente remoto. El resultado llegará en breve.]".to_string()
    }

    /// Hermes remote mode: submit prompt via Hermes protocol
    /// (POST /v1/runs with {"input": prompt}, per-run events, cancel via POST /v1/runs/{id}/stop).
    async fn run_remote_hermes(&self, transport: Arc<HttpAgentTransport>, task: String) -> String {
        let query = build_agent_query(&task, &self.config.prompt);
        let proactive_tx = self.proactive_tx.clone();
        let agent_name = self.config.name.clone();
        tokio::spawn(async move {
            info!(target: "hermes", "RunAgentTool(hermes): task started: {:?}", task);

            // ── Submit prompt creates a new run ─────────────────────────────
            let cancel = transport.cancellation_token();
            let submit_transport: Arc<HttpAgentTransport> = Arc::clone(&transport);
            let query_submit = query.clone();

            let submit_handle = tokio::spawn(async move {
                // For Hermes we pass an empty session_id — the run is created
                // by submit_prompt using session_create_path (/v1/runs).
                submit_transport
                    .submit_prompt("", &query_submit, cancel)
                    .await
            });

            // ── Subscribe to SSE events once the run_id is available ─────────
            let event_transport: Arc<HttpAgentTransport> = Arc::clone(&transport);
            let event_agent_name = agent_name.clone();
            let event_proactive_tx = proactive_tx.clone();
            let event_task = tokio::spawn(async move {
                // Poll for the run_id to become available (Hermes creates it)
                let run_id = loop {
                    if let Some(rid) = event_transport.get_last_run_id().await {
                        break rid;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                };

                let (mut milestone_rx, cancel_token) =
                    event_transport.subscribe_hermes_events(&run_id);

                while let Some(ms) = milestone_rx.recv().await {
                    let _ = event_proactive_tx
                        .send(ProactiveEvent::AgentMilestone {
                            agent_name: event_agent_name.clone(),
                            milestone: ms.milestone,
                            correlation_id: ms.correlation_id,
                        })
                        .await;
                }

                cancel_token.cancel();
            });

            // ── Wait for prompt submission result ─────────────────────────────
            let result = match submit_handle.await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    warn!(target: "hermes", "Hermes prompt submission failed: {e}");
                    // Cancel the run if submission failed
                    if let Some(run_id) = transport.get_last_run_id().await.as_deref() {
                        let _ = transport.cancel_run(run_id).await;
                    }
                    format!("Hermes error: {e}")
                }
                Err(e) => {
                    warn!(target: "hermes", "Hermes prompt task panicked: {e}");
                    format!("Hermes internal error: {e}")
                }
            };

            // Cancel the event subscriber
            event_task.abort();

            info!(target: "hermes", "RunAgentTool(hermes): task complete ({} chars)", result.len());

            if proactive_tx
                .send(ProactiveEvent::AgentResult {
                    task,
                    result,
                    tool_call_id: None,
                    correlation_id: String::new(),
                })
                .await
                .is_err()
            {
                warn!(
                    "RunAgentTool(hermes): failed to deliver agent result: main loop channel closed"
                );
            }
        });

        "[Tarea delegada al agente remoto (Hermes). El resultado llegará en breve.]".to_string()
    }

    /// Visible mode: send prompt to a PTY-based visible agent session and
    /// poll for output. The user can watch the agent in a Terminal window.
    async fn run_visible(&self, task: String) -> String {
        let command = match &self.config.command {
            Some(c) => c.clone(),
            None => return "Error: Visible agent command not configured.".to_string(),
        };
        let query = build_agent_query(&task, &self.config.prompt);
        let proactive_tx = self.proactive_tx.clone();
        let visible_mgr = match &self.visible_manager {
            Some(m) => Arc::clone(m),
            None => return "Error: Visible session manager not configured.".to_string(),
        };
        let agent_name = self.config.name.clone();
        let session_dir = self.session_dir.clone();

        let handle = tokio::runtime::Handle::current();

        tokio::task::spawn_blocking(move || {
            info!(target: "agent", "RunAgentTool(visible): task started: {:?}", task);

            // Get or create visible session
            let session = match visible_mgr.get_or_create(&agent_name, &command, &session_dir) {
                Ok(s) => s,
                Err(e) => {
                    warn!(target: "agent", "Failed to create visible session: {e}");
                    let _ = handle.block_on(proactive_tx.send(ProactiveEvent::AgentResult {
                        task,
                        result: format!("Visible session error: {e}"),
                        tool_call_id: None,
                        correlation_id: String::new(),
                    }));
                    return;
                }
            };

            // Send the query
            if let Err(e) = session.send(&query) {
                warn!(target: "agent", "Failed to send to visible agent: {e}");
                let _ = handle.block_on(proactive_tx.send(ProactiveEvent::AgentResult {
                    task,
                    result: format!("Visible agent send error: {e}"),
                    tool_call_id: None,
                    correlation_id: String::new(),
                }));
                return;
            }

            // Poll for output with timeout
            let max_idle = std::time::Duration::from_secs(5); // 5s idle -> consider done
            let hard_timeout = std::time::Duration::from_secs(300); // 5min max
            let poll_interval = std::time::Duration::from_millis(200);
            let start = std::time::Instant::now();
            let mut last_output = std::time::Instant::now();
            let mut accumulated = String::new();

            loop {
                // Check hard timeout
                if start.elapsed() > hard_timeout {
                    info!(target: "agent", "RunAgentTool(visible): hard timeout reached");
                    break;
                }

                // Poll for new output
                if let Some(lines) = session.receive()
                    && !lines.is_empty()
                {
                    accumulated.push_str(&lines);
                    accumulated.push('\n');
                    last_output = std::time::Instant::now();
                }

                // Check if agent has gone idle (no output for 5s + process may have exited)
                if last_output.elapsed() > max_idle {
                    // Give one more brief chance and break
                    std::thread::sleep(poll_interval);
                    if let Some(lines) = session.receive()
                        && !lines.is_empty()
                    {
                        accumulated.push_str(&lines);
                        accumulated.push('\n');
                    }
                    info!(target: "agent", "RunAgentTool(visible): idle timeout — accumulated {} chars", accumulated.len());
                    break;
                }

                std::thread::sleep(poll_interval);
            }

            let result = if accumulated.is_empty() {
                accumulated
            } else {
                accumulated.trim().to_string()
            };

            info!(target: "agent", "RunAgentTool(visible): task complete ({} chars)", result.len());

            if handle
                .block_on(proactive_tx.send(ProactiveEvent::AgentResult {
                    task,
                    result,
                    tool_call_id: None,
                    correlation_id: String::new(),
                }))
                .is_err()
            {
                warn!(
                    "RunAgentTool(visible): failed to deliver agent result: main loop channel closed"
                );
            }
        });

        "[Tarea delegada al agente visible. El resultado llegará en breve.]".to_string()
    }

    /// CLI mode: spawn agent as one-shot subprocess, deliver result proactively.
    async fn run_cli(&self, task: String) -> String {
        let command = match &self.config.command {
            Some(c) => c.clone(),
            None => return "Error: CLI agent command not configured.".to_string(),
        };
        let query = build_agent_query(&task, &self.config.prompt);
        let proactive_tx = self.proactive_tx.clone();

        tokio::spawn(async move {
            info!("RunAgentTool(cli): task started: {:?}", task);
            let result = call_agent(command, query).await;
            info!("RunAgentTool(cli): task complete ({} chars)", result.len());
            if proactive_tx
                .send(ProactiveEvent::AgentResult {
                    task,
                    result,
                    tool_call_id: None,
                    correlation_id: String::new(),
                })
                .await
                .is_err()
            {
                warn!(
                    "RunAgentTool(cli): failed to deliver agent result: main loop channel closed"
                );
            }
        });

        "[Tarea delegada al agente. El resultado llegará en breve.]".to_string()
    }

    /// ACP mode: spawn a new ACP process per task, deliver result proactively.
    async fn run_acp(&self, task: String) -> String {
        let task_id = Uuid::new_v4().to_string();
        let task_id_return = task_id.clone();
        info!(target: "agent", "RunAgentTool(acp): task started: {:?} (id={})", task, task_id);

        let query = build_agent_query(&task, &self.config.prompt);
        let task_c = task.clone();
        let task_map = Arc::clone(&self.task_map);
        let proactive_tx = self.proactive_tx.clone();
        let acp_command = self.config.acp_command.clone();
        let agent_name = self.config.name.clone();
        let config = self.config.clone();
        let session_mgr = self.session_manager.clone();
        let permission_gate = self.permission_gate.clone();

        tokio::spawn(async move {
            let writer_arc: Arc<Mutex<AcpWriter>>;
            let inbound_rx: Arc<Mutex<mpsc::Receiver<JsonRpcMessage>>>;
            let session_id: String;
            let session_event_tx: Option<SessionEventTx> =
                session_mgr.as_ref().and_then(|m| m.event_sender());
            let owned_process = if let Some(ref mgr) = session_mgr {
                let sess = match mgr.get_or_create_session(&config).await {
                    Ok(e) => e,
                    Err(e) => {
                        let _ = proactive_tx
                            .send(ProactiveEvent::AgentResult {
                                task: task_c,
                                result: format!("ACP session error: {e}"),
                                tool_call_id: None,
                                correlation_id: String::new(),
                            })
                            .await;
                        return;
                    }
                };
                writer_arc = sess.writer;
                inbound_rx = sess.inbound_rx;
                session_id = sess.session_id;
                mgr.mark_session_busy(&agent_name);
                mgr.add_task(&agent_name, &task_id);
                false
            } else {
                let (mut writer, mut rx) = match AcpWriter::spawn(&acp_command).await {
                    Ok(pair) => pair,
                    Err(e) => {
                        let _ = proactive_tx
                            .send(ProactiveEvent::AgentResult {
                                task: task_c,
                                result: format!("ACP spawn error: {e}"),
                                tool_call_id: None,
                                correlation_id: String::new(),
                            })
                            .await;
                        return;
                    }
                };

                let cwd = std::env::current_dir()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let sid = match writer.initialize(&mut rx, &cwd).await {
                    Ok(sid) => sid,
                    Err(e) => {
                        let _ = writer.kill().await;
                        let _ = proactive_tx
                            .send(ProactiveEvent::AgentResult {
                                task: task_c,
                                result: format!("ACP init error: {e}"),
                                tool_call_id: None,
                                correlation_id: String::new(),
                            })
                            .await;
                        return;
                    }
                };
                writer_arc = Arc::new(Mutex::new(writer));
                inbound_rx = Arc::new(Mutex::new(rx));
                session_id = sid;
                true
            };

            let latency_start = std::time::Instant::now();

            // ── Drain any pending warm-up response before the real prompt ─────
            // The startup warm-up prompt (`prewarm_agent`) does not block on its
            // own reply; if it is still buffered when the first task runs, those
            // `agent_message_chunk` notifications (which carry no request id)
            // would be appended to this task's accumulated result. Wait for and
            // discard that response first (bounded by PREWARM_DRAIN_TIMEOUT_SECS).
            if let Some(ref mgr) = session_mgr {
                let drain_timeout = std::time::Duration::from_secs(PREWARM_DRAIN_TIMEOUT_SECS);
                mgr.drain_pending_prewarm(&agent_name, drain_timeout).await;
            }

            // ── Send prompt ───────────────────────────────────────────────────
            let send_result = {
                let mut w = writer_arc.lock().await;
                w.send_prompt(&session_id, &query).await
            };
            let prompt_request_id = match send_result {
                Ok(id) => id,
                Err(e) => {
                    if let Some(ref mgr) = session_mgr {
                        mgr.mark_session_error(&agent_name);
                        mgr.remove_task(&agent_name, &task_id);
                    }
                    let mut w = writer_arc.lock().await;
                    let _ = w.kill().await;
                    let _ = proactive_tx
                        .send(ProactiveEvent::AgentResult {
                            task: task_c,
                            result: format!("ACP send error: {e}"),
                            tool_call_id: None,
                            correlation_id: String::new(),
                        })
                        .await;
                    return;
                }
            };

            if let Some(ref tx) = session_event_tx {
                let preview = if task_c.chars().count() > 200 {
                    let t: String = task_c.chars().take(200).collect();
                    format!("{t}…")
                } else {
                    task_c.clone()
                };
                let _ = tx.try_send(SessionEvent::UserMessage {
                    agent_name: agent_name.clone(),
                    session_id: session_id.clone(),
                    text: preview,
                    correlation_id: task_id.clone(),
                });
            }

            // ── Register active task in task_map ─────────────────────────────
            let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
            let active = ActiveTask {
                task_id: task_id.clone(),
                agent_name: agent_name.clone(),
                session_id: session_id.to_string(),
                prompt_request_id,
                task_text: task_c.clone(),
                state: TaskState::Running,
                created_at: std::time::Instant::now(),
                last_progress: None,
                accumulated_text: String::new(),
                cancel_handle: cancel_tx,
            };
            task_map.insert(task_id.clone(), active);

            // ── Collect responses ─────────────────────────────────────────────
            let acp_writer_for_collect: Arc<Mutex<Option<AcpWriter>>> = Arc::new(Mutex::new(None));

            let mut rx_guard = inbound_rx.lock().await;
            let result = collect_acp_response(
                Arc::clone(&acp_writer_for_collect),
                &mut rx_guard,
                proactive_tx.clone(),
                session_id.clone(),
                prompt_request_id,
                cancel_rx,
                task_id.clone(),
                agent_name.clone(),
                session_event_tx.clone(),
                session_mgr.clone(),
            )
            .await;
            let latency_ms = latency_start.elapsed().as_millis();
            info!(target: "acp", latency_ms, task_id, "ACP round-trip complete");
            drop(rx_guard);

            // ── Cancel any dangling permission slots for this task's scope ──
            // Issued at the end of the ACP prompt so a finishing turn never
            // leaves permissions hanging past the turn boundary (issue #167).
            // Any permission that was already answered (normal or HTTP) is
            // no longer in the gate, so cancel_scope is a no-op for those.
            if let Some(gate) = &permission_gate {
                let cancelled = gate.cancel_scope(&task_id);
                if cancelled > 0 {
                    info!(
                        target: "acp",
                        "Cancelled {cancelled} dangling permission(s) at end of task={task_id}",
                    );
                }
            }

            // ── Cleanup ──────────────────────────────────────────────────────
            if let Some(ref mgr) = session_mgr {
                mgr.remove_task(&agent_name, &task_id);
                if result.starts_with("ACP error:") || result.starts_with("ACP send error:") {
                    mgr.mark_session_error(&agent_name);
                } else if owned_process {
                    mgr.mark_session_done(&agent_name);
                } else if !mgr.has_tasks(&agent_name) {
                    mgr.mark_session_idle(&agent_name);
                }
            }

            if owned_process {
                {
                    let mut w = writer_arc.lock().await;
                    let _ = w.kill().await;
                }
            }

            if let Some(mut entry) = task_map.get_mut(&task_id) {
                entry.state = TaskState::Completed;
            }
            task_map.remove(&task_id);

            info!(target: "acp", "Agent task complete [{}] — sending result ({} chars)", task_id, result.len());
            if proactive_tx
                .send(ProactiveEvent::AgentResult {
                    task: task_c,
                    result,
                    tool_call_id: None,
                    correlation_id: String::new(),
                })
                .await
                .is_err()
            {
                warn!(
                    "RunAgentTool(acp): failed to deliver agent result: main loop channel closed"
                );
            }
        });

        format!(
            "[Tarea ACP delegada al agente ({}). El resultado llegará en breve.]",
            task_id_return
        )
    }
}

#[async_trait]
impl Tool for RunAgentTool {
    fn name(&self) -> &str {
        self.tool_name
            .get_or_init(|| Box::leak(format!("run_{}", self.config.name).into_boxed_str()))
    }

    fn is_background(&self) -> bool {
        true
    }

    fn description(&self) -> &str {
        &self.tool_description
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": &self.task_description
                }
            },
            "required": ["task"],
            "additionalProperties": false
        })
    }

    async fn run(&self, args: &str) -> String {
        let task = parse_task(args);
        info!(target: "agent", "run_agent({}) invoked: mode={} raw_args={:?} task={:?}", self.config.name, self.config.mode, args, task);
        if task.is_empty() {
            warn!(target: "agent", "run_agent called with empty task");
            return "Error: run_agent requires a task description.".to_string();
        }

        // Inline commands — no subprocess needed.
        let lower = task.trim().to_lowercase();
        if lower.starts_with("cancel") {
            return self.cancel().await;
        }
        if lower.starts_with("status") {
            return self.status().await;
        }

        match self.config.mode.as_str() {
            "remote" => self.run_remote(task).await,
            "acp" => self.run_acp(task).await,
            "visible" => self.run_visible(task).await,
            _ => self.run_cli(task).await,
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build the prompt sent to the agent.
///
/// Prepends the agent's own prompt (role, capabilities, style) when available
/// so the agent knows how to behave, followed by the delegated task.
fn build_agent_query(task: &str, prompt: &str) -> String {
    let mut parts = Vec::new();
    if !prompt.is_empty() {
        parts.push(format!("[Prompt del agente]: {prompt}"));
    }
    parts.push(format!("[Tarea delegada]: {task}"));
    parts.join("\n\n")
}

fn parse_task(args: &str) -> String {
    serde_json::from_str::<serde_json::Value>(args)
        .ok()
        .and_then(|v| v["task"].as_str().map(String::from))
        .unwrap_or_else(|| args.to_string())
}
// ── AcpWriter (moved to seneschal-common, re-exported for compat) ──────────
pub use seneschal_common::acp_writer::AcpWriter;
pub use seneschal_common::acp_writer::JsonRpcMessage;

// ── ActiveAcpTask ─────────────────────────────────────────────────────────────

/// Tracks a single in-flight ACP task.
pub struct ActiveAcpTask {
    #[allow(dead_code)]
    pub session_id: String,
    /// The JSON-RPC request id for the prompt, used for cancellation.
    pub prompt_request_id: u64,
    /// Sending on this channel cancels the task's collect loop.
    pub cancel_tx: oneshot::Sender<()>,
}

// ── Per-task ACP runtime types ────────────────────────────────────────────────

/// Lifecycle state of a delegated agent task.
#[derive(Debug, Clone, PartialEq)]
pub enum TaskState {
    Running,
    AwaitingUserInput,
    Completed,
    Cancelled,
    Failed,
}

/// Full state for a single delegated task. Each task owns its own ACP process.
#[derive(Debug)]
pub struct ActiveTask {
    pub task_id: String,
    pub agent_name: String,
    pub session_id: String,
    pub prompt_request_id: u64,
    pub task_text: String,
    pub state: TaskState,
    pub created_at: std::time::Instant,
    pub last_progress: Option<String>,
    pub accumulated_text: String,
    pub cancel_handle: oneshot::Sender<()>,
}

/// Handle returned to the caller when a task is spawned.
/// Gives the caller access to the task's ACP writer, message receiver, and cancel channel.
pub struct AgentTaskHandle {
    pub task_id: String,
    pub writer: AcpWriter,
    pub rx: mpsc::Receiver<JsonRpcMessage>,
    pub session_id: String,
    pub prompt_request_id: u64,
    pub state: Arc<std::sync::atomic::AtomicU8>,
    pub cancel_handle: oneshot::Sender<()>,
    pub created_at: std::time::Instant,
}

impl std::fmt::Debug for AgentTaskHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentTaskHandle")
            .field("task_id", &self.task_id)
            .field("session_id", &self.session_id)
            .field("prompt_request_id", &self.prompt_request_id)
            .finish_non_exhaustive()
    }
}

/// Entry for an agent-initiated interaction that requires user input.
/// Prefer [`seneschal_common::PermissionGate`] for new code; this type remains
/// for any external callers that still build a local FIFO.
#[derive(Debug)]
pub struct PendingInteractionEntry {
    pub task_id: String,
    pub agent_name: String,
    pub server_request_id: u64,
    pub question: String,
    pub options: Vec<seneschal_common::PermissionOptionWire>,
    pub response_tx: tokio::sync::oneshot::Sender<String>,
}

// ── collect_acp_response ──────────────────────────────────────────────────────

/// Drive the ACP inbound message loop for one task.
///
/// Handles streaming session/update notifications, permission requests, and
/// cancellation. Returns the accumulated text result or an error/cancel string.
#[allow(clippy::too_many_arguments)]
async fn collect_acp_response(
    acp_writer: Arc<Mutex<Option<AcpWriter>>>,
    inbound_rx: &mut mpsc::Receiver<JsonRpcMessage>,
    proactive_tx: mpsc::Sender<ProactiveEvent>,
    session_id: String,
    prompt_request_id: u64,
    mut cancel_rx: oneshot::Receiver<()>,
    task_id: String,
    agent_name: String,
    session_event_tx: Option<SessionEventTx>,
    session_mgr: Option<Arc<AcpSessionManager>>,
) -> String {
    let emit_log = |tx: &Option<SessionEventTx>, text: String| {
        if let Some(tx) = tx {
            let _ = tx.try_send(SessionEvent::AgentMessage {
                agent_name: agent_name.clone(),
                session_id: session_id.clone(),
                text,
                correlation_id: task_id.clone(),
            });
        }
    };
    let mut accumulated_text = String::new();
    let mut progress: Vec<String> = Vec::new();

    loop {
        let maybe_msg = tokio::select! {
            biased;
            _ = &mut cancel_rx => None,
            msg = inbound_rx.recv() => msg,
        };

        match maybe_msg {
            None => {
                // Cancel fired or channel closed — send cancel to the agent.
                let mut guard = acp_writer.lock().await;
                if let Some(w) = guard.as_mut() {
                    let _ = w.send_cancel(prompt_request_id).await;
                }
                return "[Tarea cancelada.]".to_string();
            }
            // ── Response to our prompt request → task complete ─────────────
            Some(JsonRpcMessage::Response { id, result, error }) if id == prompt_request_id => {
                if let Some(err) = error {
                    return format!("ACP error: {}", err);
                }
                let stop_reason = result
                    .as_ref()
                    .and_then(|r| r["stopReason"].as_str())
                    .unwrap_or("unknown");
                debug!(target: "acp", "Prompt complete, stopReason={}", stop_reason);

                if accumulated_text.is_empty() && !progress.is_empty() {
                    return format!("[Progreso: {}]", progress.join("; "));
                }
                if !accumulated_text.is_empty() && !progress.is_empty() {
                    return format!(
                        "{}\n\n[Progreso: {}]",
                        accumulated_text.trim(),
                        progress.join("; ")
                    );
                }
                if accumulated_text.is_empty() {
                    return format!("[Agente terminó con stopReason={stop_reason}]");
                }
                return accumulated_text.trim().to_string();
            }
            // ── session/update notification → streaming content ───────────
            Some(JsonRpcMessage::Notification { method, params }) if method == "session/update" => {
                let params = params.unwrap_or_default();
                let update = &params["update"];
                let session_update = update["sessionUpdate"].as_str().unwrap_or("");

                match session_update {
                    "agent_message_chunk" => {
                        if let Some(text) = update["content"]["text"].as_str() {
                            accumulated_text.push_str(text);
                            debug!(target: "acp", "Agent chunk: {}", text);
                            emit_log(&session_event_tx, text.to_string());
                        }
                    }
                    "agent_thought_chunk" => {
                        if let Some(text) = update["content"]["text"].as_str() {
                            debug!(target: "acp", "Thought: {}", text);
                            emit_log(&session_event_tx, format!("thinking: {text}"));
                        }
                    }
                    "tool_call" => {
                        let tool_name = update["name"].as_str().unwrap_or("unknown");
                        info!(target: "acp", "Tool start: {}", tool_name);
                        progress.push(format!("usando {tool_name}"));
                        if let Some(ref tx) = session_event_tx {
                            let _ = tx.try_send(SessionEvent::ToolCall {
                                agent_name: agent_name.clone(),
                                session_id: session_id.clone(),
                                tool_name: tool_name.to_string(),
                                task_id: task_id.clone(),
                                correlation_id: task_id.clone(),
                            });
                        }
                    }
                    "tool_call_update" => {
                        let tool_name = update["name"].as_str().unwrap_or("unknown");
                        let status = update["status"].as_str().unwrap_or("");
                        debug!(target: "acp", "Tool update: {}", tool_name);
                        if let Some(ref tx) = session_event_tx {
                            let _ = tx.try_send(SessionEvent::ToolResult {
                                agent_name: agent_name.clone(),
                                session_id: session_id.clone(),
                                tool_name: tool_name.to_string(),
                                task_id: task_id.clone(),
                                result: status.to_string(),
                                correlation_id: task_id.clone(),
                            });
                        }
                    }
                    other => {
                        debug!(target: "acp", "Ignored session update: {}", other);
                    }
                }
            }
            // ── session/request_permission request → auto-allow or ask user ─
            Some(JsonRpcMessage::Request { id, method, params })
                if method == "session/request_permission" =>
            {
                let params = params.unwrap_or_default();

                // Structured ACP options (optionId + label + kind) for Control / voice.
                let options =
                    seneschal_common::permission_options_from_acp_json(&params["options"]);

                // Build a description from the toolCall info
                let tool_call = &params["toolCall"];
                let tool_name = tool_call["name"].as_str().unwrap_or("acción desconocida");
                let description = if let Some(input) = tool_call["input"].as_str() {
                    let truncated = if input.len() > 200 {
                        format!("{}...", &input[..200])
                    } else {
                        input.to_string()
                    };
                    format!("{tool_name}: {truncated}")
                } else {
                    tool_name.to_string()
                };

                if let Some(ref mgr) = session_mgr {
                    mgr.mark_session_needs_input(&agent_name);
                }
                emit_log(&session_event_tx, format!("? ¿permiso: {description}?"));

                let (resp_tx, resp_rx) = oneshot::channel::<String>();
                let _ = proactive_tx
                    .send(ProactiveEvent::AgentQuestion {
                        task_id: task_id.clone(),
                        agent_name: agent_name.clone(),
                        question: description,
                        options,
                        response_tx: resp_tx,
                    })
                    .await;

                let outcome_option_id =
                    match tokio::time::timeout(std::time::Duration::from_secs(60), resp_rx).await {
                        Ok(Ok(ans)) => ans,
                        _ => {
                            warn!(target: "acp", "Permission timeout — defaulting to cancelled");
                            String::new() // will send cancelled outcome
                        }
                    };

                if let Some(ref mgr) = session_mgr {
                    mgr.mark_session_busy(&agent_name);
                }

                // Build the response: AllowedOutcome or DeniedOutcome
                let result = if outcome_option_id.is_empty() || outcome_option_id == "cancelled" {
                    serde_json::json!({"outcome": "cancelled"})
                } else {
                    serde_json::json!({"outcome": "selected", "optionId": outcome_option_id})
                };

                let mut guard = acp_writer.lock().await;
                if let Some(w) = guard.as_mut() {
                    let _ = w.send_response(id, result).await;
                }
            }
            // ── Unmatched response (different id) ─────────────────────────
            Some(JsonRpcMessage::Response { id, .. }) => {
                debug!(target: "acp", "Ignored response for id={}", id);
            }
            // ── Other notifications / requests ────────────────────────────
            Some(other) => {
                debug!(target: "acp", "Ignored: {:?}", other);
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::*;

    use seneschal_agents::AgentConfig;

    fn test_agent_config(name: &str, mode: &str, command: Option<String>) -> AgentConfig {
        AgentConfig {
            name: name.to_string(),
            mode: mode.to_string(),
            command,
            acp_command: format!("{name} acp"),
            remote_url: String::new(),
            remote_dir: String::new(),
            remote_session_path: String::new(),
            remote_message_path: String::new(),
            remote_event_path: String::new(),
            remote_api_key: String::new(),
            when_to_use: "Test".to_string(),
            prompt: "Test prompt".to_string(),
            description: String::new(),
            task_description: String::new(),
        }
    }

    fn cli_tool(command: &str, tx: mpsc::Sender<ProactiveEvent>) -> RunAgentTool {
        let config = test_agent_config("hermes", "cli", Some(command.to_string()));
        RunAgentTool::new(config, Arc::new(DashMap::new()), tx)
    }

    fn acp_tool(tx: mpsc::Sender<ProactiveEvent>) -> RunAgentTool {
        let config = test_agent_config("hermes", "acp", None);
        RunAgentTool::new(config, Arc::new(DashMap::new()), tx)
    }

    // ── strip_hermes_cli_noise ────────────────────────────────────────────────

    #[test]
    fn strip_noise_quiet_mode_output() {
        let input = "\r\n╭─ ⚕ Hermes ──────────────────────────────────────────────────────────────────╮\r\nEl resultado es 42.\n\nsession_id: 20260403_121303_abc\n";
        assert_eq!(strip_hermes_cli_noise(input), "El resultado es 42.");
    }

    #[test]
    fn strip_noise_clean_output() {
        let input = "Respuesta limpia sin ruido.";
        assert_eq!(strip_hermes_cli_noise(input), "Respuesta limpia sin ruido.");
    }

    #[test]
    fn strip_noise_only_structural_lines() {
        let input = "╭─ header ─╮\nsession_id: abc\n";
        assert_eq!(strip_hermes_cli_noise(input), "");
    }

    #[test]
    fn strip_noise_multiline_response() {
        let input = "╭─ Hermes ─╮\nPrimera línea.\nSegunda línea.\n\nsession_id: xyz\n";
        assert_eq!(
            strip_hermes_cli_noise(input),
            "Primera línea.\nSegunda línea."
        );
    }

    // ── RunAgentTool — name / description ─────────────────────────────────────

    #[test]
    fn tool_name_and_description() {
        let (tx, _rx) = mpsc::channel::<ProactiveEvent>(8);
        let tool = cli_tool("echo", tx);
        assert_eq!(tool.name(), "run_hermes");
        assert!(!tool.description().is_empty());
    }

    // ── RunAgentTool — CLI mode ───────────────────────────────────────────────

    #[tokio::test]
    async fn cli_empty_args_returns_error() {
        let (tx, _rx) = mpsc::channel::<ProactiveEvent>(8);
        let tool = cli_tool("echo", tx);
        let result = tool.run("").await;
        assert!(result.to_lowercase().contains("error"), "got: {result:?}");
    }

    #[tokio::test]
    async fn cli_returns_acknowledgment_immediately() {
        let (tx, _rx) = mpsc::channel::<ProactiveEvent>(8);
        let tool = cli_tool("sleep 2", tx);
        let start = std::time::Instant::now();
        let result = tool.run(r#"{"task": "slow task"}"#).await;
        assert!(
            start.elapsed().as_millis() < 200,
            "should return immediately"
        );
        assert!(
            !result.is_empty(),
            "should return acknowledgment: {result:?}"
        );
    }

    #[tokio::test]
    async fn visible_returns_acknowledgment_immediately() {
        let (tx, _rx) = mpsc::channel::<ProactiveEvent>(8);
        let config = test_agent_config("test-visible", "visible", Some("echo".to_string()));
        let tool = RunAgentTool::new(config, Arc::new(DashMap::new()), tx);
        let start = std::time::Instant::now();
        let result = tool.run(r#"{"task": "test task"}"#).await;
        assert!(
            start.elapsed().as_millis() < 200,
            "should return immediately"
        );
        assert!(
            !result.is_empty(),
            "should return acknowledgment: {result:?}"
        );
    }

    #[tokio::test]
    async fn visible_without_manager_returns_error() {
        let (tx, _rx) = mpsc::channel::<ProactiveEvent>(8);
        let config = test_agent_config("test-visible", "visible", Some("echo".to_string()));
        let tool = RunAgentTool::new(config, Arc::new(DashMap::new()), tx);
        let result = tool.run("test task").await;
        assert!(
            result.contains("Visible session manager not configured"),
            "should report missing manager: {result:?}"
        );
    }

    #[tokio::test]
    async fn cli_delivers_result_to_proactive_channel() {
        let (tx, mut rx) = mpsc::channel::<ProactiveEvent>(8);
        let tool = cli_tool("echo agent_done", tx);
        tool.run(r#"{"task": "some task"}"#).await;

        let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");

        match event {
            ProactiveEvent::AgentResult { task, result, .. } => {
                assert!(task.contains("some task"), "task: {task:?}");
                assert!(!result.is_empty(), "result should not be empty");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn cli_receives_only_task_not_history() {
        let (tx, mut rx) = mpsc::channel::<ProactiveEvent>(8);
        let tool = cli_tool("echo done", tx);
        tool.run(r#"{"task": "busca noticias"}"#).await;

        let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");

        match event {
            ProactiveEvent::AgentResult { task, .. } => {
                assert!(task.contains("busca noticias"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn cli_delivers_error_on_launch_failure() {
        let (tx, mut rx) = mpsc::channel::<ProactiveEvent>(8);
        let tool = cli_tool("__nonexistent__", tx);
        tool.run(r#"{"task": "task"}"#).await;

        let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");

        match event {
            ProactiveEvent::AgentResult { result, .. } => {
                assert!(result.to_lowercase().contains("error"), "got: {result:?}");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    // ── RunAgentTool — cancel / status inline commands ────────────────────────

    #[tokio::test]
    async fn cancel_returns_no_task_when_idle() {
        let (tx, _rx) = mpsc::channel::<ProactiveEvent>(8);
        let tool = acp_tool(tx);
        let result = tool.run(r#"{"task": "cancel"}"#).await;
        assert!(result.contains("No hay"), "got: {result:?}");
    }

    #[tokio::test]
    async fn cancel_fires_cancel_channel() {
        let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
        let task_map: Arc<DashMap<String, ActiveTask>> = Arc::new(DashMap::new());
        let active = ActiveTask {
            task_id: "t1".to_string(),
            agent_name: "hermes".to_string(),
            session_id: "s1".to_string(),
            prompt_request_id: 2,
            task_text: "test task".to_string(),
            state: TaskState::Running,
            created_at: std::time::Instant::now(),
            last_progress: None,
            accumulated_text: String::new(),
            cancel_handle: cancel_tx,
        };
        task_map.insert("t1".to_string(), active);

        let (tx, _rx) = mpsc::channel::<ProactiveEvent>(8);
        let config = test_agent_config("hermes", "acp", None);
        let tool = RunAgentTool::new(config, task_map, tx);
        let result = tool.run(r#"{"task": "cancel"}"#).await;
        assert!(result.contains("cancelada"), "got: {result:?}");
        assert!(
            cancel_rx.try_recv().is_ok(),
            "cancel channel should have fired"
        );
    }

    #[tokio::test]
    async fn status_returns_idle_when_no_task() {
        let (tx, _rx) = mpsc::channel::<ProactiveEvent>(8);
        let tool = acp_tool(tx);
        let result = tool.run(r#"{"task": "status"}"#).await;
        assert!(result.contains("no tiene"), "got: {result:?}");
    }

    // ── JSON-RPC helpers ─────────────────────────────────────────────────────

    #[test]
    fn jsonrpc_request_has_correct_structure() {
        let msg = jsonrpc_request(0, "initialize", serde_json::json!({"protocolVersion": 1}));
        assert_eq!(msg["jsonrpc"], "2.0");
        assert_eq!(msg["id"], 0);
        assert_eq!(msg["method"], "initialize");
        assert_eq!(msg["params"]["protocolVersion"], 1);
    }

    #[test]
    fn jsonrpc_notification_has_no_id() {
        let msg = jsonrpc_notification("session/cancel", serde_json::json!({"requestId": 5}));
        assert_eq!(msg["jsonrpc"], "2.0");
        assert!(msg.get("id").is_none(), "notification must not have id");
        assert_eq!(msg["method"], "session/cancel");
        assert_eq!(msg["params"]["requestId"], 5);
    }

    #[test]
    fn parse_jsonrpc_response() {
        let v: Value = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":0,"result":{"protocolVersion":1,"agentInfo":{"name":"hermes","version":"0.1.0"}}}"#
        ).unwrap();
        let msg = parse_jsonrpc(&v).unwrap();
        match msg {
            JsonRpcMessage::Response { id, result, error } => {
                assert_eq!(id, 0);
                assert!(result.is_some());
                assert!(error.is_none());
                assert_eq!(result.unwrap()["protocolVersion"], 1);
            }
            other => panic!("expected Response, got: {:?}", other),
        }
    }

    #[test]
    fn parse_jsonrpc_notification() {
        let v: Value = serde_json::from_str(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"}}}}"#
        ).unwrap();
        let msg = parse_jsonrpc(&v).unwrap();
        match msg {
            JsonRpcMessage::Notification { method, params } => {
                assert_eq!(method, "session/update");
                let params = params.unwrap();
                assert_eq!(params["sessionId"], "s1");
                assert_eq!(params["update"]["sessionUpdate"], "agent_message_chunk");
                assert_eq!(params["update"]["content"]["text"], "hello");
            }
            other => panic!("expected Notification, got: {:?}", other),
        }
    }

    #[test]
    fn parse_jsonrpc_request_from_server() {
        let v: Value = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":5,"method":"session/request_permission","params":{"sessionId":"s1","options":[{"optionId":"allow","name":"Allow","kind":"allow"}],"toolCall":{"name":"bash"}}}"#
        ).unwrap();
        let msg = parse_jsonrpc(&v).unwrap();
        match msg {
            JsonRpcMessage::Request { id, method, params } => {
                assert_eq!(id, 5);
                assert_eq!(method, "session/request_permission");
                let params = params.unwrap();
                assert_eq!(params["options"][0]["optionId"], "allow");
                assert_eq!(params["toolCall"]["name"], "bash");
            }
            other => panic!("expected Request, got: {:?}", other),
        }
    }

    #[test]
    fn parse_jsonrpc_error_response() {
        let v: Value = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Invalid request"}}"#,
        )
        .unwrap();
        let msg = parse_jsonrpc(&v).unwrap();
        match msg {
            JsonRpcMessage::Response { id, result, error } => {
                assert_eq!(id, 1);
                assert!(result.is_none());
                assert!(error.is_some());
                assert_eq!(error.unwrap()["message"], "Invalid request");
            }
            other => panic!("expected Response, got: {:?}", other),
        }
    }

    // ── Initialize request format ────────────────────────────────────────────

    #[test]
    fn initialize_request_uses_camel_case() {
        let msg = jsonrpc_request(
            0,
            "initialize",
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": {"name": "seneschal", "version": "0.1.0"}
            }),
        );
        assert_eq!(msg["params"]["protocolVersion"], 1);
        assert!(msg["params"]["clientCapabilities"].is_object());
        assert_eq!(msg["params"]["clientInfo"]["name"], "seneschal");
    }

    // ── Prompt request format ────────────────────────────────────────────────

    #[test]
    fn prompt_request_uses_session_id_camel_case() {
        let msg = jsonrpc_request(
            2,
            "session/prompt",
            serde_json::json!({
                "sessionId": "abc123",
                "prompt": [{"type": "text", "text": "hello"}]
            }),
        );
        assert_eq!(msg["method"], "session/prompt");
        assert_eq!(msg["params"]["sessionId"], "abc123");
        assert_eq!(msg["params"]["prompt"][0]["type"], "text");
        assert_eq!(msg["params"]["prompt"][0]["text"], "hello");
    }

    // ── Cancel notification format ───────────────────────────────────────────

    #[test]
    fn cancel_notification_uses_request_id() {
        let msg = jsonrpc_notification(
            "session/cancel",
            serde_json::json!({
                "requestId": 2
            }),
        );
        assert_eq!(msg["method"], "session/cancel");
        assert_eq!(msg["params"]["requestId"], 2);
        assert!(
            msg.get("id").is_none(),
            "cancel must be a notification (no id)"
        );
    }

    // ── Permission response format ───────────────────────────────────────────

    #[test]
    fn permission_response_allowed_format() {
        let result = serde_json::json!({"outcome": "selected", "optionId": "allow"});
        let msg = serde_json::json!({"jsonrpc": "2.0", "id": 5, "result": result});
        assert_eq!(msg["result"]["outcome"], "selected");
        assert_eq!(msg["result"]["optionId"], "allow");
    }

    #[test]
    fn permission_response_denied_format() {
        let result = serde_json::json!({"outcome": "cancelled"});
        let msg = serde_json::json!({"jsonrpc": "2.0", "id": 5, "result": result});
        assert_eq!(msg["result"]["outcome"], "cancelled");
    }

    // ── Permission request description enrichment ─────────────────────────────

    #[test]
    fn permission_description_includes_tool_input() {
        let params = serde_json::json!({
            "toolCall": {
                "name": "bash",
                "input": "cargo build --release"
            }
        });
        let tool_call = &params["toolCall"];
        let tool_name = tool_call["name"].as_str().unwrap_or("acción desconocida");
        let description = if let Some(input) = tool_call["input"].as_str() {
            format!("{tool_name}: {input}")
        } else {
            tool_name.to_string()
        };
        assert_eq!(description, "bash: cargo build --release");
    }

    #[test]
    fn permission_description_no_input_fallback() {
        let params = serde_json::json!({
            "toolCall": {
                "name": "bash"
            }
        });
        let tool_call = &params["toolCall"];
        let tool_name = tool_call["name"].as_str().unwrap_or("acción desconocida");
        let description = if let Some(input) = tool_call["input"].as_str() {
            format!("{tool_name}: {input}")
        } else {
            tool_name.to_string()
        };
        assert_eq!(description, "bash");
    }

    #[test]
    fn permission_description_input_is_null() {
        let params = serde_json::json!({
            "toolCall": {
                "name": "read",
                "input": null
            }
        });
        let tool_call = &params["toolCall"];
        let tool_name = tool_call["name"].as_str().unwrap_or("acción desconocida");
        let description = if let Some(input) = tool_call["input"].as_str() {
            format!("{tool_name}: {input}")
        } else {
            tool_name.to_string()
        };
        assert_eq!(description, "read");
    }

    #[test]
    fn permission_description_truncates_long_input() {
        let long_arg = "x".repeat(250);
        let params = serde_json::json!({
            "toolCall": {
                "name": "bash",
                "input": long_arg
            }
        });
        let tool_call = &params["toolCall"];
        let tool_name = tool_call["name"].as_str().unwrap_or("acción desconocida");
        let description = if let Some(input) = tool_call["input"].as_str() {
            let truncated = if input.len() > 200 {
                format!("{}...", &input[..200])
            } else {
                input.to_string()
            };
            format!("{tool_name}: {truncated}")
        } else {
            tool_name.to_string()
        };
        assert!(description.starts_with("bash: "));
        assert!(description.ends_with("..."));
        assert_eq!(description.len(), "bash: ".len() + 200 + 3);
    }

    // ── Permission options enrichment ─────────────────────────────────────────

    #[test]
    fn permission_options_include_labels() {
        let params = serde_json::json!({
            "options": [
                {"optionId": "allow", "label": "Allow once"},
                {"optionId": "deny", "label": "Deny"},
                {"optionId": "always_allow", "label": "Always allow"}
            ]
        });
        let options: Vec<String> = params["options"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|o| {
                        let id = o["optionId"].as_str().unwrap_or("?");
                        let label = o["label"].as_str().or_else(|| o["description"].as_str());
                        match label {
                            Some(l) if !l.is_empty() => format!("{l} ({id})"),
                            _ => id.to_string(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(options[0], "Allow once (allow)");
        assert_eq!(options[1], "Deny (deny)");
        assert_eq!(options[2], "Always allow (always_allow)");
    }

    #[test]
    fn permission_options_no_label_fallback_to_option_id() {
        let params = serde_json::json!({
            "options": [
                {"optionId": "allow"},
                {"optionId": "deny"}
            ]
        });
        let options: Vec<String> = params["options"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|o| {
                        let id = o["optionId"].as_str().unwrap_or("?");
                        let label = o["label"].as_str().or_else(|| o["description"].as_str());
                        match label {
                            Some(l) if !l.is_empty() => format!("{l} ({id})"),
                            _ => id.to_string(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(options[0], "allow");
        assert_eq!(options[1], "deny");
    }

    #[test]
    fn permission_options_use_description_fallback() {
        let params = serde_json::json!({
            "options": [
                {"optionId": "allow", "description": "Grant permission once"}
            ]
        });
        let options: Vec<String> = params["options"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|o| {
                        let id = o["optionId"].as_str().unwrap_or("?");
                        let label = o["label"].as_str().or_else(|| o["description"].as_str());
                        match label {
                            Some(l) if !l.is_empty() => format!("{l} ({id})"),
                            _ => id.to_string(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(options[0], "Grant permission once (allow)");
    }

    #[test]
    fn permission_options_empty_label_uses_option_id() {
        let params = serde_json::json!({
            "options": [
                {"optionId": "allow", "label": ""}
            ]
        });
        let options: Vec<String> = params["options"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|o| {
                        let id = o["optionId"].as_str().unwrap_or("?");
                        let label = o["label"].as_str().or_else(|| o["description"].as_str());
                        match label {
                            Some(l) if !l.is_empty() => format!("{l} ({id})"),
                            _ => id.to_string(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(options[0], "allow");
    }
}

// ── Integration tests (require running `hermes acp`) ──────────────────────────

#[cfg(test)]
mod integration_tests {
    use super::*;

    /// Full initialize + session/new handshake with a real `hermes acp` process.
    ///
    /// Requires `hermes acp` to be available in PATH.
    /// Run with: `cargo test acp_initialize_handshake -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn acp_initialize_handshake() {
        let (mut writer, mut rx) = AcpWriter::spawn("hermes acp")
            .await
            .expect("failed to spawn hermes acp");

        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();

        let session_id = writer
            .initialize(&mut rx, &cwd)
            .await
            .expect("initialize failed");

        assert!(!session_id.is_empty(), "session_id should not be empty");
        println!("Got session_id: {session_id}");

        writer.kill().await;
    }

    /// Full flow: initialize → session/new → prompt("say hello") → collect response.
    ///
    /// Requires `hermes acp` to be available in PATH.
    /// Run with: `cargo test acp_simple_prompt -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn acp_simple_prompt() {
        let (mut writer, mut rx) = AcpWriter::spawn("hermes acp")
            .await
            .expect("failed to spawn hermes acp");

        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();

        let session_id = writer
            .initialize(&mut rx, &cwd)
            .await
            .expect("initialize failed");

        let prompt_id = writer
            .send_prompt(&session_id, "Say hello in one sentence.")
            .await
            .expect("send_prompt failed");

        println!("Sent prompt with request id={prompt_id}");

        let writer = Arc::new(Mutex::new(Some(writer)));
        let (proactive_tx, _proactive_rx) = tokio::sync::mpsc::channel(8);
        let (_cancel_tx, cancel_rx) = oneshot::channel::<()>();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            collect_acp_response(
                writer.clone(),
                &mut rx,
                proactive_tx,
                session_id,
                prompt_id,
                cancel_rx,
                String::new(),
                String::new(),
                None,
                None,
            ),
        )
        .await
        .expect("timed out waiting for agent response");

        println!("Agent response: {result}");
        assert!(!result.is_empty(), "response should not be empty");
        assert!(
            !result.contains("error") && !result.contains("Error"),
            "should not be an error: {result}"
        );

        let mut guard = writer.lock().await;
        if let Some(w) = guard.as_mut() {
            w.kill().await;
        }
    }

    /// Start a prompt, immediately cancel it, verify we get the cancel result.
    ///
    /// Requires `hermes acp` to be available in PATH.
    /// Run with: `cargo test acp_cancel_running_task -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn acp_cancel_running_task() {
        let (mut writer, mut rx) = AcpWriter::spawn("hermes acp")
            .await
            .expect("failed to spawn hermes acp");

        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();

        let session_id = writer
            .initialize(&mut rx, &cwd)
            .await
            .expect("initialize failed");

        let prompt_id = writer
            .send_prompt(
                &session_id,
                "Write a very long essay about the history of computing.",
            )
            .await
            .expect("send_prompt failed");

        // Give the agent a moment to start processing, then cancel
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        writer
            .send_cancel(prompt_id)
            .await
            .expect("send_cancel failed");

        println!("Sent cancel for request id={prompt_id}");

        // Drain messages until we get the prompt response
        let result = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                match rx.recv().await {
                    Some(JsonRpcMessage::Response { id, result, .. }) if id == prompt_id => {
                        let stop_reason = result
                            .as_ref()
                            .and_then(|r| r["stopReason"].as_str())
                            .unwrap_or("unknown");
                        return stop_reason.to_string();
                    }
                    Some(_) => continue,
                    None => return "channel closed".to_string(),
                }
            }
        })
        .await
        .expect("timed out waiting for cancel response");

        println!("Stop reason after cancel: {result}");

        writer.kill().await;
    }
}
