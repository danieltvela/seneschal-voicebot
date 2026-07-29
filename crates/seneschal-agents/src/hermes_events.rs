//! SSE event types and milestone narration for the Hermes remote agent protocol.
//!
//! Provides parsing for Server-Sent Events emitted by Hermes' per-run event
//! stream (`GET /v1/runs/{id}/events`), and extraction of narratable milestone
//! strings that Seneschal can speak aloud to keep the user informed of progress.

/// Events that can be received from the Hermes SSE event stream.
#[derive(Debug, Clone, PartialEq)]
pub enum HermesEvent {
    /// A run has been created and is starting.
    RunStarted { run_id: String },
    /// The run completed successfully.
    RunCompleted { run_id: String },
    /// The run failed with an error.
    RunFailed { run_id: String, error: String },
    /// The run was cancelled by the user.
    RunCancelled { run_id: String },
    /// A delta (partial) message from the agent.
    MessageDelta { content: String },
    /// The agent invoked a tool — narratable.
    ToolStarted { tool_name: String },
    /// A tool completed execution.
    ToolCompleted { tool_name: String },
    /// The agent is requesting user approval.
    ApprovalRequested { action: String },
}

/// A narratable milestone extracted from a Hermes SSE event.
///
/// This is the high-level representation that gets forwarded to the proactive
/// pipeline for TTS narration.
#[derive(Debug, Clone)]
pub struct HermesMilestone {
    /// Human-readable milestone text (e.g. "Está usando bash").
    pub milestone: String,
    /// Correlation identifier for tracing.
    pub correlation_id: String,
}

/// Parse an SSE text block into a `HermesEvent`.
///
/// The parser looks for `event:` and `data:` fields in the SSE stream,
/// splitting on newlines. Returns `None` for unrecognised event types or
/// malformed input.
pub fn parse_hermes_event(text: &str) -> Option<HermesEvent> {
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
        "run.started" => {
            let run_id = parse_string_field(data, "run_id")
                .or_else(|| parse_string_field(data, "id"))
                .unwrap_or_else(|| data.trim_matches('"').to_string());
            Some(HermesEvent::RunStarted { run_id })
        }
        "run.completed" => {
            let run_id = parse_string_field(data, "run_id")
                .or_else(|| parse_string_field(data, "id"))
                .unwrap_or_default();
            Some(HermesEvent::RunCompleted { run_id })
        }
        "run.failed" => {
            let run_id = parse_string_field(data, "run_id").unwrap_or_default();
            let error = parse_string_field(data, "error").unwrap_or_else(|| data.to_string());
            Some(HermesEvent::RunFailed { run_id, error })
        }
        "run.cancelled" => {
            let run_id = parse_string_field(data, "run_id").unwrap_or_default();
            Some(HermesEvent::RunCancelled { run_id })
        }
        "message.delta" => {
            let content = parse_string_field(data, "content")
                .or_else(|| parse_string_field(data, "delta"))
                .unwrap_or_else(|| data.to_string());
            Some(HermesEvent::MessageDelta { content })
        }
        "tool.started" => {
            let tool_name = parse_string_field(data, "name")
                .or_else(|| parse_string_field(data, "tool_name"))
                .or_else(|| parse_string_field(data, "toolName"))
                .unwrap_or_else(|| data.to_string());
            Some(HermesEvent::ToolStarted { tool_name })
        }
        "tool.completed" => {
            let tool_name = parse_string_field(data, "name")
                .or_else(|| parse_string_field(data, "tool_name"))
                .or_else(|| parse_string_field(data, "toolName"))
                .unwrap_or_else(|| "unknown".to_string());
            Some(HermesEvent::ToolCompleted { tool_name })
        }
        "approval.request" | "approval.requested" => {
            let action = parse_string_field(data, "action")
                .or_else(|| parse_string_field(data, "description"))
                .or_else(|| parse_string_field(data, "toolName"))
                .unwrap_or_else(|| data.to_string());
            Some(HermesEvent::ApprovalRequested { action })
        }
        _ => {
            tracing::debug!(
                target: "hermes",
                "Ignored Hermes SSE event type: {}",
                event_type
            );
            None
        }
    }
}

/// Extract a narratable milestone string from a Hermes event.
///
/// Returns `None` for events that are too noisy (e.g. `MessageDelta`).
pub fn extract_milestone(event: &HermesEvent) -> Option<String> {
    match event {
        HermesEvent::ToolStarted { tool_name } => Some(format!("Está usando {tool_name}")),
        HermesEvent::ToolCompleted { tool_name } => Some(format!("Terminó de usar {tool_name}")),
        HermesEvent::ApprovalRequested { action } => {
            Some(format!("Hermes pide permiso para {action}"))
        }
        HermesEvent::RunStarted { .. } => Some("Iniciando tarea remota".to_string()),
        HermesEvent::RunCompleted { .. } => Some("Tarea remota completada".to_string()),
        // Skip these — too noisy or terminal
        HermesEvent::MessageDelta { .. } => None,
        HermesEvent::RunFailed { .. } => None,
        HermesEvent::RunCancelled { .. } => None,
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

    // ── parse_hermes_event ────────────────────────────────────────────────────

    #[test]
    fn parse_run_started_event() {
        let sse = "event: run.started\ndata: {\"run_id\":\"run-abc\"}\n\n";
        let event = parse_hermes_event(sse).unwrap();
        assert_eq!(
            event,
            HermesEvent::RunStarted {
                run_id: "run-abc".to_string()
            }
        );
    }

    #[test]
    fn parse_run_completed_event() {
        let sse = "event: run.completed\ndata: {\"run_id\":\"run-123\"}\n\n";
        let event = parse_hermes_event(sse).unwrap();
        assert_eq!(
            event,
            HermesEvent::RunCompleted {
                run_id: "run-123".to_string()
            }
        );
    }

    #[test]
    fn parse_run_failed_event() {
        let sse = "event: run.failed\ndata: {\"run_id\":\"run-err\",\"error\":\"timeout\"}\n\n";
        let event = parse_hermes_event(sse).unwrap();
        assert_eq!(
            event,
            HermesEvent::RunFailed {
                run_id: "run-err".to_string(),
                error: "timeout".to_string(),
            }
        );
    }

    #[test]
    fn parse_run_cancelled_event() {
        let sse = "event: run.cancelled\ndata: {\"run_id\":\"run-cancel\"}\n\n";
        let event = parse_hermes_event(sse).unwrap();
        assert_eq!(
            event,
            HermesEvent::RunCancelled {
                run_id: "run-cancel".to_string()
            }
        );
    }

    #[test]
    fn parse_message_delta_event() {
        let sse = "event: message.delta\ndata: {\"content\":\"Hello\"}\n\n";
        let event = parse_hermes_event(sse).unwrap();
        assert_eq!(
            event,
            HermesEvent::MessageDelta {
                content: "Hello".to_string()
            }
        );
    }

    #[test]
    fn parse_tool_started_event() {
        let sse = "event: tool.started\ndata: {\"name\":\"bash\"}\n\n";
        let event = parse_hermes_event(sse).unwrap();
        assert_eq!(
            event,
            HermesEvent::ToolStarted {
                tool_name: "bash".to_string()
            }
        );
    }

    #[test]
    fn parse_tool_completed_event() {
        let sse = "event: tool.completed\ndata: {\"name\":\"read_file\"}\n\n";
        let event = parse_hermes_event(sse).unwrap();
        assert_eq!(
            event,
            HermesEvent::ToolCompleted {
                tool_name: "read_file".to_string()
            }
        );
    }

    #[test]
    fn parse_approval_requested_event() {
        let sse = "event: approval.request\ndata: {\"action\":\"ejecutar bash\"}\n\n";
        let event = parse_hermes_event(sse).unwrap();
        assert_eq!(
            event,
            HermesEvent::ApprovalRequested {
                action: "ejecutar bash".to_string()
            }
        );
    }

    #[test]
    fn parse_unknown_event_returns_none() {
        let sse = "event: unknown.type\ndata: {}\n\n";
        assert!(parse_hermes_event(sse).is_none());
    }

    #[test]
    fn parse_missing_event_field_returns_none() {
        let sse = "data: {}\n\n";
        assert!(parse_hermes_event(sse).is_none());
    }

    // ── extract_milestone ─────────────────────────────────────────────────────

    #[test]
    fn tool_started_extracts_milestone() {
        let event = HermesEvent::ToolStarted {
            tool_name: "bash".to_string(),
        };
        let ms = extract_milestone(&event).unwrap();
        assert_eq!(ms, "Está usando bash");
    }

    #[test]
    fn tool_completed_extracts_milestone() {
        let event = HermesEvent::ToolCompleted {
            tool_name: "read_file".to_string(),
        };
        let ms = extract_milestone(&event).unwrap();
        assert_eq!(ms, "Terminó de usar read_file");
    }

    #[test]
    fn approval_requested_extracts_milestone() {
        let event = HermesEvent::ApprovalRequested {
            action: "ejecutar bash".to_string(),
        };
        let ms = extract_milestone(&event).unwrap();
        assert_eq!(ms, "Hermes pide permiso para ejecutar bash");
    }

    #[test]
    fn run_started_extracts_milestone() {
        let event = HermesEvent::RunStarted {
            run_id: "run-1".to_string(),
        };
        let ms = extract_milestone(&event).unwrap();
        assert_eq!(ms, "Iniciando tarea remota");
    }

    #[test]
    fn run_completed_extracts_milestone() {
        let event = HermesEvent::RunCompleted {
            run_id: "run-1".to_string(),
        };
        let ms = extract_milestone(&event).unwrap();
        assert_eq!(ms, "Tarea remota completada");
    }

    #[test]
    fn message_delta_skipped() {
        let event = HermesEvent::MessageDelta {
            content: "Hello".to_string(),
        };
        assert!(extract_milestone(&event).is_none());
    }

    #[test]
    fn run_failed_skipped() {
        let event = HermesEvent::RunFailed {
            run_id: "r".to_string(),
            error: "err".to_string(),
        };
        assert!(extract_milestone(&event).is_none());
    }
}
