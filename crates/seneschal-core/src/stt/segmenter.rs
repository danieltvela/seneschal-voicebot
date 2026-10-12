//! Shared local VAD + utterance segmentation for the STT providers.
//!
//! Whisper, Parakeet and the remote OpenAI-compatible provider (issue #17) all
//! need the same thing: turn a stream of 16 kHz mono f32 chunks into
//! `SpeechStart` / utterance-closed events using Silero VAD, with a
//! consecutive-probe confirmation window, a silence timeout, a pre-silence pad
//! so the recognizer has acoustic context, and a hard cap on segment length.
//!
//! Before this module each provider carried its own copy of that state machine.
//! It now lives here once. The three consumers are configured to preserve their
//! historical behavior exactly:
//!
//! | Provider  | `post_roll_ms` | `trim_leading_silence` |
//! |-----------|----------------|------------------------|
//! | whisper   | 0 (disabled)   | false                  |
//! | parakeet  | 250            | true                   |
//! | openai    | 0 (disabled)   | false                  |
//!
//! `speech_recognizer.rs` (macOS SFSpeechRecognizer) intentionally keeps its own
//! logic: it streams audio to Apple and uses a different probe cadence
//! (100 ms) plus a short-utterance fallback, and it only compiles on macOS.
//!
//! The state machine is platform- and model-agnostic: speech detection is
//! injected through [`SpeechDetector`], so the logic is unit-testable without
//! loading Silero or any STT model.

use anyhow::{Result, ensure};
use whisper_cpp_plus::WhisperVadProcessor;

/// Pipeline sample rate (16 kHz mono f32).
pub const SAMPLE_RATE: usize = 16_000;

/// Probe size for the VAD. Silero prefers 20–200 ms windows; 200 ms gives good
/// accuracy without adding too much latency.
pub const VAD_PROBE_MS: usize = 200;
pub const VAD_PROBE_SAMPLES: usize = SAMPLE_RATE * VAD_PROBE_MS / 1000;

/// Silence prepended to the first speech segment so the recognizer has acoustic
/// context before the onset. Without this, the first word is often
/// mistranscribed because the model sees an abrupt audio start.
pub const PRE_SILENCE_MS: usize = 200;
pub const PRE_SILENCE_SAMPLES: usize = SAMPLE_RATE * PRE_SILENCE_MS / 1000;

/// Hard cap on a single speech segment before forcing a cut.
const MAX_SEGMENT_MS: usize = 20_000;
const MAX_SEGMENT_SAMPLES: usize = SAMPLE_RATE * MAX_SEGMENT_MS / 1000;

/// Maximum probes to accumulate before giving up on confirmation.
/// At 200 ms per probe, 50 probes = 10 seconds. Prevents unbounded growth when
/// the user says something very short and the VAD never confirms.
const MAX_ACCUM_PROBES: usize = 50;

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
}

/// Events emitted by [`VadSegmenter::feed`].
#[derive(Debug, Clone, PartialEq)]
pub enum SegmenterEvent {
    /// VAD committed to a speech segment (drives barge-in / LISTENING).
    SpeechStart,
    /// A finalized utterance ready to be transcribed.
    SegmentReady(Vec<f32>),
}

/// VAD-driven utterance segmenter, generic over the speech detector so the
/// state machine can be exercised without models.
pub struct VadSegmenter<D: SpeechDetector> {
    detector: D,
    vad_start_threshold: f32,
    vad_end_threshold: f32,
    silence_samples_threshold: usize,
    post_roll_samples: usize,
    trim: bool,
    vad_confirm_probes: usize,

    // State machine
    in_speech: bool,
    in_post_roll: bool,
    speech_buf: Vec<f32>,
    silence_samples: usize,
    post_roll_remaining: usize,

    // Confirmation window: require N consecutive speech probes before committing.
    consecutive_speech_probes: usize,
    accumulating: bool,
    accum_buf: Vec<f32>,
    accum_probes_total: usize,

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

        Ok(Self {
            detector,
            vad_start_threshold: config.vad_start_threshold,
            vad_end_threshold,
            silence_samples_threshold: SAMPLE_RATE * config.silence_ms as usize / 1000,
            post_roll_samples: SAMPLE_RATE * config.post_roll_ms / 1000,
            trim: config.trim_leading_silence,
            vad_confirm_probes: config.vad_confirm_probes.max(1),
            in_speech: false,
            in_post_roll: false,
            speech_buf: Vec::with_capacity(MAX_SEGMENT_SAMPLES),
            silence_samples: 0,
            post_roll_remaining: 0,
            consecutive_speech_probes: 0,
            accumulating: false,
            accum_buf: Vec::new(),
            accum_probes_total: 0,
            probe_carry: Vec::with_capacity(VAD_PROBE_SAMPLES),
        })
    }

    /// Feed a chunk of 16 kHz mono f32 audio and collect the events it produced.
    pub fn feed(&mut self, audio: &[f32]) -> Vec<SegmenterEvent> {
        let mut events = Vec::new();
        if audio.is_empty() {
            return events;
        }

        self.probe_carry.extend_from_slice(audio);
        while self.probe_carry.len() >= VAD_PROBE_SAMPLES {
            let chunk: Vec<f32> = self.probe_carry.drain(..VAD_PROBE_SAMPLES).collect();
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

            let is_speech = avg_prob >= self.vad_start_threshold;

            if is_speech {
                self.consecutive_speech_probes += 1;

                if self.consecutive_speech_probes >= self.vad_confirm_probes {
                    // CONFIRMED: transition to speech state.
                    self.in_speech = true;
                    self.accumulating = false;
                    self.speech_buf.clear();
                    self.speech_buf
                        .extend(std::iter::repeat_n(0.0f32, PRE_SILENCE_SAMPLES));
                    self.speech_buf.append(&mut self.accum_buf);
                    events.push(SegmenterEvent::SpeechStart);
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
            let is_speech = avg_prob >= self.vad_start_threshold;

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
    }

    fn finalize_segment(&mut self, events: &mut Vec<SegmenterEvent>) {
        let audio = std::mem::take(&mut self.speech_buf);
        self.in_speech = false;
        self.in_post_roll = false;
        self.silence_samples = 0;
        self.post_roll_remaining = 0;

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
            "Finalizing segment: {:.2}s",
            segment.len() as f32 / SAMPLE_RATE as f32
        );
        events.push(SegmenterEvent::SegmentReady(segment));
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
        }
    }

    /// Feed one speech/silence probe (with the given probability) per call.
    fn run(seg: &mut VadSegmenter<ScriptedDetector>, probs: &[f32]) -> Vec<SegmenterEvent> {
        let chunk = vec![0.1f32; VAD_PROBE_SAMPLES];
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
            .filter(|e| matches!(e, SegmenterEvent::SpeechStart))
            .count()
    }

    fn segments(events: &[SegmenterEvent]) -> Vec<Vec<f32>> {
        events
            .iter()
            .filter_map(|e| match e {
                SegmenterEvent::SegmentReady(a) => Some(a.clone()),
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
        // confirm (2 probes) then close with silence (300 ms / 200 ms probe = 2 probes).
        let mut seg = build(0, false);
        let events = run(&mut seg, &[0.9, 0.9, 0.0, 0.0]);
        let segs = segments(&events);
        assert_eq!(segs.len(), 1);
        // accum_buf (2 probes) + 200 ms pre-silence + 2 closing probes.
        assert!(
            segs[0].len() >= PRE_SILENCE_SAMPLES,
            "expected pre-silence pad, got {} samples",
            segs[0].len()
        );
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
        // 250 ms post-roll = 4000 samples grace. A single silent probe
        // (200 ms < 300 ms threshold) must not even trigger the silence timeout.
        let mut seg = build(250, false);
        let events = run(&mut seg, &[0.9, 0.9, 0.0, 0.9, 0.9, 0.0, 0.0, 0.0, 0.0]);
        let segs = segments(&events);
        assert_eq!(starts(&events), 1, "got {events:?}");
        assert_eq!(segs.len(), 1, "got {events:?}");
    }

    #[test]
    fn post_roll_resumes_on_speech() {
        let mut seg = build(250, false);
        // confirm, silence timeout (enter post-roll), then speech again (cancel),
        // then final silence timeout.
        let events = run(
            &mut seg,
            &[0.9, 0.9, 0.0, 0.0, 0.9, 0.9, 0.0, 0.0, 0.0, 0.0],
        );
        assert_eq!(segments(&events).len(), 1, "got {events:?}");
    }

    #[test]
    fn accumulation_guard_discards() {
        // A single speech blip followed by far more than MAX_ACCUM_PROBES probes
        // never confirms and never emits events.
        let mut probs = vec![0.9f32];
        probs.extend(std::iter::repeat_n(0.0f32, MAX_ACCUM_PROBES + 5));
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
