//! OpenAI-compatible TTS provider (issue #16).
//!
//! Consumes an OpenAI-compatible speech API (`POST /v1/audio/speech`). The
//! provider targets the API shape, not a specific runtime, so it serves both
//! the original vLLM-Omni container and NVIDIA NeMo-Speech.cpp (MagpieTTS +
//! NanoCodec, issue #20). Audio is streamed to the caller as raw 16-bit mono
//! PCM chunks so playback can start before the sentence finishes synthesising.
//!
//! Verified against:
//! - `vllm/vllm-omni:qwen-image21` serving `Qwen3-TTS-12Hz-1.7B-CustomVoice`:
//!   ~24 kHz output, first non-silent sample ~0.4 s, ~5× real-time throughput.
//! - `NeMo-Speech.cpp` serving MagpieTTS Multilingual + NanoCodec: 22050 Hz
//!   output, whole-utterance buffered responses (`stream`/`stream_format` are
//!   accepted but ignored), voices/languages listed by `GET /v1/models`.
//!
//! Because the runtimes disagree on PCM rate and voice discovery, the sample
//! rate is configurable (`OPENAI_TTS_SAMPLE_RATE`) and voice discovery falls
//! back to `GET /v1/models` when `/v1/audio/voices` is absent.

pub mod error;
pub mod pcm;
pub mod voices;

use std::time::Duration;

pub use error::{OpenAiTtsError, OpenAiTtsResult};
pub use voices::{UploadedVoice, VoicesResponse};

use serde::Serialize;
use tokio::sync::mpsc;

/// Default sample rate of the PCM returned in `response_format=pcm` by the
/// Qwen3-TTS deployment. Overridden per deployment via
/// `OPENAI_TTS_SAMPLE_RATE` (MagpieTTS/NanoCodec emits 22050 Hz).
pub const OPENAI_TTS_SAMPLE_RATE: u32 = 24_000;

/// Bounded depth (in chunks) of the synthesis → playback channel. Provides
/// backpressure so a fast producer cannot grow memory unbounded.
const STREAM_CHANNEL_CAPACITY: usize = 32;

/// Runtime configuration for [`OpenAiTts`].
#[derive(Debug, Clone)]
pub struct OpenAiTtsConfig {
    /// Service base URL, e.g. `http://localhost:8025`.
    pub base_url: String,
    /// Preset voice name.
    pub voice: String,
    /// Language hint: `"auto"` (case-insensitive) is sent as the literal
    /// `"Auto"`; any other value is forwarded verbatim.
    pub language: String,
    /// Optional natural-language style/emotion instruction.
    pub instructions: String,
    /// Speech speed (> 0 is forwarded; 0 omits the field).
    pub speed: f32,
    /// PCM sample rate (Hz) requested from the service and reported to the
    /// pipeline. `0` omits the field and falls back to
    /// [`OPENAI_TTS_SAMPLE_RATE`]. Set 22050 for MagpieTTS (issue #20).
    pub sample_rate: u32,
    /// Per-request timeout (must cover the ~90 s container warmup).
    pub timeout_ms: u64,
}

impl OpenAiTtsConfig {
    /// The wire value for the `language` field. `"auto"` maps to `"Auto"`.
    fn wire_language(&self) -> String {
        if self.language.eq_ignore_ascii_case("auto") {
            "Auto".to_string()
        } else {
            self.language.clone()
        }
    }

    /// PCM rate reported to the pipeline and requested from the service. `0`
    /// falls back to [`OPENAI_TTS_SAMPLE_RATE`].
    fn effective_sample_rate(&self) -> u32 {
        if self.sample_rate == 0 {
            OPENAI_TTS_SAMPLE_RATE
        } else {
            self.sample_rate
        }
    }
}

/// Body of a `POST /v1/audio/speech` request (only the fields we use).
#[derive(Debug, Serialize)]
struct SpeechRequest<'a> {
    input: &'a str,
    voice: &'a str,
    response_format: &'a str,
    stream: bool,
    stream_format: &'a str,
    language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speed: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sample_rate: Option<u32>,
}

/// Streaming synthesis handle handed to the pipeline.
#[derive(Debug)]
pub struct OpenAiTtsStream {
    /// Receives mono f32 PCM chunks as they are decoded.
    pub rx: mpsc::Receiver<Vec<f32>>,
    /// Sample rate of the received chunks.
    pub sample_rate: u32,
}

/// The OpenAI-compatible TTS client.
pub struct OpenAiTts {
    client: reqwest::Client,
    cfg: OpenAiTtsConfig,
}

impl OpenAiTts {
    /// Build a client. Performs a best-effort voice check against the service
    /// (a warning, never a hard failure, when the service is unreachable at
    /// startup).
    pub fn new(cfg: OpenAiTtsConfig) -> OpenAiTtsResult<Self> {
        if cfg.voice.trim().is_empty() {
            return Err(OpenAiTtsError::Invalid("OPENAI_TTS_VOICE is empty".into()));
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_millis(cfg.timeout_ms.max(1_000)))
            .build()
            .map_err(voices::map_reqwest)?;
        Ok(Self { client, cfg })
    }

    /// Verify the configured voice exists on the service.
    ///
    /// Returns the provider error on a rejection (unknown voice) or transport
    /// failure; callers that should not abort on a transient outage can log and
    /// continue instead.
    pub async fn verify_voice(&self) -> OpenAiTtsResult<()> {
        match voices::validate_voice(&self.client, &self.cfg.base_url, &self.cfg.voice).await {
            Ok(()) => {
                tracing::info!(
                    target: "tts",
                    "OpenAI TTS voice '{}' accepted by {}",
                    self.cfg.voice,
                    self.cfg.base_url
                );
                Ok(())
            }
            Err(OpenAiTtsError::Invalid(msg)) => {
                tracing::warn!(target: "tts", "{msg}");
                Err(OpenAiTtsError::Invalid(msg))
            }
            Err(e) => {
                tracing::warn!(
                    target: "tts",
                    "Could not verify voice against {}: {e} (service may still be starting)",
                    self.cfg.base_url
                );
                Err(e)
            }
        }
    }

    /// Warm up the service with a throwaway request. The first request after
    /// the container starts takes ~90 s; doing it at startup keeps the first
    /// real turn fast.
    pub async fn warmup(&self) -> OpenAiTtsResult<()> {
        let started = std::time::Instant::now();
        tracing::info!(target: "tts", "Warming up OpenAI TTS service at {} ...", self.cfg.base_url);
        let mut stream = self.synthesize_stream("Hola.").await?;
        let mut got_audio = false;
        while let Some(chunk) = stream.rx.recv().await {
            if !chunk.is_empty() {
                got_audio = true;
            }
        }
        tracing::info!(
            target: "tts",
            "OpenAI TTS warmup finished in {} ms (audio={got_audio})",
            started.elapsed().as_millis()
        );
        Ok(())
    }

    /// Sample rate of the produced audio.
    pub fn sample_rate(&self) -> u32 {
        self.cfg.effective_sample_rate()
    }

    /// Start streaming synthesis of `text`.
    ///
    /// Spawns a task that POSTs to `/v1/audio/speech` and pushes decoded f32
    /// chunks into a bounded channel. The channel is bounded so a stalled
    /// consumer applies backpressure to the HTTP read.
    pub async fn synthesize_stream(&self, text: &str) -> OpenAiTtsResult<OpenAiTtsStream> {
        if text.trim().is_empty() {
            return Err(OpenAiTtsError::Invalid("empty synthesis text".into()));
        }

        let body = SpeechRequest {
            input: text,
            voice: &self.cfg.voice,
            response_format: "pcm",
            stream: true,
            stream_format: "audio",
            language: self.cfg.wire_language(),
            instructions: if self.cfg.instructions.trim().is_empty() {
                None
            } else {
                Some(self.cfg.instructions.as_str())
            },
            speed: if self.cfg.speed > 0.0 {
                Some(self.cfg.speed)
            } else {
                None
            },
            // Only requested when explicitly configured: the original Qwen3-TTS
            // deployment ignores the field and always emits 24 kHz.
            sample_rate: if self.cfg.sample_rate > 0 {
                Some(self.cfg.sample_rate)
            } else {
                None
            },
        };

        let url = format!(
            "{}/v1/audio/speech",
            self.cfg.base_url.trim_end_matches('/')
        );

        // Establish the connection eagerly so HTTP/connection errors surface
        // here rather than inside the spawned producer task.
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(voices::map_reqwest)?;

        let status = resp.status();
        if !status.is_success() {
            let code = status.as_u16();
            let raw = resp.text().await.unwrap_or_default();
            return Err(OpenAiTtsError::Service {
                status: code,
                message: extract_error_message(&raw),
            });
        }

        let (tx, rx) = mpsc::channel::<Vec<f32>>(STREAM_CHANNEL_CAPACITY);
        tokio::spawn(async move {
            if let Err(e) = pump_response(resp, tx).await {
                tracing::warn!(target: "tts", "OpenAI TTS stream ended with error: {e}");
            }
        });

        Ok(OpenAiTtsStream {
            rx,
            sample_rate: self.cfg.effective_sample_rate(),
        })
    }

    /// `--list-voices` implementation.
    pub async fn list_voices(&self) -> OpenAiTtsResult<()> {
        voices::print_voices(&self.client, &self.cfg.base_url).await
    }
}

/// Read the HTTP body incrementally, decode PCM and forward f32 chunks.
async fn pump_response(resp: reqwest::Response, tx: mpsc::Sender<Vec<f32>>) -> OpenAiTtsResult<()> {
    use futures_util::StreamExt;

    let mut converter = pcm::Pcm16ToF32::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(voices::map_reqwest)?;
        let samples = converter.push(&bytes);
        if !samples.is_empty() && tx.send(samples).await.is_err() {
            // Consumer went away (barge-in) — stop reading.
            return Ok(());
        }
    }
    let dropped = converter.finish();
    if dropped > 0 {
        tracing::debug!(target: "tts", "OpenAI TTS stream ended with {dropped} dangling byte(s)");
    }
    Ok(())
}

/// Extract the human-readable detail from an OpenAI-style error body.
fn extract_error_message(raw: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(msg) = v
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            return msg.to_string();
        }
        if let Some(msg) = v.get("detail").and_then(|d| d.as_str()) {
            return msg.to_string();
        }
    }
    if raw.trim().is_empty() {
        "<no body>".to_string()
    } else {
        raw.trim().to_string()
    }
}
