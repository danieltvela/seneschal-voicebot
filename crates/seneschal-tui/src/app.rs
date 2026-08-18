use std::sync::{Arc, Mutex};

use super::events::{InputSource, PipelineState, TuiEvent};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEventKind};
use ratatui::layout::Rect;
use seneschal_common::tools::{ConversationMode, PromptBuildState};

/// Action returned by key event handling.
#[derive(Debug)]
pub enum Action {
    Quit,
    /// Send typed text to the main Seneschal pipeline.
    SubmitToSeneschal(String),
    ToggleTts,
    /// Re-pin the chat view to the bottom and re-enable auto-follow.
    ScrollToBottom,
}

/// Role label for conversation messages.
#[derive(Clone, Debug, PartialEq)]
pub enum Role {
    User(InputSource),
    Assistant,
    Tool,
    Error,
    System,
    Splash,
    AgentTask,
}

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

/// Metadata for agent task messages (only meaningful when role == AgentTask).
#[derive(Clone, Debug)]
pub struct AgentTaskInfo {
    pub task_id: String,
    pub agent_name: String,
    pub status: AgentTaskStatus,
    pub options: Vec<String>,
}

/// A clickable segment of the status bar.
#[derive(Clone, Debug)]
pub struct StatusBarSegment {
    #[allow(dead_code)]
    pub label: String,
    pub action: StatusBarAction,
    pub region: Rect,
}

/// Actions that can be triggered by clicking on a status bar segment.
#[derive(Clone, Debug, PartialEq)]
pub enum StatusBarAction {
    ToggleTts,
    ScrollToBottom,
}

/// A single message in the conversation view.
#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Local>,
    /// Agent task metadata (only meaningful when role == AgentTask).
    pub agent_task: Option<AgentTaskInfo>,
    /// Whether the message is currently collapsed (shows a 1-line summary).
    pub collapsed: bool,
    /// Whether the user can collapse/expand this message via click or key.
    pub expandable: bool,
}

impl ChatMessage {
    pub fn new(role: Role, content: String) -> Self {
        let expandable = matches!(
            role,
            Role::Tool | Role::Error | Role::System | Role::AgentTask
        );
        let collapsed = matches!(role, Role::Tool | Role::AgentTask);
        Self {
            role,
            content,
            timestamp: chrono::Local::now(),
            agent_task: None,
            collapsed,
            expandable,
        }
    }

    /// Convenience constructor for agent task messages.
    pub fn agent_task(info: AgentTaskInfo, content: String) -> Self {
        Self {
            role: Role::AgentTask,
            content,
            timestamp: chrono::Local::now(),
            agent_task: Some(info),
            collapsed: true,
            expandable: true,
        }
    }
}

/// TUI application state.
pub struct App {
    /// Finalized conversation messages.
    pub messages: Vec<ChatMessage>,
    /// Current streaming assistant text (accumulates tokens).
    pub streaming_buffer: String,
    /// Current pipeline state.
    pub state: PipelineState,
    /// Text input buffer.
    pub input: String,
    /// Cursor position within input.
    pub cursor: usize,
    /// TTS enabled.
    pub tts_enabled: bool,
    /// Whether the app should quit.
    pub should_quit: bool,
    /// Shared conversation mode — read each render tick directly from the pipeline.
    pub conv_mode: Arc<Mutex<ConversationMode>>,
    /// Shared prompt-build state — read each render tick directly from the pipeline.
    pub prompt_build_state: Arc<Mutex<PromptBuildState>>,
    /// Scroll offset in lines (first visible line in the history view).
    pub scroll_offset: usize,
    /// If true, the chat view will auto-scroll to the bottom whenever new
    /// content is rendered. Manual scroll up disables this; scroll to bottom
    /// or message submission re-enables it.
    pub auto_scroll_to_bottom: bool,
    /// Per-message display line ranges computed during the last render (used for click mapping).
    pub item_line_ranges: Vec<(usize, usize)>,
    /// Total number of display lines in the chat history (for scrollbar).
    pub total_chat_lines: usize,
    /// Number of lines hidden below the current viewport (derived each frame
    /// when the view is not pinned to the bottom). Drives the "↓ N nuevos"
    /// status-bar indicator.
    pub new_lines_below: usize,
    /// Status bar segment regions computed during the last render (used for click mapping).
    pub status_bar_segments: Vec<StatusBarSegment>,
    /// The history area from the last render (used for mouse click mapping).
    pub history_area: Rect,
}

impl App {
    pub fn new(
        conv_mode: Arc<Mutex<ConversationMode>>,
        prompt_build_state: Arc<Mutex<PromptBuildState>>,
    ) -> Self {
        Self {
            input: String::new(),
            cursor: 0,
            messages: Vec::new(),
            streaming_buffer: String::new(),
            state: PipelineState::Idle,
            tts_enabled: true,
            conv_mode,
            prompt_build_state,
            should_quit: false,
            scroll_offset: 0,
            auto_scroll_to_bottom: true,
            item_line_ranges: Vec::new(),
            total_chat_lines: 0,
            new_lines_below: 0,
            status_bar_segments: Vec::new(),
            history_area: Rect::default(),
        }
    }

    pub fn input_destination_label(&self) -> String {
        "→ Seneschal".to_string()
    }

    /// Hard cap for UI history rehydration so large sessions do not freeze the TUI.
    pub const UI_HISTORY_HARD_CAP: usize = 100;

    /// Seed the chat buffer with messages restored from persistent session history.
    ///
    /// Roles are matched case-insensitively (`User`/`user`, etc.). `ToolExchanges`
    /// and other non-display roles are skipped. When `history` exceeds
    /// [`UI_HISTORY_HARD_CAP`], only the most recent messages are kept.
    ///
    /// Returns the number of messages actually appended.
    pub fn seed_history(&mut self, history: &[(String, String)]) -> usize {
        let slice = if history.len() > Self::UI_HISTORY_HARD_CAP {
            &history[history.len() - Self::UI_HISTORY_HARD_CAP..]
        } else {
            history
        };

        let mut seeded = 0usize;
        for (role, content) in slice {
            if content.is_empty() {
                continue;
            }
            let chat_role = match role.trim().to_ascii_lowercase().as_str() {
                "user" => Role::User(InputSource::Text),
                "assistant" => Role::Assistant,
                "system" => Role::System,
                // Skip ToolExchanges and any unknown role to avoid dumping JSON blobs.
                _ => continue,
            };
            self.messages
                .push(ChatMessage::new(chat_role, content.clone()));
            seeded += 1;
        }
        self.auto_scroll_to_bottom = true;
        seeded
    }

    /// Find an existing agent task message by task_id, returning a mutable reference.
    fn find_agent_task_mut(&mut self, task_id: &str) -> Option<&mut ChatMessage> {
        self.messages
            .iter_mut()
            .rev()
            .find(|msg| msg.agent_task.as_ref().map(|i| i.task_id.as_str()) == Some(task_id))
    }

    /// Process a pipeline event and update app state.
    pub fn handle_tui_event(&mut self, event: TuiEvent) {
        match event {
            TuiEvent::StateChange(s) => {
                self.state = s;
            }
            TuiEvent::UserMessage { text, source } => {
                self.messages
                    .push(ChatMessage::new(Role::User(source), text));
                self.auto_scroll_to_bottom = true;
            }
            TuiEvent::AssistantToken(token) => {
                self.streaming_buffer.push_str(&token);
            }
            TuiEvent::AssistantDone => {
                if !self.streaming_buffer.is_empty() {
                    let content = std::mem::take(&mut self.streaming_buffer);
                    self.messages
                        .push(ChatMessage::new(Role::Assistant, content));
                }
            }
            TuiEvent::Error(msg) => {
                self.messages.push(ChatMessage::new(Role::Error, msg));
            }
            TuiEvent::SystemNotification { text } => {
                self.messages.push(ChatMessage::new(Role::System, text));
            }
            TuiEvent::ToolCall { name, result } => {
                let short = if result.len() > 120 {
                    format!("{}...", &result[..120])
                } else {
                    result
                };
                self.messages
                    .push(ChatMessage::new(Role::Tool, format!("{name} -> {short}")));
            }
            TuiEvent::Splash => {
                self.messages
                    .push(ChatMessage::new(Role::Splash, String::new()));
            }
            TuiEvent::PromptBuildUpdate { prompt: new_text } => {
                let mut state = self.prompt_build_state.lock().unwrap();
                if let PromptBuildState::Active { ref mut prompt, .. } = *state {
                    *prompt = new_text;
                }
            }
            TuiEvent::PromptBuildStateChange { active } => {
                let mut state = self.prompt_build_state.lock().unwrap();
                if active {
                    if !state.is_active() {
                        *state = PromptBuildState::Active {
                            prompt: String::new(),
                        };
                    }
                } else {
                    *state = PromptBuildState::Inactive;
                }
            }
            // Agent task lifecycle events (timeline.inline — qwen-audio-agent style).
            TuiEvent::AgentTaskStarted {
                task_id,
                agent_name,
                objective,
            } => {
                // Dedup: skip if already exists and not terminal.
                if let Some(existing) = self.find_agent_task_mut(&task_id)
                    && !matches!(
                        existing.agent_task.as_ref().map(|i| &i.status),
                        Some(AgentTaskStatus::Completed) | Some(AgentTaskStatus::Failed)
                    )
                {
                    return;
                }
                self.messages.push(ChatMessage::agent_task(
                    AgentTaskInfo {
                        task_id,
                        agent_name: agent_name.clone(),
                        status: AgentTaskStatus::Started,
                        options: vec![],
                    },
                    format!("[{agent_name}] {objective}"),
                ));
            }
            TuiEvent::AgentTaskRunning { task_id, objective } => {
                if let Some(msg) = self.find_agent_task_mut(&task_id) {
                    if let Some(ref mut info) = msg.agent_task {
                        info.status = AgentTaskStatus::Running;
                    }
                    msg.content = objective;
                } else {
                    self.messages.push(ChatMessage::agent_task(
                        AgentTaskInfo {
                            task_id,
                            agent_name: String::new(),
                            status: AgentTaskStatus::Running,
                            options: vec![],
                        },
                        objective,
                    ));
                }
            }
            TuiEvent::AgentTaskDelegated { task_id, objective } => {
                let content = format!("[Proyecto en ejecución] {objective}");
                if let Some(msg) = self.find_agent_task_mut(&task_id) {
                    if let Some(ref mut info) = msg.agent_task {
                        info.status = AgentTaskStatus::Delegated;
                    }
                    msg.content = content;
                } else {
                    self.messages.push(ChatMessage::agent_task(
                        AgentTaskInfo {
                            task_id,
                            agent_name: String::new(),
                            status: AgentTaskStatus::Delegated,
                            options: vec![],
                        },
                        content,
                    ));
                }
            }
            TuiEvent::AgentTaskFinalizing { task_id, objective } => {
                if let Some(msg) = self.find_agent_task_mut(&task_id) {
                    if let Some(ref mut info) = msg.agent_task {
                        info.status = AgentTaskStatus::Finalizing;
                    }
                    msg.content = objective;
                } else {
                    self.messages.push(ChatMessage::agent_task(
                        AgentTaskInfo {
                            task_id,
                            agent_name: String::new(),
                            status: AgentTaskStatus::Finalizing,
                            options: vec![],
                        },
                        objective,
                    ));
                }
            }
            TuiEvent::AgentTaskCompleted {
                task_id,
                objective: _,
                result,
            } => {
                if let Some(msg) = self.find_agent_task_mut(&task_id) {
                    if let Some(ref mut info) = msg.agent_task {
                        info.status = AgentTaskStatus::Completed;
                    }
                    msg.content = result;
                } else {
                    self.messages.push(ChatMessage::agent_task(
                        AgentTaskInfo {
                            task_id,
                            agent_name: String::new(),
                            status: AgentTaskStatus::Completed,
                            options: vec![],
                        },
                        result,
                    ));
                }
            }
            TuiEvent::AgentTaskPermissionRequested {
                task_id,
                agent_name,
                description,
                options,
            } => {
                self.messages.push(ChatMessage::agent_task(
                    AgentTaskInfo {
                        task_id,
                        agent_name: agent_name.clone(),
                        status: AgentTaskStatus::PermissionRequested,
                        options: options.clone(),
                    },
                    description,
                ));
            }
            TuiEvent::AgentTaskFailed { task_id, message } => {
                if let Some(msg) = self.find_agent_task_mut(&task_id) {
                    if let Some(ref mut info) = msg.agent_task {
                        info.status = AgentTaskStatus::Failed;
                    }
                    msg.content = message;
                } else {
                    self.messages.push(ChatMessage::agent_task(
                        AgentTaskInfo {
                            task_id,
                            agent_name: String::new(),
                            status: AgentTaskStatus::Failed,
                            options: vec![],
                        },
                        message,
                    ));
                }
            }
        }
    }

    /// Toggle the collapse state of a message at the given index.
    /// Only works for messages with `expandable == true`.
    pub fn toggle_message(&mut self, index: usize) {
        if let Some(msg) = self.messages.get_mut(index)
            && msg.expandable
        {
            msg.collapsed = !msg.collapsed;
        }
    }

    fn take_submit_action(&mut self) -> Option<Action> {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return None;
        }
        self.input.clear();
        self.cursor = 0;
        Some(Action::SubmitToSeneschal(text))
    }

    /// Maximum scroll offset for the current history viewport. Returns 0 when
    /// the viewport has not been computed yet (pre-render).
    pub fn current_bottom(&self) -> usize {
        if self.history_area.height == 0 {
            0
        } else {
            self.total_chat_lines
                .saturating_sub(self.history_area.height as usize)
        }
    }

    /// Scroll down by `n` lines, clamped to the current bottom. Re-pins
    /// auto-scroll when the bottom is reached.
    pub fn scroll_down_lines(&mut self, n: usize) {
        let bottom = self.current_bottom();
        self.scroll_offset = self.scroll_offset.saturating_add(n).min(bottom);
        if self.scroll_offset >= bottom {
            self.auto_scroll_to_bottom = true;
        }
    }

    /// Scroll up by `n` lines (saturating). Disables auto-scroll so the user
    /// can browse history without being yanked back to the bottom.
    pub fn scroll_up_lines(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        self.auto_scroll_to_bottom = false;
    }

    /// Process a crossterm event. Returns an Action if one should be taken.
    pub fn handle_event(&mut self, event: Event, history_area: Rect) -> Option<Action> {
        match event {
            Event::Mouse(mouse) => self.handle_mouse_event(mouse, history_area),
            Event::Key(key) => self.handle_key(key),
            _ => None,
        }
    }

    fn handle_mouse_event(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        history_area: Rect,
    ) -> Option<Action> {
        use crossterm::event::MouseButton;
        tracing::info!(
            target: "tui.mouse",
            kind = ?mouse.kind,
            col = mouse.column,
            row = mouse.row,
            "TUI mouse event received"
        );
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let col = mouse.column;
                let row = mouse.row;
                // Check status bar clicks
                for segment in &self.status_bar_segments {
                    if col >= segment.region.x
                        && col < segment.region.right()
                        && row >= segment.region.y
                        && row < segment.region.bottom()
                    {
                        return match segment.action {
                            StatusBarAction::ToggleTts => Some(Action::ToggleTts),
                            StatusBarAction::ScrollToBottom => Some(Action::ScrollToBottom),
                        };
                    }
                }
                // Check history area clicks
                if col >= history_area.x
                    && col < history_area.right()
                    && row >= history_area.y
                    && row < history_area.bottom()
                {
                    let relative_y = (row - history_area.y) as usize + self.scroll_offset;
                    for (i, &(start, end)) in self.item_line_ranges.iter().enumerate() {
                        if relative_y >= start && relative_y < end {
                            self.toggle_message(i);
                            return None;
                        }
                    }
                }
                None
            }
            MouseEventKind::ScrollDown => {
                self.scroll_down_lines(3);
                None
            }
            MouseEventKind::ScrollUp => {
                self.scroll_up_lines(3);
                None
            }
            _ => None,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        let KeyEvent {
            code, modifiers, ..
        } = key;

        if modifiers.contains(KeyModifiers::CONTROL) {
            match code {
                KeyCode::Char('c') => return Some(Action::Quit),
                KeyCode::Char('t') => return Some(Action::ToggleTts),
                _ => {}
            }
        }

        self.handle_insert_key(code, modifiers)
    }

    fn handle_insert_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Option<Action> {
        match (modifiers, code) {
            (_, KeyCode::Esc) => None,
            (_, KeyCode::Enter) => self.take_submit_action(),
            // Scroll with Up/Down when input is empty
            (m, KeyCode::Up) if m.is_empty() && self.input.is_empty() => {
                self.scroll_up_lines(3);
                None
            }
            (m, KeyCode::Down) if m.is_empty() && self.input.is_empty() => {
                self.scroll_down_lines(3);
                None
            }
            (m, KeyCode::PageUp) if m.is_empty() => {
                self.scroll_up_lines(15);
                None
            }
            (m, KeyCode::PageDown) if m.is_empty() => {
                self.scroll_down_lines(15);
                None
            }
            // G jumps to the bottom (re-pins auto-scroll) when input is empty.
            (m, KeyCode::Char(c))
                if (m == KeyModifiers::NONE || m == KeyModifiers::SHIFT)
                    && self.input.is_empty()
                    && c.eq_ignore_ascii_case(&'g') =>
            {
                Some(Action::ScrollToBottom)
            }
            // Space toggles expand/collapse on the last expandable message
            (m, KeyCode::Char(' ')) if m.is_empty() && self.input.is_empty() => {
                for msg in self.messages.iter_mut().rev() {
                    if msg.expandable {
                        msg.collapsed = !msg.collapsed;
                        break;
                    }
                }
                None
            }
            (_, KeyCode::Backspace) => {
                if self.cursor > 0 {
                    let prev = self.input[..self.cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.input.drain(prev..self.cursor);
                    self.cursor = prev;
                }
                None
            }
            (_, KeyCode::Delete) => {
                if self.cursor < self.input.len() {
                    let next = self.input[self.cursor..]
                        .char_indices()
                        .nth(1)
                        .map(|(i, _)| self.cursor + i)
                        .unwrap_or(self.input.len());
                    self.input.drain(self.cursor..next);
                }
                None
            }
            (_, KeyCode::Left) => {
                if self.cursor > 0 {
                    self.cursor = self.input[..self.cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                }
                None
            }
            (_, KeyCode::Right) => {
                if self.cursor < self.input.len() {
                    self.cursor = self.input[self.cursor..]
                        .char_indices()
                        .nth(1)
                        .map(|(i, _)| self.cursor + i)
                        .unwrap_or(self.input.len());
                }
                None
            }
            (_, KeyCode::Home) => {
                self.cursor = 0;
                None
            }
            (_, KeyCode::End) => {
                self.cursor = self.input.len();
                None
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
                self.input.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn test_app() -> App {
        App::new(
            Arc::new(Mutex::new(ConversationMode::Active)),
            Arc::new(Mutex::new(PromptBuildState::Inactive)),
        )
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    #[test]
    fn insert_char_grows_input() {
        let mut app = test_app();
        app.handle_event(key(KeyCode::Char('a'), KeyModifiers::NONE), Rect::default());
        assert_eq!(app.input, "a");
    }

    #[test]
    fn enter_submit_with_esc_does_nothing() {
        let mut app = test_app();
        app.handle_event(key(KeyCode::Esc, KeyModifiers::NONE), Rect::default());
        assert!(app.input.is_empty());
    }

    #[test]
    fn enter_submit_seneschal_when_conversation_focused() {
        let mut app = test_app();
        app.input = "hello".into();
        app.cursor = 5;
        let action = app.handle_event(key(KeyCode::Enter, KeyModifiers::NONE), Rect::default());
        match action {
            Some(Action::SubmitToSeneschal(t)) => assert_eq!(t, "hello"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn destination_label() {
        let app = test_app();
        assert_eq!(app.input_destination_label(), "→ Seneschal");
    }

    #[test]
    fn seed_history_maps_roles_case_insensitive() {
        let mut app = test_app();
        let history = vec![
            ("User".into(), "hola".into()),
            ("assistant".into(), "buenos dias".into()),
            ("SYSTEM".into(), "note".into()),
            ("ToolExchanges".into(), r#"[{"role":"tool"}]"#.into()),
            ("unknown".into(), "skip me".into()),
            ("User".into(), "".into()), // empty skipped
        ];
        let n = app.seed_history(&history);
        assert_eq!(n, 3);
        assert_eq!(app.messages.len(), 3);
        assert!(matches!(
            app.messages[0].role,
            Role::User(InputSource::Text)
        ));
        assert_eq!(app.messages[0].content, "hola");
        assert_eq!(app.messages[1].role, Role::Assistant);
        assert_eq!(app.messages[1].content, "buenos dias");
        assert_eq!(app.messages[2].role, Role::System);
        assert_eq!(app.messages[2].content, "note");
    }

    #[test]
    fn seed_history_empty_is_noop() {
        let mut app = test_app();
        assert_eq!(app.seed_history(&[]), 0);
        assert!(app.messages.is_empty());
    }

    #[test]
    fn seed_history_hard_cap_keeps_most_recent() {
        let mut app = test_app();
        let mut history = Vec::new();
        for i in 0..(App::UI_HISTORY_HARD_CAP + 25) {
            history.push(("User".into(), format!("msg {i}")));
        }
        let n = app.seed_history(&history);
        assert_eq!(n, App::UI_HISTORY_HARD_CAP);
        assert_eq!(app.messages.len(), App::UI_HISTORY_HARD_CAP);
        // First kept message is the 26th overall (index 25).
        assert_eq!(app.messages[0].content, "msg 25");
        assert_eq!(
            app.messages.last().unwrap().content,
            format!("msg {}", App::UI_HISTORY_HARD_CAP + 24)
        );
    }

    // scroll helpers — issue #219

    #[test]
    fn current_bottom_is_zero_when_history_area_unset() {
        // history_area == Rect::default() (pre-primer render) -> 0
        let app = test_app();
        assert_eq!(app.history_area, Rect::default());
        assert_eq!(app.current_bottom(), 0);
    }

    #[test]
    fn current_bottom_uses_viewport_height() {
        let mut app = test_app();
        app.history_area = Rect::new(0, 0, 80, 20);
        app.total_chat_lines = 100;
        assert_eq!(app.current_bottom(), 80);
    }

    #[test]
    fn scroll_down_lines_jump_re_pins_when_reaching_bottom() {
        // Partimos con offset lejano y pin desactivado (como si el usuario
        // estuviera leyendo el historico). Un solo paso grande debe clavar
        // offset al bottom y re-activar auto_scroll_to_bottom.
        let mut app = test_app();
        app.history_area = Rect::new(0, 0, 80, 20);
        app.total_chat_lines = 100;
        app.scroll_offset = 10;
        app.auto_scroll_to_bottom = false;
        app.scroll_down_lines(500);
        assert_eq!(app.scroll_offset, 80);
        assert!(app.auto_scroll_to_bottom);
    }

    #[test]
    fn scroll_up_lines_with_pin_active_disables_auto_scroll() {
        // Partimos con pin activo (recien enviado / nuevo arranque). Un scroll
        // up debe desactivar auto_scroll para que el usuario pueda leer.
        let mut app = test_app();
        app.history_area = Rect::new(0, 0, 80, 20);
        app.total_chat_lines = 100;
        app.scroll_offset = 80;
        app.auto_scroll_to_bottom = true;
        app.scroll_up_lines(5);
        assert_eq!(app.scroll_offset, 75);
        assert!(!app.auto_scroll_to_bottom);
    }

    #[test]
    fn key_g_with_empty_input_jumps_to_bottom() {
        // G con input vacio -> Action::ScrollToBottom
        let mut app = test_app();
        let action = app.handle_event(key(KeyCode::Char('g'), KeyModifiers::NONE), Rect::default());
        match action {
            Some(Action::ScrollToBottom) => {}
            other => panic!("expected ScrollToBottom, got {other:?}"),
        }
    }

    #[test]
    fn key_g_with_non_empty_input_inserts_letter() {
        // G con input NO vacio -> inserta la letra 'g' (no se dispara como
        // atajo de scroll). Patron equivalente a Up/Down/Space.
        let mut app = test_app();
        app.input = "hola".into();
        app.cursor = 4;
        let action = app.handle_event(key(KeyCode::Char('g'), KeyModifiers::NONE), Rect::default());
        assert!(
            action.is_none(),
            "G must not produce an action while typing"
        );
        assert_eq!(app.input, "holag");
        assert_eq!(app.cursor, 5);
    }
}
