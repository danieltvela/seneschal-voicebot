use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, info, warn};
use whisper_cpp_plus::WhisperVadProcessor;
use whisper_cpp_plus::enhanced::fallback::calculate_compression_ratio;

use speech::prelude::*;

use super::SpeechEvent;
use super::no_speech_gate::TranscriptionQuality;
use super::provider::SttProvider;
use super::whisper::WhisperSTTVADConfig;

const SAMPLE_RATE: f64 = 16_000.0;
const CHANNELS: usize = 1;
const SR_USIZE: usize = 16_000;
const VAD_PROBE_MS: usize = 100;
const VAD_PROBE_SAMPLES: usize = SR_USIZE * VAD_PROBE_MS / 1000;
const PRE_ROLL_MS: usize = 300;
const PRE_ROLL_SAMPLES: usize = SR_USIZE * PRE_ROLL_MS / 1000;
const POST_ROLL_MS: usize = 500;
const POST_ROLL_SAMPLES: usize = SR_USIZE * POST_ROLL_MS / 1000;
const MAX_ACCUM_PROBES: usize = 50;

/// Shared mutable state for the recognition task and event buffer.
struct SharedState {
    task: Option<AudioBufferRecognitionTask>,
    task_done: bool,
    awaiting_finalize: bool,
    speech_start_sent: bool,
    event_buffer: Vec<RecognitionTaskEvent>,
}

/// Action returned by VAD probe processing.
enum VadAction {
    /// Silence or accumulating — feed nothing to Apple.
    None,
    /// First speech-like probe — emit SpeechStart immediately (fast barge-in).
    AccumStarted,
    /// Speech confirmed — create task and feed the accumulated buffer.
    Start(Vec<f32>),
    /// Short unconfirmed utterance — create task, feed buffer, end_audio immediately.
    StartShort(Vec<f32>),
    /// In speech — feed this chunk to Apple.
    Feed(Vec<f32>),
    /// Silence timeout — call end_audio() on the task.
    End,
    /// Accumulation timed out without confirmation — emit empty SpeechEnd to reset state.
    Discard,
}

/// STT provider using macOS SFSpeechRecognizer via the `speech` crate.
///
/// Hybrid architecture: Silero VAD gates audio feeding and drives endpointing.
/// On silence timeout, `end_audio()` forces Apple to finalize immediately (~200ms),
/// avoiding Apple's slow internal endpointer (2-4s).
pub struct SpeechRecognizerSttProvider {
    recognizer: SpeechRecognizer,
    state: Arc<Mutex<SharedState>>,
    event_tx: mpsc::UnboundedSender<RecognitionTaskEvent>,
    #[allow(dead_code)]
    drain_handle: Option<tokio::task::JoinHandle<()>>,

    // ── Silero VAD ──
    vad: WhisperVadProcessor,
    vad_start_threshold: f32,
    vad_end_threshold: f32,

    // ── VAD state machine ──
    in_speech: bool,
    in_post_roll: bool,
    pre_roll: VecDeque<f32>,
    silence_samples: usize,
    silence_samples_threshold: usize,
    post_roll_remaining: usize,

    vad_confirm_probes: usize,
    consecutive_speech_probes: usize,
    accumulating: bool,
    accum_buf: Vec<f32>,
    accum_probes_total: usize,
    /// Consecutive silence probes while accumulating (short-utterance fallback).
    accum_silence_probes: usize,
    probe_carry: Vec<f32>,
}

impl SpeechRecognizerSttProvider {
    pub fn new(base: WhisperSTTVADConfig) -> Result<Self> {
        let locale = match base.language.as_str() {
            "en" => "en-US",
            "es" => "es-ES",
            other => other,
        };

        let recognizer = SpeechRecognizer::with_locale_checked(locale)
            .with_context(|| format!("Failed to create SpeechRecognizer for locale '{locale}'"))?;

        let vad =
            WhisperVadProcessor::new(&base.vad_model).context("Failed to load Silero VAD model")?;

        let vad_end_threshold = if base.vad_end_threshold > base.vad_start_threshold {
            warn!(
                target: "stt",
                "VAD end ({}) > start ({}). Clamping.",
                base.vad_end_threshold, base.vad_start_threshold
            );
            base.vad_start_threshold
        } else {
            base.vad_end_threshold
        };

        let silence_samples_threshold = SR_USIZE * base.silence_ms as usize / 1000;
        let vad_confirm_probes = base.vad_confirm_probes.max(1);

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let state = Arc::new(Mutex::new(SharedState {
            task: None,
            task_done: false,
            awaiting_finalize: false,
            speech_start_sent: false,
            event_buffer: Vec::new(),
        }));

        let state_clone = Arc::clone(&state);
        let drain_handle = tokio::spawn(async move {
            let mut rx = event_rx;
            while let Some(event) = rx.recv().await {
                let mut st = state_clone.lock().await;
                match &event {
                    RecognitionTaskEvent::DidFinishSuccessfully(true) => {
                        st.task_done = true;
                        st.awaiting_finalize = false;
                        st.speech_start_sent = false;
                    }
                    RecognitionTaskEvent::WasCancelled => {
                        st.task_done = true;
                        st.awaiting_finalize = false;
                        st.speech_start_sent = false;
                    }
                    _ => {}
                }
                st.event_buffer.push(event);
            }
        });

        info!(
            target: "stt",
            "SpeechRecognizerSttProvider ready (locale: {}, vad: {}, silence_ms: {}, start: {:.2}, end: {:.2}, confirm: {})",
            locale, base.vad_model, base.silence_ms,
            base.vad_start_threshold, vad_end_threshold, vad_confirm_probes
        );

        Ok(Self {
            recognizer,
            state,
            event_tx,
            drain_handle: Some(drain_handle),
            vad,
            vad_start_threshold: base.vad_start_threshold,
            vad_end_threshold,
            in_speech: false,
            in_post_roll: false,
            pre_roll: VecDeque::with_capacity(PRE_ROLL_SAMPLES),
            silence_samples: 0,
            silence_samples_threshold,
            post_roll_remaining: 0,
            vad_confirm_probes,
            consecutive_speech_probes: 0,
            accumulating: false,
            accum_buf: Vec::new(),
            accum_probes_total: 0,
            accum_silence_probes: 0,
            probe_carry: Vec::with_capacity(VAD_PROBE_SAMPLES),
        })
    }

    /// Process a single VAD probe (100ms chunk). Returns the action to take.
    fn process_probe(&mut self, chunk: &[f32]) -> VadAction {
        // Update pre-roll
        for &s in chunk {
            if self.pre_roll.len() >= PRE_ROLL_SAMPLES {
                self.pre_roll.pop_front();
            }
            self.pre_roll.push_back(s);
        }

        let has_speech = self.vad.detect_speech(chunk);
        let avg_prob = if has_speech && !self.vad.get_probs().is_empty() {
            self.vad.get_probs().iter().sum::<f32>() / self.vad.get_probs().len() as f32
        } else {
            0.0
        };

        if self.in_speech {
            if self.in_post_roll {
                let silence = avg_prob < self.vad_end_threshold;
                if silence {
                    self.post_roll_remaining = self.post_roll_remaining.saturating_sub(chunk.len());
                    if self.post_roll_remaining == 0 {
                        // Feed this last chunk, then end
                        return VadAction::End;
                    }
                } else {
                    self.in_post_roll = false;
                    self.post_roll_remaining = 0;
                    self.silence_samples = 0;
                }
            } else {
                let silence = avg_prob < self.vad_end_threshold;
                if silence {
                    self.silence_samples += chunk.len();
                } else {
                    self.silence_samples = 0;
                }
                if self.silence_samples >= self.silence_samples_threshold {
                    self.in_post_roll = true;
                    self.post_roll_remaining = POST_ROLL_SAMPLES;
                }
            }
            // Feed this chunk to Apple
            VadAction::Feed(chunk.to_vec())
        } else if self.accumulating {
            self.accum_buf.extend_from_slice(chunk);
            self.accum_probes_total += 1;

            let is_speech = avg_prob >= self.vad_start_threshold;
            if is_speech {
                self.accum_silence_probes = 0;
            } else {
                self.accum_silence_probes += 1;
            }

            if self.accum_probes_total >= MAX_ACCUM_PROBES {
                debug!(target: "stt", "Accum timeout after {} probes", self.accum_probes_total);
                self.reset_accum();
                return VadAction::Discard;
            }

            if is_speech {
                self.consecutive_speech_probes += 1;
                if self.consecutive_speech_probes >= self.vad_confirm_probes {
                    let buf = std::mem::take(&mut self.accum_buf);
                    self.in_speech = true;
                    self.accumulating = false;
                    self.accum_silence_probes = 0;
                    debug!(
                        target: "stt",
                        "Speech confirmed after {} probes ({:.1}s)",
                        self.consecutive_speech_probes,
                        buf.len() as f32 / SR_USIZE as f32
                    );
                    return VadAction::Start(buf);
                }
            } else {
                if self.consecutive_speech_probes > 0 {
                    debug!(
                        target: "stt",
                        "Accum reset: {} consecutive, needed {} (prob={:.3})",
                        self.consecutive_speech_probes,
                        self.vad_confirm_probes,
                        avg_prob
                    );
                }
                self.consecutive_speech_probes = 0;

                // Short-utterance fallback: ≥1 speech probe already seen (we're in
                // accumulating), then a full silence window → finalize without confirm.
                let short_silence_probes = (self.silence_samples_threshold + VAD_PROBE_SAMPLES - 1)
                    / VAD_PROBE_SAMPLES;
                if self.accum_silence_probes >= short_silence_probes.max(1) {
                    let buf = std::mem::take(&mut self.accum_buf);
                    self.accumulating = false;
                    self.consecutive_speech_probes = 0;
                    self.accum_silence_probes = 0;
                    self.accum_probes_total = 0;
                    debug!(
                        target: "stt",
                        "Short utterance finalized ({:.1}s, silence_probes={})",
                        buf.len() as f32 / SR_USIZE as f32,
                        short_silence_probes.max(1)
                    );
                    return VadAction::StartShort(buf);
                }
            }
            VadAction::None
        } else {
            let is_speech = avg_prob >= self.vad_start_threshold;
            if is_speech {
                self.accumulating = true;
                self.consecutive_speech_probes = 1;
                self.accum_probes_total = 1;
                self.accum_silence_probes = 0;
                self.accum_buf.clear();
                self.accum_buf.extend_from_slice(chunk);
                debug!(
                    target: "stt",
                    "Start accumulating (1/{}) prob={:.3}",
                    self.vad_confirm_probes, avg_prob
                );
                return VadAction::AccumStarted;
            }
            VadAction::None
        }
    }

    fn reset_accum(&mut self) {
        self.accumulating = false;
        self.consecutive_speech_probes = 0;
        self.accum_probes_total = 0;
        self.accum_silence_probes = 0;
        self.accum_buf.clear();
    }

    fn reset_vad(&mut self) {
        self.in_speech = false;
        self.in_post_roll = false;
        self.reset_accum();
        self.silence_samples = 0;
    }

    /// Create a new recognition task.
    async fn create_task(&self) -> Result<()> {
        let mut st = self.state.lock().await;
        st.task_done = false;
        st.awaiting_finalize = false;
        // Do not reset speech_start_sent — VAD may have already emitted SpeechStart
        // on AccumStarted (fast barge-in). Resetting would cause a duplicate on the
        // first Apple partial.

        let request = AudioBufferRecognitionRequest::new().with_options(
            RecognitionRequestOptions::new()
                .with_task_hint(TaskHint::Dictation)
                .with_should_report_partial_results(true)
                .with_requires_on_device_recognition(true)
                .with_adds_punctuation(true),
        );

        let event_tx = self.event_tx.clone();
        let task = self
            .recognizer
            .start_audio_buffer_task(&request, move |event| {
                let _ = event_tx.send(event);
            })
            .context("Failed to start audio buffer recognition task")?;

        st.task = Some(task);
        Ok(())
    }

    /// Feed audio to the recognition task.
    async fn feed_audio(&self, audio: &[f32]) -> Result<()> {
        if audio.is_empty() {
            return Ok(());
        }
        let st = self.state.lock().await;
        if let Some(ref task) = st.task {
            task.append_interleaved_f32(SAMPLE_RATE, CHANNELS, audio)
                .context("Failed to append audio")?;
        }
        Ok(())
    }

    /// Signal end of audio to force Apple finalization.
    async fn signal_end_audio(&self) {
        let mut st = self.state.lock().await;
        if let Some(ref task) = st.task
            && !st.awaiting_finalize
        {
            debug!(target: "stt", "Calling end_audio()");
            task.end_audio();
            st.awaiting_finalize = true;
        }
    }

    /// Drain buffered events into the pipeline channel.
    async fn drain_events(&self, tx: &mpsc::Sender<SpeechEvent>) -> Result<()> {
        let (events, mut speech_start_sent) = {
            let mut st = self.state.lock().await;
            (std::mem::take(&mut st.event_buffer), st.speech_start_sent)
        };

        let mut got_final = false;
        for event in events {
            match event {
                RecognitionTaskEvent::DidDetectSpeech => {
                    debug!(target: "stt", "Apple detected speech");
                }
                RecognitionTaskEvent::DidHypothesizeTranscription(t) => {
                    let text = t.formatted_string;
                    if !text.is_empty() {
                        if !speech_start_sent {
                            speech_start_sent = true;
                            debug!(target: "stt", "SpeechStart (from first partial)");
                            let _ = tx.send(SpeechEvent::SpeechStart).await;
                        }
                        debug!(target: "stt", "Partial: {}", text);
                        let _ = tx.send(SpeechEvent::Speech(text)).await;
                    }
                }
                RecognitionTaskEvent::DidFinishRecognition(r) => {
                    let text = r.best_transcription.formatted_string.trim().to_string();
                    let compression_ratio = if !text.is_empty() {
                        calculate_compression_ratio(&text)
                    } else {
                        0.0
                    };

                    let quality = TranscriptionQuality {
                        text,
                        no_speech_prob: 0.0,
                        avg_logprob: 0.0,
                        compression_ratio,
                    };

                    info!(target: "stt", "SpeechEnd: {}", quality.text);
                    let _ = tx.send(SpeechEvent::SpeechEnd(quality)).await;
                    got_final = true;
                }
                RecognitionTaskEvent::DidFinishSuccessfully(success) => {
                    debug!(target: "stt", "Task finished: {}", success);
                }
                RecognitionTaskEvent::WasCancelled => {
                    debug!(target: "stt", "Task cancelled");
                }
                RecognitionTaskEvent::FinishedReadingAudio => {
                    debug!(target: "stt", "Finished reading audio");
                }
                RecognitionTaskEvent::DidProcessAudioDuration(duration) => {
                    debug!(target: "stt", "Processed: {:.2}s", duration);
                }
            }
        }

        if got_final {
            speech_start_sent = false;
        }
        let mut st = self.state.lock().await;
        st.speech_start_sent = speech_start_sent;

        Ok(())
    }
}

#[async_trait]
impl SttProvider for SpeechRecognizerSttProvider {
    fn provider_name(&self) -> &'static str {
        "speech"
    }

    async fn process_audio(&mut self, audio: &[f32], tx: &mpsc::Sender<SpeechEvent>) -> Result<()> {
        if audio.is_empty() {
            return Ok(());
        }

        // Request authorization on first call
        let status = SpeechRecognizer::authorization_status();
        if !status.is_authorized() {
            info!(target: "stt", ?status, "Requesting authorization...");
            let granted = SpeechRecognizer::request_authorization();
            if !granted.is_authorized() {
                bail!(
                    "Speech recognition not authorized (status: {:?}). Enable Microphone in System Settings.",
                    granted
                );
            }
        }

        // Process audio through VAD probe-by-probe
        self.probe_carry.extend_from_slice(audio);

        while self.probe_carry.len() >= VAD_PROBE_SAMPLES {
            let chunk: Vec<f32> = self.probe_carry.drain(..VAD_PROBE_SAMPLES).collect();
            match self.process_probe(&chunk) {
                VadAction::None => {}
                VadAction::AccumStarted => {
                    // Fast barge-in: emit SpeechStart on the first speech-like probe.
                    {
                        let mut st = self.state.lock().await;
                        st.speech_start_sent = true;
                    }
                    debug!(target: "stt", "SpeechStart (from VAD onset)");
                    let _ = tx.send(SpeechEvent::SpeechStart).await;
                }
                VadAction::Start(buf) => {
                    self.create_task().await?;
                    self.feed_audio(&buf).await?;
                }
                VadAction::StartShort(buf) => {
                    // Short unconfirmed utterance: feed Apple and finalize immediately.
                    // SpeechStart was already emitted on AccumStarted.
                    self.create_task().await?;
                    self.feed_audio(&buf).await?;
                    self.signal_end_audio().await;
                }
                VadAction::Feed(chunk) => {
                    self.feed_audio(&chunk).await?;
                }
                VadAction::End => {
                    // Feed remaining pre-roll as context before finalizing
                    let pr: Vec<f32> = self.pre_roll.iter().copied().collect();
                    if !pr.is_empty() {
                        self.feed_audio(&pr).await.ok();
                    }
                    self.signal_end_audio().await;
                    self.reset_vad();
                }
                VadAction::Discard => {
                    // Continuous non-speech that never confirmed: close the utterance
                    // so main.rs can reject empty text and return TUI to Idle.
                    {
                        let mut st = self.state.lock().await;
                        st.speech_start_sent = false;
                    }
                    let quality = TranscriptionQuality {
                        text: String::new(),
                        no_speech_prob: 0.0,
                        avg_logprob: 0.0,
                        compression_ratio: 0.0,
                    };
                    debug!(target: "stt", "SpeechEnd (accum discard, empty)");
                    let _ = tx.send(SpeechEvent::SpeechEnd(quality)).await;
                }
            }
        }

        // Drain buffered events
        self.drain_events(tx).await?;

        // Reset VAD state if task is done AND we're not currently in speech.
        // Don't reset while accumulating — the user may be starting a new utterance.
        let task_done = {
            let st = self.state.lock().await;
            st.task_done
        };
        if task_done && !self.in_speech && !self.accumulating {
            self.reset_vad();
        }

        Ok(())
    }

    fn transcribe_complete(&self, _audio: &[f32]) -> Result<TranscriptionQuality> {
        bail!("SpeechRecognizerSttProvider does not support transcribe_complete.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compression_ratio_calculation() {
        let ratio = calculate_compression_ratio("Hello, how are you today?");
        assert!(ratio > 0.0 && ratio < 2.0);

        let ratio2 = calculate_compression_ratio("aaaa bbbb cccc aaaa bbbb cccc");
        assert!(ratio2 >= ratio);
    }

    #[test]
    fn empty_text_quality() {
        let q = TranscriptionQuality {
            text: String::new(),
            no_speech_prob: 0.0,
            avg_logprob: 0.0,
            compression_ratio: 0.0,
        };
        assert!(q.text.is_empty());
    }
}
