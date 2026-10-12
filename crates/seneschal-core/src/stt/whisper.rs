use anyhow::{Context, Result};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;
use whisper_cpp_plus::enhanced::fallback::calculate_compression_ratio;
use whisper_cpp_plus::{
    FullParams, SamplingStrategy, VadContextParams, WhisperContext, WhisperVadProcessor,
};

use super::SpeechEvent;
use super::no_speech_gate::TranscriptionQuality;
use super::provider::SttProvider;
use super::segmenter::{
    SAMPLE_RATE, SegmenterConfig, SegmenterEvent, SileroDetector, VadSegmenter,
};

#[derive(Clone)]
pub struct WhisperSTTVADConfig {
    pub whisper_model: String,
    pub vad_model: String,
    pub language: String,
    /// Milliseconds of continuous silence required to close a speech segment.
    pub silence_ms: u32,
    /// Speech probability threshold to transition from silence -> speech.
    pub vad_start_threshold: f32,
    /// Speech probability threshold to stay in speech (speech -> silence when below).
    pub vad_end_threshold: f32,
    /// Minimum consecutive speech probes required before committing to STT.
    /// A single 200ms probe above threshold is no longer enough — this many
    /// consecutive probes must all be above threshold. Default: 2 (400ms).
    /// Set to 1 to disable (current behavior).
    pub vad_confirm_probes: usize,
    /// GPU device for the Silero VAD context. `None` = CPU.
    pub vad_gpu: Option<i32>,
    /// GPU device for the Parakeet ONNX model. `None` = CPU.
    pub parakeet_gpu: Option<i32>,
}

impl Default for WhisperSTTVADConfig {
    fn default() -> Self {
        Self {
            whisper_model: "models/ggml-large-v3-turbo.bin".to_string(),
            vad_model: "models/ggml-silero-vad.bin".to_string(),
            language: "es".to_string(),
            silence_ms: 500,
            vad_start_threshold: 0.65,
            vad_end_threshold: 0.45,
            vad_confirm_probes: 2,
            vad_gpu: None,
            parakeet_gpu: None,
        }
    }
}

/// Load the Silero VAD context on `gpu` when requested (issue #14).
///
/// Whisper itself is not device-pinned: upstream `whisper-cpp-plus` always
/// uses whisper.cpp's default device (GPU 0 of the visible devices when built
/// with the `cuda` feature). Use `CUDA_VISIBLE_DEVICES` to reorder devices.
pub(crate) fn load_vad(model_path: &str, gpu: Option<i32>) -> Result<WhisperVadProcessor> {
    let params = VadContextParams {
        use_gpu: gpu.is_some(),
        gpu_device: gpu.unwrap_or(0),
        ..Default::default()
    };
    WhisperVadProcessor::new_with_params(model_path, params).context("Failed to load VAD model")
}

pub struct WhisperSttProvider {
    ctx: Arc<WhisperContext>,
    language: String,
    segmenter: VadSegmenter<SileroDetector>,
}

impl WhisperSttProvider {
    pub fn new(config: WhisperSTTVADConfig) -> Result<Self> {
        let ctx = Arc::new(
            WhisperContext::new(&config.whisper_model).context("Failed to load Whisper model")?,
        );

        let vad = load_vad(&config.vad_model, config.vad_gpu)?;
        let segmenter = VadSegmenter::new(
            SileroDetector::new(vad),
            SegmenterConfig {
                silence_ms: config.silence_ms,
                vad_start_threshold: config.vad_start_threshold,
                vad_end_threshold: config.vad_end_threshold,
                vad_confirm_probes: config.vad_confirm_probes,
                // Whisper keeps its historical single-stage close (no post-roll)
                // and does not trim the finalized segment.
                post_roll_ms: 0,
                trim_leading_silence: false,
            },
        )?;

        tracing::info!(
            target: "sttvad",
            "WhisperSTTVAD ready (whisper: {}, vad: {}, lang: {}, silence_ms: {}, start_thr: {:.2}, end_thr: {:.2}, confirm_probes: {}, vad_gpu: {})",
            config.whisper_model,
            config.vad_model,
            config.language,
            config.silence_ms,
            config.vad_start_threshold,
            config.vad_end_threshold,
            config.vad_confirm_probes.max(1),
            config.vad_gpu.map(|i| i.to_string()).unwrap_or_else(|| "cpu".into()),
        );

        Ok(Self {
            ctx,
            language: config.language,
            segmenter,
        })
    }

    /// Feed a chunk of 16 kHz mono f32 audio. Emits events as the VAD/state
    /// machine advances. Transcription runs on a blocking thread and the loop
    /// awaits it, so events keep their historical ordering (SpeechStart before
    /// the matching SpeechEnd).
    pub async fn process_audio(
        &mut self,
        audio: &[f32],
        tx: &mpsc::Sender<SpeechEvent>,
    ) -> Result<()> {
        for event in self.segmenter.feed(audio) {
            match event {
                SegmenterEvent::SpeechStart => {
                    let _ = tx.send(SpeechEvent::SpeechStart).await;
                }
                SegmenterEvent::SegmentReady(audio) => {
                    tracing::debug!(
                        target: "sttvad",
                        "Finalizing segment: {:.2}s",
                        audio.len() as f32 / SAMPLE_RATE as f32
                    );

                    let ctx = Arc::clone(&self.ctx);
                    let language = self.language.clone();
                    let quality =
                        tokio::task::spawn_blocking(move || -> Result<TranscriptionQuality> {
                            transcribe(&ctx, &language, &audio)
                        })
                        .await
                        .context("transcription task join")??;

                    tracing::info!(target: "sttvad", "SpeechEnd: {}", quality.text);
                    let _ = tx.send(SpeechEvent::SpeechEnd(quality)).await;
                }
            }
        }

        Ok(())
    }

    /// Blocking one-shot transcription (used as a fallback / sanity check).
    #[allow(dead_code)]
    pub fn transcribe_complete(&self, audio: &[f32]) -> Result<TranscriptionQuality> {
        transcribe(&self.ctx, &self.language, audio)
    }
}

#[async_trait]
impl SttProvider for WhisperSttProvider {
    fn provider_name(&self) -> &'static str {
        "whisper"
    }

    async fn process_audio(&mut self, audio: &[f32], tx: &mpsc::Sender<SpeechEvent>) -> Result<()> {
        WhisperSttProvider::process_audio(self, audio, tx).await
    }

    fn transcribe_complete(&self, audio: &[f32]) -> Result<TranscriptionQuality> {
        WhisperSttProvider::transcribe_complete(self, audio)
    }
}

pub type WhisperSTTVAD = WhisperSttProvider;

/// Map the configured STT language to the value passed to whisper-cpp.
///
/// "auto" (case-insensitive) or an empty string selects language
/// auto-detection. whisper-cpp-plus's `FullParams::new()` calls
/// `whisper_full_default_params`, which defaults `language` to "en" — so we
/// must explicitly pass an empty string to opt into auto-detection (the C
/// layer treats "" / nullptr / "auto" as auto-detect, see
/// `whisper_full_with_state`). Any concrete code ("es", "en", "es-ES") is
/// passed through unchanged to pin the language.
pub fn resolve_whisper_language(language: &str) -> &str {
    if language.eq_ignore_ascii_case("auto") || language.is_empty() {
        ""
    } else {
        language
    }
}

fn transcribe(ctx: &WhisperContext, language: &str, audio: &[f32]) -> Result<TranscriptionQuality> {
    if audio.is_empty() {
        return Ok(TranscriptionQuality {
            text: String::new(),
            no_speech_prob: 1.0,
            avg_logprob: 0.0,
            compression_ratio: 0.0,
        });
    }

    let mut state = ctx.create_state()?;
    let params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 })
        .language(resolve_whisper_language(language))
        .print_special(false)
        .print_progress(false)
        .print_realtime(false)
        .print_timestamps(false)
        .no_timestamps(true)
        .single_segment(true);

    state.full(params, audio)?;

    let n = state.full_n_segments();
    let mut text = String::new();
    for i in 0..n {
        if let Ok(seg) = state.full_get_segment_text(i) {
            text.push_str(seg.trim());
            text.push(' ');
        }
    }
    let text = text.trim().to_string();

    // Calculate avg logprob from token probabilities
    let avg_logprob = {
        let mut total_logprob = 0.0f32;
        let mut total_tokens = 0i32;
        for i in 0..n {
            let n_tokens = state.full_n_tokens(i);
            if n_tokens > 0 {
                for t in 0..n_tokens {
                    let prob = state.full_get_token_prob(i, t);
                    if prob > 0.0 {
                        total_logprob += prob.ln();
                    }
                }
                total_tokens += n_tokens;
            }
        }
        if total_tokens > 0 {
            total_logprob / total_tokens as f32
        } else {
            0.0
        }
    };

    let compression_ratio = if !text.is_empty() {
        calculate_compression_ratio(&text)
    } else {
        0.0
    };

    // no_speech_prob: WhisperState.ptr is pub(crate), so we can't access it
    // from outside the whisper-cpp-plus crate. We use 0.0 as a placeholder.
    // The quality gate still works with avg_logprob and compression_ratio.
    let no_speech_prob = 0.0;

    Ok(TranscriptionQuality {
        text,
        no_speech_prob,
        avg_logprob,
        compression_ratio,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "auto" (any case) and empty string must map to "" so whisper-cpp
    /// auto-detects the spoken language (issue #217 — code-switching).
    /// A concrete code is passed through unchanged.
    #[test]
    fn resolve_whisper_language_auto_detects() {
        assert_eq!(resolve_whisper_language("auto"), "");
        assert_eq!(resolve_whisper_language("AUTO"), "");
        assert_eq!(resolve_whisper_language("Auto"), "");
        assert_eq!(resolve_whisper_language(""), "");
        // Concrete codes pin the language verbatim.
        assert_eq!(resolve_whisper_language("es"), "es");
        assert_eq!(resolve_whisper_language("en"), "en");
        assert_eq!(resolve_whisper_language("es-ES"), "es-ES");
    }
}
