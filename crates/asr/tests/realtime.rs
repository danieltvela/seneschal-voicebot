//! End-to-end transport test against an in-process fake
//! NeMo-Speech.cpp realtime server. It verifies the wire order (config, PCM),
//! PCM16LE encoding and the event mapping without loading the real model.

use std::time::Duration;

use asr::config::{AsrConfig, SAMPLE_RATE};
use asr::provider::NemotronAsrProvider;
use futures_util::{SinkExt, StreamExt};
use seneschal_core::stt::{SpeechEvent, SttProvider};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::protocol::Message;

fn cfg(port: u16) -> AsrConfig {
    AsrConfig {
        base_url: format!("http://127.0.0.1:{port}"),
        api_key: String::new(),
        model: "nemotron-3.5".to_string(),
        language: "es-ES".to_string(),
        endpointing_ms: 300,
        connect_timeout: Duration::from_secs(2),
        http_timeout: Duration::from_secs(2),
    }
}

#[tokio::test]
async fn streams_pcm_and_maps_partials_and_finals() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();

        // The provider must configure punctuation before sending audio.
        let update = match ws.next().await {
            Some(Ok(Message::Text(text))) => text.to_string(),
            other => panic!("expected session.update text, got {other:?}"),
        };
        let update: serde_json::Value = serde_json::from_str(&update).unwrap();
        assert_eq!(update["type"], "session.update");
        assert_eq!(update["session"]["sample_rate"], SAMPLE_RATE);
        assert_eq!(update["session"]["automatic_punctuation"], true);
        assert_eq!(update["session"]["language"], "es-ES");
        assert_eq!(update["session"]["endpointing_ms"], 300);

        // One 100 ms chunk: 1600 f32 samples at 0.5 -> 1600 PCM16LE values.
        let audio = match ws.next().await {
            Some(Ok(Message::Binary(bytes))) => bytes,
            other => panic!("expected binary PCM, got {other:?}"),
        };
        assert_eq!(audio.len(), 3_200);
        assert_eq!(
            i16::from_le_bytes([audio[0], audio[1]]),
            (0.5 * i16::MAX as f32).round() as i16
        );

        for delta in ["Hola", " señor."] {
            ws.send(Message::text(
                serde_json::json!({
                    "type": "conversation.item.input_audio_transcription.delta",
                    "delta": delta,
                    "audio_processed": 0.32,
                })
                .to_string(),
            ))
            .await
            .unwrap();
        }
        ws.send(Message::text(
            serde_json::json!({
                "type": "conversation.item.input_audio_transcription.completed",
                "transcript": "Hola señor.",
                "audio_processed": 1.2,
                "event_id": "event_42",
            })
            .to_string(),
        ))
        .await
        .unwrap();

        while let Some(message) = ws.next().await {
            match message {
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });

    let (event_tx, mut event_rx) = mpsc::channel(8);
    let mut provider = NemotronAsrProvider::new(cfg(port)).unwrap();
    provider
        .process_audio(&[0.5; 1_600], &event_tx)
        .await
        .unwrap();

    async fn event(rx: &mut mpsc::Receiver<SpeechEvent>) -> SpeechEvent {
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("event must arrive")
            .expect("event channel stays open")
    }

    assert!(matches!(
        event(&mut event_rx).await,
        SpeechEvent::SpeechStart { segment_seq: 1 }
    ));
    assert_eq!(
        match event(&mut event_rx).await {
            SpeechEvent::Speech(text) => text,
            other => panic!("expected Speech, got {other:?}"),
        },
        "Hola"
    );
    assert_eq!(
        match event(&mut event_rx).await {
            SpeechEvent::Speech(text) => text,
            other => panic!("expected Speech, got {other:?}"),
        },
        "Hola señor."
    );
    match event(&mut event_rx).await {
        SpeechEvent::SpeechEnd {
            segment_seq,
            quality,
        } => {
            assert_eq!(segment_seq, 1);
            assert_eq!(quality.text, "Hola señor.");
        }
        other => panic!("expected SpeechEnd, got {other:?}"),
    }

    // Normal shutdown closes the command channel; the fake server observes it.
    drop(provider);
    server.await.unwrap();
}
