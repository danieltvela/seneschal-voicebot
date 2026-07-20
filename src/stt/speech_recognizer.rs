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

/// Decision from pure accumulator bookkeeping (no VAD model required).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccumDecision {
    /// Reached `confirm_probes` consecutive speech probes.
    Confirmed,
    /// Silence window after unconfirmed speech → short-utterance finalize.
    ShortFinalize,
    /// Hit `MAX_ACCUM_PROBES` without confirming or short-finalizing.
    Discard,
    /// Keep accumulating.
    Continue,
}

/// Tracks consecutive speech/silence probes while waiting for VAD confirmation.
/// Pure logic — unit-testable without Silero or Apple STT.
#[derive(Debug, Clone)]
struct AccumTracker {
    confirm_probes: usize,
    short_silence_probes: usize,
    consecutive_speech_probes: usize,
    silence_probes: usize,
    probes_total: usize,
}

impl AccumTracker {
    fn new(confirm_probes: usize, silence_samples_threshold: usize) -> Self {
        Self {
            confirm_probes: confirm_probes.max(1),
            short_silence_probes: silence_samples_threshold.div_ceil(VAD_PROBE_SAMPLES).max(1),
            consecutive_speech_probes: 0,
            silence_probes: 0,
            probes_total: 0,
        }
    }

    /// Record the first speech probe that started accumulation.
    /// Does not return Confirmed (SpeechStart is emitted separately as AccumStarted).
    fn begin_speech(&mut self) {
        self.consecutive_speech_probes = 1;
        self.silence_probes = 0;
        self.probes_total = 1;
    }

    fn reset(&mut self) {
        self.consecutive_speech_probes = 0;
        self.silence_probes = 0;
        self.probes_total = 0;
    }

    /// Process one subsequent probe while accumulating.
    fn on_probe(&mut self, is_speech: bool) -> AccumDecision {
        self.probes_total += 1;

        if is_speech {
            self.silence_probes = 0;
            self.consecutive_speech_probes += 1;
        } else {
            self.consecutive_speech_probes = 0;
            self.silence_probes += 1;
        }

        if self.probes_total >= MAX_ACCUM_PROBES {
            self.reset();
            return AccumDecision::Discard;
        }

        if is_speech && self.consecutive_speech_probes >= self.confirm_probes {
            return AccumDecision::Confirmed;
        }

        if !is_speech && self.silence_probes >= self.short_silence_probes {
            self.reset();
            return AccumDecision::ShortFinalize;
        }

        AccumDecision::Continue
    }
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
    accumulating: bool,
    accum_buf: Vec<f32>,
    accum: AccumTracker,
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
            accumulating: false,
            accum_buf: Vec::new(),
            accum: AccumTracker::new(vad_confirm_probes, silence_samples_threshold),
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

            let is_speech = avg_prob >= self.vad_start_threshold;
            let prev_consec = self.accum.consecutive_speech_probes;
            let decision = self.accum.on_probe(is_speech);

            if !is_speech && prev_consec > 0 {
                debug!(
                    target: "stt",
                    "Accum reset: {} consecutive, needed {} (prob={:.3})",
                    prev_consec,
                    self.vad_confirm_probes,
                    avg_prob
                );
            }

            match decision {
                AccumDecision::Discard => {
                    debug!(
                        target: "stt",
                        "Accum timeout after {} probes",
                        MAX_ACCUM_PROBES
                    );
                    self.reset_accum();
                    VadAction::Discard
                }
                AccumDecision::Confirmed => {
                    let buf = std::mem::take(&mut self.accum_buf);
                    let confirmed_probes = self.accum.consecutive_speech_probes;
                    self.in_speech = true;
                    self.accumulating = false;
                    self.accum.reset();
                    debug!(
                        target: "stt",
                        "Speech confirmed after {} probes ({:.1}s)",
                        confirmed_probes,
                        buf.len() as f32 / SR_USIZE as f32
                    );
                    VadAction::Start(buf)
                }
                AccumDecision::ShortFinalize => {
                    let buf = std::mem::take(&mut self.accum_buf);
                    self.accumulating = false;
                    debug!(
                        target: "stt",
                        "Short utterance finalized ({:.1}s, silence_probes={})",
                        buf.len() as f32 / SR_USIZE as f32,
                        self.accum.short_silence_probes
                    );
                    VadAction::StartShort(buf)
                }
                AccumDecision::Continue => VadAction::None,
            }
        } else {
            let is_speech = avg_prob >= self.vad_start_threshold;
            if is_speech {
                self.accumulating = true;
                self.accum.begin_speech();
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
        self.accum.reset();
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

    /// silence_ms=300 → silence_samples=4800; probe=1600 → short_silence_probes=3.
    fn tracker_300ms_silence_confirm2() -> AccumTracker {
        let silence_samples = SR_USIZE * 300 / 1000;
        AccumTracker::new(2, silence_samples)
    }

    #[test]
    fn accum_two_consecutive_speech_confirms() {
        let mut t = tracker_300ms_silence_confirm2();
        t.begin_speech(); // probe 1 speech
        assert_eq!(t.on_probe(true), AccumDecision::Confirmed); // probe 2 speech
    }

    #[test]
    fn accum_one_speech_then_three_silence_short_finalize() {
        let mut t = tracker_300ms_silence_confirm2();
        t.begin_speech(); // 1 speech
        assert_eq!(t.on_probe(false), AccumDecision::Continue); // silence 1
        assert_eq!(t.on_probe(false), AccumDecision::Continue); // silence 2
        assert_eq!(t.on_probe(false), AccumDecision::ShortFinalize); // silence 3
    }

    #[test]
    fn accum_one_speech_then_two_silence_continues() {
        let mut t = tracker_300ms_silence_confirm2();
        t.begin_speech();
        assert_eq!(t.on_probe(false), AccumDecision::Continue);
        assert_eq!(t.on_probe(false), AccumDecision::Continue);
        // still need one more silence probe
        assert_eq!(t.silence_probes, 2);
        assert_eq!(t.short_silence_probes, 3);
    }

    #[test]
    fn accum_never_confirms_discards_at_max_probes() {
        let mut t = tracker_300ms_silence_confirm2();
        t.begin_speech(); // probes_total = 1 (speech)
        // Alternate silence/speech so consecutive never reaches 2
        // and silence never reaches 3 in a row.
        // After begin (S): F, S, F, S, ...
        let mut last = AccumDecision::Continue;
        // begin already counted 1; need MAX_ACCUM_PROBES-1 more to hit discard
        for i in 0..(MAX_ACCUM_PROBES - 1) {
            let is_speech = i % 2 == 1; // F, S, F, S, ...
            last = t.on_probe(is_speech);
            if last == AccumDecision::Discard {
                break;
            }
            assert_ne!(
                last,
                AccumDecision::Confirmed,
                "should not confirm on alternating pattern"
            );
            assert_ne!(
                last,
                AccumDecision::ShortFinalize,
                "should not short-finalize on alternating pattern"
            );
        }
        assert_eq!(last, AccumDecision::Discard);
    }

    #[test]
    fn accum_speech_after_silence_resets_silence_counter() {
        let mut t = tracker_300ms_silence_confirm2();
        t.begin_speech();
        assert_eq!(t.on_probe(false), AccumDecision::Continue);
        assert_eq!(t.on_probe(false), AccumDecision::Continue);
        assert_eq!(t.silence_probes, 2);
        // speech resets silence counter — no premature ShortFinalize
        assert_eq!(t.on_probe(true), AccumDecision::Continue);
        assert_eq!(t.silence_probes, 0);
        assert_eq!(t.consecutive_speech_probes, 1);
    }
}
