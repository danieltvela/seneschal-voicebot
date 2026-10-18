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
use super::streaming::{LiveDecoder, LiveStreamConfig, LiveStreamer};

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
    fn default() -> Self {
        Self {
            whisper_model: "models/ggml-large-v3-turbo.bin".to_string(),
            vad_model: "models/ggml-silero-vad.bin".to_string(),
            language: "es".to_string(),
            silence_ms: 500,
            vad_start_threshold: 0.65,
            vad_end_threshold: 0.45,
            vad_confirm_probes: 2,
            probe_ms: 100,
            preroll_ms: 300,
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
    SegmenterConfig {
        silence_ms: base.silence_ms,
        vad_start_threshold: base.vad_start_threshold,
        vad_end_threshold: base.vad_end_threshold,
        vad_confirm_probes: base.vad_confirm_probes,
        post_roll_ms: base.post_roll_ms,
        trim_leading_silence: false,
        probe_ms: base.probe_ms,
        preroll_ms: base.preroll_ms,
    }
}

/// Blocking Whisper decoder over one snapshot of the open segment (live).
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
    ctx: Arc<WhisperContext>,
    language: String,
    segmenter: VadSegmenter<SileroDetector>,
    streamer: LiveStreamer,
}

impl WhisperSttProvider {
    pub fn new(config: WhisperSTTVADConfig) -> Result<Self> {
        let ctx = Arc::new(
            WhisperContext::new(&config.whisper_model).context("Failed to load Whisper model")?,
        );

        let vad = load_vad(&config.vad_model, config.vad_gpu)?;
        let segmenter = VadSegmenter::new(SileroDetector::new(vad), segmenter_config(&config))?;
        let streamer = LiveStreamer::new(config.live.clone());

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

        Ok(Self {
            ctx,
            language: config.language,
            segmenter,
            streamer,
        })
    }

    /// Feed a chunk of 16 kHz mono f32 audio. Emits events as the VAD/state
    /// machine advances. Live partial decodes run while the segment is open
    /// (contract P3) and the close completes via a detached task (contract
    /// P4) — decoding never blocks the VAD feed.
    pub async fn process_audio(
        &mut self,
        audio: &[f32],
        tx: &mpsc::Sender<SpeechEvent>,
    ) -> Result<()> {
        self.streamer.poll(tx).await;

        for event in self.segmenter.feed(audio) {
            match event {
                SegmenterEvent::SpeechStart { segment_seq } => {
                    self.streamer.on_segment_start(segment_seq);
                    let _ = tx.send(SpeechEvent::SpeechStart { segment_seq }).await;
                }
                SegmenterEvent::SpeechAborted { segment_seq } => {
                    // The onset (barge-in already fired) never confirmed: emit
                    // an empty end so the pipeline drops it and leaves
                    // LISTENING. NoSpeechGate rejects the empty transcription.
                    let _ = tx
                        .send(SpeechEvent::SpeechEnd {
                            segment_seq,
                            quality: TranscriptionQuality::empty(),
                        })
                        .await;
                }
                SegmenterEvent::SegmentReady {
                    segment_seq,
                    audio,
                    trailing_silence,
                } => {
                    tracing::debug!(
                        target: "sttvad",
                        "Finalizing segment {segment_seq}: {:.2}s",
                        audio.len() as f32 / SAMPLE_RATE as f32
                    );

                    let speech_end_offset = audio.len().saturating_sub(trailing_silence);
                    let decoder = WhisperDecoder {
                        ctx: Arc::clone(&self.ctx),
                        language: self.language.clone(),
                    };
                    self.streamer.finish_detached(
                        segment_seq,
                        audio,
                        speech_end_offset,
                        decoder,
                        tx.clone(),
                    );
                }
            }
        }

        // Live cadence while the segment stays open (decodes run detached on
        // the blocking pool). The one-shot closing trigger fires the
        // last-chance decode when the post-roll grace opens.
        let seq = self.segmenter.current_segment_seq();
        self.streamer.maybe_spawn(
            seq,
            self.segmenter.in_speech(),
            self.segmenter.take_closing_trigger(),
            self.segmenter.speech_buf(),
            WhisperDecoder {
                ctx: Arc::clone(&self.ctx),
                language: self.language.clone(),
            },
        );

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

    fn inflight_transcriptions(&self) -> usize {
        self.streamer.inflight()
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
