//! Shared local VAD + utterance segmentation for the STT providers.
//!
//! Whisper, Parakeet and the remote OpenAI-compatible provider (issue #17) all
//! need the same thing: turn a stream of 16 kHz mono f32 chunks into
//! `SpeechStart` / utterance-closed events using Silero VAD, with a
//! consecutive-probe confirmation window, a silence timeout and a real-audio
//! pre-roll so the recognizer never loses the onset.
//!
//! A segment is closed by VAD silence alone: once `silence_ms` of audio below
//! the end threshold has accumulated and the post-roll grace has run out, the
//! utterance is finalized. The silence is measured at the detector's window
//! resolution (not per whole probe), so a pause shorter than `silence_ms`
//! cannot be counted as the timeout. There is no time-based cut either, so an
//! utterance stays one segment for as long as the user keeps speaking (contract
//! P5).
//!
//! Contract properties locked by tests (see `doc/stt-contract.md`):
//! P1 speech-start commit budget, P4 onset preservation via pre-roll, P5
//! silence-only close, and the hysteresis behavior that keeps soft tails and
//! mid-sentence dips inside one segment.
//!
//! Before this module each provider carried its own copy of that state machine.
//! It now lives here once. The three consumers are configured to share the
//! same defaults (uniform contract):
//!
//! | Provider  | probe | pre-roll        | post-roll |
//! |-----------|-------|-----------------|-----------|
//! | whisper   | 200ms | 300ms real audio| 200ms     |
//! | parakeet  | 200ms | 300ms real audio| 200ms     |
//! | openai    | 200ms | 300ms real audio| 200ms     |
//!
//! `speech_recognizer.rs` (macOS SFSpeechRecognizer) intentionally keeps its own
//! logic: it streams audio to Apple and uses a different state machine with its
//! own pre-roll, post-roll and short-utterance fallback, and it only compiles
//! on macOS.
//!
//! The state machine is platform- and model-agnostic: speech detection is
//! injected through [`SpeechDetector`], so the logic is unit-testable without
//! loading Silero or any STT model.

use anyhow::{Result, ensure};
use whisper_cpp_plus::WhisperVadProcessor;

/// Pipeline sample rate (16 kHz mono f32).
pub const SAMPLE_RATE: usize = 16_000;

/// Default VAD probe window. Silero-via-whisper.cpp probabilities are stable
/// on 200 ms windows; 100 ms windows oscillate strongly (calibration audit
/// `vad_probe_calibration`), breaking the confirmation chain on natural
/// inter-word gaps. Configurable via `VAD_PROBE_MS`.
pub const DEFAULT_PROBE_MS: usize = 200;

/// Default real-audio pre-roll prepended to a confirmed speech segment
/// (`VAD_PREROLL_MS`). Probes whose speech probability never crossed the start
/// threshold still carry the utterance onset; without this ring buffer that
/// audio was discarded and the first word was mistranscribed.
pub const DEFAULT_PREROLL_MS: usize = 300;

/// Peak the VAD input is normalized to when the captured signal is quiet.
pub const VAD_TARGET_PEAK: f32 = 0.5;

/// Probes quieter than this are boosted before the VAD (see [`vad_input_gain`]).
pub const VAD_NORMALIZE_BELOW: f32 = 0.1;

/// Cap on that boost, so a near-silent probe stays quiet instead of amplifying
/// the noise floor into speech. Silero is content-based above a sane level
/// (measured: white noise boosted to a 0.5 peak still scores 0.02–0.11), so the
/// cap can be wide enough to compensate a capture 50 dB below full scale.
pub const VAD_MAX_GAIN: f32 = 500.0;

/// Gain applied to one probe before the VAD model.
///
/// Conference speakerphones and some USB/ALSA capture paths deliver a signal
/// 20–40 dB below full scale, and Silero's probabilities collapse on very quiet
/// input — measured on this model: the same speech scores 0.6–0.9 at peak 0.1,
/// 0.2–0.8 at peak 0.03 and 0.05–0.36 at peak 0.01. Speech then reads as
/// silence, so the turn closes while the user is still talking (observed in
/// production: 1 s silence credits plus live partials growing with new words).
/// Probes below [`VAD_NORMALIZE_BELOW`] are scaled up to [`VAD_TARGET_PEAK`],
/// capped at [`VAD_MAX_GAIN`] so genuine silence stays below the thresholds.
pub fn vad_input_gain(peak: f32) -> f32 {
    if peak >= VAD_NORMALIZE_BELOW || peak <= f32::EPSILON {
        return 1.0;
    }
    (VAD_TARGET_PEAK / peak).min(VAD_MAX_GAIN)
}

/// Fallback pad of zeros when the pre-roll ring is empty (speech starts on the
/// very first probe of the stream). Gives the recognizer acoustic context
/// before an abrupt audio start.
pub const PRE_SILENCE_MS: usize = 200;

/// Preallocation for the segment buffer (4 s of speech). The buffer grows with
/// the utterance; a segment has no length limit (contract P5).
const SEGMENT_BUF_INITIAL_SAMPLES: usize = SAMPLE_RATE * 4;

/// Size of the sliding decision window used to confirm a speech start:
/// `vad_confirm_probes` speech decisions within the last 3 probes.
const CONFIRM_WINDOW: usize = 3;

/// Time budget for the accumulation window: audio from probes that never reach
/// the confirmation threshold is discarded after this long (very short
/// utterance the VAD never confirms, e.g. a cough).
const MAX_ACCUM_MS: usize = 10_000;

const TRIM_WINDOW_MS: usize = 20;
const TRIM_WINDOW_SAMPLES: usize = SAMPLE_RATE * TRIM_WINDOW_MS / 1000;
const MAX_TRIM_PERCENT: usize = 90;

/// Speech probability for one probe chunk.
///
/// Implemented by [`SileroDetector`] in production and by a scripted fake in
/// unit tests.
pub trait SpeechDetector: Send {
    /// Return the average speech probability for one `VAD_PROBE_SAMPLES` chunk
    /// (0.0 when the detector reports no speech at all).
    fn probe(&mut self, chunk: &[f32]) -> f32;

    /// Per-window speech probabilities of the model for the last
    /// [`SpeechDetector::probe`] call, in chronological order — the finest
    /// resolution the detector exposes (7 windows per 200 ms probe on the
    /// reference Silero model). The segmenter charges a probe only for the
    /// audio that is really below the end threshold (contract P5). An empty
    /// slice makes it treat the probe as a single all-or-nothing decision.
    fn window_probs(&self) -> &[f32] {
        &[]
    }

    /// Gain the detector applied to the last probe (1.0 = untouched). Detectors
    /// that normalize a quiet capture surface it here so the logs report the
    /// real input level.
    fn input_gain(&self) -> f32 {
        1.0
    }
}

/// [`SpeechDetector`] backed by the Silero VAD model via `whisper-cpp-plus`.
pub struct SileroDetector {
    vad: WhisperVadProcessor,
    /// Per-window probabilities of the last probe.
    window_probs: Vec<f32>,
    /// Gain applied to the last probe (1.0 = untouched).
    input_gain: f32,
    /// Reused buffer for a boosted probe.
    scratch: Vec<f32>,
}

impl SileroDetector {
    pub fn new(vad: WhisperVadProcessor) -> Self {
        Self {
            vad,
            window_probs: Vec::new(),
            input_gain: 1.0,
            scratch: Vec::new(),
        }
    }
}

impl SpeechDetector for SileroDetector {
    fn probe(&mut self, chunk: &[f32]) -> f32 {
        let peak = chunk.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let gain = vad_input_gain(peak);
        self.input_gain = gain;
        let samples: &[f32] = if gain == 1.0 {
            chunk
        } else {
            self.scratch.clear();
            self.scratch.extend(chunk.iter().map(|s| s * gain));
            &self.scratch
        };

        // `detect_speech` runs the model and fills the probability array. The
        // raw windows are used even when the model's own (0.5) threshold
        // reports "no speech", so the configured start/end thresholds decide.
        let _ = self.vad.detect_speech(samples);
        self.window_probs = self.vad.get_probs();
        if self.window_probs.is_empty() {
            0.0
        } else {
            self.window_probs.iter().sum::<f32>() / self.window_probs.len() as f32
        }
    }

    fn window_probs(&self) -> &[f32] {
        &self.window_probs
    }

    fn input_gain(&self) -> f32 {
        self.input_gain
    }
}

/// Tunables of the segmentation state machine.
#[derive(Debug, Clone)]
pub struct SegmenterConfig {
    /// Milliseconds of continuous silence required to close a speech segment.
    pub silence_ms: u32,
    /// Speech probability threshold to transition from silence -> speech.
    pub vad_start_threshold: f32,
    /// Speech probability threshold to stay in speech (speech -> silence below).
    pub vad_end_threshold: f32,
    /// Minimum consecutive speech probes required before committing to STT.
    pub vad_confirm_probes: usize,
    /// Grace period after the silence timeout during which resumed speech
    /// cancels the closure. `0` disables it (single-stage close).
    pub post_roll_ms: usize,
    /// Trim leading silence from the finalized segment before transcription.
    pub trim_leading_silence: bool,
    /// VAD probe window in ms (default 100).
    pub probe_ms: usize,
    /// Real-audio pre-roll prepended to confirmed segments, in ms (default 300).
    pub preroll_ms: usize,
}

impl SegmenterConfig {
    /// Uniform defaults shared by whisper / parakeet / openai (P6).
    pub fn uniform(
        silence_ms: u32,
        vad_start_threshold: f32,
        vad_end_threshold: f32,
        vad_confirm_probes: usize,
    ) -> Self {
        Self {
            silence_ms,
            vad_start_threshold,
            vad_end_threshold,
            vad_confirm_probes,
            post_roll_ms: 200,
            trim_leading_silence: false,
            probe_ms: DEFAULT_PROBE_MS,
            preroll_ms: DEFAULT_PREROLL_MS,
        }
    }
}

/// Events emitted by [`VadSegmenter::feed`].
///
/// `segment_seq` is a monotonically increasing id assigned at VAD commit; it
/// lets downstream consumers pair starts with ends and discard stale results
/// after a barge-in (contract P2).
#[derive(Debug, Clone, PartialEq)]
pub enum SegmenterEvent {
    /// VAD committed to a speech segment (drives barge-in / LISTENING).
    SpeechStart { segment_seq: u64 },
    /// A finalized utterance ready to be transcribed. `trailing_silence` is
    /// the length (samples) of the silence tail at the end of `audio`; the
    /// spoken part ends at `audio.len() - trailing_silence`.
    SegmentReady {
        segment_seq: u64,
        audio: Vec<f32>,
        trailing_silence: usize,
    },
}

/// VAD-driven utterance segmenter, generic over the speech detector so the
/// state machine can be exercised without models.
pub struct VadSegmenter<D: SpeechDetector> {
    detector: D,
    vad_start_threshold: f32,
    vad_end_threshold: f32,
    probe_samples: usize,
    preroll_samples: usize,
    silence_samples_threshold: usize,
    max_accum_samples: usize,
    post_roll_samples: usize,
    trim: bool,
    vad_confirm_probes: usize,

    // Segment ids: assigned at commit, echoed by every event of the segment.
    next_segment_seq: u64,
    current_segment_seq: u64,

    // State machine
    in_speech: bool,
    in_post_roll: bool,
    speech_buf: Vec<f32>,
    silence_samples: usize,
    post_roll_remaining: usize,

    /// Real-audio ring of the probes that preceded the utterance onset. Frozen
    /// while accumulating / in speech; consumed at commit.
    pre_roll: Vec<f32>,

    // Confirmation window: require `vad_confirm_probes` speech decisions
    // within a sliding window of (N+1) probes — natural inter-word gaps
    // (espeak pauses measure 100–200 ms, Silero dips below threshold on
    // them) must not break the confirmation chain, but an isolated burst
    // (cough/click) still fails to confirm.
    decision_window: [bool; CONFIRM_WINDOW],
    decision_ring_fill: usize,
    accumulating: bool,
    accum_buf: Vec<f32>,
    accum_samples_total: usize,

    /// One-shot flag: the silence timeout opened the post-roll grace (the
    /// utterance is probably over). The provider consumes it to fire the
    /// last-chance live decode that covers all speech before the close.
    closing_trigger: bool,

    // Leftover samples that didn't fill a probe window.
    probe_carry: Vec<f32>,
}

impl<D: SpeechDetector> VadSegmenter<D> {
    pub fn new(detector: D, config: SegmenterConfig) -> Result<Self> {
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

        let probe_samples = SAMPLE_RATE * config.probe_ms.max(1) / 1000;

        Ok(Self {
            detector,
            vad_start_threshold: config.vad_start_threshold,
            vad_end_threshold,
            probe_samples,
            preroll_samples: SAMPLE_RATE * config.preroll_ms / 1000,
            silence_samples_threshold: SAMPLE_RATE * config.silence_ms as usize / 1000,
            max_accum_samples: SAMPLE_RATE * MAX_ACCUM_MS / 1000,
            post_roll_samples: SAMPLE_RATE * config.post_roll_ms / 1000,
            trim: config.trim_leading_silence,
            vad_confirm_probes: config.vad_confirm_probes.max(1),
            next_segment_seq: 1,
            current_segment_seq: 0,
            in_speech: false,
            in_post_roll: false,
            speech_buf: Vec::with_capacity(SEGMENT_BUF_INITIAL_SAMPLES),
            silence_samples: 0,
            post_roll_remaining: 0,
            pre_roll: Vec::with_capacity(SAMPLE_RATE * config.preroll_ms / 1000),
            accumulating: false,
            accum_buf: Vec::new(),
            accum_samples_total: 0,
            decision_window: [false; CONFIRM_WINDOW],
            decision_ring_fill: 0,
            closing_trigger: false,
            probe_carry: Vec::with_capacity(probe_samples),
        })
    }

    /// Stream-time budget of the start commit: the confirmation window can
    /// take up to CONFIRM_WINDOW probes (2-of-3 with one gap), in ms. Locked
    /// by the P1 audit.
    pub fn commit_budget_ms(&self) -> usize {
        self.probe_samples * CONFIRM_WINDOW * 1000 / SAMPLE_RATE
    }

    /// True while a confirmed segment is open.
    pub fn in_speech(&self) -> bool {
        self.in_speech
    }

    /// Seq of the segment currently open (0 = none).
    pub fn current_segment_seq(&self) -> u64 {
        self.current_segment_seq
    }

    /// True once the open segment started accumulating end-of-speech silence.
    /// Live transcription uses this to trigger the last decode that covers all
    /// speech before the segment closes.
    pub fn closing_silence_started(&self) -> bool {
        self.in_post_roll
    }

    /// One-shot: consume the closing trigger (post-roll opened). The provider
    /// fires its last-chance live decode when this returns true.
    pub fn take_closing_trigger(&mut self) -> bool {
        std::mem::take(&mut self.closing_trigger)
    }

    /// Snapshot of the open segment's audio (empty when idle).
    pub fn speech_buf(&self) -> &[f32] {
        &self.speech_buf
    }

    /// Samples of `speech_buf` that belong to speech (the trailing silence
    /// tail starts after this offset).
    pub fn speech_end_offset(&self) -> usize {
        self.speech_buf.len().saturating_sub(self.silence_samples)
    }

    pub fn probe_samples(&self) -> usize {
        self.probe_samples
    }

    /// Feed a chunk of 16 kHz mono f32 audio and collect the events it produced.
    pub fn feed(&mut self, audio: &[f32]) -> Vec<SegmenterEvent> {
        let mut events = Vec::new();
        if audio.is_empty() {
            return events;
        }

        self.probe_carry.extend_from_slice(audio);
        while self.probe_carry.len() >= self.probe_samples {
            let chunk: Vec<f32> = self.probe_carry.drain(..self.probe_samples).collect();
            self.process_probe(&chunk, &mut events);
        }

        events
    }

    fn process_probe(&mut self, chunk: &[f32], events: &mut Vec<SegmenterEvent>) {
        let avg_prob = self.detector.probe(chunk);

        if self.in_speech {
            if self.in_post_roll {
                // ── Post-roll grace period ──
                self.speech_buf.extend_from_slice(chunk);
                let silent_samples = self.probe_silence_samples(chunk.len(), avg_prob);
                if silent_samples > 0 {
                    self.post_roll_remaining = self.post_roll_remaining.saturating_sub(chunk.len());
                    // Count post-roll silence too, so the finalized segment can
                    // report where the spoken part ends.
                    self.silence_samples += silent_samples;
                    if self.post_roll_remaining == 0 {
                        self.finalize_segment(events);
                    }
                } else {
                    self.in_post_roll = false;
                    self.post_roll_remaining = 0;
                    self.silence_samples = 0;
                }
            } else {
                // ── Confirmed speech, accumulating ──
                self.speech_buf.extend_from_slice(chunk);
                let silent_samples = self.probe_silence_samples(chunk.len(), avg_prob);

                if silent_samples > 0 {
                    self.silence_samples += silent_samples;
                    tracing::debug!(
                        target: "sttvad",
                        "Silence in segment {}: +{:.0}ms (avg_prob={:.3}, input_gain=×{:.0}) → {:.0}ms of {:.0}ms",
                        self.current_segment_seq,
                        silent_samples as f32 * 1000.0 / SAMPLE_RATE as f32,
                        avg_prob,
                        self.detector.input_gain(),
                        self.silence_samples as f32 * 1000.0 / SAMPLE_RATE as f32,
                        self.silence_samples_threshold as f32 * 1000.0 / SAMPLE_RATE as f32,
                    );
                } else {
                    self.silence_samples = 0;
                }

                let silence_timeout = self.silence_samples >= self.silence_samples_threshold;

                if silence_timeout {
                    if self.post_roll_samples == 0 {
                        self.finalize_segment(events);
                    } else {
                        self.in_post_roll = true;
                        self.post_roll_remaining = self.post_roll_samples;
                        self.closing_trigger = true;
                    }
                }
            }
        } else if self.accumulating {
            // ── Accumulating: waiting for confirmation ──
            // Always append to accum_buf so the audio is continuous from the
            // first probe to confirmation, regardless of intermediate silence.
            self.accum_buf.extend_from_slice(chunk);
            self.accum_samples_total += chunk.len();

            // Guard against unbounded accumulation (e.g. very short utterance
            // that never reaches the confirmation threshold).
            if self.accum_samples_total >= self.max_accum_samples {
                tracing::debug!(
                    target: "sttvad",
                    "Accumulation timeout after {:.1}s, discarding",
                    self.accum_samples_total as f32 / SAMPLE_RATE as f32
                );
                self.accumulating = false;
                self.decision_ring_fill = 0;
                self.accum_samples_total = 0;
                self.accum_buf.clear();
            }

            let is_speech = avg_prob >= self.vad_start_threshold;

            if is_speech {
                self.push_decision(true);

                if self.confirm_reached() {
                    // CONFIRMED: transition to speech state. The real-audio
                    // pre-roll (probes that never crossed the start threshold)
                    // is prepended so the utterance onset is never lost (P4);
                    // fall back to a zero pad when the ring is empty (speech
                    // started on the very first probe of the stream).
                    self.in_speech = true;
                    self.accumulating = false;
                    self.current_segment_seq = self.next_segment_seq;
                    self.next_segment_seq += 1;
                    self.speech_buf.clear();
                    let preroll_len = self.pre_roll.len();
                    if preroll_len == 0 {
                        let pad = SAMPLE_RATE * PRE_SILENCE_MS / 1000;
                        self.speech_buf.extend(std::iter::repeat_n(0.0f32, pad));
                    } else {
                        self.speech_buf.extend(self.pre_roll.drain(..));
                    }
                    self.speech_buf.append(&mut self.accum_buf);
                    events.push(SegmenterEvent::SpeechStart {
                        segment_seq: self.current_segment_seq,
                    });
                    let gain = self.detector.input_gain();
                    tracing::debug!(
                        target: "sttvad",
                        "Speech confirmed after {} probes ({:.1}s, pre-roll {}ms, input_gain=×{:.0})",
                        self.vad_confirm_probes,
                        self.speech_buf.len() as f32 / SAMPLE_RATE as f32,
                        preroll_len * 1000 / SAMPLE_RATE,
                        gain
                    );
                    if gain > 2.0 {
                        tracing::warn!(
                            target: "sttvad",
                            "Quiet microphone input: VAD probe peak is low, boosting ×{:.0} before the model. \
                             Speech detection and the silence timeout become unreliable at this capture level; \
                             raise the microphone gain (or the device's level) if the transcripts suffer.",
                            gain
                        );
                    }
                }
            } else {
                // Silence during accumulation — keep accumulating audio (it's
                // part of the continuous stream); the sliding decision window
                // tolerates one inter-word gap.
                self.push_decision(false);
                tracing::debug!(
                    target: "sttvad",
                    "Accumulation probe below threshold (avg_prob={:.3})",
                    avg_prob
                );
            }
        } else {
            // ── Silence: keep the real-audio pre-roll ring up to date, then
            // start accumulating if speech detected. The probe that starts
            // accumulation must NOT enter the ring: it is already the first
            // entry of accum_buf.
            let is_speech = avg_prob >= self.vad_start_threshold;

            if is_speech {
                self.accumulating = true;
                self.decision_window = [false; CONFIRM_WINDOW];
                self.decision_ring_fill = 1;
                self.decision_window[CONFIRM_WINDOW - 1] = true;
                self.accum_samples_total = chunk.len();
                self.accum_buf.clear();
                self.accum_buf.extend_from_slice(chunk);
                tracing::debug!(
                    target: "sttvad",
                    "Start accumulating (probe 1/{}), avg_prob={:.3}, pre-roll {:.0}ms",
                    self.vad_confirm_probes,
                    avg_prob,
                    self.pre_roll.len() as f32 / SAMPLE_RATE as f32 * 1000.0
                );
            } else {
                self.push_pre_roll(chunk);
            }
        }
    }

    /// Push a decision bit into the sliding confirmation window.
    fn push_decision(&mut self, is_speech: bool) {
        self.decision_window.rotate_left(1);
        self.decision_window[CONFIRM_WINDOW - 1] = is_speech;
        self.decision_ring_fill = (self.decision_ring_fill + 1).min(CONFIRM_WINDOW);
    }

    /// True when the window holds `vad_confirm_probes` speech decisions
    /// (`vad_confirm_probes` of the last CONFIRM_WINDOW probes). With N=1 the
    /// first speech probe confirms immediately.
    fn confirm_reached(&self) -> bool {
        let count = self.decision_window.iter().filter(|&&b| b).count();
        count >= self.vad_confirm_probes.max(1)
    }

    /// Append a non-speech probe to the pre-roll ring, dropping the oldest
    /// samples beyond the configured window.
    fn push_pre_roll(&mut self, chunk: &[f32]) {
        self.pre_roll.extend_from_slice(chunk);
        let overflow = self.pre_roll.len().saturating_sub(self.preroll_samples);
        if overflow > 0 {
            self.pre_roll.drain(..overflow);
        }
    }

    /// Samples of one probe that count as silence: the part of the probe whose
    /// per-window probability is below the end threshold, at the detector's
    /// finest resolution.
    ///
    /// Two conditions must hold, otherwise the probe breaks the silence run
    /// (returns 0): the probe average is below the end threshold — a probe
    /// carrying speech ends the run, so a fading tail or a soft syllable cannot
    /// accumulate silence (contract P5 hysteresis) — and most of its windows
    /// are below the threshold. When the detector exposes no windows the
    /// average decides and the whole probe counts.
    ///
    /// Probe-granular counting charged a probe that merely brushed the
    /// threshold a full `probe_ms`, so a pause well under `VAD_SILENCE_MS` could
    /// be counted as the timeout (contract P5: the timeout is real
    /// below-threshold audio, not probe arithmetic).
    fn probe_silence_samples(&self, chunk_len: usize, avg_prob: f32) -> usize {
        if avg_prob >= self.vad_end_threshold {
            return 0;
        }
        let probs = self.detector.window_probs();
        if probs.is_empty() {
            return chunk_len;
        }
        let below = probs
            .iter()
            .filter(|&&p| p < self.vad_end_threshold)
            .count();
        if below * 2 < probs.len() {
            return 0;
        }
        chunk_len * below / probs.len()
    }

    fn finalize_segment(&mut self, events: &mut Vec<SegmenterEvent>) {
        // Trailing silence at close = the accumulated below-end-threshold tail
        // (silence probes + post-roll probes; reset to 0 whenever speech
        // resumed). Captured before the buffer is taken.
        let trailing = self.silence_samples;
        let audio = std::mem::take(&mut self.speech_buf);
        self.in_speech = false;
        self.in_post_roll = false;
        self.silence_samples = 0;
        self.post_roll_remaining = 0;
        self.closing_trigger = false;
        let seq = self.current_segment_seq;

        let segment = if self.trim {
            trim_leading_silence(&audio)
        } else {
            audio
        };
        if segment.is_empty() {
            return;
        }

        tracing::debug!(
            target: "sttvad",
            "Finalizing segment {seq}: {:.2}s",
            segment.len() as f32 / SAMPLE_RATE as f32
        );
        events.push(SegmenterEvent::SegmentReady {
            segment_seq: seq,
            trailing_silence: trailing,
            audio: segment,
        });
    }
}

/// Drop leading silence (by RMS windows) while keeping at least 10% of the
/// audio. Used by providers that opt into `trim_leading_silence`.
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

    /// Detector that returns a scripted probability per probe.
    struct ScriptedDetector {
        probs: Vec<f32>,
        idx: usize,
    }

    impl ScriptedDetector {
        fn new(probs: &[f32]) -> Self {
            Self {
                probs: probs.to_vec(),
                idx: 0,
            }
        }
    }

    impl SpeechDetector for ScriptedDetector {
        fn probe(&mut self, _chunk: &[f32]) -> f32 {
            let p = self.probs.get(self.idx).copied().unwrap_or(0.0);
            self.idx += 1;
            p
        }
    }

    /// Detector whose probes expose per-window probabilities, so the
    /// window-resolved silence accounting can be exercised.
    struct WindowDetector {
        probes: Vec<Vec<f32>>,
        idx: usize,
    }

    impl SpeechDetector for WindowDetector {
        fn probe(&mut self, _chunk: &[f32]) -> f32 {
            let windows = self.probes.get(self.idx).cloned().unwrap_or_default();
            self.idx += 1;
            if windows.is_empty() {
                0.0
            } else {
                windows.iter().sum::<f32>() / windows.len() as f32
            }
        }

        fn window_probs(&self) -> &[f32] {
            self.probes
                .get(self.idx.saturating_sub(1))
                .map(|w| w.as_slice())
                .unwrap_or(&[])
        }
    }

    fn config(post_roll_ms: usize, trim: bool) -> SegmenterConfig {
        SegmenterConfig {
            silence_ms: 300,
            vad_start_threshold: 0.65,
            vad_end_threshold: 0.45,
            vad_confirm_probes: 2,
            post_roll_ms,
            trim_leading_silence: trim,
            probe_ms: 100,
            preroll_ms: 300,
        }
    }

    /// Feed one speech/silence probe (with the given probability) per call.
    fn run(seg: &mut VadSegmenter<ScriptedDetector>, probs: &[f32]) -> Vec<SegmenterEvent> {
        let chunk = vec![0.1f32; seg.probe_samples()];
        let mut events = Vec::new();
        for p in probs {
            seg.detector.probs.push(*p);
            events.extend(seg.feed(&chunk));
        }
        events
    }

    fn build(post_roll_ms: usize, trim: bool) -> VadSegmenter<ScriptedDetector> {
        VadSegmenter::new(ScriptedDetector::new(&[]), config(post_roll_ms, trim)).unwrap()
    }

    fn starts(events: &[SegmenterEvent]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, SegmenterEvent::SpeechStart { .. }))
            .count()
    }

    fn segments(events: &[SegmenterEvent]) -> Vec<Vec<f32>> {
        events
            .iter()
            .filter_map(|e| match e {
                SegmenterEvent::SegmentReady { audio, .. } => Some(audio.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn silence_emits_nothing() {
        let mut seg = build(0, false);
        let events = run(&mut seg, &[0.0, 0.0, 0.0, 0.0, 0.0]);
        assert!(events.is_empty(), "got {events:?}");
    }

    #[test]
    fn single_probe_does_not_confirm() {
        // Default confirm_probes=2: one speech probe then silence must not start.
        let mut seg = build(0, false);
        let events = run(&mut seg, &[0.9, 0.0, 0.0]);
        assert_eq!(starts(&events), 0, "got {events:?}");
        assert!(segments(&events).is_empty());
    }

    #[test]
    fn two_consecutive_probes_confirm_and_emit_start() {
        let mut seg = build(0, false);
        let events = run(&mut seg, &[0.9, 0.9]);
        assert_eq!(starts(&events), 1, "got {events:?}");
    }

    #[test]
    fn pre_silence_is_prepended_to_the_segment() {
        // confirm (2 probes) then close with silence (300 ms silence threshold
        // / 100 ms probe = 3 probes). No prior probes → pre-roll is empty →
        // the zero-pad fallback applies.
        let mut seg = build(0, false);
        let events = run(&mut seg, &[0.9, 0.9, 0.0, 0.0, 0.0]);
        let segs = segments(&events);
        assert_eq!(segs.len(), 1);
        // zero pad (200 ms) + accum_buf (2 probes) + 3 closing probes.
        let pad = SAMPLE_RATE * PRE_SILENCE_MS / 1000;
        assert_eq!(
            segs[0].len(),
            pad + 2 * seg.probe_samples() + 3 * seg.probe_samples(),
            "expected zero-pad fallback + speech + trailing silence"
        );
    }

    /// P4 (contract): probes that never crossed the start threshold still
    /// carry the utterance onset. The confirmed segment must open with that
    /// real audio, never with discarded silence.
    #[test]
    fn onset_preserved_via_preroll() {
        // Two sub-threshold probes (soft onset) then two confirm probes, then
        // 3 silent probes to close.
        let mut seg = build(0, false);
        let events = run(&mut seg, &[0.0, 0.3, 0.9, 0.9, 0.0, 0.0, 0.0]);
        let segs = segments(&events);
        assert_eq!(segs.len(), 1, "got {events:?}");
        let probe = seg.probe_samples();
        // Pre-roll holds the two pre-threshold probes; the segment must open
        // with their real audio (amplitude 0.1), in order.
        assert!(
            segs[0][..2 * probe].iter().all(|&s| (s - 0.1).abs() < 1e-6),
            "segment must start with the pre-threshold audio, got {:?}",
            &segs[0][..probe]
        );
        // Followed by the 2 confirmed (accumulated) probes.
        assert!(
            segs[0][2 * probe..4 * probe]
                .iter()
                .all(|&s| (s - 0.1).abs() < 1e-6),
            "accumulated probes must follow the pre-roll"
        );
        assert_eq!(segs[0].len(), 7 * probe);
    }

    #[test]
    fn silence_timeout_closes_segment_without_post_roll() {
        // 3 probes of 200 ms = 600 ms > 300 ms silence threshold.
        let mut seg = build(0, false);
        let events = run(&mut seg, &[0.9, 0.9, 0.0, 0.0, 0.0]);
        assert_eq!(segments(&events).len(), 1, "got {events:?}");
    }

    #[test]
    fn post_roll_bridges_short_silence_dip() {
        // 250 ms post-roll. A single silent probe (100 ms < 300 ms threshold)
        // must not even trigger the silence timeout.
        let mut seg = build(250, false);
        let events = run(
            &mut seg,
            &[0.9, 0.9, 0.0, 0.9, 0.9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        let segs = segments(&events);
        assert_eq!(starts(&events), 1, "got {events:?}");
        assert_eq!(segs.len(), 1, "got {events:?}");
    }

    #[test]
    fn post_roll_resumes_on_speech() {
        let mut seg = build(250, false);
        // confirm, silence timeout (3 probes = 300 ms → enter post-roll), then
        // speech again (cancel), then final silence timeout exhausting the
        // post-roll grace.
        let mut probs = vec![0.9, 0.9];
        probs.extend_from_slice(&[0.0; 3]);
        probs.extend_from_slice(&[0.9, 0.9]);
        probs.extend_from_slice(&[0.0; 6]);
        let events = run(&mut seg, &probs);
        assert_eq!(segments(&events).len(), 1, "got {events:?}");
    }

    #[test]
    fn accumulation_guard_discards() {
        // A single speech blip followed by more than the 10 s accumulation
        // budget never confirms and never emits events.
        let mut probs = vec![0.9f32];
        // 10 s = 100 probes of 100 ms; feed some margin beyond it.
        probs.extend(std::iter::repeat_n(0.0f32, 105));
        let mut seg = build(0, false);
        let events = run(&mut seg, &probs);
        assert_eq!(starts(&events), 0);
        assert!(segments(&events).is_empty());
    }

    #[test]
    fn repeated_utterances_emit_one_start_each() {
        let mut seg = build(0, false);
        let mut probs = Vec::new();
        // utterance 1
        probs.extend_from_slice(&[0.9, 0.9, 0.0, 0.0, 0.0]);
        // utterance 2 (needs 2 confirm probes again)
        probs.extend_from_slice(&[0.9, 0.9, 0.0, 0.0, 0.0]);
        let events = run(&mut seg, &probs);
        assert_eq!(starts(&events), 2, "got {events:?}");
        assert_eq!(segments(&events).len(), 2, "got {events:?}");
    }

    /// P1 (contract): SpeechStart must fire within the commit budget of the
    /// first above-threshold probe — the confirmation window (2-of-3) takes
    /// at most CONFIRM_WINDOW probes.
    #[test]
    fn speech_start_within_commit_budget() {
        let mut seg = build(0, false);
        // 5 silent probes, then the onset: speech, gap, speech → confirm on
        // the third probe after onset (the gap is tolerated).
        let mut probs = vec![0.0; 5];
        probs.extend_from_slice(&[0.9, 0.3, 0.9, 0.0, 0.0, 0.0]);
        let events = run(&mut seg, &probs);
        assert_eq!(starts(&events), 1, "got {events:?}");
        assert_eq!(
            seg.commit_budget_ms(),
            3 * seg.probe_samples() * 1000 / SAMPLE_RATE,
            "budget = confirm window × probe ms"
        );
    }

    /// P5 accounting: silence is charged at the detector's window resolution,
    /// so a probe that merely *contains* silence does not add a full
    /// `probe_ms`. Five half-silent probes (2 500 counted samples, with a
    /// 4 800-sample timeout) must keep the segment open; probe-granular
    /// counting would have charged 8 000 samples and closed it.
    #[test]
    fn silence_is_measured_at_window_resolution() {
        let speech = vec![0.9f32; 8];
        // 8 windows per probe, half of them below the 0.45 end threshold, and
        // an average (0.38) below it too.
        let mixed = vec![0.01, 0.01, 0.02, 0.03, 0.60, 0.70, 0.80, 0.90];
        let mut probes = vec![speech.clone(), speech];
        probes.extend(std::iter::repeat_n(mixed, 6));
        let mut seg = VadSegmenter::new(WindowDetector { probes, idx: 0 }, config(0, false))
            .expect("segmenter");
        let chunk = vec![0.1f32; seg.probe_samples()];

        let mut events = Vec::new();
        for _ in 0..2 {
            events.extend(seg.feed(&chunk)); // confirm
        }
        for _ in 0..5 {
            events.extend(seg.feed(&chunk)); // 5 × 800 samples = 4 000 < 4 800
        }
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, SegmenterEvent::SegmentReady { .. })),
            "half-silent probes must not close the segment: {events:?}"
        );
        assert!(seg.in_speech());

        events = seg.feed(&chunk); // 6th probe reaches 4 800 → timeout
        assert!(
            matches!(events.as_slice(), [SegmenterEvent::SegmentReady { .. }]),
            "the silence timeout must close the segment at 4 800 counted samples, got {events:?}"
        );
    }

    /// P5 hysteresis: a probe that is mostly speech-like breaks the run even
    /// when its quiet edges push the average under the end threshold, so a
    /// fading tail cannot add up to a timeout.
    #[test]
    fn mostly_speech_probe_breaks_the_silence_run() {
        let speech = vec![0.9f32; 8];
        // Average 0.39 (below 0.45) but only 3 of 8 windows are quiet.
        let quiet_edges = vec![0.01, 0.30, 0.44, 0.46, 0.47, 0.48, 0.49, 0.50];
        let mut probes = vec![speech.clone(), speech];
        probes.extend(std::iter::repeat_n(quiet_edges, 6));
        let mut seg = VadSegmenter::new(WindowDetector { probes, idx: 0 }, config(0, false))
            .expect("segmenter");
        let chunk = vec![0.1f32; seg.probe_samples()];

        let mut events = Vec::new();
        for _ in 0..8 {
            events.extend(seg.feed(&chunk));
        }
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, SegmenterEvent::SegmentReady { .. })),
            "a fading tail must not close the segment: {events:?}"
        );
        assert!(seg.in_speech());
    }

    /// The probe-granular fallback still applies to detectors that expose no
    /// window probabilities (contract P5 unchanged for scripted providers).
    #[test]
    fn silence_falls_back_to_probe_granularity() {
        let mut seg = build(0, false);
        let events = run(&mut seg, &[0.9, 0.9, 0.0, 0.0, 0.0]);
        assert_eq!(segments(&events).len(), 1, "got {events:?}");
    }

    /// A single isolated burst (cough/click) must fail to confirm even with
    /// the gap-tolerant window.
    #[test]
    fn isolated_burst_does_not_confirm() {
        let mut seg = build(0, false);
        let events = run(&mut seg, &[0.9, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(starts(&events), 0, "got {events:?}");
        assert!(segments(&events).is_empty());
    }

    /// A short inter-word gap (natural speech) must break through the confirm
    /// window: speech, gap, speech → confirmed at the 3rd probe.
    #[test]
    fn inter_word_gap_tolerated_by_confirm_window() {
        let mut seg = build(0, false);
        let events = run(&mut seg, &[0.9, 0.2, 0.9, 0.0, 0.0, 0.0]);
        assert_eq!(starts(&events), 1, "got {events:?}");
        assert_eq!(segments(&events).len(), 1, "got {events:?}");
    }

    /// P5 (contract): the finalized segment extends at least `silence_ms`
    /// past the last speech probe, so the utterance tail is fully included.
    #[test]
    fn segment_includes_trailing_silence() {
        let mut seg = build(0, false);
        let events = run(&mut seg, &[0.9, 0.9, 0.0, 0.0, 0.0]);
        let segs = segments(&events);
        assert_eq!(segs.len(), 1);
        let probe = seg.probe_samples();
        // zero pad (200 ms fallback, no pre-roll on the first probe) + 2 speech
        // probes + 3 closing silent probes (300 ms = silence threshold).
        let pad = SAMPLE_RATE * PRE_SILENCE_MS / 1000;
        assert_eq!(segs[0].len(), pad + 2 * probe + 3 * probe);
    }

    /// P4 detail: the pre-roll ring is capped at `preroll_ms`; long stretches
    /// of silence keep only the most recent window.
    #[test]
    fn preroll_is_capped() {
        let mut seg = build(0, false);
        let mut probs = vec![0.0; 10]; // 1 s of silence; ring keeps last 300 ms
        probs.extend_from_slice(&[0.9, 0.9, 0.0, 0.0, 0.0]);
        let events = run(&mut seg, &probs);
        let segs = segments(&events);
        assert_eq!(segs.len(), 1, "got {events:?}");
        let probe = seg.probe_samples();
        // Pre-roll (3 probes of real audio) + 2 speech + 3 closing probes.
        assert_eq!(segs[0].len(), 3 * probe + 2 * probe + 3 * probe);
        // The segment opens with real pre-roll audio (amplitude 0.1), zeros
        // only appear via the empty-ring fallback.
        assert!(
            segs[0][..3 * probe].iter().all(|&s| (s - 0.1).abs() < 1e-6),
            "pre-roll must carry real audio"
        );
    }

    /// Hysteresis (fallo 3): probes between the end and start thresholds are
    /// speech-like enough to keep the segment open; only `silence_ms` of
    /// below-end-threshold audio closes it.
    #[test]
    fn noise_between_thresholds_keeps_segment_open() {
        let mut seg = build(0, false);
        let mut probs = vec![0.9, 0.9];
        probs.extend(std::iter::repeat_n(0.5, 30)); // 3 s of 0.45–0.65 noise
        let events = run(&mut seg, &probs);
        assert_eq!(starts(&events), 1, "got {events:?}");
        assert!(
            segments(&events).is_empty(),
            "noise must keep the segment open"
        );
        assert!(seg.in_speech());
        assert!(!seg.closing_silence_started());
    }

    /// P5 (contract): only VAD silence closes a segment. Continuous speech
    /// longer than any previous fixed cap stays one open utterance until the
    /// user stops speaking, and the finalized audio keeps every spoken probe.
    #[test]
    fn continuous_speech_is_not_cut_before_silence() {
        let mut seg = build(0, false);
        // 30 s of unbroken speech (100 ms test probes) — longer than the old
        // 20 s hard cap.
        let mut probs = vec![0.9f32; 300];
        let events = run(&mut seg, &probs);
        assert!(
            segments(&events).is_empty(),
            "speech must not be cut mid-utterance, got {events:?}"
        );
        assert!(seg.in_speech(), "the segment must still be open");

        // The user stops: only then does the silence timeout close it.
        probs.clear();
        probs.extend_from_slice(&[0.0, 0.0, 0.0]);
        let events = run(&mut seg, &probs);
        let segs = segments(&events);
        assert_eq!(segs.len(), 1, "got {events:?}");
        assert!(
            segs[0].len() >= SAMPLE_RATE * 30,
            "the finalized segment must keep all 30 s of speech, got {:.2}s",
            segs[0].len() as f32 / SAMPLE_RATE as f32
        );
        assert!(!seg.in_speech(), "the segment must be closed");
    }

    /// Segment ids are monotonic and every SegmentReady echoes the seq of the
    /// SpeechStart it closes (P2 pairing).
    #[test]
    fn segment_seq_monotonic_and_paired() {
        let mut seg = build(0, false);
        let mut probs = Vec::new();
        probs.extend_from_slice(&[0.9, 0.9, 0.0, 0.0, 0.0]);
        probs.extend_from_slice(&[0.9, 0.9, 0.0, 0.0, 0.0]);
        let events = run(&mut seg, &probs);
        let starts: Vec<u64> = events
            .iter()
            .filter_map(|e| match e {
                SegmenterEvent::SpeechStart { segment_seq } => Some(*segment_seq),
                _ => None,
            })
            .collect();
        let ends: Vec<u64> = events
            .iter()
            .filter_map(|e| match e {
                SegmenterEvent::SegmentReady { segment_seq, .. } => Some(*segment_seq),
                _ => None,
            })
            .collect();
        assert_eq!(starts, vec![1, 2]);
        assert_eq!(ends, vec![1, 2]);
    }

    #[test]
    fn trim_removes_quiet_prefix() {
        let quiet: Vec<f32> = vec![0.0; TRIM_WINDOW_SAMPLES * 3];
        let speech: Vec<f32> = vec![0.5; TRIM_WINDOW_SAMPLES * 5];
        let audio = [quiet, speech].concat();
        let trimmed = trim_leading_silence(&audio);
        assert!(trimmed.len() < audio.len());
        assert!(trimmed.len() >= TRIM_WINDOW_SAMPLES * 5);
    }

    #[test]
    fn trim_keeps_all_when_no_silence() {
        let audio: Vec<f32> = vec![0.5; TRIM_WINDOW_SAMPLES * 10];
        assert_eq!(trim_leading_silence(&audio), audio);
    }

    #[test]
    fn trim_keeps_at_least_ten_percent() {
        let audio: Vec<f32> = vec![0.0; TRIM_WINDOW_SAMPLES * 100];
        let trimmed = trim_leading_silence(&audio);
        assert!(trimmed.len() >= audio.len() / 10);
    }

    #[test]
    fn rejects_out_of_range_thresholds() {
        let cfg = SegmenterConfig {
            vad_start_threshold: 1.5,
            ..config(0, false)
        };
        assert!(VadSegmenter::new(ScriptedDetector::new(&[]), cfg).is_err());
    }

    #[test]
    fn clamps_end_threshold_above_start() {
        let cfg = SegmenterConfig {
            vad_start_threshold: 0.4,
            vad_end_threshold: 0.9,
            ..config(0, false)
        };
        let seg = VadSegmenter::new(ScriptedDetector::new(&[]), cfg).unwrap();
        assert_eq!(seg.vad_end_threshold, 0.4);
    }

    /// Quiet captures are boosted to a sane peak so Silero keeps reporting
    /// speech; loud ones and near-silence are left alone.
    #[test]
    fn vad_input_gain_normalizes_quiet_probes() {
        let approx = |a: f32, b: f32| (a - b).abs() <= b * 1e-3;
        assert_eq!(vad_input_gain(0.5), 1.0, "normal level is untouched");
        assert_eq!(vad_input_gain(0.1), 1.0, "at the threshold, no boost");
        assert!(approx(vad_input_gain(0.02), 25.0), "0.02 → 0.5");
        assert!(approx(vad_input_gain(0.01), 50.0), "0.01 → 0.5");
        assert!(
            approx(vad_input_gain(0.001), VAD_MAX_GAIN),
            "cap keeps silence quiet"
        );
        assert_eq!(vad_input_gain(0.0), 1.0, "digital silence is untouched");
    }
}
