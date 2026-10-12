#[cfg(feature = "avspeech")]
pub mod avspeech;
#[cfg(feature = "kokoro")]
pub mod kokoro;
pub mod sentence;
#[cfg(feature = "vllm-tts")]
pub mod vllm;

#[cfg(feature = "avspeech")]
pub use avspeech::AvSpeechTts;
#[cfg(feature = "kokoro")]
pub use kokoro::KokoroTts;
pub use sentence::SentenceSplitter;
#[cfg(feature = "vllm-tts")]
pub use vllm::{VllmTts, VllmTtsConfig};

use tokio::sync::mpsc;

/// Streaming synthesis handle shared by every TTS backend.
///
/// A provider that truly streams (vLLM-Omni) fills `rx` progressively; the
/// non-streaming backends (AVSpeech, Kokoro, Mock) send a single `Vec<f32>`
/// with the whole utterance and close the channel. The pipeline therefore has
/// one code path: pump `rx` into the speaker as chunks arrive.
pub struct TtsStreamHandle {
    pub rx: mpsc::Receiver<Vec<f32>>,
    pub sample_rate: u32,
}

/// Unified TTS backend.
///
/// Select at startup via `TTS_PROVIDER` env var:
/// - `"avspeech"` (default) — Native macOS `AVSpeechSynthesizer`; requires `--features avspeech`.
///   Configured by `AVSPEECH_VOICE` / `AVSPEECH_RATE`.
/// - `"kokoro"` — Kokoro ONNX model; requires `--features kokoro` and espeak-ng.
/// - `"openai"` — Qwen3-TTS via a local vLLM-Omni service (OpenAI-compatible
///   speech API); requires `--features vllm-tts`. Configured by `VLLM_TTS_*`.
///
/// All variants expose the same `synthesize` / `synthesize_stream` /
/// `sample_rate` interface so the rest of the pipeline is backend-agnostic.
pub enum TtsEngine {
    #[cfg(feature = "avspeech")]
    AvSpeech(AvSpeechTts),
    #[cfg(feature = "kokoro")]
    Kokoro(KokoroTts),
    #[cfg(feature = "vllm-tts")]
    Vllm(VllmTts),
    /// Test-only variant: captures synthesized text instead of producing audio.
    Mock(self::mock_tts::MockTts),
}

impl TtsEngine {
    /// Synthesize `text` into a complete mono f32 buffer.
    ///
    /// Kept for callers that need the whole utterance (remote TTS routing) and
    /// for the non-streaming backends. New pipeline code should prefer
    /// [`TtsEngine::synthesize_stream`].
    #[allow(unreachable_patterns)]
    #[allow(unused_variables)]
    pub fn synthesize(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        match self {
            #[cfg(feature = "avspeech")]
            Self::AvSpeech(t) => t.synthesize(text),
            #[cfg(feature = "kokoro")]
            Self::Kokoro(t) => t.synthesize(text),
            Self::Mock(t) => t.synthesize(text),
            #[cfg(feature = "vllm-tts")]
            Self::Vllm(_) => Err(anyhow::anyhow!(
                "the vllm provider is streaming-only; use synthesize_stream()"
            )),
            _ => unreachable!("no TTS backend enabled"),
        }
    }

    /// Begin synthesising `text`, returning a stream of PCM chunks.
    ///
    /// Backends that cannot stream emit a single chunk with the full buffer.
    #[allow(unreachable_patterns)]
    #[allow(unused_variables)]
    pub async fn synthesize_stream(&self, text: &str) -> anyhow::Result<TtsStreamHandle> {
        match self {
            #[cfg(feature = "vllm-tts")]
            Self::Vllm(t) => {
                let stream = t.synthesize_stream(text).await?;
                Ok(TtsStreamHandle {
                    rx: stream.rx,
                    sample_rate: stream.sample_rate,
                })
            }
            other => {
                let samples = other.synthesize(text)?;
                let rate = other.sample_rate();
                let (tx, rx) = mpsc::channel(1);
                let _ = tx.try_send(samples);
                Ok(TtsStreamHandle {
                    rx,
                    sample_rate: rate,
                })
            }
        }
    }

    #[allow(unreachable_patterns)]
    #[allow(unused_variables)]
    pub fn sample_rate(&self) -> u32 {
        match self {
            #[cfg(feature = "avspeech")]
            Self::AvSpeech(t) => t.sample_rate(),
            #[cfg(feature = "kokoro")]
            Self::Kokoro(t) => t.sample_rate(),
            #[cfg(feature = "vllm-tts")]
            Self::Vllm(t) => t.sample_rate(),
            Self::Mock(t) => t.sample_rate(),
            _ => unreachable!("no TTS backend enabled"),
        }
    }
}

/// Test-only TTS backend.
pub mod mock_tts {
    use std::sync::{Arc, Mutex};

    pub struct MockTts(pub Arc<Mutex<Vec<String>>>);

    impl MockTts {
        /// Returns `(MockTts, captured)` where `captured` is shared with the engine
        /// and accumulates every sentence text passed to `synthesize()`.
        pub fn new() -> (Self, Arc<Mutex<Vec<String>>>) {
            let cap = Arc::new(Mutex::new(Vec::new()));
            (Self(Arc::clone(&cap)), cap)
        }

        pub fn synthesize(&self, text: &str) -> anyhow::Result<Vec<f32>> {
            self.0.lock().unwrap().push(text.to_string());
            Ok(vec![0.0f32]) // 1 silent sample → playback returns immediately
        }

        pub fn sample_rate(&self) -> u32 {
            22050
        }
    }
}
