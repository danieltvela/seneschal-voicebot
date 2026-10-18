//! Integration tests for the OpenAI-compatible remote STT provider (issue #17).
//!
//! The HTTP contract is exercised with `wiremock` and an injected scripted
//! detector, so no `seneschal-asr` service and no Silero/Whisper models are
//! required. Feature-gated behind `openai-stt`.
//!
//! Live tests (ignored) run against a real service; `make test-stt-openai`
//! wraps them and skips when the endpoint is unreachable.

#![cfg(feature = "openai-stt")]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use seneschal_core::stt::openai::{OpenAiSttConfig, OpenAiSttProvider};
use seneschal_core::stt::segmenter::SpeechDetector;
use seneschal_core::stt::streaming::LiveStreamConfig;
use seneschal_core::stt::{SpeechEvent, WhisperSTTVADConfig};
use tokio::sync::mpsc;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Probe window the shared segmenter uses by default (100 ms @ 16 kHz).
const PROBE: usize = 1600;

/// Deterministic speech detector driven by a per-probe probability script.
struct ScriptedDetector {
    probs: Arc<Mutex<VecDeque<f32>>>,
}

impl SpeechDetector for ScriptedDetector {
    fn probe(&mut self, _chunk: &[f32]) -> f32 {
        self.probs.lock().unwrap().pop_front().unwrap_or(0.0)
    }
}

fn scripted(probs: &[f32]) -> (ScriptedDetector, usize) {
    (
        ScriptedDetector {
            probs: Arc::new(Mutex::new(probs.iter().copied().collect())),
        },
        probs.len(),
    )
}

fn base_config() -> WhisperSTTVADConfig {
    WhisperSTTVADConfig {
        whisper_model: String::new(),
        vad_model: String::new(),
        language: "auto".to_string(),
        silence_ms: 300,
        vad_start_threshold: 0.65,
        vad_end_threshold: 0.45,
        vad_confirm_probes: 2,
        probe_ms: 100,
        preroll_ms: 300,
        post_roll_ms: 200,
        // Live partials off by default in these tests; the dedicated live test
        // below turns them on with a fast interval.
        live: LiveStreamConfig {
            enabled: false,
            interval_ms: 1000,
            window_ms: 15_000,
        },
        vad_gpu: None,
        parakeet_gpu: None,
    }
}

fn cfg(url: &str) -> OpenAiSttConfig {
    OpenAiSttConfig {
        base_url: url.to_string(),
        api_key: String::new(),
        model: "qwen3-asr".to_string(),
        language: "auto".to_string(),
        timeout_ms: 10_000,
    }
}

/// Two confirmed probes, then enough silence to close the segment (uniform
/// post-roll contract; silence_ms = 300 with 100 ms probes = 3 silent probes
/// after the 200 ms post-roll grace).
const ONE_UTTERANCE: &[f32] = &[0.9, 0.9, 0.0, 0.0, 0.0, 0.0, 0.0];

type Provider = OpenAiSttProvider<ScriptedDetector>;

fn provider(probs: &[f32], cfg: OpenAiSttConfig) -> (Provider, usize) {
    let (detector, len) = scripted(probs);
    let provider = OpenAiSttProvider::with_detector(detector, &base_config(), cfg)
        .expect("provider construction");
    (provider, len)
}

async fn feed(provider: &mut Provider, tx: &mpsc::Sender<SpeechEvent>, probes: usize) {
    for _ in 0..probes {
        let chunk = vec![0.1f32; PROBE];
        provider.process_audio(&chunk, tx).await.unwrap();
        // Production feeds one 100 ms chunk per real 100 ms, so detached HTTP
        // work finishes while the utterance is still open. Without this pause
        // the whole utterance is consumed in a single poll and every live
        // request is still in flight when the segment closes.
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

fn drain(rx: &mut mpsc::Receiver<SpeechEvent>) -> Vec<SpeechEvent> {
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    events
}

fn starts(events: &[SpeechEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, SpeechEvent::SpeechStart { .. }))
        .count()
}

async fn next_speech_end(rx: &mut mpsc::Receiver<SpeechEvent>) -> Option<String> {
    loop {
        match tokio::time::timeout(Duration::from_secs(3), rx.recv()).await {
            Ok(Some(SpeechEvent::SpeechEnd { quality, .. })) => return Some(quality.text),
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => return None,
        }
    }
}

fn body_str(req: &wiremock::Request) -> String {
    String::from_utf8_lossy(&req.body).to_string()
}

#[tokio::test]
async fn auto_language_is_omitted_and_request_shape_is_correct() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "Hola mundo",
            "usage": {"type": "duration", "seconds": 2}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let (mut p, n) = provider(ONE_UTTERANCE, cfg(&server.uri()));
    let (tx, mut rx) = mpsc::channel(16);
    feed(&mut p, &tx, n).await;

    let events = drain(&mut rx);
    assert_eq!(starts(&events), 1, "expected SpeechStart, got {events:?}");
    // The detached final task may have completed before the drain (it runs
    // while the feed loop yields): accept the event from either side.
    let drained = events.iter().find_map(|e| match e {
        SpeechEvent::SpeechEnd { quality, .. } => Some(quality.text.clone()),
        _ => None,
    });
    let text = match drained {
        Some(text) => Some(text),
        None => next_speech_end(&mut rx).await,
    };
    assert_eq!(text.as_deref(), Some("Hola mundo"));

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body = body_str(&requests[0]);
    assert!(body.contains("name=\"model\""), "body: {body}");
    assert!(body.contains("qwen3-asr"), "body: {body}");
    assert!(body.contains("name=\"response_format\""), "body: {body}");
    assert!(body.contains("json"), "body: {body}");
    assert!(body.contains("filename=\"segment.wav\""), "body: {body}");
    // `auto` means auto-detect: the field must be omitted (servers reject "auto").
    assert!(!body.contains("name=\"language\""), "body: {body}");
    assert!(
        requests[0].headers.get("authorization").is_none(),
        "unexpected auth header"
    );
}

#[tokio::test]
async fn pinned_language_is_normalized_to_primary_subtag() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": "x"})))
        .expect(1)
        .mount(&server)
        .await;

    let mut c = cfg(&server.uri());
    c.language = "es-ES".to_string();
    let (mut p, n) = provider(ONE_UTTERANCE, c);
    let (tx, mut rx) = mpsc::channel(16);
    feed(&mut p, &tx, n).await;
    let _ = next_speech_end(&mut rx).await;

    let body = body_str(&server.received_requests().await.unwrap()[0]);
    assert!(body.contains("name=\"language\""), "body: {body}");
    assert!(body.contains("\r\n\r\nes\r\n"), "body: {body}");
}

#[tokio::test]
async fn bearer_auth_is_sent_when_configured() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .and(header("authorization", "Bearer s3cret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": "ok"})))
        .expect(1)
        .mount(&server)
        .await;

    let mut c = cfg(&server.uri());
    c.api_key = "s3cret".to_string();
    let (mut p, n) = provider(ONE_UTTERANCE, c);
    let (tx, mut rx) = mpsc::channel(16);
    feed(&mut p, &tx, n).await;
    assert_eq!(next_speech_end(&mut rx).await.as_deref(), Some("ok"));
}

#[tokio::test]
async fn empty_transcription_still_emits_speech_end_for_the_gate() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"text": "", "usage": {"seconds": 3}})),
        )
        .mount(&server)
        .await;

    let (mut p, n) = provider(ONE_UTTERANCE, cfg(&server.uri()));
    let (tx, mut rx) = mpsc::channel(16);
    feed(&mut p, &tx, n).await;
    assert_eq!(next_speech_end(&mut rx).await.as_deref(), Some(""));
}

#[tokio::test]
async fn service_error_does_not_emit_speech_end() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
            "error": {"message": "boom", "type": "InternalServerError"}
        })))
        .mount(&server)
        .await;

    let (mut p, n) = provider(ONE_UTTERANCE, cfg(&server.uri()));
    let (tx, mut rx) = mpsc::channel(16);
    feed(&mut p, &tx, n).await;
    assert_eq!(starts(&drain(&mut rx)), 1);
    assert!(next_speech_end(&mut rx).await.is_none());
}

#[tokio::test]
async fn malformed_body_does_not_emit_speech_end() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;

    let (mut p, n) = provider(ONE_UTTERANCE, cfg(&server.uri()));
    let (tx, mut rx) = mpsc::channel(16);
    feed(&mut p, &tx, n).await;
    let _ = drain(&mut rx);
    assert!(next_speech_end(&mut rx).await.is_none());
}

#[tokio::test]
async fn timeout_does_not_emit_speech_end() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"text": "late"}))
                .set_delay(Duration::from_millis(3_000)),
        )
        .mount(&server)
        .await;

    let mut c = cfg(&server.uri());
    c.timeout_ms = 1_000; // minimum enforced by the provider
    let (mut p, n) = provider(ONE_UTTERANCE, c);
    let (tx, mut rx) = mpsc::channel(16);
    feed(&mut p, &tx, n).await;
    let _ = drain(&mut rx);
    assert!(next_speech_end(&mut rx).await.is_none());
}

#[tokio::test]
async fn unreachable_service_does_not_hang_or_emit() {
    // Port 1 is never listening.
    let c = cfg("http://127.0.0.1:1");
    let (mut p, n) = provider(ONE_UTTERANCE, c);
    let (tx, mut rx) = mpsc::channel(16);
    feed(&mut p, &tx, n).await;
    assert_eq!(starts(&drain(&mut rx)), 1);
    assert!(next_speech_end(&mut rx).await.is_none());
}

#[tokio::test]
async fn a_delayed_request_completes_when_not_cancelled() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"text": "slow but fine"}))
                .set_delay(Duration::from_millis(300)),
        )
        .mount(&server)
        .await;

    let (mut p, n) = provider(ONE_UTTERANCE, cfg(&server.uri()));
    let (tx, mut rx) = mpsc::channel(16);
    feed(&mut p, &tx, n).await;
    let _ = drain(&mut rx);
    assert_eq!(
        next_speech_end(&mut rx).await.as_deref(),
        Some("slow but fine")
    );
}

#[tokio::test]
async fn transcribe_complete_runs_off_runtime() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "one shot"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let (p, _) = provider(&[], cfg(&server.uri()));
    let quality = p
        .transcribe_complete(&vec![0.1f32; 16_000])
        .expect("transcribe_complete");
    assert_eq!(quality.text, "one shot");
}

#[tokio::test]
async fn verify_server_accepts_known_model_and_rejects_http_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "object": "list",
            "data": [{"id": "qwen3-asr", "object": "model"}]
        })))
        .mount(&server)
        .await;

    let (p, _) = provider(&[], cfg(&server.uri()));
    assert!(p.verify_server().await.is_ok());

    // Unknown model: still Ok (a warning), because gateways may alias models.
    let mut c = cfg(&server.uri());
    c.model = "does-not-exist".to_string();
    let (p, _) = provider(&[], c);
    assert!(p.verify_server().await.is_ok());

    // HTTP error is surfaced.
    let server2 = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(503).set_body_string("down"))
        .mount(&server2)
        .await;
    let (p, _) = provider(&[], cfg(&server2.uri()));
    assert!(p.verify_server().await.is_err());
}

#[tokio::test]
async fn rejects_empty_url_and_model_at_construction() {
    let mut c = cfg("   ");
    c.base_url = "  ".to_string();
    assert!(OpenAiSttProvider::with_detector(scripted(&[]).0, &base_config(), c).is_err());

    let mut c = cfg("http://localhost:9");
    c.model = String::new();
    assert!(OpenAiSttProvider::with_detector(scripted(&[]).0, &base_config(), c).is_err());
}

/// Contract P3 (live): while the user speaks, prefix requests emit
/// `Speech` partials BEFORE the close; the final request covers the complete
/// utterance and produces exactly one `SpeechEnd`.
#[tokio::test]
async fn live_prefix_requests_emit_partials_before_final() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "hola"
        })))
        .mount(&server)
        .await;

    let mut base = base_config();
    base.live = LiveStreamConfig {
        enabled: true,
        interval_ms: 200, // one live request every 2 probes
        window_ms: 15_000,
    };
    // 20 speech probes (2 s) + 5 closing silent probes (post-roll contract).
    let mut probs = vec![0.9f32; 20];
    probs.extend_from_slice(&[0.0; 5]);
    let (detector, len) = scripted(&probs);
    let mut p = OpenAiSttProvider::with_detector(detector, &base, cfg(&server.uri()))
        .expect("provider construction");

    let (tx, mut rx) = mpsc::channel(64);
    feed(&mut p, &tx, len).await;

    // Collect until SpeechEnd; a partial must precede it.
    let mut partials = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let ev = tokio::time::timeout_at(deadline, rx.recv()).await;
        match ev {
            Ok(Some(SpeechEvent::Speech(_))) => partials += 1,
            Ok(Some(SpeechEvent::SpeechEnd { .. })) => break,
            Ok(Some(_)) => continue,
            _ => panic!("stream ended without SpeechEnd (partials={partials})"),
        }
    }
    assert!(
        partials >= 1,
        "live transcription must emit partials during speech (P3)"
    );

    // Live prefix POSTs + the final covering POST.
    let requests = server.received_requests().await.unwrap().len();
    assert!(
        requests >= 2,
        "expected live + final requests, got {requests}"
    );
}

/// P3 (production path): `SpeechStart` fires at the acoustic onset and opens
/// the live state; the pipeline never cancels STT on barge-in, so the segment
/// the user just started keeps sending prefix requests and emitting partials.
#[tokio::test]
async fn speechstart_keeps_live_transcription() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "hola"
        })))
        .mount(&server)
        .await;

    let mut base = base_config();
    base.live = LiveStreamConfig {
        enabled: true,
        interval_ms: 200,
        window_ms: 15_000,
    };
    let mut probs = vec![0.9f32; 20];
    probs.extend_from_slice(&[0.0; 5]);
    let (detector, len) = scripted(&probs);
    let mut p = OpenAiSttProvider::with_detector(detector, &base, cfg(&server.uri()))
        .expect("provider construction");

    let (tx, mut rx) = mpsc::channel(64);
    // Feed up to the SpeechStart (fired at the onset probe).
    feed(&mut p, &tx, 2).await;
    assert_eq!(
        starts(&drain(&mut rx)),
        1,
        "the segment must open at the onset probe"
    );

    feed(&mut p, &tx, len - 2).await;

    let mut partials = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(SpeechEvent::Speech(_))) => partials += 1,
            Ok(Some(SpeechEvent::SpeechEnd { .. })) => break,
            Ok(Some(_)) => continue,
            _ => panic!("stream ended without SpeechEnd (partials={partials})"),
        }
    }
    assert!(
        partials >= 1,
        "live transcription must run during the segment (P3)"
    );
}

// ── Live service integration (ignored by default) ──────────────────────────────
// Run against a real OpenAI-compatible STT service:
//   STT_TEST_URL=http://localhost:8024 \
//     cargo test -p seneschal-core --features openai-stt -- --ignored stt_openai_live
//
// `make test-stt-openai` wraps this and skips when the service is unreachable.

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf()
}

fn vad_model_path() -> Option<PathBuf> {
    let candidates = [
        std::env::var("VAD_MODEL").unwrap_or_default(),
        "models/ggml-silero-v5.1.2.bin".to_string(),
        "models/ggml-silero-vad.bin".to_string(),
    ];
    candidates.iter().find_map(|c| {
        if c.is_empty() {
            return None;
        }
        let p = Path::new(c);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            workspace_root().join(p)
        };
        abs.exists().then_some(abs)
    })
}

fn read_wav_f32(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("read wav");
    let mut pos = 12usize;
    let mut data: Option<Vec<u8>> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        if id == b"data" {
            data = Some(bytes[body..body + size.min(bytes.len() - body)].to_vec());
            break;
        }
        pos = body + size + (size % 2);
    }
    let data = data.expect("wav data chunk");
    data.as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
        .collect()
}

fn test_url() -> String {
    std::env::var("STT_TEST_URL").unwrap_or_else(|_| "http://localhost:8024".to_string())
}

/// Batch path against the real service: no VAD needed, exercises the HTTP
/// contract end to end.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible STT service (STT_TEST_URL)"]
async fn stt_openai_live_batch_transcription() {
    let audio = read_wav_f32(&workspace_root().join("tests/fixtures/en_short_greeting.wav"));
    let (p, _) = provider(&[], cfg(&test_url()));
    let quality = p.transcribe_complete(&audio).expect("transcribe_complete");
    println!("[stt_openai_live_batch] {:?}", quality.text);
    let text = quality.text.to_lowercase();
    assert!(
        text.contains("hello") || text.contains("how are you"),
        "unexpected transcription: {:?}",
        quality.text
    );
}

/// Full local VAD + segmentation + remote transcription against the real
/// service. Skips when the Silero VAD model is not present.
#[tokio::test]
#[ignore = "requires a running STT service and the local Silero VAD model"]
async fn stt_openai_live_pipeline_transcribes_english() {
    let Some(vad) = vad_model_path() else {
        eprintln!("SKIP: Silero VAD model not found (set VAD_MODEL)");
        return;
    };

    let audio = read_wav_f32(&workspace_root().join("tests/fixtures/en_short_greeting.wav"));
    let mut base = base_config();
    base.vad_model = vad.to_string_lossy().to_string();
    let mut c = cfg(&test_url());
    c.language = "auto".to_string();
    let mut p = OpenAiSttProvider::new(base, c).expect("provider construction");

    let (tx, mut rx) = mpsc::channel(64);
    for chunk in audio.chunks(1_600) {
        p.process_audio(chunk, &tx).await.unwrap();
    }
    // Flush with silence so the final segment closes.
    let silence = vec![0.0f32; 16_000 * 4];
    for chunk in silence.chunks(1_600) {
        p.process_audio(chunk, &tx).await.unwrap();
    }

    let text = next_speech_end(&mut rx).await.expect("no SpeechEnd");
    println!("[stt_openai_live_pipeline] {text:?}");
    let lower = text.to_lowercase();
    assert!(
        lower.contains("hello") || lower.contains("how are you"),
        "unexpected transcription: {text:?}"
    );
}
