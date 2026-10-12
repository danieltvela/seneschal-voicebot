use anyhow::{Context, Result, anyhow, ensure};
use async_trait::async_trait;
use parakeet_rs::{ParakeetTDT, Transcriber};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use whisper_cpp_plus::WhisperVadProcessor;
use whisper_cpp_plus::enhanced::fallback::calculate_compression_ratio;

use super::SpeechEvent;
use super::no_speech_gate::TranscriptionQuality;
use super::provider::SttProvider;
use super::whisper::WhisperSTTVADConfig;

const SAMPLE_RATE: usize = 16_000;
const VAD_PROBE_MS: usize = 200;
const VAD_PROBE_SAMPLES: usize = SAMPLE_RATE * VAD_PROBE_MS / 1000;
const PRE_ROLL_MS: usize = 300;
const PRE_ROLL_SAMPLES: usize = SAMPLE_RATE * PRE_ROLL_MS / 1000;
/// Silence prepended to the first speech segment so the STT model has
/// acoustic context before the onset.
const PRE_SILENCE_MS: usize = 200;
const PRE_SILENCE_SAMPLES: usize = SAMPLE_RATE * PRE_SILENCE_MS / 1000;
const POST_ROLL_MS: usize = 250;
const POST_ROLL_SAMPLES: usize = SAMPLE_RATE * POST_ROLL_MS / 1000;
const MAX_SEGMENT_MS: usize = 20_000;
const MAX_SEGMENT_SAMPLES: usize = SAMPLE_RATE * MAX_SEGMENT_MS / 1000;
/// Maximum probes to accumulate before giving up on confirmation.
const MAX_ACCUM_PROBES: usize = 50;

const TRIM_WINDOW_MS: usize = 20;
const TRIM_WINDOW_SAMPLES: usize = SAMPLE_RATE * TRIM_WINDOW_MS / 1000;
const MAX_TRIM_PERCENT: usize = 90;

pub struct ParakeetSttProvider {
    model: Arc<Mutex<ParakeetTDT>>,
    vad: WhisperVadProcessor,
    vad_start_threshold: f32,
    vad_end_threshold: f32,

    // State machine
    in_speech: bool,
    in_post_roll: bool,
    speech_buf: Vec<f32>,
    pre_roll: VecDeque<f32>,
    silence_samples: usize,
    silence_samples_threshold: usize,
    post_roll_remaining: usize,

    // Confirmation window: require N consecutive speech probes before committing.
    vad_confirm_probes: usize,
    consecutive_speech_probes: usize,
    accumulating: bool,
    accum_buf: Vec<f32>,
    accum_probes_total: usize,

    // Leftover samples that didn't fill a probe window.
    probe_carry: Vec<f32>,
}

impl ParakeetSttProvider {
    pub fn new(base: WhisperSTTVADConfig, parakeet_model_dir: Option<&str>) -> Result<Self> {
        ensure!(
            (0.0..=1.0).contains(&base.vad_start_threshold),
            "vad_start_threshold must be in [0.0, 1.0], got {}",
            base.vad_start_threshold
        );
        ensure!(
            (0.0..=1.0).contains(&base.vad_end_threshold),
            "vad_end_threshold must be in [0.0, 1.0], got {}",
            base.vad_end_threshold
        );

        let vad_end_threshold = if base.vad_end_threshold > base.vad_start_threshold {
            tracing::warn!(
                target: "sttvad",
                "VAD end threshold ({}) is higher than start threshold ({}). Clamping end to start.",
                base.vad_end_threshold,
                base.vad_start_threshold
            );
            base.vad_start_threshold
        } else {
            base.vad_end_threshold
        };

        let model_dir = parakeet_model_dir
            .ok_or_else(|| anyhow!("PARAKEET_MODEL_DIR must be set when STT_PROVIDER=parakeet"))?;
        let model = ParakeetTDT::from_pretrained(model_dir, None)
            .with_context(|| format!("Failed to load Parakeet TDT model from: {}\n\nThe model directory must contain ONNX Runtime files (encoder-model.onnx, decoder_joint-model.onnx, vocab.txt).\nYou may have downloaded the wrong format — parakeet-rs requires the ONNX export, not the HuggingFace Transformers model (.safetensors) or NeMo checkpoint (.nemo).\n\nSolution: download the correct ONNX model from HuggingFace:\n  mkdir -p {}\n  cd {}\n  wget https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/encoder-model.onnx\n  wget https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/decoder_joint-model.onnx\n  wget https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/vocab.txt\n  # Optional external data (if present):\n  wget https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/encoder-model.onnx.data\n", model_dir, model_dir, model_dir))?;

        let vad = WhisperVadProcessor::new(&base.vad_model).context("Failed to load VAD model")?;

        let silence_samples_threshold = SAMPLE_RATE * base.silence_ms as usize / 1000;
        let vad_confirm_probes = base.vad_confirm_probes.max(1);

        tracing::info!(
            target: "sttvad",
            "ParakeetSttProvider ready (parakeet: {}, vad: {}, lang: {}, silence_ms: {}, start_thr: {:.2}, end_thr: {:.2}, confirm_probes: {})",
            model_dir,
            base.vad_model,
            base.language,
            base.silence_ms,
            base.vad_start_threshold,
            vad_end_threshold,
            vad_confirm_probes
        );

        Ok(Self {
            model: Arc::new(Mutex::new(model)),
            vad,
            vad_start_threshold: base.vad_start_threshold,
            vad_end_threshold,
            in_speech: false,
            in_post_roll: false,
            speech_buf: Vec::with_capacity(MAX_SEGMENT_SAMPLES),
            pre_roll: VecDeque::with_capacity(PRE_ROLL_SAMPLES),
            silence_samples: 0,
            silence_samples_threshold,
            post_roll_remaining: 0,
            vad_confirm_probes,
            consecutive_speech_probes: 0,
            accumulating: false,
            accum_buf: Vec::new(),
            accum_probes_total: 0,
            probe_carry: Vec::with_capacity(VAD_PROBE_SAMPLES),
        })
    }

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
            // ── Already confirmed speech ──
            if self.in_post_roll {
                // ── Post-roll grace period ──
                self.speech_buf.extend_from_slice(chunk);
                let threshold = self.vad_end_threshold;
                let silence = avg_prob < threshold;
                if silence {
                    self.post_roll_remaining = self.post_roll_remaining.saturating_sub(chunk.len());
                    if self.post_roll_remaining == 0 {
                        self.finalize_segment(tx).await?;
                    }
                } else {
                    self.in_post_roll = false;
                    self.post_roll_remaining = 0;
                    self.silence_samples = 0;
                }
            } else {
                // ── Confirmed speech, accumulating ──
                self.speech_buf.extend_from_slice(chunk);
                let threshold = self.vad_end_threshold;
                let silence = avg_prob < threshold;

                if silence {
                    self.silence_samples += chunk.len();
                } else {
                    self.silence_samples = 0;
                }

                let max_segment_reached = self.speech_buf.len() >= MAX_SEGMENT_SAMPLES;
                let silence_timeout = self.silence_samples >= self.silence_samples_threshold;

                if max_segment_reached {
                    self.finalize_segment(tx).await?;
                } else if silence_timeout {
                    self.in_post_roll = true;
                    self.post_roll_remaining = POST_ROLL_SAMPLES;
                }
            }
        } else if self.accumulating {
            // ── Accumulating: waiting for confirmation ──
            // Always append to accum_buf so the audio is continuous from the
            // first probe to confirmation, regardless of intermediate silence.
            self.accum_buf.extend_from_slice(chunk);
            self.accum_probes_total += 1;

            // Guard against unbounded accumulation.
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
                    // Prepend silence so the STT model has acoustic context.
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

    async fn finalize_segment(&mut self, tx: &mpsc::Sender<SpeechEvent>) -> Result<()> {
        let audio = std::mem::take(&mut self.speech_buf);
        self.in_speech = false;
        self.in_post_roll = false;
        self.silence_samples = 0;
        self.post_roll_remaining = 0;

        let trimmed = trim_leading_silence(&audio);
        if trimmed.is_empty() {
            return Ok(());
        }

        tracing::debug!(
            target: "sttvad",
            "Finalizing segment: {:.2}s (trimmed from {:.2}s)",
            trimmed.len() as f32 / SAMPLE_RATE as f32,
            audio.len() as f32 / SAMPLE_RATE as f32
        );

        let model = Arc::clone(&self.model);
        let quality = tokio::task::spawn_blocking(move || -> Result<TranscriptionQuality> {
            transcribe(&model, &trimmed)
        })
        .await
        .context("transcription task join")??;

        tracing::info!(target: "sttvad", "SpeechEnd: {}", quality.text);
        let _ = tx.send(SpeechEvent::SpeechEnd(quality)).await;
        Ok(())
    }

    pub fn transcribe_complete(&self, audio: &[f32]) -> Result<TranscriptionQuality> {
        transcribe(&self.model, audio)
    }
}

#[async_trait]
impl SttProvider for ParakeetSttProvider {
    fn provider_name(&self) -> &'static str {
        "parakeet"
    }

    async fn process_audio(&mut self, audio: &[f32], tx: &mpsc::Sender<SpeechEvent>) -> Result<()> {
        ParakeetSttProvider::process_audio(self, audio, tx).await
    }

    fn transcribe_complete(&self, audio: &[f32]) -> Result<TranscriptionQuality> {
        ParakeetSttProvider::transcribe_complete(self, audio)
    }
}

fn transcribe(model: &Arc<Mutex<ParakeetTDT>>, audio: &[f32]) -> Result<TranscriptionQuality> {
    if audio.is_empty() {
        return Ok(TranscriptionQuality {
            text: String::new(),
            no_speech_prob: 0.0,
            avg_logprob: 0.0,
            compression_ratio: 0.0,
        });
    }

    let mut model = model
        .lock()
        .map_err(|_| anyhow!("Parakeet model lock poisoned"))?;

    let result = model
        .transcribe_samples(audio.to_vec(), SAMPLE_RATE as u32, 1, None)
        .context("Parakeet transcription failed")?;

    let text = result.text.trim().to_string();

    let compression_ratio = if !text.is_empty() {
        calculate_compression_ratio(&text)
    } else {
        0.0
    };

    Ok(TranscriptionQuality {
        text,
        no_speech_prob: 0.0,
        avg_logprob: 0.0,
        compression_ratio,
    })
}

fn trim_leading_silence(audio: &[f32]) -> Vec<f32> {
    if audio.len() < TRIM_WINDOW_SAMPLES {
        return audio.to_vec();
    }

    let rms_values: Vec<f32> = audio
        .chunks(TRIM_WINDOW_SAMPLES)
        .map(|window| {
            let sum_sq: f32 = window.iter().map(|&s| s * s).sum();
            (sum_sq / window.len() as f32).sqrt()
        })
        .collect();

    let max_rms = rms_values.iter().copied().fold(0.0f32, f32::max);
    if max_rms <= 0.0 {
        return audio.to_vec();
    }

    let threshold = (max_rms * 0.05).max(0.001);
    let max_trim_windows = (audio.len() * MAX_TRIM_PERCENT / 100) / TRIM_WINDOW_SAMPLES;

    let mut trim_windows = 0;
    for (i, &rms) in rms_values.iter().enumerate() {
        if i >= max_trim_windows {
            break;
        }
        if rms >= threshold {
            break;
        }
        trim_windows += 1;
    }

    let trim_samples = trim_windows * TRIM_WINDOW_SAMPLES;
    audio[trim_samples..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silence(samples: usize) -> Vec<f32> {
        vec![0.0f32; samples]
    }

    fn sine_wave(freq_hz: f32, sample_rate: usize, num_samples: usize) -> Vec<f32> {
        (0..num_samples)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                (2.0 * std::f32::consts::PI * freq_hz * t).sin()
            })
            .collect()
    }

    #[test]
    fn trim_leading_silence_removes_quiet_prefix() {
        let quiet = silence(TRIM_WINDOW_SAMPLES * 3);
        let speech_len = TRIM_WINDOW_SAMPLES * 5;
        let speech = sine_wave(440.0, SAMPLE_RATE, speech_len);
        let audio = [quiet, speech].concat();

        let trimmed = trim_leading_silence(&audio);
        assert!(
            trimmed.len() <= audio.len() - TRIM_WINDOW_SAMPLES * 3 + TRIM_WINDOW_SAMPLES,
            "expected most of the leading silence to be trimmed"
        );
        assert!(trimmed.len() >= speech_len, "speech portion should remain");
    }

    #[test]
    fn trim_leading_silence_keeps_short_audio() {
        let audio = sine_wave(440.0, SAMPLE_RATE, TRIM_WINDOW_SAMPLES / 2);
        let trimmed = trim_leading_silence(&audio);
        assert_eq!(trimmed, audio);
    }

    #[test]
    fn trim_leading_silence_does_not_remove_all() {
        let audio = silence(TRIM_WINDOW_SAMPLES * 100);
        let trimmed = trim_leading_silence(&audio);
        assert!(
            !trimmed.is_empty(),
            "should keep at least 10% of pure silence"
        );
        assert!(
            trimmed.len() >= audio.len() / 10,
            "should respect MAX_TRIM_PERCENT"
        );
    }

    #[test]
    fn trim_leading_silence_no_trim_when_all_speech() {
        let audio = sine_wave(440.0, SAMPLE_RATE, TRIM_WINDOW_SAMPLES * 10);
        let trimmed = trim_leading_silence(&audio);
        assert_eq!(trimmed.len(), audio.len());
    }
}
