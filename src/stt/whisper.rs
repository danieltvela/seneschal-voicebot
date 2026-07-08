use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::mpsc;
use whisper_cpp_plus::enhanced::fallback::calculate_compression_ratio;
use whisper_cpp_plus::{FullParams, SamplingStrategy, WhisperContext, WhisperVadProcessor};

use super::SpeechEvent;
use super::no_speech_gate::TranscriptionQuality;
use super::provider::SttProvider;

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
        }
    }
}

const SAMPLE_RATE: usize = 16_000;
/// Probe size for the VAD. Silero prefers 20–200 ms windows; 200 ms gives good
/// accuracy without adding too much latency.
const VAD_PROBE_MS: usize = 200;
const VAD_PROBE_SAMPLES: usize = SAMPLE_RATE * VAD_PROBE_MS / 1000;
/// Audio retained before the VAD onset so the first phoneme isn't clipped.
const PRE_ROLL_MS: usize = 300;
const PRE_ROLL_SAMPLES: usize = SAMPLE_RATE * PRE_ROLL_MS / 1000;
/// Silence prepended to the first speech segment so Whisper has acoustic
/// context before the onset. Without this, the first word is often
/// mistranscribed because the model sees an abrupt audio start.
const PRE_SILENCE_MS: usize = 200;
const PRE_SILENCE_SAMPLES: usize = SAMPLE_RATE * PRE_SILENCE_MS / 1000;
/// Hard cap on a single speech segment before forcing a cut.
const MAX_SEGMENT_MS: usize = 20_000;
const MAX_SEGMENT_SAMPLES: usize = SAMPLE_RATE * MAX_SEGMENT_MS / 1000;
/// Maximum probes to accumulate before giving up on confirmation.
/// At 200ms per probe, 50 probes = 10 seconds. Prevents unbounded growth
/// when the user says something very short and the VAD never confirms.
const MAX_ACCUM_PROBES: usize = 50;

pub struct WhisperSttProvider {
    ctx: Arc<WhisperContext>,
    vad: WhisperVadProcessor,
    language: String,
    vad_start_threshold: f32,
    vad_end_threshold: f32,

    // State machine
    in_speech: bool,
    speech_buf: Vec<f32>,
    pre_roll: VecDeque<f32>,
    silence_samples: usize,
    silence_samples_threshold: usize,

    // Confirmation window: require N consecutive speech probes before committing.
    vad_confirm_probes: usize,
    consecutive_speech_probes: usize,
    accumulating: bool,
    accum_buf: Vec<f32>,
    /// Total probes processed during accumulation (for max-duration guard).
    accum_probes_total: usize,

    // Leftover samples that didn't fill a probe window.
    probe_carry: Vec<f32>,
}

impl WhisperSttProvider {
    pub fn new(config: WhisperSTTVADConfig) -> Result<Self> {
        ensure!(
            (0.0..=1.0).contains(&config.vad_start_threshold),
            "vad_start_threshold must be in [0.0, 1.0], got {}",
            config.vad_start_threshold
        );
        ensure!(
            (0.0..=1.0).contains(&config.vad_end_threshold),
            "vad_end_threshold must be in [0.0, 1.0], got {}",
            config.vad_end_threshold
        );

        let vad_end_threshold = if config.vad_end_threshold > config.vad_start_threshold {
            tracing::warn!(
                target: "sttvad",
                "VAD end threshold ({}) is higher than start threshold ({}). Clamping end to start.",
                config.vad_end_threshold,
                config.vad_start_threshold
            );
            config.vad_start_threshold
        } else {
            config.vad_end_threshold
        };

        let ctx = Arc::new(
            WhisperContext::new(&config.whisper_model).context("Failed to load Whisper model")?,
        );

        let vad =
            WhisperVadProcessor::new(&config.vad_model).context("Failed to load VAD model")?;

        let silence_samples_threshold = SAMPLE_RATE * config.silence_ms as usize / 1000;
        let vad_confirm_probes = config.vad_confirm_probes.max(1);

        tracing::info!(
            target: "sttvad",
            "WhisperSTTVAD ready (whisper: {}, vad: {}, lang: {}, silence_ms: {}, start_thr: {:.2}, end_thr: {:.2}, confirm_probes: {})",
            config.whisper_model,
            config.vad_model,
            config.language,
            config.silence_ms,
            config.vad_start_threshold,
            vad_end_threshold,
            vad_confirm_probes
        );

        Ok(Self {
            ctx,
            vad,
            language: config.language,
            vad_start_threshold: config.vad_start_threshold,
            vad_end_threshold,
            in_speech: false,
            speech_buf: Vec::with_capacity(MAX_SEGMENT_SAMPLES),
            pre_roll: VecDeque::with_capacity(PRE_ROLL_SAMPLES),
            silence_samples: 0,
            silence_samples_threshold,
            vad_confirm_probes,
            consecutive_speech_probes: 0,
            accumulating: false,
            accum_buf: Vec::new(),
            accum_probes_total: 0,
            probe_carry: Vec::with_capacity(VAD_PROBE_SAMPLES),
        })
    }

    /// Feed a chunk of 16 kHz mono f32 audio. Emits events as the VAD/state
    /// machine advances. Transcription happens synchronously on the caller
    /// thread (blocking); it's acceptable for a single-user interactive loop.
    pub async fn process_audio(
        &mut self,
        audio: &[f32],
        tx: &mpsc::Sender<SpeechEvent>,
    ) -> Result<()> {
        if audio.is_empty() {
            return Ok(());
        }

        self.probe_carry.extend_from_slice(audio);

        while self.probe_carry.len() >= VAD_PROBE_SAMPLES {
            let chunk: Vec<f32> = self.probe_carry.drain(..VAD_PROBE_SAMPLES).collect();
            self.process_probe(&chunk, tx).await?;
        }

        Ok(())
    }

    async fn process_probe(&mut self, chunk: &[f32], tx: &mpsc::Sender<SpeechEvent>) -> Result<()> {
        let has_speech = self.vad.detect_speech(chunk);
        let avg_prob = if !has_speech {
            0.0
        } else {
            let probs = self.vad.get_probs();
            if probs.is_empty() {
                0.0
            } else {
                probs.iter().sum::<f32>() / probs.len() as f32
            }
        };

        // Update pre-roll buffer first (always) so it has the latest audio.
        for &s in chunk {
            if self.pre_roll.len() >= PRE_ROLL_SAMPLES {
                self.pre_roll.pop_front();
            }
            self.pre_roll.push_back(s);
        }

        if self.in_speech {
            // ── Already confirmed speech: accumulate and check for end ──
            self.speech_buf.extend_from_slice(chunk);

            let threshold = self.vad_end_threshold;
            let silence = avg_prob < threshold;

            if silence {
                self.silence_samples += chunk.len();
            } else {
                self.silence_samples = 0;
            }

            let should_finalize = self.silence_samples >= self.silence_samples_threshold
                || self.speech_buf.len() >= MAX_SEGMENT_SAMPLES;

            if should_finalize {
                let audio = std::mem::take(&mut self.speech_buf);
                self.in_speech = false;
                self.silence_samples = 0;

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
        } else if self.accumulating {
            // ── Accumulating: waiting for confirmation ──
            // Always append to accum_buf so the audio is continuous from the
            // first probe to confirmation, regardless of intermediate silence.
            self.accum_buf.extend_from_slice(chunk);
            self.accum_probes_total += 1;

            // Guard against unbounded accumulation (e.g. very short utterance
            // that never reaches the confirmation threshold).
            if self.accum_probes_total >= MAX_ACCUM_PROBES {
                tracing::debug!(
                    target: "sttvad",
                    "Accumulation timeout after {} probes, discarding",
                    self.accum_probes_total
                );
                self.accumulating = false;
                self.consecutive_speech_probes = 0;
                self.accum_probes_total = 0;
                self.accum_buf.clear();
            }

            let threshold = self.vad_start_threshold;
            let is_speech = avg_prob >= threshold;

            if is_speech {
                self.consecutive_speech_probes += 1;

                if self.consecutive_speech_probes >= self.vad_confirm_probes {
                    // CONFIRMED: transition to speech state.
                    // Prepend silence so Whisper has acoustic context before
                    // the speech onset. Without this, the first word is often
                    // mistranscribed (abrupt audio start).
                    self.in_speech = true;
                    self.accumulating = false;
                    self.speech_buf.clear();
                    self.speech_buf
                        .extend(std::iter::repeat_n(0.0f32, PRE_SILENCE_SAMPLES));
                    self.speech_buf.append(&mut self.accum_buf);
                    let _ = tx.send(SpeechEvent::SpeechStart).await;
                    tracing::debug!(
                        target: "sttvad",
                        "Speech confirmed after {} probes ({:.1}s, +{}ms pre-silence)",
                        self.consecutive_speech_probes,
                        self.speech_buf.len() as f32 / SAMPLE_RATE as f32,
                        PRE_SILENCE_MS
                    );
                }
            } else {
                // Silence during accumulation — reset consecutive counter but
                // keep accumulating audio (it's part of the continuous stream).
                if self.consecutive_speech_probes > 0 {
                    tracing::debug!(
                        target: "sttvad",
                        "Accumulation probe reset: {} consecutive, needed {} (avg_prob={:.3})",
                        self.consecutive_speech_probes,
                        self.vad_confirm_probes,
                        avg_prob
                    );
                }
                self.consecutive_speech_probes = 0;
            }
        } else {
            // ── Silence: start accumulating if speech detected ──
            let threshold = self.vad_start_threshold;
            let is_speech = avg_prob >= threshold;

            if is_speech {
                self.accumulating = true;
                self.consecutive_speech_probes = 1;
                self.accum_probes_total = 1;
                self.accum_buf.clear();
                self.accum_buf.extend_from_slice(chunk);
                tracing::debug!(
                    target: "sttvad",
                    "Start accumulating (probe 1/{}), avg_prob={:.3}",
                    self.vad_confirm_probes,
                    avg_prob
                );
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
        .language(language)
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
