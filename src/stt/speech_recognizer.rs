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
/// Leading silence pad so Apple SFSpeechRecognizer has acoustic context before onset.
const PRE_SILENCE_MS: usize = 200;
const PRE_SILENCE_SAMPLES: usize = SR_USIZE * PRE_SILENCE_MS / 1000;
const POST_ROLL_MS: usize = 500;
const POST_ROLL_SAMPLES: usize = SR_USIZE * POST_ROLL_MS / 1000;
const MAX_ACCUM_PROBES: usize = 50;
/// Minimum silence probes before short-utterance finalize (500ms). Brief VAD dips
/// mid-phrase must not chop "Uno, dos, tres" off before "Probando…".
const MIN_SHORT_SILENCE_PROBES: usize = 5;

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
    /// Single noise blip — silent reset (no Apple task, no SpeechStart).
    Abort,
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
    /// Total speech probes seen this accumulation (not necessarily consecutive).
    speech_probes_seen: usize,
    silence_probes: usize,
    probes_total: usize,
}

impl AccumTracker {
    fn new(confirm_probes: usize, silence_samples_threshold: usize) -> Self {
        let from_silence = silence_samples_threshold.div_ceil(VAD_PROBE_SAMPLES).max(1);
        Self {
            confirm_probes: confirm_probes.max(1),
            // At least MIN_SHORT_SILENCE_PROBES so brief dips don't short-finalize.
            short_silence_probes: from_silence.max(MIN_SHORT_SILENCE_PROBES),
            consecutive_speech_probes: 0,
            speech_probes_seen: 0,
            silence_probes: 0,
            probes_total: 0,
        }
    }

    /// Record the first speech probe that started accumulation.
    /// Does not return Confirmed (SpeechStart is emitted separately as AccumStarted).
    fn begin_speech(&mut self) {
        self.consecutive_speech_probes = 1;
        self.speech_probes_seen = 1;
        self.silence_probes = 0;
        self.probes_total = 1;
    }

    fn reset(&mut self) {
        self.consecutive_speech_probes = 0;
        self.speech_probes_seen = 0;
        self.silence_probes = 0;
        self.probes_total = 0;
    }

    /// Process one subsequent probe while accumulating.
    fn on_probe(&mut self, is_speech: bool) -> AccumDecision {
        self.probes_total += 1;

        if is_speech {
            self.silence_probes = 0;
            self.consecutive_speech_probes += 1;
            self.speech_probes_seen += 1;
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
            // A single probe blip (cough/noise) → abort quietly. Real short words
            // usually leave ≥1 speech probe in the buffer with enough energy that
            // confirm nearly fired; still allow ShortFinalize when we saw speech.
            if self.speech_probes_seen < self.confirm_probes && self.speech_probes_seen <= 1 {
                self.reset();
                return AccumDecision::Abort;
            }
            self.reset();
            return AccumDecision::ShortFinalize;
        }

        AccumDecision::Continue
    }
}

/// Utterance waiting because the previous Apple task is still finalizing.
struct PendingUtterance {
    audio: Vec<f32>,
    /// If true, call end_audio() immediately after creating the task.
    end_immediately: bool,
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
    /// Audio immediately before VAD onset (from pre_roll, excludes first speech probe).
    onset_prefix: Vec<f32>,
    /// Next utterance held while the previous Apple task finishes.
    pending: Option<PendingUtterance>,
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
                // Mark task terminal on ANY finish (success or failure) / cancel.
                // Do NOT clear speech_start_sent here — that races with drain_events
                // and caused duplicate SpeechStart + "Too short (0ms)" dropped finals.
                match &event {
                    RecognitionTaskEvent::DidFinishSuccessfully(_)
                    | RecognitionTaskEvent::WasCancelled => {
                        st.task_done = true;
                        st.awaiting_finalize = false;
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
            onset_prefix: Vec::new(),
            pending: None,
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
                AccumDecision::Abort => {
                    debug!(target: "stt", "Accum abort (noise blip)");
                    self.reset_accum();
                    VadAction::None
                }
                AccumDecision::Confirmed => {
                    let buf = self.take_utterance_audio();
                    let confirmed_probes = self.accum.consecutive_speech_probes;
                    self.in_speech = true;
                    self.accumulating = false;
                    self.accum.reset();
                    debug!(
                        target: "stt",
                        "Speech confirmed after {} probes ({:.1}s incl. onset)",
                        confirmed_probes,
                        buf.len() as f32 / SR_USIZE as f32
                    );
                    VadAction::Start(buf)
                }
                AccumDecision::ShortFinalize => {
                    let buf = self.take_utterance_audio();
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
                // pre_roll already includes this chunk; keep only audio *before* onset.
                let pr: Vec<f32> = self.pre_roll.iter().copied().collect();
                let prefix_len = pr.len().saturating_sub(chunk.len());
                self.onset_prefix = pr[..prefix_len].to_vec();

                self.accumulating = true;
                self.accum.begin_speech();
                self.accum_buf.clear();
                self.accum_buf.extend_from_slice(chunk);
                debug!(
                    target: "stt",
                    "Start accumulating (1/{}) prob={:.3} onset_prefix_ms={}",
                    self.vad_confirm_probes,
                    avg_prob,
                    self.onset_prefix.len() * 1000 / SR_USIZE
                );
                return VadAction::AccumStarted;
            }
            VadAction::None
        }
    }

    /// Build the audio package for Apple: leading silence + pre-onset pad + accum.
    fn take_utterance_audio(&mut self) -> Vec<f32> {
        let prefix = std::mem::take(&mut self.onset_prefix);
        let accum = std::mem::take(&mut self.accum_buf);
        let mut out = Vec::with_capacity(PRE_SILENCE_SAMPLES + prefix.len() + accum.len());
        out.extend(std::iter::repeat_n(0.0f32, PRE_SILENCE_SAMPLES));
        out.extend(prefix);
        out.extend(accum);
        out
    }

    fn reset_accum(&mut self) {
        self.accumulating = false;
        self.accum.reset();
        self.accum_buf.clear();
        self.onset_prefix.clear();
    }

    fn reset_vad(&mut self) {
        self.in_speech = false;
        self.in_post_roll = false;
        self.reset_accum();
        self.silence_samples = 0;
    }

    async fn task_is_busy(&self) -> bool {
        let st = self.state.lock().await;
        st.task.is_some() && !st.task_done
    }

    /// Start Apple recognition, or queue audio if the previous task is still finalizing.
    async fn start_recognition(
        &mut self,
        buf: Vec<f32>,
        end_immediately: bool,
        tx: &mpsc::Sender<SpeechEvent>,
    ) -> Result<()> {
        self.emit_speech_start_if_needed(tx).await;

        if self.task_is_busy().await {
            debug!(
                target: "stt",
                "Queuing {:.1}s audio — previous task still finalizing",
                buf.len() as f32 / SR_USIZE as f32
            );
            if let Some(ref mut p) = self.pending {
                p.audio.extend_from_slice(&buf);
                p.end_immediately = p.end_immediately || end_immediately;
            } else {
                self.pending = Some(PendingUtterance {
                    audio: buf,
                    end_immediately,
                });
            }
            return Ok(());
        }

        self.create_task().await?;
        self.feed_audio(&buf).await?;
        if end_immediately {
            self.signal_end_audio().await;
        }
        Ok(())
    }

    /// After a task completes, start any queued utterance.
    async fn flush_pending(&mut self, tx: &mpsc::Sender<SpeechEvent>) -> Result<()> {
        let Some(pending) = self.pending.take() else {
            return Ok(());
        };
        if self.task_is_busy().await {
            // Still busy — put it back.
            self.pending = Some(pending);
            return Ok(());
        }
        debug!(
            target: "stt",
            "Flushing queued utterance ({:.1}s, end_immediately={})",
            pending.audio.len() as f32 / SR_USIZE as f32,
            pending.end_immediately
        );
        // New utterance — allow a fresh SpeechStart.
        {
            let mut st = self.state.lock().await;
            st.speech_start_sent = false;
        }
        self.emit_speech_start_if_needed(tx).await;
        self.create_task().await?;
        self.feed_audio(&pending.audio).await?;
        if pending.end_immediately {
            self.signal_end_audio().await;
        }
        Ok(())
    }

    /// Emit SpeechStart once per utterance (idempotent via speech_start_sent).
    async fn emit_speech_start_if_needed(&self, tx: &mpsc::Sender<SpeechEvent>) {
        let should_emit = {
            let mut st = self.state.lock().await;
            if st.speech_start_sent {
                false
            } else {
                st.speech_start_sent = true;
                true
            }
        };
        if should_emit {
            debug!(target: "stt", "SpeechStart (from VAD commit)");
            let _ = tx.send(SpeechEvent::SpeechStart).await;
        }
    }

    /// Create a new recognition task.
    async fn create_task(&self) -> Result<()> {
        let mut st = self.state.lock().await;
        st.task_done = false;
        st.awaiting_finalize = false;
        // Do not reset speech_start_sent — VAD emits SpeechStart on Start/StartShort
        // just before create_task. Clearing it here would allow a duplicate from
        // any late Apple partial path.

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
        let mut task_terminal = false;

        for event in events {
            match event {
                RecognitionTaskEvent::DidDetectSpeech => {
                    debug!(target: "stt", "Apple detected speech");
                }
                RecognitionTaskEvent::DidHypothesizeTranscription(t) => {
                    let text = t.formatted_string;
                    if !text.is_empty() {
                        // SpeechStart is emitted by VAD on AccumStarted (fast barge-in).
                        // Never re-emit from Apple partials — that caused a second
                        // SpeechStart right before SpeechEnd, making main.rs discard
                        // the final as "Too short (0ms)".
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
                    task_terminal = true;
                }
                RecognitionTaskEvent::WasCancelled => {
                    debug!(target: "stt", "Task cancelled");
                    task_terminal = true;
                }
                RecognitionTaskEvent::FinishedReadingAudio => {
                    debug!(target: "stt", "Finished reading audio");
                }
                RecognitionTaskEvent::DidProcessAudioDuration(duration) => {
                    debug!(target: "stt", "Processed: {:.2}s", duration);
                }
            }
        }

        // Apple can finish with success=false and no DidFinishRecognition (e.g. too
        // little audio, recognition error). Without SpeechEnd the TUI stays LISTENING.
        if task_terminal && !got_final && speech_start_sent {
            let quality = TranscriptionQuality {
                text: String::new(),
                no_speech_prob: 0.0,
                avg_logprob: 0.0,
                compression_ratio: 0.0,
            };
            debug!(target: "stt", "SpeechEnd (task terminal without recognition, empty)");
            let _ = tx.send(SpeechEvent::SpeechEnd(quality)).await;
            got_final = true;
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
                    // First speech-like probe only starts accumulation. SpeechStart is
                    // deferred until Confirm (Start) or short-utterance finalize
                    // (StartShort) so a single noisy 100ms probe does not flip the
                    // TUI to LISTENING or fire barge-in.
                    debug!(target: "stt", "VAD onset (accumulating, SpeechStart deferred)");
                }
                VadAction::Start(buf) => {
                    self.start_recognition(buf, false, tx).await?;
                }
                VadAction::StartShort(buf) => {
                    self.start_recognition(buf, true, tx).await?;
                }
                VadAction::Feed(chunk) => {
                    // If the previous task is still finalizing, this audio belongs
                    // to the queued follow-up utterance — don't feed the dying task.
                    if let Some(ref mut p) = self.pending {
                        p.audio.extend_from_slice(&chunk);
                    } else {
                        self.feed_audio(&chunk).await?;
                    }
                }
                VadAction::End => {
                    if let Some(ref mut p) = self.pending {
                        // Follow-up utterance ended before it could start — finalize when flushed.
                        p.end_immediately = true;
                        debug!(target: "stt", "End while queued — will end_audio on flush");
                    } else {
                        // Do not re-feed pre_roll: already streamed via Feed.
                        self.signal_end_audio().await;
                    }
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

        // Drain buffered events (may emit SpeechEnd for failed tasks)
        self.drain_events(tx).await?;

        // Reset VAD state if task is done.
        // If the task died while we still thought we were in_speech, force-reset
        // so the next utterance is not blocked on a dead Apple task.
        let (task_done, speech_start_sent) = {
            let st = self.state.lock().await;
            (st.task_done, st.speech_start_sent)
        };
        if task_done && self.in_speech {
            warn!(target: "stt", "Recognition task ended while in_speech — force VAD reset");
            self.reset_vad();
            if speech_start_sent {
                let quality = TranscriptionQuality {
                    text: String::new(),
                    no_speech_prob: 0.0,
                    avg_logprob: 0.0,
                    compression_ratio: 0.0,
                };
                debug!(target: "stt", "SpeechEnd (mid-speech task death, empty)");
                let _ = tx.send(SpeechEvent::SpeechEnd(quality)).await;
                let mut st = self.state.lock().await;
                st.speech_start_sent = false;
            }
        } else if task_done && !self.in_speech && !self.accumulating {
            self.reset_vad();
        }

        // Previous task finished — start any utterance that was queued behind it.
        if task_done {
            self.flush_pending(tx).await?;
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
    fn accum_single_blip_aborts_after_min_silence() {
        let mut t = tracker_300ms_silence_confirm2();
        t.begin_speech(); // 1 speech only
        for _ in 0..(MIN_SHORT_SILENCE_PROBES - 1) {
            assert_eq!(t.on_probe(false), AccumDecision::Continue);
        }
        // Single-probe noise blip → Abort (no Apple task)
        assert_eq!(t.on_probe(false), AccumDecision::Abort);
    }

    #[test]
    fn accum_two_speech_probes_then_silence_short_finalize() {
        let mut t = tracker_300ms_silence_confirm2();
        t.begin_speech(); // seen=1
        assert_eq!(t.on_probe(false), AccumDecision::Continue);
        assert_eq!(t.on_probe(true), AccumDecision::Continue); // seen=2, not consecutive enough to confirm
        for _ in 0..(MIN_SHORT_SILENCE_PROBES - 1) {
            assert_eq!(t.on_probe(false), AccumDecision::Continue);
        }
        assert_eq!(t.on_probe(false), AccumDecision::ShortFinalize);
    }

    #[test]
    fn accum_one_speech_then_two_silence_continues() {
        let mut t = tracker_300ms_silence_confirm2();
        t.begin_speech();
        assert_eq!(t.on_probe(false), AccumDecision::Continue);
        assert_eq!(t.on_probe(false), AccumDecision::Continue);
        assert_eq!(t.silence_probes, 2);
        // short window is at least MIN_SHORT_SILENCE_PROBES (500ms)
        assert_eq!(t.short_silence_probes, MIN_SHORT_SILENCE_PROBES);
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
