//! Shared orchestration for the local (in-process) STT providers.
//!
//! Whisper and Parakeet run the identical pipeline: a shared
//! [`VadSegmenter`] turns the audio stream into `SpeechStart` /
//! `SegmentReady` events, a [`LiveStreamer`] re-decodes the open segment
//! while the user speaks (contract P3), and the close hands the final
//! decode to a detached task (contract P4). Only the decoder differs —
//! whisper.cpp vs the Parakeet ONNX model — so the orchestration lives
//! here once, generic over a [`LocalTranscriber`].
//!
//! Out of scope by design:
//! - the macOS `speech` provider streams audio to Apple and runs its own
//!   state machine (`speech_recognizer.rs`);
//! - the remote `openai` provider keeps its own live path (HTTP requests
//!   with cancellation tokens, `openai.rs`).
//!
//! Generic over the [`SpeechDetector`] so the orchestration is
//! unit-testable without loading Silero or any model (see `tests`).

use anyhow::Result;
use tokio::sync::mpsc;

use super::SpeechEvent;
use super::no_speech_gate::TranscriptionQuality;
use super::segmenter::{SAMPLE_RATE, SegmenterEvent, SileroDetector, SpeechDetector, VadSegmenter};
use super::streaming::{LiveDecoder, LiveStreamConfig, LiveStreamer};

/// Blocking decoder over one snapshot of the open segment, cheap to clone
/// (every spawned task needs an owned handle). Production impls wrap the
/// model context in an `Arc`, so the clone is a pointer bump.
///
/// Blanket-implemented: every `LiveDecoder + Clone + 'static` type is a
/// `LocalTranscriber`; providers just derive `Clone` on their decoder.
pub trait LocalTranscriber: LiveDecoder + Clone + 'static {}

impl<T: LiveDecoder + Clone + 'static> LocalTranscriber for T {}

/// VAD segmentation + live transcription + final decode hand-off, shared by
/// every local provider.
pub struct LocalSttProvider<T, D = SileroDetector>
where
    T: LocalTranscriber,
    D: SpeechDetector,
{
    segmenter: VadSegmenter<D>,
    streamer: LiveStreamer,
    transcriber: T,
}

impl<T, D> LocalSttProvider<T, D>
where
    T: LocalTranscriber,
    D: SpeechDetector,
{
    /// Assemble from a ready segmenter (the caller loads the detector's
    /// model), the blocking decoder and the live cadence config.
    pub fn new(segmenter: VadSegmenter<D>, transcriber: T, live: LiveStreamConfig) -> Self {
        Self {
            segmenter,
            streamer: LiveStreamer::new(live),
            transcriber,
        }
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
                    self.streamer.finish_detached(
                        segment_seq,
                        audio,
                        speech_end_offset,
                        self.transcriber.clone(),
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
            self.transcriber.clone(),
        );

        Ok(())
    }

    /// Blocking one-shot transcription (used as a fallback / sanity check).
    pub fn transcribe_complete(&self, audio: &[f32]) -> Result<TranscriptionQuality> {
        self.transcriber.decode(audio.to_vec())
    }

    /// Number of decode tasks currently running (observability / tests).
    pub fn inflight_transcriptions(&self) -> usize {
        self.streamer.inflight()
    }
}

#[cfg(test)]
mod tests {
    use super::super::segmenter::SegmenterConfig;
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// Detector that pops one scripted probability per probe — same shape as
    /// the segmenter's unit-test fake, but owned via `Arc<Mutex<_>>` so the
    /// provider can hold it behind the `SpeechDetector: Send` bound.
    struct ScriptedDetector {
        probs: Arc<std::sync::Mutex<Vec<f32>>>,
    }

    impl ScriptedDetector {
        fn new(probs: &[f32]) -> Self {
            Self {
                probs: Arc::new(std::sync::Mutex::new(probs.to_vec())),
            }
        }
    }

    impl SpeechDetector for ScriptedDetector {
        fn probe(&mut self, _chunk: &[f32]) -> f32 {
            let mut probs = self.probs.lock().unwrap();
            if probs.is_empty() {
                0.0
            } else {
                probs.remove(0)
            }
        }
    }

    /// Blocking fake decoder that always returns the same text, sleeps
    /// `delay_ms` per decode and counts invocations.
    #[derive(Clone)]
    struct FakeTranscriber {
        text: &'static str,
        delay_ms: u64,
        calls: Arc<AtomicUsize>,
    }

    impl LiveDecoder for FakeTranscriber {
        fn decode(&self, _audio: Vec<f32>) -> Result<TranscriptionQuality> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(self.delay_ms));
            Ok(TranscriptionQuality {
                text: self.text.to_string(),
                no_speech_prob: 0.0,
                avg_logprob: 0.0,
                compression_ratio: 0.0,
            })
        }
    }

    fn config() -> SegmenterConfig {
        SegmenterConfig {
            silence_ms: 300,
            vad_start_threshold: 0.65,
            vad_end_threshold: 0.45,
            vad_confirm_probes: 2,
            post_roll_ms: 0,
            trim_leading_silence: false,
            probe_ms: 100,
            preroll_ms: 300,
        }
    }

    fn provider(
        probs: &[f32],
        text: &'static str,
        live: LiveStreamConfig,
    ) -> (
        LocalSttProvider<FakeTranscriber, ScriptedDetector>,
        Arc<AtomicUsize>,
    ) {
        let segmenter = VadSegmenter::new(ScriptedDetector::new(probs), config()).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let transcriber = FakeTranscriber {
            text,
            delay_ms: 0,
            calls: Arc::clone(&calls),
        };
        (LocalSttProvider::new(segmenter, transcriber, live), calls)
    }

    /// One probe of 100 ms of audio (matches the test config's probe_ms).
    fn probe_chunk(seg: &LocalSttProvider<FakeTranscriber, ScriptedDetector>) -> Vec<f32> {
        vec![0.1f32; seg.segmenter.probe_samples()]
    }

    async fn next_event(rx: &mut mpsc::Receiver<SpeechEvent>) -> SpeechEvent {
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("event must arrive")
            .expect("channel open")
    }

    /// The orchestration forwards the segmenter's Start → Final pairing with
    /// the same `segment_seq`, and the final decode runs exactly once.
    #[tokio::test]
    async fn start_and_final_share_the_segment_seq() {
        // Onset + confirm + 3 closing silent probes (300 ms threshold).
        let (mut p, calls) = provider(
            &[0.9, 0.9, 0.0, 0.0, 0.0],
            "hola",
            LiveStreamConfig {
                enabled: false,
                ..Default::default()
            },
        );
        let (tx, mut rx) = mpsc::channel(16);
        let chunk = probe_chunk(&p);

        for _ in 0..4 {
            p.process_audio(&chunk, &tx).await.unwrap();
        }
        let first = next_event(&mut rx).await;
        assert!(
            matches!(first, SpeechEvent::SpeechStart { segment_seq: 1 }),
            "expected SpeechStart(1), got {first:?}"
        );

        // The 5th probe reaches the silence threshold and closes the segment;
        // the final decode is detached, hence the timeout-bounded recv.
        p.process_audio(&chunk, &tx).await.unwrap();
        match next_event(&mut rx).await {
            SpeechEvent::SpeechEnd {
                segment_seq,
                quality,
            } => {
                assert_eq!(segment_seq, 1, "final must echo the start's seq");
                assert_eq!(quality.text, "hola");
            }
            other => panic!("expected SpeechEnd, got {other:?}"),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one final decode only");
    }

    /// An unconfirmed onset (cough/click) emits the paired empty `SpeechEnd`
    /// and never runs a decode (contract P1's abort path).
    #[tokio::test]
    async fn aborted_onset_emits_empty_end_without_decode() {
        // One speech probe, then a full silence window: abort.
        let (mut p, calls) = provider(
            &[0.9, 0.0, 0.0, 0.0],
            "hola",
            LiveStreamConfig {
                enabled: false,
                ..Default::default()
            },
        );
        let (tx, mut rx) = mpsc::channel(16);
        let chunk = probe_chunk(&p);

        for _ in 0..4 {
            p.process_audio(&chunk, &tx).await.unwrap();
        }

        let first = next_event(&mut rx).await;
        assert!(
            matches!(first, SpeechEvent::SpeechStart { segment_seq: 1 }),
            "onset must fire SpeechStart, got {first:?}"
        );
        match next_event(&mut rx).await {
            SpeechEvent::SpeechEnd {
                segment_seq,
                quality,
            } => {
                assert_eq!(segment_seq, 1);
                assert!(quality.text.is_empty(), "abort closes with empty text");
            }
            other => panic!("expected empty SpeechEnd, got {other:?}"),
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "an aborted onset must not decode"
        );
    }

    /// With live enabled, the orchestration emits partials from the in-flight
    /// decode and the final from the authoritative close decode (contract
    /// P3/P4 through one code path). The decoder sleeps so the first flight
    /// finishes between feeds and `poll` delivers its partial before the
    /// close.
    #[tokio::test]
    async fn live_partial_then_final_through_one_pipeline() {
        let calls = Arc::new(AtomicUsize::new(0));
        let transcriber = FakeTranscriber {
            text: "hola",
            delay_ms: 30,
            calls: Arc::clone(&calls),
        };
        let segmenter =
            VadSegmenter::new(ScriptedDetector::new(&[0.9, 0.9, 0.0, 0.0, 0.0]), config()).unwrap();
        // Interval 250 ms: after the first partial (buffer 200 ms), the next
        // spawn needs 250 ms of NEW audio — more than the closing silence
        // provides, so exactly one live decode runs.
        let mut p = LocalSttProvider::new(
            segmenter,
            transcriber,
            LiveStreamConfig {
                enabled: true,
                interval_ms: 250,
                window_ms: 15_000,
            },
        );
        let (tx, mut rx) = mpsc::channel(16);
        let chunk = vec![0.1f32; p.segmenter.probe_samples()];

        // Probe 1: onset (SpeechStart). Probe 2: confirmation with 200 ms of
        // new audio — a live decode spawns and sleeps 30 ms.
        p.process_audio(&chunk, &tx).await.unwrap();
        p.process_audio(&chunk, &tx).await.unwrap();
        let first = next_event(&mut rx).await;
        assert!(
            matches!(first, SpeechEvent::SpeechStart { .. }),
            "{first:?}"
        );

        // Let the flight finish, then keep feeding: the next poll delivers
        // its partial (contract P3).
        tokio::time::sleep(Duration::from_millis(60)).await;
        // Probes 3–5: closing silence (3 × 100 ms = the 300 ms threshold) →
        // the segment closes and the authoritative full decode runs (the
        // flight only covered 200 ms of the 500 ms spoken+preroll window).
        for _ in 0..3 {
            p.process_audio(&chunk, &tx).await.unwrap();
        }

        let mut saw_partial = false;
        loop {
            match next_event(&mut rx).await {
                SpeechEvent::Speech(text) => {
                    assert_eq!(text, "hola", "live partial");
                    saw_partial = true;
                }
                SpeechEvent::SpeechEnd {
                    segment_seq,
                    quality,
                } => {
                    assert_eq!(segment_seq, 1);
                    assert_eq!(quality.text, "hola");
                    break;
                }
                other => panic!("unexpected event {other:?}"),
            }
        }
        assert!(saw_partial, "live partial must precede the final");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "one live decode + one final decode"
        );
    }

    /// The blocking one-shot path goes through the same decoder.
    #[tokio::test]
    async fn transcribe_complete_uses_the_decoder() {
        let (p, calls) = provider(
            &[],
            "directo",
            LiveStreamConfig {
                enabled: false,
                ..Default::default()
            },
        );
        let quality = p.transcribe_complete(&[0.1f32; 1600]).unwrap();
        assert_eq!(quality.text, "directo");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
