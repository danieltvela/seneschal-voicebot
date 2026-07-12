//! SSE event types and milestone narration for the OpenCode remote agent protocol.
//!
//! Provides parsing for Server-Sent Events emitted by OpenCode's event stream,
//! and extraction of narratable milestone strings that seneschal can speak aloud
//! to keep the user informed of agent progress.

/// Events that can be received from the OpenCode SSE event stream.
#[derive(Debug, Clone, PartialEq)]
pub enum OpenCodeEvent {
    /// A session was updated (connection-level, usually skipped).
    SessionUpdated { session_id: String },
    /// A new message was created (too noisy, skipped for narration).
    MessageCreated { message: String },
    /// The agent invoked a tool — narratable.
    ToolInvoked { tool_name: String },
    /// A tool completed execution.
    ToolCompleted { tool_name: String, result: String },
    /// The agent is requesting user permission — narratable.
    PermissionRequested { action: String },
}

/// A narratable milestone extracted from an OpenCode SSE event.
///
/// This is the high-level representation that gets forwarded to the proactive
/// pipeline for TTS narration.
#[derive(Debug, Clone)]
pub struct OpenCodeMilestone {
    /// Human-readable milestone text (e.g. "Está usando bash").
    pub milestone: String,
    /// Correlation identifier for tracing.
    pub correlation_id: String,
}

/// Parse an SSE text block into an `OpenCodeEvent`.
///
/// The parser looks for `event:` and `data:` fields in the SSE stream,
/// splitting on newlines. Returns `None` for unrecognised event types or
/// malformed input.
pub fn parse_opencode_event(text: &str) -> Option<OpenCodeEvent> {
    let mut event_type: Option<&str> = None;
    let mut data: Option<&str> = None;

    for line in text.lines() {
        if let Some(val) = line.strip_prefix("event: ") {
            event_type = Some(val);
        } else if let Some(val) = line.strip_prefix("data: ") {
            data = Some(val);
        }
    }

    let event_type = event_type?;
    let data = data.unwrap_or("");

    match event_type {
        "session.updated" => {
            let session_id = parse_string_field(data, "sessionId")
                .or_else(|| parse_string_field(data, "id"))
                .unwrap_or_else(|| data.trim_matches('"').to_string());
            Some(OpenCodeEvent::SessionUpdated { session_id })
        }
        "message.created" => {
            let message = parse_string_field(data, "content").unwrap_or_else(|| data.to_string());
            Some(OpenCodeEvent::MessageCreated { message })
        }
        "tool.invoked" => {
            let tool_name = parse_string_field(data, "name")
                .or_else(|| parse_string_field(data, "toolName"))
                .unwrap_or_else(|| data.to_string());
            Some(OpenCodeEvent::ToolInvoked { tool_name })
        }
        "tool.completed" => {
            let tool_name =
                parse_string_field(data, "name").unwrap_or_else(|| "unknown".to_string());
            let result = parse_string_field(data, "result").unwrap_or_default();
            Some(OpenCodeEvent::ToolCompleted { tool_name, result })
        }
        "permission.requested" => {
            let action = parse_string_field(data, "action")
                .or_else(|| parse_string_field(data, "toolName"))
                .or_else(|| parse_string_field(data, "description"))
                .unwrap_or_else(|| data.to_string());
            Some(OpenCodeEvent::PermissionRequested { action })
        }
        _ => {
            tracing::debug!(
                target: "opencode",
                "Ignored OpenCode SSE event type: {}",
                event_type
            );
            None
        }
    }
}

/// Extract a narratable milestone string from an OpenCode event.
///
/// Returns `None` for events that are too noisy or connection-level
/// (e.g. `MessageCreated`, `SessionUpdated`).
pub fn extract_milestone(event: &OpenCodeEvent) -> Option<String> {
    match event {
        OpenCodeEvent::ToolInvoked { tool_name } => Some(format!("Está usando {tool_name}")),
        OpenCodeEvent::ToolCompleted { tool_name, .. } => {
            Some(format!("Terminó de usar {tool_name}"))
        }
        OpenCodeEvent::PermissionRequested { action } => {
            Some(format!("OpenCode pide permiso para {action}"))
        }
        // Skip these — too noisy or connection-level
        OpenCodeEvent::MessageCreated { .. } => None,
        OpenCodeEvent::SessionUpdated { .. } => None,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Try to parse a JSON field as string from raw data.
fn parse_string_field(data: &str, field: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(data)
        .ok()
        .and_then(|v| v.get(field)?.as_str().map(String::from))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_opencode_event ──────────────────────────────────────────────────

    #[test]
    fn parse_tool_invoked_event() {
        let sse = "event: tool.invoked\ndata: {\"name\":\"bash\"}\n\n";
        let event = parse_opencode_event(sse).unwrap();
        assert_eq!(
            event,
            OpenCodeEvent::ToolInvoked {
                tool_name: "bash".to_string()
            }
        );
    }

    #[test]
    fn parse_tool_completed_event() {
        let sse =
            "event: tool.completed\ndata: {\"name\":\"read_file\",\"result\":\"content\"}\n\n";
        let event = parse_opencode_event(sse).unwrap();
        assert_eq!(
            event,
            OpenCodeEvent::ToolCompleted {
                tool_name: "read_file".to_string(),
                result: "content".to_string(),
            }
        );
    }

    #[test]
    fn parse_permission_requested_event() {
        let sse = "event: permission.requested\ndata: {\"action\":\"ejecutar bash\"}\n\n";
        let event = parse_opencode_event(sse).unwrap();
        assert_eq!(
            event,
            OpenCodeEvent::PermissionRequested {
                action: "ejecutar bash".to_string()
            }
        );
    }

    #[test]
    fn parse_message_created_event() {
        let sse = "event: message.created\ndata: {\"content\":\"Hola\"}\n\n";
        let event = parse_opencode_event(sse).unwrap();
        assert_eq!(
            event,
            OpenCodeEvent::MessageCreated {
                message: "Hola".to_string()
            }
        );
    }

    #[test]
    fn parse_session_updated_event() {
        let sse = "event: session.updated\ndata: {\"sessionId\":\"abc123\"}\n\n";
        let event = parse_opencode_event(sse).unwrap();
        assert_eq!(
            event,
            OpenCodeEvent::SessionUpdated {
                session_id: "abc123".to_string()
            }
        );
    }

    #[test]
    fn parse_unknown_event_returns_none() {
        let sse = "event: unknown.type\ndata: {}\n\n";
        assert!(parse_opencode_event(sse).is_none());
    }

    #[test]
    fn parse_missing_event_field_returns_none() {
        let sse = "data: {}\n\n";
        assert!(parse_opencode_event(sse).is_none());
    }

    // ── extract_milestone ─────────────────────────────────────────────────────

    #[test]
    fn tool_invoked_extracts_milestone() {
        let event = OpenCodeEvent::ToolInvoked {
            tool_name: "bash".to_string(),
        };
        let ms = extract_milestone(&event).unwrap();
        assert_eq!(ms, "Está usando bash");
    }

    #[test]
    fn tool_completed_extracts_milestone() {
        let event = OpenCodeEvent::ToolCompleted {
            tool_name: "read_file".to_string(),
            result: "content".to_string(),
        };
        let ms = extract_milestone(&event).unwrap();
        assert_eq!(ms, "Terminó de usar read_file");
    }

    #[test]
    fn permission_requested_extracts_milestone() {
        let event = OpenCodeEvent::PermissionRequested {
            action: "ejecutar bash".to_string(),
        };
        let ms = extract_milestone(&event).unwrap();
        assert_eq!(ms, "OpenCode pide permiso para ejecutar bash");
    }

    #[test]
    fn message_created_skipped() {
        let event = OpenCodeEvent::MessageCreated {
            message: "Hola".to_string(),
        };
        assert!(extract_milestone(&event).is_none());
    }

    #[test]
    fn session_updated_skipped() {
        let event = OpenCodeEvent::SessionUpdated {
            session_id: "abc".to_string(),
        };
        assert!(extract_milestone(&event).is_none());
    }
}
