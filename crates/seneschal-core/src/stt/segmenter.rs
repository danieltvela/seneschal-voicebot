//! Shared local VAD + utterance segmentation for the STT providers.
//!
//! Whisper, Parakeet and the remote OpenAI-compatible provider (issue #17) all
//! need the same thing: turn a stream of 16 kHz mono f32 chunks into
//! `SpeechStart` / utterance-closed events using Silero VAD, with a
//! consecutive-probe confirmation window, a silence timeout, a real-audio
//! pre-roll so the recognizer never loses the onset, and a hard cap on
//! segment length.
//!
//! Contract properties locked by tests (see `doc/stt-contract.md`):
//! P1 speech-start commit budget, P4 onset preservation via pre-roll, and the
//! hysteresis behavior that keeps soft tails and mid-sentence dips inside one
//! segment.
//!
//! Before this module each provider carried its own copy of that state machine.
//! It now lives here once. The three consumers are configured to share the
//! same defaults (uniform contract):
//!
//! | Provider  | probe | pre-roll        | post-roll |
//! |-----------|-------|-----------------|-----------|
//! | whisper   | 100ms | 300ms real audio| 200ms     |
//! | parakeet  | 100ms | 300ms real audio| 200ms     |
//! | openai    | 100ms | 300ms real audio| 200ms     |
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

/// Default VAD probe window. Silero prefers 20–200 ms windows; 100 ms keeps
/// the start-of-speech commit budget low (P1: ~2 probes ≈ 200 ms) without
/// hurting accuracy. Configurable via `VAD_PROBE_MS`.
pub const DEFAULT_PROBE_MS: usize = 100;

/// Default real-audio pre-roll prepended to a confirmed speech segment
/// (`VAD_PREROLL_MS`). Probes whose speech probability never crossed the start
/// threshold still carry the utterance onset; without this ring buffer that
/// audio was discarded and the first word was mistranscribed.
pub const DEFAULT_PREROLL_MS: usize = 300;

/// Fallback pad of zeros when the pre-roll ring is empty (speech starts on the
/// very first probe of the stream). Gives the recognizer acoustic context
/// before an abrupt audio start.
pub const PRE_SILENCE_MS: usize = 200;

/// Hard cap on a single speech segment before forcing a cut.
const MAX_SEGMENT_MS: usize = 20_000;
pub(crate) const MAX_SEGMENT_SAMPLES: usize = SAMPLE_RATE * MAX_SEGMENT_MS / 1000;

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
}

/// [`SpeechDetector`] backed by the Silero VAD model via `whisper-cpp-plus`.
pub struct SileroDetector {
    vad: WhisperVadProcessor,
}

impl SileroDetector {
    pub fn new(vad: WhisperVadProcessor) -> Self {
        Self { vad }
    }
}

impl SpeechDetector for SileroDetector {
    fn probe(&mut self, chunk: &[f32]) -> f32 {
        if !self.vad.detect_speech(chunk) {
            return 0.0;
        }
        let probs = self.vad.get_probs();
        if probs.is_empty() {
            0.0
        } else {
            probs.iter().sum::<f32>() / probs.len() as f32
        }
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
    /// A finalized utterance ready to be transcribed.
    SegmentReady { segment_seq: u64, audio: Vec<f32> },
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

    // Confirmation window: require N consecutive speech probes before committing.
    consecutive_speech_probes: usize,
    accumulating: bool,
    accum_buf: Vec<f32>,
    accum_samples_total: usize,

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
            speech_buf: Vec::with_capacity(MAX_SEGMENT_SAMPLES),
            silence_samples: 0,
            post_roll_remaining: 0,
            pre_roll: Vec::with_capacity(SAMPLE_RATE * config.preroll_ms / 1000),
            consecutive_speech_probes: 0,
            accumulating: false,
            accum_buf: Vec::new(),
            accum_samples_total: 0,
            probe_carry: Vec::with_capacity(probe_samples),
        })
    }

    /// Stream-time budget of the start commit: probes needed to confirm, in
    /// ms. Locked by the P1 test.
    pub fn commit_budget_ms(&self) -> usize {
        self.probe_samples * self.vad_confirm_probes.max(1) * 1000 / SAMPLE_RATE
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
        self.in_speech && !self.in_post_roll && self.silence_samples > 0
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
                let silence = avg_prob < self.vad_end_threshold;
                if silence {
                    self.post_roll_remaining = self.post_roll_remaining.saturating_sub(chunk.len());
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
                let silence = avg_prob < self.vad_end_threshold;

                if silence {
                    self.silence_samples += chunk.len();
                } else {
                    self.silence_samples = 0;
                }

                let max_segment_reached = self.speech_buf.len() >= MAX_SEGMENT_SAMPLES;
                let silence_timeout = self.silence_samples >= self.silence_samples_threshold;

                if max_segment_reached {
                    self.finalize_segment(events);
                } else if silence_timeout {
                    if self.post_roll_samples == 0 {
                        self.finalize_segment(events);
                    } else {
                        self.in_post_roll = true;
                        self.post_roll_remaining = self.post_roll_samples;
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
                self.consecutive_speech_probes = 0;
                self.accum_samples_total = 0;
                self.accum_buf.clear();
            }

            let is_speech = avg_prob >= self.vad_start_threshold;

            if is_speech {
                self.consecutive_speech_probes += 1;

                if self.consecutive_speech_probes >= self.vad_confirm_probes {
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
                    tracing::debug!(
                        target: "sttvad",
                        "Speech confirmed after {} probes ({:.1}s, pre-roll {}ms)",
                        self.vad_confirm_probes,
                        self.speech_buf.len() as f32 / SAMPLE_RATE as f32,
                        preroll_len * 1000 / SAMPLE_RATE
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
            // ── Silence: keep the real-audio pre-roll ring up to date, then
            // start accumulating if speech detected. The probe that starts
            // accumulation must NOT enter the ring: it is already the first
            // entry of accum_buf.
            let is_speech = avg_prob >= self.vad_start_threshold;

            if is_speech {
                self.accumulating = true;
                self.consecutive_speech_probes = 1;
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

    /// Append a non-speech probe to the pre-roll ring, dropping the oldest
    /// samples beyond the configured window.
    fn push_pre_roll(&mut self, chunk: &[f32]) {
        self.pre_roll.extend_from_slice(chunk);
        let overflow = self.pre_roll.len().saturating_sub(self.preroll_samples);
        if overflow > 0 {
            self.pre_roll.drain(..overflow);
        }
    }

    fn finalize_segment(&mut self, events: &mut Vec<SegmenterEvent>) {
        let audio = std::mem::take(&mut self.speech_buf);
        self.in_speech = false;
        self.in_post_roll = false;
        self.silence_samples = 0;
        self.post_roll_remaining = 0;
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
    /// first above-threshold probe — `vad_confirm_probes` probes at most.
    /// With 100 ms probes and confirm=2 the commit lands ~200 ms after onset
    /// (the old 200 ms probes pushed this to 400–600 ms).
    #[test]
    fn speech_start_within_commit_budget() {
        let mut seg = build(0, false);
        // 5 silent probes, then the onset.
        let mut probs = vec![0.0; 5];
        probs.extend_from_slice(&[0.9, 0.9, 0.0, 0.0, 0.0]);
        let events = run(&mut seg, &probs);
        assert_eq!(starts(&events), 1, "got {events:?}");
        let budget_probes = seg.vad_confirm_probes.max(1);
        assert_eq!(
            seg.commit_budget_ms(),
            budget_probes * 100,
            "budget = confirm probes × probe ms"
        );
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

    /// The 20 s hard cap still forces a cut on endless speech.
    #[test]
    fn max_segment_cap_cuts() {
        let mut seg = build(0, false);
        let mut probs = vec![0.9, 0.9];
        probs.extend(std::iter::repeat_n(0.9, 250)); // > 20 s of speech
        let events = run(&mut seg, &probs);
        let segs = segments(&events);
        assert!(
            !segs.is_empty(),
            "hard cap must close the segment before 21 s"
        );
        assert!(
            segs[0].len() >= SAMPLE_RATE * 19,
            "first cut must land near the 20 s cap, got {:.2}s",
            segs[0].len() as f32 / SAMPLE_RATE as f32
        );
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
}
