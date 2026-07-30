use std::sync::{Arc, Mutex};

use super::events::{InputSource, PipelineState, TuiEvent};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use seneschal_common::tools::{ConversationMode, PromptBuildState};

/// Action returned by key event handling.
#[derive(Debug)]
pub enum Action {
    Quit,
    /// Send typed text to the main Seneschal pipeline.
    SubmitToSeneschal(String),
    ToggleTts,
}

/// Keyboard modal mode (vim-like).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Insert,
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

/// A single message in the conversation view.
#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Local>,
    /// Agent task metadata (only meaningful when role == AgentTask).
    pub agent_task: Option<AgentTaskInfo>,
}

impl ChatMessage {
    /// Convenience constructor for agent task messages.
    pub fn agent_task(info: AgentTaskInfo, content: String) -> Self {
        Self {
            role: Role::AgentTask,
            content,
            timestamp: chrono::Local::now(),
            agent_task: Some(info),
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
            input_mode: InputMode::Insert,
        }
    }

    pub fn input_destination_label(&self) -> String {
        "→ Seneschal".to_string()
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
                    agent_task: None,
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
                        agent_task: None,
                    });
                }
            }
            TuiEvent::Error(msg) => {
                self.messages.push(ChatMessage {
                    role: Role::Error,
                    content: msg,
                    timestamp: chrono::Local::now(),
                    agent_task: None,
                });
            }
            TuiEvent::SystemNotification { text } => {
                self.messages.push(ChatMessage {
                    role: Role::System,
                    content: text,
                    timestamp: chrono::Local::now(),
                    agent_task: None,
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
                    agent_task: None,
                });
            }
            TuiEvent::Splash => {
                self.messages.push(ChatMessage {
                    role: Role::Splash,
                    content: String::new(),
                    timestamp: chrono::Local::now(),
                    agent_task: None,
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
            // Agent task events — handled properly in Step 4.1.
            _ => {}
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
                _ => {}
            }
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
    fn destination_label() {
        let app = test_app();
        assert_eq!(app.input_destination_label(), "→ Seneschal");
    }
}
