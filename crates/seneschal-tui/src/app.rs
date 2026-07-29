use std::sync::{Arc, Mutex};

#[cfg(test)]
use super::acp_panel::AcpSessionState;
use super::acp_panel::AcpSessionView;
use super::events::{InputSource, PipelineState, TuiEvent};
use seneschal_common::tools::{ConversationMode, PromptBuildState};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

/// Action returned by key event handling.
#[derive(Debug)]
pub enum Action {
    Quit,
    /// Send typed text to the main Seneschal pipeline.
    SubmitToSeneschal(String),
    /// Send typed text to the focused ACP session.
    SubmitToAcp {
        session_id: String,
        agent_name: String,
        text: String,
    },
    ToggleTts,
}

/// Keyboard modal mode (vim-like).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Insert,
}

/// Which pane holds keyboard focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusTarget {
    Conversation,
    SessionStrip,
    SessionDetail,
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
}

/// A single message in the conversation view.
#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Local>,
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
    /// ACP sessions visible in the right panel.
    pub acp_sessions: Vec<AcpSessionView>,
    /// Index into `acp_sessions`.
    pub selected_session: usize,
    /// Focused pane.
    pub focus: FocusTarget,
    /// Normal vs Insert keyboard mode.
    pub input_mode: InputMode,
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
            acp_sessions: Vec::new(),
            selected_session: 0,
            focus: FocusTarget::Conversation,
            input_mode: InputMode::Insert,
        }
    }

    pub fn has_acp_sessions(&self) -> bool {
        !self.acp_sessions.is_empty()
    }

    pub fn selected_acp(&self) -> Option<&AcpSessionView> {
        self.acp_sessions.get(self.selected_session)
    }

    pub fn selected_acp_mut(&mut self) -> Option<&mut AcpSessionView> {
        self.acp_sessions.get_mut(self.selected_session)
    }

    pub fn input_destination_label(&self) -> String {
        if self.submit_targets_acp()
            && let Some(s) = self.selected_acp()
        {
            return format!("→ {}", s.label);
        }
        "→ Seneschal".to_string()
    }

    pub fn submit_targets_acp(&self) -> bool {
        self.has_acp_sessions()
            && matches!(
                self.focus,
                FocusTarget::SessionStrip | FocusTarget::SessionDetail
            )
    }

    fn clamp_selected(&mut self) {
        if self.acp_sessions.is_empty() {
            self.selected_session = 0;
            if matches!(
                self.focus,
                FocusTarget::SessionStrip | FocusTarget::SessionDetail
            ) {
                self.focus = FocusTarget::Conversation;
            }
        } else if self.selected_session >= self.acp_sessions.len() {
            self.selected_session = self.acp_sessions.len() - 1;
        }
    }

    fn cycle_focus(&mut self, forward: bool) {
        if !self.has_acp_sessions() {
            self.focus = FocusTarget::Conversation;
            return;
        }
        let order = [
            FocusTarget::Conversation,
            FocusTarget::SessionStrip,
            FocusTarget::SessionDetail,
        ];
        let idx = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        let next = if forward {
            (idx + 1) % order.len()
        } else {
            (idx + order.len() - 1) % order.len()
        };
        self.focus = order[next];
    }

    /// Process a pipeline event and update app state.
    pub fn handle_tui_event(&mut self, event: TuiEvent) {
        match event {
            TuiEvent::StateChange(s) => {
                self.state = s;
            }
            TuiEvent::UserMessage { text, source } => {
                self.messages.push(ChatMessage {
                    role: Role::User(source),
                    content: text,
                    timestamp: chrono::Local::now(),
                });
            }
            TuiEvent::AssistantToken(token) => {
                self.streaming_buffer.push_str(&token);
            }
            TuiEvent::AssistantDone => {
                if !self.streaming_buffer.is_empty() {
                    let content = std::mem::take(&mut self.streaming_buffer);
                    self.messages.push(ChatMessage {
                        role: Role::Assistant,
                        content,
                        timestamp: chrono::Local::now(),
                    });
                }
            }
            TuiEvent::Error(msg) => {
                self.messages.push(ChatMessage {
                    role: Role::Error,
                    content: msg,
                    timestamp: chrono::Local::now(),
                });
            }
            TuiEvent::SystemNotification { text } => {
                self.messages.push(ChatMessage {
                    role: Role::System,
                    content: text,
                    timestamp: chrono::Local::now(),
                });
            }
            TuiEvent::ToolCall { name, result } => {
                let short = if result.len() > 120 {
                    format!("{}...", &result[..120])
                } else {
                    result
                };
                self.messages.push(ChatMessage {
                    role: Role::Tool,
                    content: format!("{name} -> {short}"),
                    timestamp: chrono::Local::now(),
                });
            }
            TuiEvent::Splash => {
                self.messages.push(ChatMessage {
                    role: Role::Splash,
                    content: String::new(),
                    timestamp: chrono::Local::now(),
                });
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
            TuiEvent::AcpSessionUpsert {
                session_id,
                agent_name,
                label,
                state,
            } => {
                if let Some(existing) = self
                    .acp_sessions
                    .iter_mut()
                    .find(|s| s.session_id == session_id)
                {
                    existing.agent_name = agent_name;
                    existing.label = label;
                    existing.state = state;
                } else {
                    let is_first = self.acp_sessions.is_empty();
                    self.acp_sessions
                        .push(AcpSessionView::new(session_id, agent_name, label, state));
                    if is_first {
                        self.selected_session = 0;
                    }
                }
            }
            TuiEvent::AcpSessionLog {
                session_id,
                line,
                mode,
            } => {
                if let Some(s) = self
                    .acp_sessions
                    .iter_mut()
                    .find(|s| s.session_id == session_id)
                {
                    s.apply_log(mode, line);
                }
            }
            TuiEvent::AcpSessionRemove { session_id } => {
                self.acp_sessions.retain(|s| s.session_id != session_id);
                self.clamp_selected();
            }
        }
    }

    fn take_submit_action(&mut self) -> Option<Action> {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return None;
        }
        self.input.clear();
        self.cursor = 0;
        if self.submit_targets_acp()
            && let Some(s) = self.selected_acp()
        {
            return Some(Action::SubmitToAcp {
                session_id: s.session_id.clone(),
                agent_name: s.agent_name.clone(),
                text,
            });
        }
        Some(Action::SubmitToSeneschal(text))
    }

    /// Process a crossterm key event. Returns an Action if one should be taken.
    pub fn handle_key_event(&mut self, event: Event) -> Option<Action> {
        if let Event::Mouse(_) = event {
            return None;
        }

        let Event::Key(KeyEvent {
            code, modifiers, ..
        }) = event
        else {
            return None;
        };

        // Always-available shortcuts
        if modifiers.contains(KeyModifiers::CONTROL) {
            match code {
                KeyCode::Char('c') => return Some(Action::Quit),
                KeyCode::Char('t') => return Some(Action::ToggleTts),
                KeyCode::Char(d) if d.is_ascii_digit() && d != '0' => {
                    if self.has_acp_sessions() {
                        let n = (d as u8 - b'0') as usize;
                        self.selected_session = (n - 1).min(self.acp_sessions.len() - 1);
                        self.focus = FocusTarget::SessionDetail;
                    }
                    return None;
                }
                _ => {}
            }
        }

        match (modifiers, code) {
            (KeyModifiers::SHIFT, KeyCode::BackTab) | (_, KeyCode::BackTab) => {
                self.cycle_focus(false);
                return None;
            }
            (m, KeyCode::Tab) if !m.contains(KeyModifiers::CONTROL) => {
                self.cycle_focus(true);
                return None;
            }
            _ => {}
        }

        match self.input_mode {
            InputMode::Insert => self.handle_insert_key(code, modifiers),
            InputMode::Normal => self.handle_normal_key(code, modifiers),
        }
    }

    fn handle_insert_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Option<Action> {
        match (modifiers, code) {
            (_, KeyCode::Esc) => {
                self.input_mode = InputMode::Normal;
                None
            }
            (_, KeyCode::Enter) => self.take_submit_action(),
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

    fn handle_normal_key(&mut self, code: KeyCode, _modifiers: KeyModifiers) -> Option<Action> {
        match code {
            KeyCode::Char('i') => {
                self.input_mode = InputMode::Insert;
                None
            }
            KeyCode::Esc => None,
            KeyCode::Char('j') | KeyCode::Down => {
                match self.focus {
                    FocusTarget::SessionStrip if self.has_acp_sessions() => {
                        let len = self.acp_sessions.len();
                        self.selected_session = (self.selected_session + 1) % len;
                    }
                    FocusTarget::SessionDetail => {
                        if let Some(s) = self.selected_acp_mut() {
                            s.scroll = s.scroll.saturating_add(1);
                        }
                    }
                    _ => {}
                }
                None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                match self.focus {
                    FocusTarget::SessionStrip if self.has_acp_sessions() => {
                        let len = self.acp_sessions.len();
                        self.selected_session = (self.selected_session + len - 1) % len;
                    }
                    FocusTarget::SessionDetail => {
                        if let Some(s) = self.selected_acp_mut() {
                            s.scroll = s.scroll.saturating_sub(1);
                        }
                    }
                    _ => {}
                }
                None
            }
            KeyCode::PageDown => {
                if self.focus == FocusTarget::SessionDetail
                    && let Some(s) = self.selected_acp_mut()
                {
                    s.scroll = s.scroll.saturating_add(10);
                }
                None
            }
            KeyCode::PageUp => {
                if self.focus == FocusTarget::SessionDetail
                    && let Some(s) = self.selected_acp_mut()
                {
                    s.scroll = s.scroll.saturating_sub(10);
                }
                None
            }
            KeyCode::Enter => {
                if self.focus == FocusTarget::SessionStrip && self.has_acp_sessions() {
                    self.focus = FocusTarget::SessionDetail;
                }
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

    fn push_two_sessions(app: &mut App) {
        app.handle_tui_event(TuiEvent::AcpSessionUpsert {
            session_id: "s1".into(),
            agent_name: "hermes".into(),
            label: "hermes".into(),
            state: AcpSessionState::Active,
        });
        app.handle_tui_event(TuiEvent::AcpSessionUpsert {
            session_id: "s2".into(),
            agent_name: "oracle".into(),
            label: "oracle".into(),
            state: AcpSessionState::Idle,
        });
    }

    #[test]
    fn insert_char_grows_input() {
        let mut app = test_app();
        assert_eq!(app.input_mode, InputMode::Insert);
        app.handle_key_event(key(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(app.input, "a");
    }

    #[test]
    fn esc_enters_normal_j_does_not_insert() {
        let mut app = test_app();
        app.handle_key_event(key(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.input_mode, InputMode::Normal);
        app.handle_key_event(key(KeyCode::Char('j'), KeyModifiers::NONE));
        assert!(app.input.is_empty());
    }

    #[test]
    fn normal_i_enters_insert() {
        let mut app = test_app();
        app.input_mode = InputMode::Normal;
        app.handle_key_event(key(KeyCode::Char('i'), KeyModifiers::NONE));
        assert_eq!(app.input_mode, InputMode::Insert);
    }

    #[test]
    fn ctrl_2_selects_second_session() {
        let mut app = test_app();
        push_two_sessions(&mut app);
        app.handle_key_event(key(KeyCode::Char('2'), KeyModifiers::CONTROL));
        assert_eq!(app.selected_session, 1);
        assert_eq!(app.focus, FocusTarget::SessionDetail);
    }

    #[test]
    fn tab_cycles_focus_with_sessions() {
        let mut app = test_app();
        push_two_sessions(&mut app);
        assert_eq!(app.focus, FocusTarget::Conversation);
        app.handle_key_event(key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focus, FocusTarget::SessionStrip);
        app.handle_key_event(key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focus, FocusTarget::SessionDetail);
        app.handle_key_event(key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focus, FocusTarget::Conversation);
    }

    #[test]
    fn enter_submit_seneschal_when_conversation_focused() {
        let mut app = test_app();
        app.input = "hello".into();
        app.cursor = 5;
        let action = app.handle_key_event(key(KeyCode::Enter, KeyModifiers::NONE));
        match action {
            Some(Action::SubmitToSeneschal(t)) => assert_eq!(t, "hello"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn enter_submit_acp_when_detail_focused() {
        let mut app = test_app();
        push_two_sessions(&mut app);
        app.focus = FocusTarget::SessionDetail;
        app.input = "allow".into();
        app.cursor = 5;
        let action = app.handle_key_event(key(KeyCode::Enter, KeyModifiers::NONE));
        match action {
            Some(Action::SubmitToAcp {
                session_id,
                agent_name,
                text,
            }) => {
                assert_eq!(session_id, "s1");
                assert_eq!(agent_name, "hermes");
                assert_eq!(text, "allow");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn upsert_log_remove() {
        let mut app = test_app();
        app.handle_tui_event(TuiEvent::AcpSessionUpsert {
            session_id: "s1".into(),
            agent_name: "hermes".into(),
            label: "hermes".into(),
            state: AcpSessionState::Active,
        });
        app.handle_tui_event(TuiEvent::AcpSessionLog {
            session_id: "s1".into(),
            line: "hi".into(),
            mode: crate::acp_panel::AcpLogMode::Line,
        });
        assert_eq!(app.acp_sessions[0].lines.len(), 1);
        assert_eq!(app.acp_sessions[0].lines[0].text, "hi");
        app.handle_tui_event(TuiEvent::AcpSessionRemove {
            session_id: "s1".into(),
        });
        assert!(app.acp_sessions.is_empty());
    }

    #[test]
    fn destination_label() {
        let mut app = test_app();
        assert_eq!(app.input_destination_label(), "→ Seneschal");
        push_two_sessions(&mut app);
        app.focus = FocusTarget::SessionDetail;
        assert_eq!(app.input_destination_label(), "→ hermes");
    }
}
