# Add TUI section to visualize ACP sessions

## Context
- Origin: Gitea issue #157 — Add a new section in the TUI to visualize ACP sessions
- Summary of what is requested: Side-by-side TUI layout (conversation | ACP). Right panel has a session strip (tabs with status icons) + selected-session detail log. Collapse ACP panel when no sessions. Single shared bottom input whose destination follows focus (`[→ Seneschal]` vs `[→ <session>]`). Vim-like Normal/Insert keyboard modes. Tab/Shift+Tab focus cycle; Ctrl+1..9 jump to session N. Define `AcpSessionState` and wire events so the strip updates live.
- Proposed branch: `feature/issue-157-add-a-new-section-in-the-tui-to-vis`
- Base branch: master
- Assumptions made:
  1. Feature stays behind existing `tui` feature flag (no new Cargo feature).
  2. UI view-model lives in `src/tui/` (`AcpSessionState`, per-session log lines). Domain `SessionStatus` in `session_manager.rs` is extended only as needed for emission; TUI maps domain status → `AcpSessionState`.
  3. Event path: emitters → `SessionEvent` (`session_manager`) → bridge task → `TuiEvent::Acp*` → `App`. Do not give the TUI an `Arc<AcpSessionManager>` (avoids locking writers from the UI thread).
  4. Input to ACP when destination is a session: if that session is `NeedsInput` and a pending oneshot exists, resolve it; otherwise send a follow-up ACP prompt via a small `AcpInputCommand` channel handled outside the TUI draw loop. Do not block the TUI on ACP I/O.
  5. Proportions: 58% conversation / 42% ACP when sessions exist (matches issue sketch). Input + status stay full-width under both columns.
  6. Esc in Insert → Normal (does **not** quit). Esc in Normal with empty input / no special focus still quits only via **Ctrl+C** (change from today where Esc quits always). Document this in the status-bar help text. Keep **Ctrl+C** = Quit always.

## Phase 1: Domain session state + list payload

- [x] Step 1.1: Extend `SessionStatus` with UI-relevant variants
  - File(s): `src/agents/session_manager.rs`
  - Change:
    1. Change `SessionStatus` to:
       ```rust
       pub enum SessionStatus {
           Started,
           Idle,
           Busy,
           NeedsInput,
           Done,
           Error,
           Closed,
       }
       ```
    2. Update `Display` impl: `needs_input`, `done`, `error` for the new variants; keep existing strings for the old ones.
    3. Update **all** match arms / tests in this file that exhaustively match `SessionStatus` (search the file for `SessionStatus::` and `match.*status`).
    4. Add methods if missing (same pattern as existing mark helpers):
       - `mark_session_needs_input(&self, agent_name: &str)`
       - `mark_session_done(&self, agent_name: &str)`
       - `mark_session_error(&self, agent_name: &str)`
       Each sets `entry.status` and updates `last_used = Instant::now()` except `Error`/`Done`/`Closed` may skip `last_used` if you prefer consistency with `mark_session_closed` (match `mark_session_closed` for Closed; for Done/Error/NeedsInput update `last_used`).
  - Acceptance criteria:
    - `cargo test --lib agents::session_manager` passes (or full `cargo test` if that is how tests are named).
    - No exhaustive-match compile errors anywhere in the crate for `SessionStatus`.

- [x] Step 1.2: Enrich `SessionInfo` with status
  - File(s): `src/agents/session_manager.rs`
  - Change:
    1. Add `pub status: SessionStatus` to `SessionInfo`.
    2. In `list_sessions()`, copy `e.status` into each `SessionInfo`.
    3. Fix any tests constructing `SessionInfo` or asserting on its fields.
  - Acceptance criteria:
    - `list_sessions()` returns correct status for busy/idle sessions in existing tests (add one unit test if none assert status: create session, `mark_session_busy`, assert `list_sessions()[0].status == Busy`).

- [x] Step 1.3: Optional event sender on `AcpSessionManager`
  - File(s): `src/agents/session_manager.rs`
  - Change:
    1. Add field `event_tx: Option<SessionEventTx>` to `AcpSessionManager` (default `None` so existing `AcpSessionManager::new()` / `Default` keep working).
    2. Add `pub fn with_event_tx(mut self, tx: SessionEventTx) -> Self` **or** `pub fn set_event_tx(&self, ...)` — prefer constructor-style if `new()` stays simple:
       ```rust
       pub fn new() -> Self { Self::default() }
       pub fn set_event_tx(&mut self, tx: SessionEventTx) { self.event_tx = Some(tx); }
       ```
       Because the manager is wrapped in `Arc` after creation in `main.rs`, use interior mutability: store `event_tx: Arc<std::sync::Mutex<Option<SessionEventTx>>>` **or** set the channel **before** wrapping in `Arc`:
       ```rust
       let mut manager = AcpSessionManager::new();
       manager.set_event_tx(tx);
       let session_manager = Arc::new(manager);
       ```
       Prefer set-before-Arc (no extra Mutex).
    3. Add private helper:
       ```rust
       fn emit(&self, event: SessionEvent) {
           if let Some(tx) = &self.event_tx {
               let _ = tx.try_send(event); // non-blocking; drop if full
           }
       }
       ```
       If `SessionEventTx` is bounded `mpsc::Sender`, `try_send` is correct. Keep capacity 16 from `create_session_event_channel()`.
    4. Call `emit(SessionEvent::Status { ... })` from every `mark_session_*` method with `correlation_id: String::new()` (or a short constant `"status"`).
    5. Call `emit(SessionEvent::Error { ... })` from paths that already surface session failures if easy in this file (e.g. close after failure); primary error emission is Phase 2 in `run_agent.rs`.
  - Acceptance criteria:
    - Unit test: create channel, `set_event_tx`, `mark_session_busy("x")` after inserting a dummy session (use existing test helpers / `AcpWriter::dummy`), recv one `SessionEvent::Status` with `Busy`.
    - Existing session_manager tests still pass.

## Phase 2: Emit session lifecycle + log lines from ACP runtime

- [x] Step 2.1: Mark busy/idle/done/error around `run_acp`
  - File(s): `src/tools/run_agent.rs` (`run_acp` async block ~714–890)
  - Change:
    1. After successful `get_or_create_session`, call `mgr.mark_session_busy(&agent_name)` and `mgr.add_task(&agent_name, &task_id)`.
    2. On every early-return error path that already sends `ProactiveEvent::AgentResult` with an error string, also call `mgr.mark_session_error(&agent_name)` when `session_mgr` is `Some` **and** a session was created; if creation failed, skip mark.
    3. After `collect_acp_response` returns successfully (normal completion), call `mgr.remove_task(...)`, then if the session has no remaining tasks (`get` entry task_ids empty — use `remove_task` then check via a new helper `has_tasks(&self, agent_name) -> bool` **or** existing patterns), `mark_session_idle` if process kept alive (`!owned_process`), else `mark_session_done` then `close_session` as appropriate for owned one-shot processes.
    4. Prefer: managed pool session → end in `Idle`; owned_process → `Done` + kill (already kills).
  - Acceptance criteria:
    - Compiles with `cargo build --features tui`.
    - No double-free / panic; existing run_agent unit tests still pass.

- [x] Step 2.2: Forward streaming ACP chunks as `SessionEvent` log lines
  - File(s): `src/tools/run_agent.rs` (`collect_acp_response` ~1508–1680)
  - Change:
    1. Add optional parameter `session_event_tx: Option<crate::agents::SessionEventTx>` to `collect_acp_response` (and thread it from `run_acp`). Obtain tx by cloning from manager if you add `AcpSessionManager::event_sender(&self) -> Option<SessionEventTx>` (clone the sender).
    2. On `agent_message_chunk`: after appending to `accumulated_text`, `try_send` `SessionEvent::AgentMessage { agent_name, session_id, text: chunk.to_string(), correlation_id: task_id.clone() }`.
    3. On `agent_thought_chunk`: `try_send` a line prefixed in text with e.g. `"thinking: {text}"` via `AgentMessage` **or** reuse `AgentMessage` with a clear prefix — do **not** add a new SessionEvent variant unless necessary.
    4. On `tool_call` / `tool_call_update`: `try_send` `SessionEvent::ToolCall` / `SessionEvent::ToolResult` (for update with status string as `result`).
    5. On `session/request_permission` **before** waiting on oneshot: if manager available, `mark_session_needs_input(&agent_name)` and emit `SessionEvent::Status { status: NeedsInput, ... }`. After permission resolved, `mark_session_busy` again.
    6. Pass `session_id` into collect (already have `_session_id` — rename to `session_id` and use it).
  - Acceptance criteria:
    - Permission path still auto-routes via `ProactiveEvent::AgentQuestion` (unchanged voice path).
    - `cargo test` for run_agent / session_events still passes.
    - Clippy clean on touched signatures (`#[allow(clippy::too_many_arguments)]` already present — keep it).

- [x] Step 2.3: Checkpoint compile
  - File(s): none (verify only)
  - Change: Run `cargo build --features tui,remote,control` and `cargo test --lib`.
  - Acceptance criteria: build + lib tests green before TUI work.

## Phase 3: TUI model — state, events, focus, input mode

- [x] Step 3.1: Add `AcpSessionState` and view types in TUI
  - File(s): create `src/tui/acp_panel.rs` (new module); update `src/tui/mod.rs` with `mod acp_panel;`
  - Change: Define exactly:
    ```rust
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum AcpSessionState {
        Active,      // working — maps from Busy/Started
        NeedsInput,  // waiting on user
        Done,        // finished cleanly
        Error,       // failed
        Idle,        // alive but idle (optional display as dim Active/gray)
    }

    #[derive(Clone, Debug)]
    pub struct AcpSessionView {
        pub session_id: String,
        pub agent_name: String,
        pub label: String,          // short strip label, e.g. agent_name or first 12 chars of task
        pub state: AcpSessionState,
        pub lines: Vec<String>,     // detail log, newest at end; cap at 500 lines
        pub scroll: u16,            // lines scrolled up from bottom (0 = pinned to end)
    }

    impl AcpSessionView {
        pub fn push_line(&mut self, line: impl Into<String>) {
            self.lines.push(line.into());
            if self.lines.len() > 500 {
                let excess = self.lines.len() - 500;
                self.lines.drain(0..excess);
            }
        }
    }

    pub fn map_session_status(s: crate::agents::SessionStatus) -> AcpSessionState {
        use crate::agents::SessionStatus::*;
        match s {
            Started | Busy => AcpSessionState::Active,
            NeedsInput => AcpSessionState::NeedsInput,
            Idle => AcpSessionState::Idle,
            Done | Closed => AcpSessionState::Done,
            Error => AcpSessionState::Error,
        }
    }

    /// Icon + color for strip rendering.
    pub fn state_style(state: AcpSessionState) -> (char, ratatui::style::Color) {
        use ratatui::style::Color;
        match state {
            AcpSessionState::Active => ('●', Color::Green),
            AcpSessionState::NeedsInput => ('!', Color::Yellow),
            AcpSessionState::Done => ('✓', Color::Gray),
            AcpSessionState::Error => ('✗', Color::Red),
            AcpSessionState::Idle => ('○', Color::DarkGray),
        }
    }
    ```
  - Acceptance criteria:
    - Module compiles under `feature = "tui"`.
    - Unit tests in `acp_panel.rs`: `map_session_status` for each `SessionStatus` variant; `push_line` caps at 500.

- [x] Step 3.2: Extend `TuiEvent` with ACP variants
  - File(s): `src/tui/events.rs`
  - Change: Append variants (keep existing ones unchanged):
    ```rust
    /// Full replace/upsert of one ACP session row (strip + optional seed lines).
    AcpSessionUpsert {
        session_id: String,
        agent_name: String,
        label: String,
        state: crate::tui::acp_panel::AcpSessionState,
    },
    /// Append one log line to a session detail pane.
    AcpSessionLog {
        session_id: String,
        line: String,
    },
    /// Session removed / closed — drop from strip when Done/Closed after optional delay, or immediately:
    AcpSessionRemove { session_id: String },
    ```
    Re-export or use paths that compile; if `acp_panel` is private, put `AcpSessionState` in `events.rs` instead and re-export from `acp_panel` — **prefer defining `AcpSessionState` in `events.rs` or `acp_panel.rs` once and referencing it**. Avoid circular mods: define state enum in `acp_panel.rs`, import in `events.rs` via `super::acp_panel::AcpSessionState`.
  - Acceptance criteria: `TuiEvent` still `Clone + Debug`; all existing match sites in `app.rs` updated in Step 3.3.

- [x] Step 3.3: Extend `App` with ACP panel + focus + mode fields
  - File(s): `src/tui/app.rs`
  - Change:
    1. Add enums:
       ```rust
       #[derive(Clone, Copy, Debug, PartialEq, Eq)]
       pub enum InputMode { Normal, Insert }

       #[derive(Clone, Copy, Debug, PartialEq, Eq)]
       pub enum FocusTarget {
           Conversation, // left pane (scroll reserved for later; default visual focus)
           SessionStrip,
           SessionDetail,
       }
       ```
    2. Add fields on `App`:
       ```rust
       pub acp_sessions: Vec<crate::tui::acp_panel::AcpSessionView>,
       pub selected_session: usize, // index into acp_sessions; 0 if empty
       pub focus: FocusTarget,
       pub input_mode: InputMode,   // default Insert to preserve current “type immediately” UX on first paint
       ```
       Default: `acp_sessions: vec![]`, `selected_session: 0`, `focus: FocusTarget::Conversation`, `input_mode: InputMode::Insert`.
    3. In `handle_tui_event`:
       - `AcpSessionUpsert`: find by `session_id`; update or push `AcpSessionView` (preserve existing `lines`/`scroll` on update). If new session and it is the only one, set `selected_session` to it.
       - `AcpSessionLog`: find by id, `push_line`.
       - `AcpSessionRemove`: remove by id; clamp `selected_session`.
    4. Helpers on `App`:
       ```rust
       pub fn has_acp_sessions(&self) -> bool { !self.acp_sessions.is_empty() }
       pub fn selected_acp(&self) -> Option<&AcpSessionView> { self.acp_sessions.get(self.selected_session) }
       pub fn input_destination_label(&self) -> String {
           // If focus is SessionStrip or SessionDetail AND sessions non-empty → format!("→ {}", selected.label)
           // Else → "→ Seneschal"
       }
       pub fn submit_targets_acp(&self) -> bool {
           self.has_acp_sessions()
               && matches!(self.focus, FocusTarget::SessionStrip | FocusTarget::SessionDetail)
       }
       ```
  - Acceptance criteria:
    - Unit-testable pure helpers: add `#[cfg(test)]` tests for upsert/log/remove and destination label (construct `App` with dummy mutexes like production `new()`).

## Phase 4: Keyboard — Normal/Insert, focus cycle, session jump

- [x] Step 4.1: Expand `Action` enum
  - File(s): `src/tui/app.rs`
  - Change: Replace/extend `Action` to:
    ```rust
    pub enum Action {
        Quit,
        /// Send typed text to the main Seneschal pipeline (existing behavior).
        SubmitToSeneschal(String),
        /// Send typed text to the focused ACP session (permission answer or follow-up).
        SubmitToAcp { session_id: String, agent_name: String, text: String },
        ToggleTts,
    }
    ```
    Update all match sites (`src/tui/mod.rs`) in Phase 6. Temporarily keep compiling by matching both in mod.rs in the same PR step as 4.2.

- [x] Step 4.2: Rewrite `handle_key_event` with modal behavior
  - File(s): `src/tui/app.rs`
  - Change: Implement this exact key matrix:

    **Always (any mode):**
    - `Ctrl+C` → `Action::Quit`
    - `Ctrl+T` → `Action::ToggleTts`
    - `Ctrl+1`..=`Ctrl+9` → if sessions non-empty, `selected_session = (n-1).min(len-1)`, `focus = SessionDetail`, no Action
    - `Tab` → cycle focus forward among available targets:
      - If no ACP sessions: only `Conversation` (no-op or stay)
      - If ACP sessions: `Conversation` → `SessionStrip` → `SessionDetail` → `Conversation`
    - `Shift+Tab` → reverse cycle

    **Insert mode:**
    - `Esc` → set `input_mode = Normal` (do **not** Quit)
    - `Enter` → if input non-empty after trim: clear input/cursor; if `submit_targets_acp()` { `SubmitToAcp{...}` } else { `SubmitToSeneschal(text)` }
    - Backspace/Delete/Left/Right/Home/End/Char → same as current insert behavior
    - Do **not** treat bare `j`/`k` as scroll in Insert

    **Normal mode:**
    - `i` → `input_mode = Insert`, `focus` unchanged (user types into shared input)
    - `Esc` → stay Normal (no quit)
    - `j` / `Down`: if `focus == SessionStrip` { selected_session = (selected+1) % len } else if `focus == SessionDetail` { selected.scroll = selected.scroll.saturating_add(1) }
    - `k` / `Up`: strip → previous index; detail → `scroll.saturating_sub(1)`
    - `PageDown` / `PageUp`: detail scroll by `10` lines when `focus == SessionDetail`
    - `Enter` in Normal on strip → `focus = SessionDetail`
    - Other printable keys → ignored (do not insert)

    Mouse events: still `None`.

  - Acceptance criteria:
    - Unit tests with synthetic `crossterm::event::KeyEvent` / `Event::Key`:
      1. Insert + char `'a'` grows input.
      2. Insert + Esc → Normal; then `'j'` does not insert `j`.
      3. Normal + `i` → Insert.
      4. With 2 fake sessions, Ctrl+2 selects index 1.
      5. Tab cycles Conversation→Strip→Detail→Conversation when sessions.len()>=1.
      6. Enter in Insert with focus Conversation yields `SubmitToSeneschal`.
      7. Enter in Insert with focus SessionDetail yields `SubmitToAcp`.

## Phase 5: Layout + rendering

- [x] Step 5.1: Refactor top-level layout in `render()`
  - File(s): `src/tui/ui.rs`
  - Change:
    1. Split **full frame** vertically first into: `main_area` (flex) + `input_area` + `status_area` (status always 1 row; input height via existing `input_display_lines` / clamp logic).
    2. Split `main_area` **horizontally**:
       - If `!app.has_acp_sessions()`: single pane 100% = left conversation stack.
       - Else: `Constraint::Percentage(58)`, `Constraint::Percentage(42)`.
    3. Left pane: existing vertical stack of history / streaming / prompt-build using a **refactored** `compute_layout_heights` that takes the left pane height (not full frame). Reuse the same function; only the `total_h` argument changes (exclude input+status already).
    4. Right pane (only if sessions): vertical split:
       ```rust
       let strip_h = (app.acp_sessions.len() as u16).saturating_add(2).min(area.height); // +2 borders/title
       // constraints: Length(strip_h), Min(0) for detail
       ```
    5. Call new render functions (Step 5.2–5.3).
    6. `render_input` / `render_status` use full width bottom regions.
  - Acceptance criteria:
    - Existing `compute_layout_heights` tests still pass (signature may gain no new params if you only change the caller’s `total_h`).
    - Visual invariant test: add unit test pure function e.g. `fn split_main(has_acp: bool) -> Vec<u16>` percentages 100 vs 58/42 — or test via extracting:
      ```rust
      pub(crate) fn acp_column_percent(has_sessions: bool) -> Option<u16> {
          if has_sessions { Some(42) } else { None }
      }
      ```

- [x] Step 5.2: Render session strip
  - File(s): `src/tui/ui.rs` (or `acp_panel.rs` with `pub fn render_session_strip(...)`)
  - Change:
    - Title line: `SESIONES ACP` + hint `[Tab]`.
    - One row per session: `[{idx+1}] {label} {icon} {state_word}` using `state_style`.
    - Highlight selected row with `Modifier::REVERSED` or bold + different bg `Color::Rgb(40,40,60)`.
    - If `focus == SessionStrip`, draw block border in Cyan; else Gray.
    - NeedsInput: icon `!` in Yellow; optional `Modifier::SLOW_BLINK` only if you verify terminal support — if unsure, bold yellow without blink.
  - Acceptance criteria: Compiles; no panic on empty (caller must not call when empty).

- [x] Step 5.3: Render session detail
  - File(s): `src/tui/ui.rs` or `acp_panel.rs`
  - Change:
    - Header: `{label} · {agent_name}` truncated to width.
    - Body: join `lines` with word wrap (`word_wrap_plain`), auto-scroll to bottom unless `scroll > 0` (skip `scroll` lines from the end).
    - Prefix agent lines with `> ` dim; needs-input prompts with `? ` yellow (detect lines starting with `?` or a flag — simplest: log permission lines already prefixed with `? ` in Phase 2 emission).
    - Border color Cyan when `focus == SessionDetail`.
  - Acceptance criteria: Long logs do not panic; empty lines show placeholder `(sin actividad)`.

- [x] Step 5.4: Update input + status chrome
  - File(s): `src/tui/ui.rs` (`render_input`, `render_status`)
  - Change:
    1. `render_input`: show destination badge on the placeholder/first line, e.g. `┌ [→ Seneschal] Type...` or `┌ [→ issue-134] ...`. When `input_mode == Normal`, show a dim suffix ` -- NORMAL (i=insert)` and **do not** set cursor (or set cursor off). When Insert, keep cursor positioning as today.
    2. `render_status`: extend help string to  
       `Ctrl+T TTS  Tab focus  i insert  Esc normal  Ctrl+C quit`  
       and show `MODE:INSERT|NORMAL` and `FOCUS:...` abbreviated.
  - Acceptance criteria: Cursor only visible in Insert mode.

## Phase 6: Wire channels in main + TUI event loop

- [x] Step 6.1: Bridge `SessionEvent` → `TuiEvent`
  - File(s): `src/main.rs` (TUI spawn section ~1210–1231); optionally small helper in `src/tui/mod.rs`
  - Change:
    1. Before `Arc::new(AcpSessionManager::new())` (search `AcpSessionManager::new` ~line 311), create:
       ```rust
       let (session_event_tx, mut session_event_rx) = agents::create_session_event_channel();
       let mut session_manager_inner = AcpSessionManager::new();
       session_manager_inner.set_event_tx(session_event_tx);
       let session_manager = Arc::new(session_manager_inner);
       ```
       Adjust to match whatever constructor API Phase 1 defined.
    2. Under `#[cfg(feature = "tui")]`, spawn a bridge task:
       ```rust
       let tui_tx_bridge = tui_tx.clone();
       tokio::spawn(async move {
           while let Some(ev) = session_event_rx.recv().await {
               let mapped = map_session_event_to_tui(ev); // local fn
               for e in mapped { let _ = tui_tx_bridge.send(e); }
           }
       });
       ```
    3. Implement `map_session_event_to_tui(ev: SessionEvent) -> Vec<TuiEvent>` in `src/tui/acp_panel.rs` or `main.rs`:
       - `Status` → `AcpSessionUpsert { state: map_session_status(status), label: agent_name.clone(), ... }`
       - `AgentMessage` → `AcpSessionLog { line: text }` + ensure upsert exists (emit upsert if needed)
       - `UserMessage` → log with prefix `"tú: "`
       - `ToolCall` → log `"⚙ {tool_name}"`
       - `ToolResult` → log `"⚙ {tool_name} → {truncated result}"`
       - `Error` → `AcpSessionUpsert` state Error + log line
       - On `Status { Closed | Done }` → optionally `AcpSessionRemove` after upsert, **or** keep Done rows until replaced — **keep Done/Error rows visible** (no auto-remove) so the user can read them; only `Closed` may `AcpSessionRemove`.
  - Acceptance criteria: Without TUI feature, session manager still builds (cfg-gate the bridge only; event_tx may still be set only when tui feature on).

- [x] Step 6.2: Handle new `Action`s in `tui::run`
  - File(s): `src/tui/mod.rs`
  - Change:
    1. Extend `run(...)` signature with:
       ```rust
       acp_input_tx: Option<mpsc::Sender<AcpInputCommand>>,
       ```
       Define in `src/tui/acp_panel.rs` or `events.rs`:
       ```rust
       pub struct AcpInputCommand {
           pub session_id: String,
           pub agent_name: String,
           pub text: String,
       }
       ```
    2. Match:
       - `SubmitToSeneschal(text)` → existing `transcript_tx.send(PipelineFrame::TextInput { text })`
       - `SubmitToAcp { .. }` → if let Some(tx) = &acp_input_tx { tx.send(AcpInputCommand{...}).await.ok(); } else { fallback SubmitToSeneschal or log error line via app }
    3. Update `main.rs` TUI spawn to pass `Some(acp_input_tx)` and spawn a consumer task that:
       - Looks up session via `session_manager` (clone Arc into this task)
       - Locks `writer` and `send_prompt(&session_id, &text).await`
       - Emits `SessionEvent::UserMessage` via manager emit if available
       - **Permission shortcut:** if you store pending permission responders, resolve them first — **minimum viable:** always `send_prompt` for follow-up; voice `AgentQuestion` path remains for permissions. Optional improvement (same step if small): hold `Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>` filled when `AgentQuestion` fires and TUI `SubmitToAcp` sends option id string into oneshot when map has entry for `agent_name`/`task_id`.
       
       **MVP required:** follow-up `send_prompt` path only is enough if wiring permission map is large; add a TODO comment only if permission map slips — prefer implementing the HashMap bridge in this step because NeedsInput is a headline status:
       - In main proactive loop where `AgentQuestion` is handled (~1414), also insert into `pending_acp_answers` map keyed by `agent_name`.
       - ACP input task: if map remove yields tx, `tx.send(text)` instead of send_prompt.
  - Acceptance criteria:
    - Typed message to Seneschal still works end-to-end.
    - `cargo build --features tui` succeeds.

- [x] Step 6.3: Emit UserMessage SessionEvent when run_acp sends prompt
  - File(s): `src/tools/run_agent.rs`
  - Change: After successful `send_prompt`, emit `SessionEvent::UserMessage` with the task/query text (truncate to 200 chars for the log if longer).
  - Acceptance criteria: Opening a delegated task populates the ACP detail pane with the user task line.

## Phase 7: Tests + QA polish

- [x] Step 7.1: Layout height regression tests
  - File(s): `src/tui/ui.rs` `#[cfg(test)]`
  - Change: Keep existing `compute_layout_heights` sum invariant tests. Add test that when simulating outer split, `history+streaming+prompt` heights are computed from `main_h` not full terminal height (call `compute_layout_heights(main_h, ...)` and assert sum == `main_h` with status/input excluded — document that status/input are outer).
  - Acceptance criteria: `cargo test --features tui` (or default if tui tests cfg-gated) passes.

- [x] Step 7.2: App key + event unit tests
  - File(s): `src/tui/app.rs` tests module
  - Change: Cover Phase 4 acceptance list and Phase 3 upsert/log/remove.
  - Acceptance criteria: All new tests green.

- [x] Step 7.3: Format, lint, full QA subset
  - File(s): none
  - Change: Run:
    ```bash
    cargo fmt
    cargo clippy --all-targets --no-deps --features tui,remote,control -- -D warnings
    cargo test --features tui,remote,control
    ```
  - Acceptance criteria: fmt/clippy/tests clean for the feature set used in CI (`make qa` if time permits).

## Phase 8: Docs touch (minimal)

- [x] Step 8.1: Document TUI ACP keys
  - File(s): `readme.md` (TUI section if present) **or** `doc/common-workflows.md` — only if an existing TUI keybinding section exists (search `Ctrl+T` in docs). If found, add 5–10 lines for Tab/focus/Normal/Insert/Ctrl+N. If not found, skip file creation; status bar is enough.
  - Acceptance criteria: No new orphan doc files; no mention of trademarked names other than Seneschal.

## Implementation order note for build agent

1. Phase 1 → 2 → checkpoint  
2. Phase 3 → 4 (App compiles with tests, UI still old layout OK)  
3. Phase 5 (layout)  
4. Phase 6 (wire main)  
5. Phase 7–8  

Do **not** invent alternate layouts (no stacked horizontal ACP). Do **not** add a second input widget. Do **not** poll `AcpSessionManager` every frame; use events.
)
