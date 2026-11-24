#[cfg(feature = "avspeech")]
pub mod avspeech;
#[cfg(feature = "kokoro")]
pub mod kokoro;
#[cfg(feature = "openai-tts")]
pub mod openai;
pub mod sentence;

#[cfg(feature = "avspeech")]
pub use avspeech::AvSpeechTts;
#[cfg(feature = "kokoro")]
pub use kokoro::KokoroTts;
#[cfg(feature = "openai-tts")]
pub use openai::{OpenAiTts, OpenAiTtsConfig};
pub use sentence::SentenceSplitter;

use tokio::sync::mpsc;

/// Streaming synthesis handle shared by every TTS backend.
///
/// A provider that truly streams (the `openai` backend) fills `rx` progressively; the
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
/// - `"openai"` — Qwen3-TTS via an OpenAI-compatible service (OpenAI-compatible
///   speech API); requires `--features openai-tts`. Configured by `OPENAI_TTS_*`.
///
/// All variants expose the same `synthesize` / `synthesize_stream` /
/// `sample_rate` interface so the rest of the pipeline is backend-agnostic.
pub enum TtsEngine {
    #[cfg(feature = "avspeech")]
    AvSpeech(AvSpeechTts),
    #[cfg(feature = "kokoro")]
    Kokoro(KokoroTts),
    #[cfg(feature = "openai-tts")]
    OpenAi(OpenAiTts),
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
            #[cfg(feature = "openai-tts")]
            Self::OpenAi(_) => Err(anyhow::anyhow!(
                "the openai provider is streaming-only; use synthesize_stream()"
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
            #[cfg(feature = "openai-tts")]
            Self::OpenAi(t) => {
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
            #[cfg(feature = "openai-tts")]
            Self::OpenAi(t) => t.sample_rate(),
            Self::Mock(t) => t.sample_rate(),
            _ => unreachable!("no TTS backend enabled"),
        }
    }

    /// Human-readable backend token (`avspeech`, `kokoro`, `openai`, `mock`).
    #[allow(unreachable_patterns)]
    pub fn provider_name(&self) -> &'static str {
        match self {
            #[cfg(feature = "avspeech")]
            Self::AvSpeech(_) => "avspeech",
            #[cfg(feature = "kokoro")]
            Self::Kokoro(_) => "kokoro",
            #[cfg(feature = "openai-tts")]
            Self::OpenAi(_) => "openai",
            Self::Mock(_) => "mock",
            _ => unreachable!("no TTS backend enabled"),
        }
    }

    /// TTS backend tokens compiled into this binary (issue #33).
    pub fn supported_providers() -> Vec<&'static str> {
        #[allow(unused_mut)]
        let mut providers = Vec::new();
        #[cfg(feature = "avspeech")]
        providers.push("avspeech");
        #[cfg(feature = "kokoro")]
        providers.push("kokoro");
        #[cfg(feature = "openai-tts")]
        providers.push("openai");
        providers
    }

    /// Apply runtime-switchable voice/rate settings without rebuilding the
    /// backend or restarting the process (issue #33). `voice` and `rate` are
    /// provider-specific config fields: Kokoro/OpenAI use "speed" as rate.
    #[allow(unreachable_patterns)]
    #[allow(unused_variables)]
    pub fn apply_voice_and_rate(
        &mut self,
        voice: Option<&str>,
        rate: Option<f32>,
    ) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "avspeech")]
            Self::AvSpeech(t) => {
                if let Some(voice) = voice {
                    t.set_voice_name(voice)?;
                }
                if let Some(rate) = rate {
                    t.set_rate(rate);
                }
            }
            #[cfg(feature = "kokoro")]
            Self::Kokoro(t) => {
                if let Some(voice) = voice {
                    t.set_voice(voice);
                }
                if let Some(rate) = rate {
                    t.set_speed(rate);
                }
            }
            #[cfg(feature = "openai-tts")]
            Self::OpenAi(t) => {
                if let Some(voice) = voice {
                    t.set_voice(voice);
                }
                if let Some(rate) = rate {
                    t.set_speed(rate);
                }
            }
            Self::Mock(_) => {}
            _ => unreachable!("no TTS backend enabled"),
        }
        Ok(())
    }
}

/// Test-only TTS backend.
pub mod mock_tts {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    pub struct MockTts {
        /// Accumulates every sentence passed to `synthesize`.
        pub captured: Arc<Mutex<Vec<String>>>,
        /// Test-only gate: when set, `synthesize` records the text then spins
        /// (yielding the OS thread) until the flag is true. Used with a
        /// multi-thread runtime to hold synthesis open while a barge-in is
        /// delivered.
        gate: Option<Arc<AtomicBool>>,
    }

    impl MockTts {
        /// Returns `(MockTts, captured)` where `captured` is shared with the engine
        /// and accumulates every sentence text passed to `synthesize()`.
        pub fn new() -> (Self, Arc<Mutex<Vec<String>>>) {
            let cap = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    captured: Arc::clone(&cap),
                    gate: None,
                },
                cap,
            )
        }

        /// Like [`MockTts::new`], but `synthesize` blocks (spinning) until the
        /// returned flag is set — used to exercise barge-in during synthesis.
        pub fn gated() -> (Self, Arc<Mutex<Vec<String>>>, Arc<AtomicBool>) {
            let cap = Arc::new(Mutex::new(Vec::new()));
            let gate = Arc::new(AtomicBool::new(false));
            (
                Self {
                    captured: Arc::clone(&cap),
                    gate: Some(Arc::clone(&gate)),
                },
                cap,
                gate,
            )
        }

        pub fn synthesize(&self, text: &str) -> anyhow::Result<Vec<f32>> {
            self.captured.lock().unwrap().push(text.to_string());
            if let Some(gate) = &self.gate {
                while !gate.load(Ordering::SeqCst) {
                    std::thread::yield_now();
                }
            }
            Ok(vec![0.0f32]) // 1 silent sample → playback returns immediately
        }

        pub fn sample_rate(&self) -> u32 {
            22050
        }
    }

    #[cfg(test)]
    mod hot_settings_tests {
        use super::super::{TtsEngine, mock_tts::MockTts};

        /// The unified hot-apply seam must work for every backend; Mock covers
        /// the shared dispatch without audio hardware.
        #[test]
        fn mock_accepts_hot_voice_and_rate() {
            let mut engine = TtsEngine::Mock(MockTts::new().0);
            engine
                .apply_voice_and_rate(Some("test"), Some(0.8))
                .unwrap();
            assert_eq!(engine.provider_name(), "mock");
        }
    }
}
