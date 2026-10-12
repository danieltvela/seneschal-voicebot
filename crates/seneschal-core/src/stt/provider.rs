use anyhow::{Result, bail};
use async_trait::async_trait;
use tokio::sync::mpsc;

use seneschal_common::config::Config;

use super::SpeechEvent;
use super::no_speech_gate::TranscriptionQuality;
use super::whisper::{WhisperSTTVADConfig, WhisperSttProvider};

#[async_trait]
pub trait SttProvider: Send {
    fn provider_name(&self) -> &'static str;
    async fn process_audio(&mut self, audio: &[f32], tx: &mpsc::Sender<SpeechEvent>) -> Result<()>;
    fn transcribe_complete(&self, audio: &[f32]) -> Result<TranscriptionQuality>;

    /// Best-effort startup check that the configured backend is reachable
    /// (issue #17: "surface a bad URL/model early").
    ///
    /// Local backends accept the no-op default; remote backends override it.
    /// Callers should log and continue on failure, since an external service may
    /// still be starting.
    async fn verify_server(&mut self) -> Result<()> {
        Ok(())
    }

    /// Abort any in-flight transcription request (barge-in / pipeline cancel).
    ///
    /// Local backends are synchronous and accept the no-op default; remote
    /// backends (issue #17) cancel the pending HTTP request so no bandwidth is
    /// wasted on a superseded utterance.
    fn cancel_inflight(&mut self) {}

    /// Number of remote transcriptions currently in flight.
    ///
    /// Synchronous/local backends return 0 (their work happens inside
    /// `process_audio`). Used for observability and by test harnesses that need
    /// to wait for out-of-band results.
    fn inflight_transcriptions(&self) -> usize {
        0
    }
}

pub fn create_provider(
    config: &Config,
    assignments: &seneschal_common::gpu::GpuAssignments,
) -> Result<Box<dyn SttProvider>> {
    let whisper_cfg = WhisperSTTVADConfig {
        whisper_model: config.whisper_model.clone(),
        vad_model: config.vad_model.clone(),
        language: config.language.clone(),
        silence_ms: config.vad_silence_ms,
        vad_start_threshold: config.vad_start_threshold,
        vad_end_threshold: config.vad_end_threshold,
        vad_confirm_probes: config.vad_confirm_probes,
        vad_gpu: assignments.vad.index(),
        parakeet_gpu: assignments.parakeet.index(),
    };

    match config.stt_provider.to_lowercase().as_str() {
        "whisper" => {
            // Whisper uses the dedicated stt_language (issue #217): "auto" by
            // default enables auto-detection for code-switched phrases. The
            // shared whisper_cfg keeps config.language for parakeet/speech.
            let mut cfg = whisper_cfg;
            cfg.language = config.stt_language.clone();
            Ok(Box::new(WhisperSttProvider::new(cfg)?))
        }
        "parakeet" => {
            #[cfg(feature = "parakeet")]
            {
                let provider = super::parakeet::ParakeetSttProvider::new(
                    whisper_cfg,
                    config.parakeet_model_dir.as_deref(),
                )?;
                Ok(Box::new(provider))
            }

            #[cfg(not(feature = "parakeet"))]
            {
                bail!(
                    "STT_PROVIDER=parakeet requested but the 'parakeet' feature is not enabled. Rebuild with: cargo run --features parakeet"
                );
            }
        }
        "speech" => {
            #[cfg(feature = "speech")]
            {
                let provider =
                    super::speech_recognizer::SpeechRecognizerSttProvider::new(whisper_cfg)?;
                Ok(Box::new(provider))
            }

            #[cfg(not(feature = "speech"))]
            {
                bail!(
                    "STT_PROVIDER=speech requested but the 'speech' feature is not enabled. Rebuild with: cargo run --features speech"
                );
            }
        }
        "openai" => {
            #[cfg(feature = "openai-stt")]
            {
                let provider = super::openai::OpenAiSttProvider::new(
                    whisper_cfg,
                    super::openai::OpenAiSttConfig {
                        base_url: config.stt_url.clone(),
                        api_key: config.stt_api_key.clone(),
                        model: config.stt_model.clone(),
                        language: config.stt_language.clone(),
                        timeout_ms: config.stt_timeout_ms,
                    },
                )?;
                Ok(Box::new(provider))
            }

            #[cfg(not(feature = "openai-stt"))]
            {
                bail!(
                    "STT_PROVIDER=openai requested but the 'openai-stt' feature is not enabled. Rebuild with: cargo run --features openai-stt"
                );
            }
        }
        other => {
            bail!(
                "Invalid STT_PROVIDER '{other}'. Supported values: \
                 whisper (default, whisper.cpp + Silero VAD, no extra feature), \
                 parakeet (needs --features parakeet + PARAKEET_MODEL_DIR), \
                 speech (macOS SFSpeechRecognizer, needs --features speech), \
                 openai (OpenAI-compatible remote STT, needs --features openai-stt). \
                 Note: for TTS backends use TTS_PROVIDER (avspeech, kokoro, openai)."
            )
        }
    }
}
