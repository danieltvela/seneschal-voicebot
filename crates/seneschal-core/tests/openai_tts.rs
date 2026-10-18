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
        sample_rate: 24_000,
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
        language: std::env::var("OPENAI_TTS_TEST_LANGUAGE").unwrap_or_else(|_| "auto".to_string()),
        instructions: String::new(),
        speed: 0.0,
        sample_rate: live_sample_rate(),
        timeout_ms: 180_000,
    }
}

/// PCM rate of the live service under test (default 24000; MagpieTTS via
/// NeMo-Speech.cpp is 22050 — set `OPENAI_TTS_TEST_SAMPLE_RATE=22050`).
fn live_sample_rate() -> u32 {
    std::env::var("OPENAI_TTS_TEST_SAMPLE_RATE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24_000)
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
    // `auto` is sent as the literal `Auto`; the configured PCM rate is requested.
    assert_eq!(json["language"], "Auto");
    assert_eq!(json["sample_rate"], 24_000);
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

/// NeMo-Speech.cpp has no `/v1/audio/voices`; voices come from `GET /v1/models`
/// (issue #20).
#[tokio::test]
async fn discovers_voices_from_models_when_audio_voices_absent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/audio/voices"))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "object": "list",
            "data": [
                {"id": "magpietts", "capability": "speech", "voices": ["John", "Sofia"]}
            ]
        })))
        .mount(&server)
        .await;

    let tts = OpenAiTts::new(cfg(&server.uri())).unwrap();
    tts.list_voices().await.unwrap();

    let mut c = cfg(&server.uri());
    c.voice = "John".to_string();
    assert!(
        OpenAiTts::new(c.clone())
            .unwrap()
            .verify_voice()
            .await
            .is_ok()
    );
    // OpenAI aliases / `default` map to the server's configured speaker.
    c.voice = "alloy".to_string();
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

/// The configured sample rate is reported to the pipeline (issue #20).
#[tokio::test]
async fn reports_configured_sample_rate() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(pcm(&[0; 8]))
                .append_header("content-type", "audio/pcm"),
        )
        .mount(&server)
        .await;

    let mut c = cfg(&server.uri());
    c.sample_rate = 22_050;
    let tts = OpenAiTts::new(c).unwrap();
    assert_eq!(tts.sample_rate(), 22_050);
    let stream = tts.synthesize_stream("Hola.").await.unwrap();
    assert_eq!(stream.sample_rate, 22_050);

    let requests = server.received_requests().await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(json["sample_rate"], 22_050);
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

/// Live check: first audio chunk arrives quickly and carries the configured PCM.
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
    assert_eq!(stream.sample_rate, live_sample_rate());

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
