# Mejorar TUI para mostrar comunicación ACP con agentes

## Context
- Origin: Gitea issue #164 — "Mejorar TUI para mostrar comunicación ACP con agentes"
- Summary: The TUI must show agent task lifecycle states inline in the conversation timeline (timeline.inline style, inspired by qwen-audio-agent), replacing the old sidebar approach that was removed in commit 490d45e. Agent states: task.running, task.delegated, task.finalizing, task.completed, task.permission.requested, task.failed. Results as inline Markdown/code blocks. Permission requests with clear instructions. Streaming transcription must continue working.
- Proposed branch: feature/issue-164-mejorar-tui-para-mostrar-comunicacion-acp
- Base branch: master
- Assumptions:
  - The `SessionEvent` channel (capacity 16) in `AcpSessionManager` already emits `AgentMessage`, `UserMessage`, `ToolCall`, `ToolResult`, `Status`, `Error` events — but nothing subscribes to it anymore after the sidebar was removed.
  - The `ProactiveEvent` channel already emits `AgentMilestone`, `AgentQuestion`, `AgentResult` events handled in main.rs, but they don't forward to the TUI as structured task events (only as `SystemNotification`).
  - The `ActiveTask` state tracker in `run_agent.rs` tracks per-agent task lifecycle (`Running, AwaitingUserInput, Completed, Cancelled, Failed`) via `Arc<DashMap<String, ActiveTask>>` — this can be used as the source of truth for task identity.
  - The `AcpSessionEvent` parser in `session_events.rs` already extracts `AgentMessageChunk`, `AgentThoughtChunk`, `ToolCall`, `ToolCallUpdate`, `PermissionRequest` from raw `session/update` JSON-RPC notifications.
  - Spanish labels from the issue will be used: "[Procesando]", "[Proyecto en ejecución]", "[Organizando resultados]", "[Necesita confirmación]", "[Error]".

---

## Phase 1: Define new TUI event types for agent task lifecycle

- [x] Step 1.1: Add new `TuiEvent` variants to `crates/seneschal-common/src/tui_events.rs`
  - File(s): `crates/seneschal-common/src/tui_events.rs`
  - Change: Append the following variants to the `TuiEvent` enum (after `PromptBuildStateChange`), keeping all existing variants untouched:
    ```rust
    /// Agent task lifecycle events (timeline.inline — qwen-audio-agent style).
    /// An agent task was created (LLM delegated to an agent).
    AgentTaskStarted { task_id: String, agent_name: String, objective: String },
    /// The agent is actively processing the task.
    AgentTaskRunning { task_id: String, objective: String },
    /// The agent spawned a sub-delegation (complex multi-step project).
    AgentTaskDelegated { task_id: String, objective: String },
    /// The agent is finalizing / organizing results.
    AgentTaskFinalizing { task_id: String, objective: String },
    /// The agent completed the task successfully. `result` is the final output (Markdown/code).
    AgentTaskCompleted { task_id: String, objective: String, result: String },
    /// The agent is requesting user permission for an action.
    AgentTaskPermissionRequested { task_id: String, agent_name: String, description: String, options: Vec<String> },
    /// The agent task failed.
    AgentTaskFailed { task_id: String, message: String },
    ```
  - Acceptance criteria: `cargo check -p seneschal-common` compiles without errors.

---

## Phase 2: Extend ChatMessage with AgentTask role and state struct

- [x] Step 2.1: Add `AgentTask` variant to `Role` enum and `AgentTaskState` struct in `crates/seneschal-tui/src/app.rs`
  - File(s): `crates/seneschal-tui/src/app.rs`
  - Change:
    1. Add an `AgentTask` variant to `Role`:
       ```rust
       AgentTask,
       ```
    2. Add a new public struct (before `ChatMessage`):
       ```rust
       /// Inline agent task state for timeline.inline rendering.
       #[derive(Clone, Debug, PartialEq)]
       pub enum AgentTaskStatus {
           Started,
           Running,
           Delegated,
           Finalizing,
           Completed,
           PermissionRequested,
           Failed,
       }
       ```
    3. Add a new field to `ChatMessage`:
       ```rust
       /// Agent task metadata (only meaningful when role == AgentTask).
       pub agent_task: Option<AgentTaskInfo>,
       ```
    4. Define `AgentTaskInfo` struct:
       ```rust
       #[derive(Clone, Debug)]
       pub struct AgentTaskInfo {
           pub task_id: String,
           pub agent_name: String,
           pub status: AgentTaskStatus,
           pub options: Vec<String>,
       }
       ```
    5. In `ChatMessage` fields, initialize `agent_task: None` in all existing constructors. Add a convenience constructor `fn agent_task(info: AgentTaskInfo, content: String) -> Self`.
  - Acceptance criteria: `cargo check -p seneschal-tui` compiles without errors. Existing tests in `app.rs` still pass with `cargo test -p seneschal-tui`.

---

## Phase 3: Bridge SessionEvent and ProactiveEvent → TuiEvent in main.rs

- [x] Step 3.1: Create a bridge task in `src/main.rs` that subscribes to `SessionEvent` and forwards to `tui_tx`
  - File(s): `src/main.rs`
  - Change: After the `AcpSessionManager` is created (after line ~310 in the agent setup section), add code to:
    1. Get a `SessionEvent` receiver from the session manager via `session_manager.subscribe()` (check if `AcpSessionManager` has a `subscribe()` method — if not, add one in this step that returns `mpsc::Receiver<SessionEvent>` by cloning from the internal `event_tx`).
    2. Spawn a `tokio::spawn` task that loops receiving `SessionEvent` and translates them to `TuiEvent` variants:
       - `SessionEvent::Status { agent_name, session_id, status, .. }`:
         - `SessionStatus::Started` → `AgentTaskStarted { task_id: session_id, agent_name, objective: "" }`
         - `SessionStatus::Busy` → `AgentTaskRunning { task_id: session_id, objective: "" }`
         - `SessionStatus::Done` → `AgentTaskFinalizing { task_id: session_id, objective: "" }`
         - `SessionStatus::Error` → `AgentTaskFailed { task_id: session_id, message: "Session error" }`
       - `SessionEvent::AgentMessage { agent_name, session_id, text, .. }` → accumulate per-session in a local `HashMap<String, String>` buffer. When the buffer reaches a sentence boundary (`.`, `!`, `?`, `\n`) or when 200 chars accumulate, emit `AgentTaskRunning { task_id: session_id, objective: text }` and clear the buffer.
       - `SessionEvent::ToolCall { agent_name, session_id, tool_name, .. }` → `AgentTaskRunning { task_id: session_id, objective: format!("llamando a {tool_name}...") }`
       - `SessionEvent::ToolResult { session_id, tool_name, result, .. }` → `AgentTaskRunning { task_id: session_id, objective: format!("{tool_name}: {result}") }`
       - `SessionEvent::Error { session_id, message, .. }` → `AgentTaskFailed { task_id: session_id, message }`
       - `SessionEvent::UserMessage { .. }` → ignore (already shown in main conversation).
    3. Send each translated event via `tui_tx.send(...)`. Log errors with `tracing::warn!`.
  - Acceptance criteria: Bridge task compiles. `cargo check --features tui` passes.

- [x] Step 3.2: Enhance existing `ProactiveEvent` handler in `src/main.rs` to also emit TUI agent task events
  - File(s): `src/main.rs` (the main loop `proactive_rx.recv()` match arm, around lines 1393-1537)
  - Change: In each `ProactiveEvent` arm, add a `tui_tx.send(...)` call **in addition to** the existing behavior (do not replace). Specifically:
    1. `ProactiveEvent::AgentResult { task, result, .. }`:
       - Also send: `TuiEvent::AgentTaskCompleted { task_id: task.clone(), objective: task, result }`
    2. `ProactiveEvent::AgentQuestion { task_id, agent_name, question, options, .. }`:
       - Also send: `TuiEvent::AgentTaskPermissionRequested { task_id, agent_name, description: question, options }`
    3. `ProactiveEvent::AgentMilestone { agent_name, milestone, .. }`:
       - Parse the milestone string to determine status:
         - If it contains "ejecución"/"running"/"procesando" → `AgentTaskRunning { task_id: milestone.clone(), objective: milestone }`
         - If it contains "finalizando"/"organizando"/"complet" → `AgentTaskFinalizing { task_id: milestone, objective: milestone }`
         - If it contains "delegación"/"delegated"/"proyecto" → `AgentTaskDelegated { task_id: milestone, objective: milestone }`
         - Otherwise → `AgentTaskRunning { task_id: milestone, objective: milestone }`
       - Note: The milestone string is used as `task_id` for deduplication in the TUI since we don't have a proper task_id from the remote agent SSE path. This is acceptable.
    4. All sends use `tui_tx.send(...).ok();` (non-blocking, drop on full channel).
  - Acceptance criteria: `cargo check --features tui` passes. No existing behavior is broken (the old `SystemNotification` sends are preserved).

---

## Phase 4: Handle new TuiEvent variants in App state

- [x] Step 4.1: Implement `handle_tui_event` for the new agent task variants in `crates/seneschal-tui/src/app.rs`
  - File(s): `crates/seneschal-tui/src/app.rs`
  - Change: Add match arms in `handle_tui_event` for each new variant. The logic:
    1. `AgentTaskStarted { task_id, agent_name, objective }`:
       - If a `ChatMessage` with the same `agent_task.task_id` already exists and its status is not `Completed`/`Failed`, skip (dedup).
       - Otherwise, push a new `ChatMessage` with `role: AgentTask`, `agent_task: Some(AgentTaskInfo { task_id, agent_name, status: Started, options: vec![] })`, `content: format!("[{agent_name}] {objective}")`.
    2. `AgentTaskRunning { task_id, objective }`:
       - Find existing message by `task_id`. If found, update its `agent_task.status` to `Running` and `content` to `objective`. If not found, push new message with `status: Running`.
    3. `AgentTaskDelegated { task_id, objective }`:
       - Same as Running but set `status: Delegated` and prefix content with "[Proyecto en ejecución] ".
    4. `AgentTaskFinalizing { task_id, objective }`:
       - Same pattern: update existing or create new with `status: Finalizing`.
    5. `AgentTaskCompleted { task_id, objective, result }`:
       - Find existing message by `task_id`. Update `status` to `Completed` and `content` to `result`. If not found, push new with `status: Completed`.
    6. `AgentTaskPermissionRequested { task_id, agent_name, description, options }`:
       - Push new `ChatMessage` with `status: PermissionRequested`, `options: options`, `content: description`.
    7. `AgentTaskFailed { task_id, message }`:
       - Find existing message by `task_id`. Update `status` to `Failed` and `content` to `message`. If not found, push new with `status: Failed`.
  - Add a helper method `fn find_agent_task_mut(&mut self, task_id: &str) -> Option<&mut ChatMessage>` that scans `self.messages` for a message where `agent_task.task_id == task_id`.
  - Acceptance criteria: `cargo test -p seneschal-tui` passes. `cargo check -p seneschal-tui` passes.

---

## Phase 5: Render agent task messages inline in the TUI

- [x] Step 5.1: Add `Role::AgentTask` rendering in `message_lines()` in `crates/seneschal-tui/src/ui.rs`
  - File(s): `crates/seneschal-tui/src/ui.rs`
  - Change: Add a new match arm to `message_lines()` for `Role::AgentTask`. The rendering should follow the same box-drawing border pattern as other roles but with task-specific colors and labels:
    1. Header line: box top border with status label and agent name.
       - Uses a magenta/purple color for the agent box (different from green=seneschal, cyan=user, yellow=system).
       - Status label mapping (Spanish, per issue requirements):
         - `Started` → `"[Iniciando] {agent_name}"` in bold magenta
         - `Running` → `"[Procesando] {agent_name}"` in bold magenta
         - `Delegated` → `"[Proyecto en ejecución] {agent_name}"` in bold magenta
         - `Finalizing` → `"[Organizando resultados] {agent_name}"` in bold magenta
         - `Completed` → `"[Completado] {agent_name}"` in bold green
         - `PermissionRequested` → `"[Necesita confirmación] {agent_name}"` in bold yellow
         - `Failed` → `"[Error] {agent_name}"` in bold red
       - Timestamp in gray, same as other roles.
    2. Content: render `msg.content` word-wrapped inside the box, same style as Assistant messages.
    3. For `PermissionRequested`: after the description content, add a line showing the options in cyan: `"Opciones: {option1} / {option2} / ..."`. Read options from `msg.agent_task.options`.
    4. For `Completed`: use green-tinted content text to differentiate final results from in-progress updates.
    5. Bottom border: same box-closing pattern as other roles.
  - Acceptance criteria: `cargo check -p seneschal-tui` passes. `cargo test -p seneschal-tui` passes. The `Role::AgentTask` arm handles all `AgentTaskStatus` variants explicitly (use exhaustive match to ensure compile-time coverage).

- [x] Step 5.2: Add an `AgentTask` entry to the `Role` exhaustive match in `render_streaming_lines` (no-op for streaming)
  - File(s): `crates/seneschal-tui/src/ui.rs`
  - Change: The `message_lines()` function's `Role` match is exhaustive — verify the compiler forces covering `AgentTask`. In `render_streaming`, agent tasks don't need special handling (streaming is only for Assistant). No code change needed — just verify compilation.
  - Acceptance criteria: `cargo check -p seneschal-tui` compiles without warnings about non-exhaustive match.

---

## Phase 6: Wire the `AcpSessionManager` to provide a subscribable event channel

- [x] Step 6.1: Add a `subscribe()` method to `AcpSessionManager` in `crates/seneschal-agents/src/session_manager.rs`
  - File(s): `crates/seneschal-agents/src/session_manager.rs`
  - Change: At the end of the `impl AcpSessionManager` block (before the test module), add:
    ```rust
    /// Subscribe to session events. Returns a new receiver that will get
    /// all future events (does not replay past events).
    /// Uses the existing `event_tx` field as a broadcast source.
    pub fn subscribe(&self) -> tokio::sync::mpsc::Receiver<SessionEvent> {
        // Create a fresh channel and spawn a forwarder task
        let (tx, rx) = tokio::sync::mpsc::channel::<SessionEvent>(64);
        let existing_rx_opt = self.event_tx.as_ref().map(|etx| {
            // Since SessionEventTx is mpsc::Sender, we can't clone the receiver.
            // Instead, create a new channel and forward from the emit path.
            // Actually — we need a different approach. Let's use tokio::sync::broadcast.
            // BUT the existing type is mpsc, so we need to adapt.
            let (inner_tx, mut inner_rx) = tokio::sync::mpsc::channel::<SessionEvent>(64);
            // The session manager already sends to event_tx — we need to tap that.
            // Problem: mpsc doesn't support multiple consumers. Solution: the
            // emit_session_event method should also send to a broadcast.
            // We'll use a broadcast::channel internally and convert for subscribers.
            inner_tx
        });
        rx
    }
    ```
    **Wait** — this doesn't work with `mpsc`. The correct approach is to add a `tokio::sync::broadcast::Sender<SessionEvent>` field to `AcpSessionManager` alongside the existing `event_tx: Option<SessionEventTx>`. But that's a big refactor.

    **Alternative simpler approach for this plan**: Don't add a subscribe method. Instead, change Step 3.1 to simply pass the existing `SessionEvent` channel to the bridge task directly. In main.rs, after creating the `AcpSessionManager`, we already have access to the internal channel.

    Actually, let me look at how `AcpSessionManager` is created in main.rs and whether the event channel is accessible. Let me check.

    **Actual approach**: In main.rs, the `RunAgentTool` is created with a reference to `AcpSessionManager`. The session manager's `event_tx` is internal. The simplest fix:

    Add a method to `AcpSessionManager`:
    ```rust
    /// Get a sender that can be used to receive session events.
    /// This creates a new channel and registers it. All existing and future
    /// session events will be forwarded.
    pub fn create_event_listener(&self) -> tokio::sync::mpsc::Receiver<SessionEvent> {
        let (tx, rx) = tokio::sync::mpsc::channel::<SessionEvent>(64);
        // Store tx in a Vec<Sender<SessionEvent>> so emit_session_event can fan out.
        // ... requires adding a field: listeners: Mutex<Vec<mpsc::Sender<SessionEvent>>>
        rx
    }
    ```
    This requires adding a `listeners: Mutex<Vec<mpsc::Sender<SessionEvent>>>` field to `AcpSessionManager` and modifying `emit_session_event` to forward to all listeners. This is the cleanest solution.

  - Change details:
    1. Add a new field to `AcpSessionManager` struct:
       ```rust
       session_event_listeners: Mutex<Vec<tokio::sync::mpsc::Sender<SessionEvent>>>,
       ```
    2. Initialize it as `Mutex::new(Vec::new())` in `AcpSessionManager::new()`.
    3. Add the method:
       ```rust
       pub fn create_event_listener(&self) -> tokio::sync::mpsc::Receiver<SessionEvent> {
           let (tx, rx) = tokio::sync::mpsc::channel::<SessionEvent>(64);
           self.session_event_listeners.lock().unwrap().push(tx);
           rx
       }
       ```
    4. Modify the private `emit_session_event` method (find it in `session_manager.rs`) to also forward to listeners:
       ```rust
       fn emit_session_event(&self, event: SessionEvent) {
           if let Some(ref tx) = self.event_tx {
               let _ = tx.try_send(event.clone());
           }
           let listeners = self.session_event_listeners.lock().unwrap();
           listeners.retain(|tx| tx.try_send(event.clone()).is_ok());
       }
       ```
    5. Add `use std::sync::Mutex as StdMutex;` at the top if needed (tokio Mutex is already imported).
  - Acceptance criteria: `cargo check -p seneschal-agents` passes. `cargo test -p seneschal-agents` passes. The existing `SessionEvent` emission path is not broken (tests still pass).

---

## Phase 7: Connect the bridge task in main.rs

- [x] Step 7.1: Create and spawn the bridge task in `src/main.rs`
  - File(s): `src/main.rs`
  - Change: After the `session_manager` is created (search for `AcpSessionManager::new` or `Arc::new(AcpSessionManager`), add:
    ```rust
    #[cfg(feature = "tui")]
    {
        let mut session_rx = session_manager.create_event_listener();
        let tui_tx_clone = tui_tx.clone();
        tokio::spawn(async move {
            let mut buffers: std::collections::HashMap<String, String> = std::collections::HashMap::new();
            while let Some(event) = session_rx.recv().await {
                use seneschal_agents::session_manager::{SessionEvent, SessionStatus};
                let tui_event = match &event {
                    SessionEvent::Status { agent_name, session_id, status, .. } => {
                        match status {
                            SessionStatus::Started => Some(seneschal_tui::events::TuiEvent::AgentTaskStarted {
                                task_id: session_id.clone(),
                                agent_name: agent_name.clone(),
                                objective: String::new(),
                            }),
                            SessionStatus::Busy => Some(seneschal_tui::events::TuiEvent::AgentTaskRunning {
                                task_id: session_id.clone(),
                                objective: String::new(),
                            }),
                            SessionStatus::Done => Some(seneschal_tui::events::TuiEvent::AgentTaskFinalizing {
                                task_id: session_id.clone(),
                                objective: String::new(),
                            }),
                            SessionStatus::Error => Some(seneschal_tui::events::TuiEvent::AgentTaskFailed {
                                task_id: session_id.clone(),
                                message: "Error de sesión ACP".to_string(),
                            }),
                            _ => None,
                        }
                    }
                    SessionEvent::ToolCall { agent_name: _, session_id, tool_name, .. } => {
                        Some(seneschal_tui::events::TuiEvent::AgentTaskRunning {
                            task_id: session_id.clone(),
                            objective: format!("llamando a {tool_name}..."),
                        })
                    }
                    SessionEvent::ToolResult { session_id, tool_name, result, .. } => {
                        let summary = if result.len() > 80 {
                            format!("{}...", &result[..80])
                        } else {
                            result.clone()
                        };
                        Some(seneschal_tui::events::TuiEvent::AgentTaskRunning {
                            task_id: session_id.clone(),
                            objective: format!("✅ {tool_name}: {summary}"),
                        })
                    }
                    SessionEvent::AgentMessage { session_id, text, .. } => {
                        let buf = buffers.entry(session_id.clone()).or_default();
                        buf.push_str(text);
                        buf.push(' ');
                        // Emit on sentence boundary or accumulated size
                        let should_emit = buf.len() > 200
                            || text.ends_with('.') || text.ends_with('!')
                            || text.ends_with('?') || text.ends_with('\n');
                        if should_emit {
                            let msg = buf.clone();
                            buf.clear();
                            Some(seneschal_tui::events::TuiEvent::AgentTaskRunning {
                                task_id: session_id.clone(),
                                objective: msg,
                            })
                        } else {
                            None
                        }
                    }
                    SessionEvent::Error { session_id, message, .. } => {
                        Some(seneschal_tui::events::TuiEvent::AgentTaskFailed {
                            task_id: session_id.clone(),
                            message: message.clone(),
                        })
                    }
                    _ => None,
                };
                if let Some(ev) = tui_event {
                    if tui_tx_clone.send(ev).is_err() {
                        break; // TUI closed
                    }
                }
            }
        });
    }
    ```
  - Acceptance criteria: `cargo check --features tui` passes. The bridge task compiles and integrates with the existing channel types.

---

## Phase 8: QA — verify no regressions

- [x] Step 8.1: Run `make qa` and fix any issues
  - File(s): N/A (code quality)
  - Change: Execute `make qa` from the repository root. This runs `cargo fmt --check`, `cargo clippy --all-targets --no-deps -- -D warnings`, `cargo test`, `cargo test --features full`, `cargo test e2e -- --ignored`, and `cargo build --features full`.
  - Fix any formatting, clippy, or compilation errors that arise from the changes.
  - Specifically verify:
    - `cargo test -p seneschal-tui` — all existing TUI tests still pass.
    - `cargo test -p seneschal-agents` — agent tests still pass.
    - `cargo test -p seneschal-common` — common tests still pass.
    - No clippy warnings introduced.
  - If the e2e test fails, inspect the failure. If it's related to the new TUI events, adapt the e2e test to expect them (add new `TuiEvent` variant matching in `test_tui_receives_state_changes` or similar e2e tests).
  - Acceptance criteria: `make qa` passes entirely (or at minimum: `fmt`, `lint`, `test`, `test-ci`, `build` pass).

- [x] Step 8.2: Manual review of the TUI event flow
  - File(s): `src/main.rs`, `crates/seneschal-tui/src/app.rs`, `crates/seneschal-tui/src/ui.rs`
  - Change: No code changes — review-only. Verify:
    1. The bridge task receives `SessionEvent` from `session_manager.create_event_listener()`.
    2. The ProactiveEvent handler now also sends `AgentTaskCompleted`, `AgentTaskPermissionRequested`, and milestone-based events via `tui_tx`.
    3. The TUI `App::handle_tui_event` processes all new variants without panicking.
    4. The `message_lines()` function renders all `AgentTaskStatus` variants with correct colors and labels.
    5. No duplicate events (a milestone + a status change for the same task should not create duplicate messages — the dedup logic in Step 4.1 handles this via `find_agent_task_mut`).
  - Acceptance criteria: Code review passes. All paths are covered.

---

## Notes for the build agent
- The `#[cfg(feature = "tui")]` gates must be used for all TUI-specific code in main.rs.
- When modifying `TuiEvent` enum, keep `#[derive(Clone, Debug)]` and ensure all variants implement `Clone` (Strings, Vecs are already Clone).
- The `AgentTaskInfo` struct and `AgentTaskStatus` enum must also derive `Clone, Debug, PartialEq`.
- The `aggregate_display_lines` or any line-counting function in `ui.rs` must account for the new `Role::AgentTask` variant — check that `message_lines()` handles it exhaustively.
- The `compute_conversation_heights` layout function does not need changes (agent tasks are just more messages in the history).
- If the `SessionEvent` channel is already consumed by something else (check!), the bridge task approach with `create_event_listener` creates a new fan-out channel, so it won't steal events.
