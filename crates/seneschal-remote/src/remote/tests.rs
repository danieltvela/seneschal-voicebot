use super::protocol::{ClientMessage, ServerMessage, TtsAudioPacket};

#[test]
fn deserialize_session_start_uses_default_sample_rate() {
    let raw = r#"{"type":"session.start"}"#;
    let m: ClientMessage = serde_json::from_str(raw).unwrap();
    match m {
        ClientMessage::SessionStart { sample_rate } => assert_eq!(sample_rate, 16000),
        _ => panic!("Wrong variant"),
    }
}

#[test]
fn deserialize_session_start_respects_explicit_sample_rate() {
    let raw = r#"{"type":"session.start","sample_rate":48000}"#;
    let m: ClientMessage = serde_json::from_str(raw).unwrap();
    match m {
        ClientMessage::SessionStart { sample_rate } => assert_eq!(sample_rate, 48000),
        _ => panic!("Wrong variant"),
    }
}

#[test]
fn deserialize_barge_in() {
    let raw = r#"{"type":"barge_in"}"#;
    let m: ClientMessage = serde_json::from_str(raw).unwrap();
    assert!(matches!(m, ClientMessage::BargeIn));
}

#[test]
fn deserialize_unknown_type_errors() {
    let raw = r#"{"type":"nope"}"#;
    let result: Result<ClientMessage, _> = serde_json::from_str(raw);
    assert!(result.is_err());
}

#[test]
fn serialize_session_ready_uses_tag() {
    let s = serde_json::to_string(&ServerMessage::SessionReady).unwrap();
    assert_eq!(s, r#"{"type":"session.ready"}"#);
}

#[test]
fn serialize_transcript_includes_text() {
    let s = serde_json::to_string(&ServerMessage::Transcript {
        text: "hola".to_string(),
    })
    .unwrap();
    assert_eq!(s, r#"{"type":"transcript","text":"hola"}"#);
}

#[test]
fn round_trip_response_end_and_error() {
    let end = ServerMessage::ResponseEnd;
    let s = serde_json::to_string(&end).unwrap();
    assert_eq!(s, r#"{"type":"response.end"}"#);

    let err = ServerMessage::Error {
        message: "boom".to_string(),
    };
    let s = serde_json::to_string(&err).unwrap();
    assert_eq!(s, r#"{"type":"error","message":"boom"}"#);
}

#[test]
fn tts_audio_packet_holds_samples() {
    let p = TtsAudioPacket {
        samples: vec![0.0, 0.5, -0.5],
        sample_rate: 22050,
    };
    assert_eq!(p.samples.len(), 3);
    assert_eq!(p.sample_rate, 22050);
}
