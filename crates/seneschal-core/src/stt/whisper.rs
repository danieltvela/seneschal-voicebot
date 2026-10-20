use anyhow::{Context, Result};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;
use whisper_cpp_plus::enhanced::fallback::calculate_compression_ratio;
use whisper_cpp_plus::{
    FullParams, SamplingStrategy, VadContextParams, WhisperContext, WhisperVadProcessor,
};

use super::SpeechEvent;
use super::local::LocalSttProvider;
use super::no_speech_gate::TranscriptionQuality;
use super::provider::SttProvider;
use super::segmenter::{
    DEFAULT_PREROLL_MS, DEFAULT_PROBE_MS, SegmenterConfig, SileroDetector, VadSegmenter,
};
use super::streaming::{LiveDecoder, LiveStreamConfig};

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
    /// Minimum speech decisions required within the sliding confirmation
    /// window (`CONFIRM_WINDOW` probes) before the VAD *confirms* the
    /// segment — live decoding and the close wait for this, while
    /// `SpeechStart`/barge-in fire earlier, on the first speech probe.
    /// One natural inter-word gap is tolerated inside the window; an
    /// isolated burst (cough/click) fails to confirm. Default: 2.
    pub vad_confirm_probes: usize,
    /// VAD probe window in ms (contract P1).
    pub probe_ms: usize,
    /// Real-audio pre-roll prepended to confirmed segments, in ms (contract P4).
    pub preroll_ms: usize,
    /// Post-roll grace before closing a segment, in ms (contract P6).
    pub post_roll_ms: usize,
    /// Live partial transcription during speech (contract P3/P4).
    pub live: LiveStreamConfig,
    /// GPU device for the Silero VAD context. `None` = CPU.
    pub vad_gpu: Option<i32>,
    /// GPU device for the Parakeet ONNX model. `None` = CPU.
    pub parakeet_gpu: Option<i32>,
}

impl Default for WhisperSTTVADConfig {
    /// Mirrors the production defaults (`Config`: VAD_SILENCE_MS/PROBE_MS/
    /// PREROLL_MS/POST_ROLL_MS and the contract, `doc/stt-contract.md`).
    /// The old values here (100 ms probe, 500 ms silence) predated the
    /// calibration and silently diverged from production.
    fn default() -> Self {
        Self {
            whisper_model: "models/ggml-large-v3-turbo.bin".to_string(),
            vad_model: "models/ggml-silero-vad.bin".to_string(),
            language: "es".to_string(),
            silence_ms: 300,
            vad_start_threshold: 0.65,
            vad_end_threshold: 0.45,
            vad_confirm_probes: 2,
            probe_ms: DEFAULT_PROBE_MS,
            preroll_ms: DEFAULT_PREROLL_MS,
            post_roll_ms: 200,
            live: LiveStreamConfig::default(),
            vad_gpu: None,
            parakeet_gpu: None,
        }
    }
}

/// Uniform [`SegmenterConfig`] shared by whisper / parakeet / openai (P6):
/// same probe size, real-audio pre-roll and post-roll grace. Leading-silence
/// trimming is disabled — the pre-roll carries the onset and trimming would
/// eat it.
pub(crate) fn segmenter_config(base: &WhisperSTTVADConfig) -> SegmenterConfig {
    let mut config = SegmenterConfig::uniform(
        base.silence_ms,
        base.vad_start_threshold,
        base.vad_end_threshold,
        base.vad_confirm_probes,
    );
    config.post_roll_ms = base.post_roll_ms;
    config.probe_ms = base.probe_ms;
    config.preroll_ms = base.preroll_ms;
    config
}

/// Blocking Whisper decoder over one snapshot of the open segment (live).
#[derive(Clone)]
struct WhisperDecoder {
    ctx: Arc<WhisperContext>,
    language: String,
}

impl LiveDecoder for WhisperDecoder {
    fn decode(&self, audio: Vec<f32>) -> Result<TranscriptionQuality> {
        transcribe(&self.ctx, &self.language, &audio)
    }
}

/// Load the Silero VAD context on `gpu` when requested (issue #14).
///
/// Whisper itself is not device-pinned: upstream `whisper-cpp-plus` always
/// uses whisper.cpp's default device (GPU 0 of the visible devices when built
/// with the `cuda` feature). Use `CUDA_VISIBLE_DEVICES` to reorder devices.
pub fn load_vad(model_path: &str, gpu: Option<i32>) -> Result<WhisperVadProcessor> {
    let params = VadContextParams {
        use_gpu: gpu.is_some(),
        gpu_device: gpu.unwrap_or(0),
        ..Default::default()
    };
    WhisperVadProcessor::new_with_params(model_path, params).context("Failed to load VAD model")
}

pub struct WhisperSttProvider {
    pipeline: LocalSttProvider<WhisperDecoder>,
}

impl WhisperSttProvider {
    pub fn new(config: WhisperSTTVADConfig) -> Result<Self> {
        let ctx = Arc::new(
            WhisperContext::new(&config.whisper_model).context("Failed to load Whisper model")?,
        );

        let vad = load_vad(&config.vad_model, config.vad_gpu)?;
        let segmenter = VadSegmenter::new(SileroDetector::new(vad), segmenter_config(&config))?;
        let pipeline = LocalSttProvider::new(
            segmenter,
            WhisperDecoder {
                ctx,
                language: config.language.clone(),
            },
            config.live.clone(),
        );

        tracing::info!(
            target: "sttvad",
            "WhisperSTTVAD ready (whisper: {}, vad: {}, lang: {}, silence_ms: {}, start_thr: {:.2}, end_thr: {:.2}, confirm_probes: {}, probe_ms: {}, preroll_ms: {}, post_roll_ms: {}, live: {}, vad_gpu: {})",
            config.whisper_model,
            config.vad_model,
            config.language,
            config.silence_ms,
            config.vad_start_threshold,
            config.vad_end_threshold,
            config.vad_confirm_probes.max(1),
            config.probe_ms,
            config.preroll_ms,
            config.post_roll_ms,
            config.live.enabled,
            config.vad_gpu.map(|i| i.to_string()).unwrap_or_else(|| "cpu".into()),
        );

        Ok(Self { pipeline })
    }
}

#[async_trait]
impl SttProvider for WhisperSttProvider {
    fn provider_name(&self) -> &'static str {
        "whisper"
    }

    async fn process_audio(&mut self, audio: &[f32], tx: &mpsc::Sender<SpeechEvent>) -> Result<()> {
        self.pipeline.process_audio(audio, tx).await
    }

    fn transcribe_complete(&self, audio: &[f32]) -> Result<TranscriptionQuality> {
        self.pipeline.transcribe_complete(audio)
    }

    fn inflight_transcriptions(&self) -> usize {
        self.pipeline.inflight_transcriptions()
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

    /// `Default` must mirror production: the same silence/probe/pre-roll/
    /// post-roll defaults that `Config` resolves when the env vars are unset
    /// (and the contract's tuning table). A drift here made the e2e config
    /// silently differ from production for years (100 ms probe vs 200).
    #[test]
    fn default_matches_production_defaults() {
        let d = WhisperSTTVADConfig::default();
        assert_eq!(d.silence_ms, 300);
        assert_eq!(d.probe_ms, DEFAULT_PROBE_MS);
        assert_eq!(d.preroll_ms, DEFAULT_PREROLL_MS);
        assert_eq!(d.post_roll_ms, 200);
        assert_eq!(d.vad_confirm_probes, 2);
        assert_eq!(d.vad_start_threshold, 0.65);
        assert_eq!(d.vad_end_threshold, 0.45);
    }

    /// `segmenter_config` must agree with the uniform defaults (P6): the
    /// shared machine receives the same probe / pre-roll / post-roll values
    /// the config advertises.
    #[test]
    fn segmenter_config_is_uniform() {
        let base = WhisperSTTVADConfig::default();
        let cfg = segmenter_config(&base);
        assert_eq!(cfg.probe_ms, DEFAULT_PROBE_MS);
        assert_eq!(cfg.preroll_ms, DEFAULT_PREROLL_MS);
        assert_eq!(cfg.post_roll_ms, base.post_roll_ms);
        assert_eq!(cfg.silence_ms, base.silence_ms);
        assert_eq!(cfg.vad_start_threshold, base.vad_start_threshold);
        assert_eq!(cfg.vad_end_threshold, base.vad_end_threshold);
        assert_eq!(cfg.vad_confirm_probes, base.vad_confirm_probes);
        assert!(!cfg.trim_leading_silence, "trimming would eat the pre-roll");
    }
}
