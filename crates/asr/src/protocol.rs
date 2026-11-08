//! Wire protocol helpers for `/v1/audio/transcriptions/realtime`.
//!
//! The server emits a small project-specific JSON protocol: `session.created`
//! / `session.updated`, `conversation.item.input_audio_transcription.delta`
//! (partials), the same event with `.completed` (endpointing finals), and
//! `error`. Binary frames are little-endian PCM16.

use anyhow::{Context, Result};
use serde_json::{Value, json};

/// Build the one allowed pre-audio configuration event.
///
/// Nemotron 3.5 performs punctuation/capitalization natively; the flag is sent
/// explicitly so the server cannot fall back to raw text. `language` may be
/// `auto` for server-side language detection / code-switching.
pub fn session_update(sample_rate: usize, language: &str, endpointing_ms: u32) -> String {
    json!({
        "type": "session.update",
        "session": {
            "sample_rate": sample_rate,
            "language": language,
            "automatic_punctuation": true,
            "verbatim": false,
            "speaker_diarization": false,
            "endpointing_ms": endpointing_ms,
        }
    })
    .to_string()
}

/// Parse one text frame as a JSON object.
pub fn parse_event(text: &str) -> Result<Value> {
    let value: Value =
        serde_json::from_str(text).with_context(|| format!("invalid realtime event: {text}"))?;
    if !value.is_object() {
        anyhow::bail!("realtime event is not a JSON object: {text}");
    }
    Ok(value)
}

/// The event's `type` field, or an empty string when absent.
pub fn event_type(event: &Value) -> &str {
    event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// String field accessor that tolerates a missing field.
pub fn string_field<'a>(event: &'a Value, name: &str) -> Option<&'a str> {
    event.get(name).and_then(Value::as_str)
}

/// Numeric field accessor (`audio_processed` is seconds).
pub fn number_field(event: &Value, name: &str) -> Option<f64> {
    event.get(name).and_then(Value::as_f64)
}

/// Human-readable detail from the server's `error` event.
pub fn error_message(event: &Value) -> String {
    let error = event.get("error");
    let message = error
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .or_else(|| string_field(event, "message"))
        .unwrap_or("unknown realtime error");
    let code = error
        .and_then(|e| e.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if code.is_empty() {
        message.to_string()
    } else {
        format!("{message} ({code})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_update_carries_punctuation_and_endpointing() {
        let update = session_update(16_000, "es-ES", 300);
        let value: Value = serde_json::from_str(&update).unwrap();
        let session = value.get("session").unwrap();
        assert_eq!(value["type"], "session.update");
        assert_eq!(session["sample_rate"], 16_000);
        assert_eq!(session["language"], "es-ES");
        assert_eq!(session["automatic_punctuation"], true);
        assert_eq!(session["verbatim"], false);
        assert_eq!(session["endpointing_ms"], 300);
    }

    #[test]
    fn parses_and_classifies_server_events() {
        let event = parse_event(
            r#"{"type":"conversation.item.input_audio_transcription.delta",
                "delta":"Hola","audio_processed":0.32,"event_id":"event_1"}"#,
        )
        .unwrap();
        assert_eq!(
            event_type(&event),
            "conversation.item.input_audio_transcription.delta"
        );
        assert_eq!(string_field(&event, "delta"), Some("Hola"));
        assert_eq!(number_field(&event, "audio_processed"), Some(0.32));

        let event = parse_event(
            r#"{"type":"conversation.item.input_audio_transcription.completed",
                "transcript":"Hola, señor.","audio_processed":1.8}"#,
        )
        .unwrap();
        assert!(event_type(&event).ends_with(".completed"));
        assert_eq!(string_field(&event, "transcript"), Some("Hola, señor."));
    }

    #[test]
    fn malformed_events_are_rejected() {
        assert!(parse_event("not json").is_err());
        assert!(parse_event("[]").is_err());
    }

    #[test]
    fn extracts_error_details() {
        let event = parse_event(
            r#"{"type":"error","error":{"message":"bad audio","code":"invalid_argument"}}"#,
        )
        .unwrap();
        assert_eq!(error_message(&event), "bad audio (invalid_argument)");

        let event = parse_event(r#"{"type":"error","message":"plain"}"#).unwrap();
        assert_eq!(error_message(&event), "plain");
    }
}
