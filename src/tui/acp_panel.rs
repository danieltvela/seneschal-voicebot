//! ACP session strip + detail view-model for the TUI.

use crate::agents::{SessionEvent, SessionStatus};

/// UI-facing ACP session state (mapped from domain `SessionStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcpSessionState {
    /// Working — maps from Busy/Started.
    Active,
    /// Waiting on user (permission / question).
    NeedsInput,
    /// Finished cleanly.
    Done,
    /// Failed.
    Error,
    /// Alive but idle.
    Idle,
}

/// Kind of a single detail-pane entry (drives styling and stream coalescing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcpEntryKind {
    /// Agent prose (streaming chunks coalesce into one entry).
    Agent,
    /// User / delegated task text.
    User,
    /// Agent thoughts (streaming chunks coalesce).
    Thought,
    /// Tool start/result.
    Tool,
    /// Permission / needs-input prompt.
    Prompt,
    /// Error line.
    Error,
}

/// How a log payload should be applied to the session detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcpLogMode {
    /// Always start a new discrete entry.
    Line,
    /// Append to the open agent stream (or open a new Agent entry).
    AgentStream,
    /// Append to the open thought stream (or open a new Thought entry).
    ThoughtStream,
}

/// One logical log entry in the ACP detail pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcpLogEntry {
    pub kind: AcpEntryKind,
    pub text: String,
}

/// One ACP session row in the TUI strip + detail log.
#[derive(Clone, Debug)]
pub struct AcpSessionView {
    pub session_id: String,
    pub agent_name: String,
    pub label: String,
    pub state: AcpSessionState,
    pub lines: Vec<AcpLogEntry>,
    /// Lines scrolled up from the bottom (0 = pinned to end).
    pub scroll: u16,
    /// Kind of the currently open streaming entry, if any.
    /// Cleared on discrete lines and on real `\n` inside a stream chunk.
    open_stream: Option<AcpEntryKind>,
}

impl AcpSessionView {
    pub fn new(
        session_id: impl Into<String>,
        agent_name: impl Into<String>,
        label: impl Into<String>,
        state: AcpSessionState,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            agent_name: agent_name.into(),
            label: label.into(),
            state,
            lines: Vec::new(),
            scroll: 0,
            open_stream: None,
        }
    }

    fn trim_cap(&mut self) {
        const MAX: usize = 500;
        if self.lines.len() > MAX {
            let excess = self.lines.len() - MAX;
            self.lines.drain(0..excess);
        }
    }

    /// Push a discrete (non-streaming) log entry. Seals any open stream.
    pub fn push_entry(&mut self, kind: AcpEntryKind, text: impl Into<String>) {
        self.open_stream = None;
        self.lines.push(AcpLogEntry {
            kind,
            text: text.into(),
        });
        self.trim_cap();
    }

    /// Backward-compatible helper used by older call sites/tests.
    pub fn push_line(&mut self, line: impl Into<String>) {
        self.push_entry(AcpEntryKind::Agent, line);
    }

    fn append_fragment(&mut self, kind: AcpEntryKind, fragment: &str) {
        if fragment.is_empty() {
            return;
        }
        if self.open_stream == Some(kind)
            && let Some(last) = self.lines.last_mut()
            && last.kind == kind
        {
            last.text.push_str(fragment);
        } else {
            self.lines.push(AcpLogEntry {
                kind,
                text: fragment.to_string(),
            });
            self.open_stream = Some(kind);
        }
    }

    /// Append streaming text, coalescing consecutive chunks of the same kind.
    ///
    /// Embedded `\n` seal the current entry so the next fragment starts a new
    /// paragraph, while tiny token chunks without newlines stay on one line.
    pub fn append_stream(&mut self, kind: AcpEntryKind, chunk: &str) {
        if chunk.is_empty() {
            return;
        }

        let mut rest = chunk;
        while let Some((head, tail)) = rest.split_once('\n') {
            self.append_fragment(kind, head);
            // Real newline: seal stream so following text is a new entry.
            self.open_stream = None;
            rest = tail;
        }
        self.append_fragment(kind, rest);
        self.trim_cap();
    }

    pub fn apply_log(&mut self, mode: AcpLogMode, text: String) {
        match mode {
            AcpLogMode::Line => {
                let kind = classify_line_kind(&text);
                let body = strip_line_prefix(kind, &text);
                self.push_entry(kind, body);
            }
            AcpLogMode::AgentStream => self.append_stream(AcpEntryKind::Agent, &text),
            AcpLogMode::ThoughtStream => {
                // Strip optional "thinking: " prefix from emitters.
                let body = text
                    .strip_prefix("thinking: ")
                    .or_else(|| text.strip_prefix("thinking:"))
                    .unwrap_or(text.as_str());
                self.append_stream(AcpEntryKind::Thought, body);
            }
        }
    }
}

fn classify_line_kind(text: &str) -> AcpEntryKind {
    if text.starts_with('?') {
        AcpEntryKind::Prompt
    } else if text.starts_with("tú:") {
        AcpEntryKind::User
    } else if text.starts_with("thinking:") {
        AcpEntryKind::Thought
    } else if text.starts_with('⚙') || text.starts_with("error:") {
        if text.starts_with("error:") {
            AcpEntryKind::Error
        } else {
            AcpEntryKind::Tool
        }
    } else {
        AcpEntryKind::Agent
    }
}

fn strip_line_prefix(kind: AcpEntryKind, text: &str) -> String {
    match kind {
        AcpEntryKind::User => text
            .strip_prefix("tú: ")
            .or_else(|| text.strip_prefix("tú:"))
            .unwrap_or(text)
            .to_string(),
        AcpEntryKind::Thought => text
            .strip_prefix("thinking: ")
            .or_else(|| text.strip_prefix("thinking:"))
            .unwrap_or(text)
            .to_string(),
        AcpEntryKind::Error => text
            .strip_prefix("error: ")
            .or_else(|| text.strip_prefix("error:"))
            .unwrap_or(text)
            .to_string(),
        _ => text.to_string(),
    }
}

/// Command to send typed text to an ACP session (handled outside the draw loop).
#[derive(Clone, Debug)]
pub struct AcpInputCommand {
    pub session_id: String,
    pub agent_name: String,
    pub text: String,
}

pub fn map_session_status(s: SessionStatus) -> AcpSessionState {
    match s {
        SessionStatus::Started | SessionStatus::Busy => AcpSessionState::Active,
        SessionStatus::NeedsInput => AcpSessionState::NeedsInput,
        SessionStatus::Idle => AcpSessionState::Idle,
        SessionStatus::Done | SessionStatus::Closed => AcpSessionState::Done,
        SessionStatus::Error => AcpSessionState::Error,
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

/// Layout helper: ACP column width percent when sessions exist.
pub fn acp_column_percent(has_sessions: bool) -> Option<u16> {
    if has_sessions { Some(42) } else { None }
}

/// Map a domain `SessionEvent` into zero or more TUI events.
pub fn map_session_event_to_tui(ev: SessionEvent) -> Vec<super::events::TuiEvent> {
    use super::events::TuiEvent;
    match ev {
        SessionEvent::Status {
            agent_name,
            session_id,
            status,
            ..
        } => {
            let state = map_session_status(status);
            let mut out = vec![TuiEvent::AcpSessionUpsert {
                session_id: session_id.clone(),
                agent_name: agent_name.clone(),
                label: agent_name,
                state,
            }];
            if matches!(status, SessionStatus::Closed) {
                out.push(TuiEvent::AcpSessionRemove { session_id });
            }
            out
        }
        SessionEvent::AgentMessage {
            agent_name,
            session_id,
            text,
            ..
        } => {
            // Thought chunks are emitted as AgentMessage with a "thinking: " prefix
            // from collect_acp_response — route them to the thought stream.
            let mode = if text.starts_with("thinking:") {
                AcpLogMode::ThoughtStream
            } else {
                AcpLogMode::AgentStream
            };
            vec![
                TuiEvent::AcpSessionUpsert {
                    session_id: session_id.clone(),
                    agent_name: agent_name.clone(),
                    label: agent_name,
                    state: AcpSessionState::Active,
                },
                TuiEvent::AcpSessionLog {
                    session_id,
                    line: text,
                    mode,
                },
            ]
        }
        SessionEvent::UserMessage {
            agent_name,
            session_id,
            text,
            ..
        } => vec![
            TuiEvent::AcpSessionUpsert {
                session_id: session_id.clone(),
                agent_name: agent_name.clone(),
                label: agent_name,
                state: AcpSessionState::Active,
            },
            TuiEvent::AcpSessionLog {
                session_id,
                line: format!("tú: {text}"),
                mode: AcpLogMode::Line,
            },
        ],
        SessionEvent::ToolCall {
            agent_name,
            session_id,
            tool_name,
            ..
        } => vec![
            TuiEvent::AcpSessionUpsert {
                session_id: session_id.clone(),
                agent_name: agent_name.clone(),
                label: agent_name,
                state: AcpSessionState::Active,
            },
            TuiEvent::AcpSessionLog {
                session_id,
                line: format!("⚙ {tool_name}"),
                mode: AcpLogMode::Line,
            },
        ],
        SessionEvent::ToolResult {
            agent_name,
            session_id,
            tool_name,
            result,
            ..
        } => {
            let short = if result.len() > 80 {
                format!("{}…", &result[..80])
            } else {
                result
            };
            vec![
                TuiEvent::AcpSessionUpsert {
                    session_id: session_id.clone(),
                    agent_name: agent_name.clone(),
                    label: agent_name,
                    state: AcpSessionState::Active,
                },
                TuiEvent::AcpSessionLog {
                    session_id,
                    line: format!("⚙ {tool_name} → {short}"),
                    mode: AcpLogMode::Line,
                },
            ]
        }
        SessionEvent::Error {
            agent_name,
            session_id,
            message,
            ..
        } => vec![
            TuiEvent::AcpSessionUpsert {
                session_id: session_id.clone(),
                agent_name: agent_name.clone(),
                label: agent_name,
                state: AcpSessionState::Error,
            },
            TuiEvent::AcpSessionLog {
                session_id,
                line: format!("error: {message}"),
                mode: AcpLogMode::Line,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_session_status_covers_all_variants() {
        assert_eq!(
            map_session_status(SessionStatus::Busy),
            AcpSessionState::Active
        );
        assert_eq!(
            map_session_status(SessionStatus::Started),
            AcpSessionState::Active
        );
        assert_eq!(
            map_session_status(SessionStatus::NeedsInput),
            AcpSessionState::NeedsInput
        );
        assert_eq!(
            map_session_status(SessionStatus::Idle),
            AcpSessionState::Idle
        );
        assert_eq!(
            map_session_status(SessionStatus::Done),
            AcpSessionState::Done
        );
        assert_eq!(
            map_session_status(SessionStatus::Closed),
            AcpSessionState::Done
        );
        assert_eq!(
            map_session_status(SessionStatus::Error),
            AcpSessionState::Error
        );
    }

    #[test]
    fn push_line_caps_at_500() {
        let mut v = AcpSessionView::new("s", "a", "a", AcpSessionState::Idle);
        for i in 0..600 {
            v.push_line(format!("line-{i}"));
        }
        assert_eq!(v.lines.len(), 500);
        assert_eq!(v.lines[0].text, "line-100");
        assert_eq!(v.lines[499].text, "line-599");
    }

    #[test]
    fn agent_stream_chunks_coalesce_into_one_entry() {
        let mut v = AcpSessionView::new("s", "a", "a", AcpSessionState::Active);
        v.append_stream(AcpEntryKind::Agent, "Hel");
        v.append_stream(AcpEntryKind::Agent, "lo ");
        v.append_stream(AcpEntryKind::Agent, "world");
        assert_eq!(v.lines.len(), 1);
        assert_eq!(v.lines[0].kind, AcpEntryKind::Agent);
        assert_eq!(v.lines[0].text, "Hello world");
    }

    #[test]
    fn agent_stream_newline_starts_new_paragraph() {
        let mut v = AcpSessionView::new("s", "a", "a", AcpSessionState::Active);
        v.append_stream(AcpEntryKind::Agent, "Hello\n");
        v.append_stream(AcpEntryKind::Agent, "World");
        assert_eq!(v.lines.len(), 2);
        assert_eq!(v.lines[0].text, "Hello");
        assert_eq!(v.lines[1].text, "World");
    }

    #[test]
    fn discrete_line_seals_stream() {
        let mut v = AcpSessionView::new("s", "a", "a", AcpSessionState::Active);
        v.append_stream(AcpEntryKind::Agent, "partial");
        v.push_entry(AcpEntryKind::Tool, "⚙ web_search");
        v.append_stream(AcpEntryKind::Agent, " after tool");
        assert_eq!(v.lines.len(), 3);
        assert_eq!(v.lines[0].text, "partial");
        assert_eq!(v.lines[2].text, " after tool");
    }

    #[test]
    fn thought_stream_strips_prefix_and_coalesces() {
        let mut v = AcpSessionView::new("s", "a", "a", AcpSessionState::Active);
        v.apply_log(AcpLogMode::ThoughtStream, "thinking: aaa".into());
        v.apply_log(AcpLogMode::ThoughtStream, "thinking: bbb".into());
        assert_eq!(v.lines.len(), 1);
        assert_eq!(v.lines[0].kind, AcpEntryKind::Thought);
        assert_eq!(v.lines[0].text, "aaabbb");
    }

    #[test]
    fn acp_column_percent_when_sessions() {
        assert_eq!(acp_column_percent(false), None);
        assert_eq!(acp_column_percent(true), Some(42));
    }
}
