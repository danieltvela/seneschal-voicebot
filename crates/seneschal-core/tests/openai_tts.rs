//! Integration tests for the OpenAI-compatible TTS provider (issue #16).
//!
//! These use `wiremock` to exercise the HTTP contract without a real service:
//! request shaping, chunked PCM decoding, error mapping and voice discovery.
//! The tests are feature-gated so they only run with `--features openai-tts`.

#![cfg(feature = "openai-tts")]

use seneschal_core::tts::openai::{OpenAiTts, OpenAiTtsConfig};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn cfg(url: &str) -> OpenAiTtsConfig {
    OpenAiTtsConfig {
        base_url: url.to_string(),
        voice: "vivian".to_string(),
        language: "auto".to_string(),
        instructions: String::new(),
        speed: 0.0,
        timeout_ms: 10_000,
    }
}

/// Live-service config: the voice comes from `OPENAI_TTS_TEST_VOICE` so the
/// same test works against a CustomVoice deployment (preset names) and a Base
/// deployment (registered voice names). The timeout covers the first request
/// per voice on Base, which costs ~60 s (reference extraction + compile).
fn live_cfg(url: &str) -> OpenAiTtsConfig {
    OpenAiTtsConfig {
        base_url: url.to_string(),
        voice: std::env::var("OPENAI_TTS_TEST_VOICE").unwrap_or_else(|_| "vivian".to_string()),
        language: "auto".to_string(),
        instructions: String::new(),
        speed: 0.0,
        timeout_ms: 180_000,
    }
}

/// Build raw i16-LE PCM bytes.
fn pcm(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

#[tokio::test]
async fn streams_raw_pcm_chunks() {
    let server = MockServer::start().await;
    let body = pcm(&[0, 0, 0, 1000, -1000, 5000, -5000, 0, 0]);
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(body)
                .append_header("content-type", "audio/pcm"),
        )
        .mount(&server)
        .await;

    let tts = OpenAiTts::new(cfg(&server.uri())).unwrap();
    let mut stream = tts.synthesize_stream("Hola mundo.").await.unwrap();
    assert_eq!(stream.sample_rate, 24_000);

    let mut samples = Vec::new();
    while let Some(chunk) = stream.rx.recv().await {
        samples.extend_from_slice(&chunk);
    }
    // Leading silence is trimmed; the rest of the samples survive.
    assert!(samples.iter().any(|s| s.abs() > 0.1), "expected real audio");
    assert!(samples.len() >= 5);
}

#[tokio::test]
async fn sends_expected_request_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(pcm(&[0; 16]))
                .append_header("content-type", "audio/pcm"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let mut c = cfg(&server.uri());
    c.language = "auto".to_string();
    c.instructions = "habla con entusiasmo".to_string();
    let tts = OpenAiTts::new(c).unwrap();
    let mut stream = tts.synthesize_stream("Prueba.").await.unwrap();
    while stream.rx.recv().await.is_some() {}

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let json: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(json["input"], "Prueba.");
    assert_eq!(json["voice"], "vivian");
    assert_eq!(json["response_format"], "pcm");
    assert_eq!(json["stream"], true);
    assert_eq!(json["stream_format"], "audio");
    // `auto` is sent as the literal `Auto`, and no fixed sample rate.
    assert_eq!(json["language"], "Auto");
    assert!(json.get("sample_rate").is_none());
    assert!(json.get("speed").is_none());
    assert_eq!(json["instructions"], "habla con entusiasmo");
}

#[tokio::test]
async fn surfaces_service_error_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": {"message": "Invalid voice 'nope'. Supported: vivian", "type": "BadRequestError"}
        })))
        .mount(&server)
        .await;

    let mut c = cfg(&server.uri());
    c.voice = "nope".to_string();
    let tts = OpenAiTts::new(c).unwrap();
    let err = tts.synthesize_stream("Hola.").await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Invalid voice"), "got: {msg}");
    assert!(msg.contains("400"), "got: {msg}");
}

#[tokio::test]
async fn lists_voices() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/audio/voices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "voices": ["aiden", "vivian", "serena"],
            "uploaded_voices": []
        })))
        .mount(&server)
        .await;

    let tts = OpenAiTts::new(cfg(&server.uri())).unwrap();
    tts.list_voices().await.unwrap();
}

#[tokio::test]
async fn validate_voice_accepts_and_rejects() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/audio/voices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "voices": ["aiden", "vivian"],
            "uploaded_voices": [{"name": "my_clone", "ref_text": "hola"}]
        })))
        .mount(&server)
        .await;

    let mut c = cfg(&server.uri());
    c.voice = "vivian".to_string();
    assert!(
        OpenAiTts::new(c.clone())
            .unwrap()
            .verify_voice()
            .await
            .is_ok()
    );
    c.voice = "my_clone".to_string();
    assert!(
        OpenAiTts::new(c.clone())
            .unwrap()
            .verify_voice()
            .await
            .is_ok()
    );
    c.voice = "missing".to_string();
    assert!(OpenAiTts::new(c).unwrap().verify_voice().await.is_err());
}

#[test]
fn empty_voice_is_rejected() {
    let mut c = cfg("http://localhost:9");
    c.voice = "  ".to_string();
    assert!(OpenAiTts::new(c).is_err());
}

#[tokio::test]
async fn empty_text_is_rejected() {
    let tts = OpenAiTts::new(cfg("http://localhost:9")).unwrap();
    assert!(tts.synthesize_stream("   ").await.is_err());
}

// ── Live service integration (ignored by default) ──────────────────────────────
//
// Run against a real OpenAI-compatible service:
//   OPENAI_TTS_TEST_URL=http://localhost:8025 \
//   OPENAI_TTS_TEST_VOICE=jorge \
//     cargo test -p seneschal-core --features openai-tts -- --ignored openai_live
//
// `OPENAI_TTS_TEST_VOICE` names the voice to synthesise with (default
// `vivian`); a Base checkpoint deployment requires a registered voice.
// `make test-openai` wraps this and skips when the service is unreachable.

/// Live check: first audio chunk arrives quickly and carries 24 kHz PCM.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible TTS service (OPENAI_TTS_TEST_URL)"]
async fn openai_live_first_chunk_latency_and_format() {
    let url = std::env::var("OPENAI_TTS_TEST_URL")
        .unwrap_or_else(|_| "http://localhost:8025".to_string());
    let tts = OpenAiTts::new(live_cfg(&url)).unwrap();

    let start = std::time::Instant::now();
    let mut stream = tts
        .synthesize_stream("Hola, esta es una prueba de integración del proveedor de voz.")
        .await
        .expect("service should accept the request");
    assert_eq!(stream.sample_rate, 24_000);

    let mut first_chunk_ms = None;
    let mut total = 0usize;
    while let Some(chunk) = stream.rx.recv().await {
        if first_chunk_ms.is_none() {
            first_chunk_ms = Some(start.elapsed().as_millis());
        }
        total += chunk.len();
    }

    let first = first_chunk_ms.expect("service produced no audio");
    assert!(
        total > 8_000,
        "expected >0.3 s of audio, got {total} samples"
    );
    // Generous bound: the shared GPU can be busy with the LLM/ASR workloads.
    assert!(first < 5_000, "first chunk took {first} ms");
    println!("first chunk in {first} ms, {total} samples total");
}
