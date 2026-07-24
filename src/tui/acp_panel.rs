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

/// One ACP session row in the TUI strip + detail log.
#[derive(Clone, Debug)]
pub struct AcpSessionView {
    pub session_id: String,
    pub agent_name: String,
    pub label: String,
    pub state: AcpSessionState,
    pub lines: Vec<String>,
    /// Lines scrolled up from the bottom (0 = pinned to end).
    pub scroll: u16,
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
        } => vec![
            TuiEvent::AcpSessionUpsert {
                session_id: session_id.clone(),
                agent_name: agent_name.clone(),
                label: agent_name,
                state: AcpSessionState::Active,
            },
            TuiEvent::AcpSessionLog {
                session_id,
                line: text,
            },
        ],
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
        let mut v = AcpSessionView {
            session_id: "s".into(),
            agent_name: "a".into(),
            label: "a".into(),
            state: AcpSessionState::Idle,
            lines: Vec::new(),
            scroll: 0,
        };
        for i in 0..600 {
            v.push_line(format!("line-{i}"));
        }
        assert_eq!(v.lines.len(), 500);
        assert_eq!(v.lines[0], "line-100");
        assert_eq!(v.lines[499], "line-599");
    }

    #[test]
    fn acp_column_percent_when_sessions() {
        assert_eq!(acp_column_percent(false), None);
        assert_eq!(acp_column_percent(true), Some(42));
    }
}
