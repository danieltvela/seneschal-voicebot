//! Live (streaming) transcription while a VAD segment is open.
//!
//! Contract P3 (`doc/stt-contract.md`): the utterance must be transcribed
//! progressively while the user speaks — never buffered whole and decoded only
//! after the segment closes. This orchestrator runs on top of the
//! [`VadSegmenter`]: while the segment is open it periodically re-decodes the
//! buffered speech (bounded window, single flight) and the provider forwards
//! the results as `SpeechEvent::Speech` partials.
//!
//! At segment close ([`LiveStreamer::finish`]) a covering flight — one that
//! already transcribed every spoken sample — is reused as the final text, so
//! the LLM can launch without paying a second full decode. When no flight
//! covers the speech, the authoritative full decode runs and the pipeline
//! waits for it (contract P4).
//!
//! `enabled: false` restores the historical batch-at-close behavior exactly:
//! no partial decodes, one full decode at close.

use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::SpeechEvent;
use super::no_speech_gate::TranscriptionQuality;
use super::segmenter::SAMPLE_RATE;

/// Tunables of the live transcription cadence.
#[derive(Debug, Clone)]
pub struct LiveStreamConfig {
    /// Master switch (`STT_LIVE_ENABLED`).
    pub enabled: bool,
    /// Minimum new segment audio (ms) between partial decodes
    /// (`STT_LIVE_INTERVAL_MS`).
    pub interval_ms: u64,
    /// Cap on the audio length per partial decode (`STT_LIVE_WINDOW_MS`);
    /// snapshots decode the tail beyond this.
    pub window_ms: u64,
}

impl Default for LiveStreamConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_ms: 1000,
            window_ms: 15_000,
        }
    }
}

/// Blocking decoder over one snapshot of the open segment. Providers implement
/// this over their model handle; the orchestrator runs it on the blocking
/// pool. Cheap to construct per spawn (it must be an owned handle).
pub trait LiveDecoder: Send + 'static {
    fn decode(&self, audio: Vec<f32>) -> Result<TranscriptionQuality>;
}

struct Flight {
    segment_seq: u64,
    /// Segment samples covered by the decoded snapshot (buffer length at
    /// spawn time).
    covered_len: usize,
    /// True when the snapshot contained the whole open segment buffer.
    full: bool,
    handle: JoinHandle<Result<TranscriptionQuality>>,
}

/// Decrements the in-flight counter when the decode task finishes (any path).
/// Owned by the spawned closure so a dropped/unawaited handle still settles.
struct InflightGuard(Arc<AtomicUsize>);

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Orchestrates partial transcriptions while the user is speaking.
pub struct LiveStreamer {
    cfg: LiveStreamConfig,
    interval_samples: usize,
    window_samples: usize,
    /// Samples of the current segment covered by the last completed decode.
    decoded_upto: usize,
    /// Seq of the segment the streamer is attached to (0 = none yet).
    seq: u64,
    in_flight: Option<Flight>,
    /// Number of decode tasks currently running (any segment).
    inflight: Arc<AtomicUsize>,
}

impl LiveStreamer {
    pub fn new(cfg: LiveStreamConfig) -> Self {
        Self {
            interval_samples: SAMPLE_RATE * cfg.interval_ms.max(1) as usize / 1000,
            window_samples: SAMPLE_RATE * cfg.window_ms.max(1) as usize / 1000,
            cfg,
            decoded_upto: 0,
            seq: 0,
            in_flight: None,
            inflight: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Number of decode tasks currently running. Providers surface it through
    /// `SttProvider::inflight_transcriptions` so test harnesses can wait for
    /// detached results.
    pub fn inflight(&self) -> usize {
        self.inflight.load(Ordering::SeqCst)
    }

    /// Called at `SpeechStart`: attach to the new segment. A still-running
    /// flight from the previous segment is left alone; its result is discarded
    /// by seq check when it completes.
    pub fn on_segment_start(&mut self, seq: u64) {
        self.seq = seq;
        self.decoded_upto = 0;
    }

    /// True when a partial decode should start now: live enabled, a segment is
    /// open, no decode in flight, and either the interval elapsed or the
    /// closing silence started with undecoded audio (that decode is the one
    /// most likely to cover all speech by close time).
    fn wants_decode(
        &self,
        seq: u64,
        in_speech: bool,
        closing_silence: bool,
        buf_len: usize,
    ) -> bool {
        self.cfg.enabled
            && seq == self.seq
            && seq > 0
            && in_speech
            && self.in_flight.is_none()
            && (buf_len.saturating_sub(self.decoded_upto) >= self.interval_samples
                || (closing_silence && buf_len > self.decoded_upto))
    }

    /// Spawn a partial decode of the open segment when the cadence allows it.
    /// Returns true when a flight was started.
    pub fn maybe_spawn<D: LiveDecoder>(
        &mut self,
        seq: u64,
        in_speech: bool,
        closing_silence: bool,
        buf: &[f32],
        decode: D,
    ) -> bool {
        if !self.wants_decode(seq, in_speech, closing_silence, buf.len()) {
            return false;
        }
        let window = self.window_samples.min(buf.len());
        let snapshot = buf[buf.len() - window..].to_vec();
        let covered_len = buf.len();
        let full = window == buf.len();
        let guard = InflightGuard(Arc::clone(&self.inflight));
        self.inflight.fetch_add(1, Ordering::SeqCst);
        let handle = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            decode.decode(snapshot)
        });
        self.in_flight = Some(Flight {
            segment_seq: seq,
            covered_len,
            full,
            handle,
        });
        true
    }

    /// Poll the in-flight decode; when finished, emit its partial (in order,
    /// only if it advanced the covered range and produced text). Returns true
    /// when a partial was emitted.
    pub async fn poll(&mut self, tx: &mpsc::Sender<SpeechEvent>) -> bool {
        let Some(flight) = &self.in_flight else {
            return false;
        };
        if !flight.handle.is_finished() {
            return false;
        }
        let flight = self.in_flight.take().unwrap();
        let quality = match flight.handle.await {
            Ok(Ok(q)) => q,
            Ok(Err(e)) => {
                tracing::debug!(target: "stt", "Live decode failed: {e:#}");
                return false;
            }
            Err(e) => {
                tracing::debug!(target: "stt", "Live decode task panicked: {e}");
                return false;
            }
        };
        if flight.segment_seq != self.seq {
            tracing::debug!(
                target: "stt",
                "Live decode for superseded segment {} discarded",
                flight.segment_seq
            );
            return false;
        }
        self.decoded_upto = self.decoded_upto.max(flight.covered_len);
        if quality.text.trim().is_empty() {
            return false;
        }
        tracing::debug!(target: "stt", "Partial (live): {}", quality.text);
        let _ = tx.send(SpeechEvent::Speech(quality.text)).await;
        true
    }

    /// Close the segment without blocking the caller: await any in-flight
    /// decode (detached) and reuse it as the final text when it already
    /// covered every spoken sample; otherwise run the authoritative full
    /// decode of the complete utterance. Either way `SpeechEnd` is sent from
    /// the detached task, so the audio loop keeps running while the model
    /// works (contract P4; staleness handled by seq pairing downstream).
    pub fn finish_detached<D: LiveDecoder>(
        &mut self,
        seq: u64,
        audio: Vec<f32>,
        speech_end_offset: usize,
        decode: D,
        tx: mpsc::Sender<SpeechEvent>,
    ) {
        let flight = self.in_flight.take().filter(|f| f.segment_seq == seq);
        self.decoded_upto = audio.len();
        let inflight_counter = Arc::clone(&self.inflight);
        tokio::spawn(async move {
            let result: Result<TranscriptionQuality> = async {
                if let Some(f) = flight {
                    match f.handle.await {
                        Ok(Ok(q)) => {
                            if f.full && f.covered_len >= speech_end_offset {
                                tracing::debug!(
                                    target: "stt",
                                    "Segment {seq}: final reused from live decode (covered {:.2}s)",
                                    f.covered_len as f32 / SAMPLE_RATE as f32
                                );
                                return Ok(q);
                            }
                        }
                        Ok(Err(e)) => {
                            tracing::debug!(target: "stt", "Live decode failed: {e:#}");
                        }
                        Err(e) => {
                            tracing::debug!(target: "stt", "Live decode task panicked: {e}");
                        }
                    }
                }
                // Authoritative final decode over the complete utterance.
                let guard = InflightGuard(Arc::clone(&inflight_counter));
                inflight_counter.fetch_add(1, Ordering::SeqCst);
                tokio::task::spawn_blocking(move || {
                    let _guard = guard;
                    decode.decode(audio)
                })
                .await?
            }
            .await;
            match result {
                Ok(quality) => {
                    tracing::info!(target: "stt", "SpeechEnd: {}", quality.text);
                    let _ = tx
                        .send(SpeechEvent::SpeechEnd {
                            segment_seq: seq,
                            quality,
                        })
                        .await;
                }
                Err(e) => {
                    tracing::warn!(target: "stt", "Final transcription failed for segment {seq}: {e:#}");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// Decoder that sleeps briefly and counts invocations.
    struct FakeDecoder {
        count: Arc<AtomicUsize>,
        delay_ms: u64,
        text: String,
    }

    impl LiveDecoder for FakeDecoder {
        fn decode(&self, _audio: Vec<f32>) -> Result<TranscriptionQuality> {
            std::thread::sleep(Duration::from_millis(self.delay_ms));
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(TranscriptionQuality {
                text: self.text.clone(),
                no_speech_prob: 0.0,
                avg_logprob: 0.0,
                compression_ratio: 0.0,
            })
        }
    }

    fn probe_buf(probes: usize) -> Vec<f32> {
        vec![0.1f32; SAMPLE_RATE / 10 * probes]
    }

    fn streamer(interval_probes: usize) -> LiveStreamer {
        LiveStreamer::new(LiveStreamConfig {
            enabled: true,
            interval_ms: (interval_probes * 100) as u64,
            window_ms: 15_000,
        })
    }

    #[tokio::test]
    async fn disabled_config_never_spawns() {
        let mut s = LiveStreamer::new(LiveStreamConfig {
            enabled: false,
            ..Default::default()
        });
        s.on_segment_start(1);
        let (tx, mut rx) = mpsc::channel(16);
        let decode = FakeDecoder {
            count: Arc::new(AtomicUsize::new(0)),
            delay_ms: 0,
            text: "x".into(),
        };
        let count = Arc::clone(&decode.count);
        assert!(!s.maybe_spawn(1, true, false, &probe_buf(20), decode));
        s.finish_detached(
            1,
            probe_buf(20),
            16_000,
            FakeDecoder {
                count: Arc::clone(&count),
                delay_ms: 0,
                text: "final".into(),
            },
            tx,
        );
        match rx.recv().await.unwrap() {
            SpeechEvent::SpeechEnd { quality, .. } => assert_eq!(quality.text, "final"),
            other => panic!("expected SpeechEnd, got {other:?}"),
        }
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "only the final decode runs"
        );
    }

    #[tokio::test]
    async fn partial_decodes_follow_interval_and_stay_single_flight() {
        let mut s = streamer(10); // 1000 ms interval
        s.on_segment_start(1);
        let (tx, mut rx) = mpsc::channel(16);

        // 20 probes = 2 s: the first maybe_spawn fires (>= interval of new audio).
        let d1 = FakeDecoder {
            count: Arc::new(AtomicUsize::new(0)),
            delay_ms: 30,
            text: "uno".into(),
        };
        let count = Arc::clone(&d1.count);
        assert!(s.maybe_spawn(1, true, false, &probe_buf(20), d1));

        // While in flight, a second spawn must be refused (single flight).
        let d2 = FakeDecoder {
            count: Arc::clone(&count),
            delay_ms: 0,
            text: "dos".into(),
        };
        assert!(!s.maybe_spawn(1, true, false, &probe_buf(25), d2));

        // Wait for the flight, poll, and the partial arrives.
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(s.poll(&tx).await);
        let partial = rx.recv().await.unwrap();
        assert!(matches!(partial, SpeechEvent::Speech(ref t) if t == "uno"));
        assert_eq!(count.load(Ordering::SeqCst), 1);

        // After the emit (decoded_upto = 20 probes), a new spawn needs a full
        // interval of NEW audio: 30 probes gives exactly 10 probes = 1000 ms.
        let d3 = FakeDecoder {
            count: Arc::clone(&count),
            delay_ms: 0,
            text: "dos".into(),
        };
        assert!(
            !s.maybe_spawn(1, true, false, &probe_buf(25), d3),
            "500 ms < interval must not spawn"
        );
        let d4 = FakeDecoder {
            count: Arc::clone(&count),
            delay_ms: 0,
            text: "dos".into(),
        };
        assert!(s.maybe_spawn(1, true, false, &probe_buf(30), d4));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(s.poll(&tx).await);
    }

    #[tokio::test]
    async fn finish_reuses_flight_that_covered_all_speech() {
        let mut s = streamer(10);
        s.on_segment_start(1);
        let (tx, mut rx) = mpsc::channel(16);

        let d = FakeDecoder {
            count: Arc::new(AtomicUsize::new(0)),
            delay_ms: 10,
            text: "hola mundo".into(),
        };
        let count = Arc::clone(&d.count);
        // Spawn over the full buffer (window 15 s > buffer), then the segment
        // closes with NO new audio after the snapshot: covered == final len.
        let buf = probe_buf(20);
        assert!(s.maybe_spawn(1, true, false, &buf, d));
        tokio::time::sleep(Duration::from_millis(40)).await;

        s.finish_detached(
            1,
            buf.clone(),
            buf.len(),
            FakeDecoder {
                count: Arc::clone(&count),
                delay_ms: 0,
                text: "otra".into(),
            },
            tx,
        );
        match rx.recv().await.unwrap() {
            SpeechEvent::SpeechEnd { quality, .. } => {
                assert_eq!(quality.text, "hola mundo", "covering flight must be reused");
            }
            other => panic!("expected SpeechEnd, got {other:?}"),
        }
        assert_eq!(count.load(Ordering::SeqCst), 1, "no second decode may run");
    }

    #[tokio::test]
    async fn finish_decodes_full_when_flight_misses_the_tail() {
        let mut s = streamer(10);
        s.on_segment_start(1);
        let (tx, mut rx) = mpsc::channel(16);

        let d = FakeDecoder {
            count: Arc::new(AtomicUsize::new(0)),
            delay_ms: 10,
            text: "prefijo".into(),
        };
        let count = Arc::clone(&d.count);
        // Flight over 20 probes; the segment then closes at 30 probes with
        // speech up to offset 26 probes: the flight missed the tail.
        assert!(s.maybe_spawn(1, true, false, &probe_buf(20), d));
        tokio::time::sleep(Duration::from_millis(40)).await;

        let buf = probe_buf(30);
        let speech_end_offset = SAMPLE_RATE / 10 * 26;
        s.finish_detached(
            1,
            buf,
            speech_end_offset,
            FakeDecoder {
                count: Arc::clone(&count),
                delay_ms: 0,
                text: "completo".into(),
            },
            tx,
        );
        match rx.recv().await.unwrap() {
            SpeechEvent::SpeechEnd { quality, .. } => assert_eq!(quality.text, "completo"),
            other => panic!("expected SpeechEnd, got {other:?}"),
        }
        assert_eq!(
            count.load(Ordering::SeqCst),
            2,
            "final full decode must run"
        );
    }

    #[tokio::test]
    async fn stale_flight_result_is_discarded() {
        let mut s = streamer(10);
        s.on_segment_start(1);
        let (tx, mut rx) = mpsc::channel(16);

        let d = FakeDecoder {
            count: Arc::new(AtomicUsize::new(0)),
            delay_ms: 60,
            text: "viejo".into(),
        };
        let count = Arc::clone(&d.count);
        assert!(s.maybe_spawn(1, true, false, &probe_buf(20), d));

        // A barge-in opened a new segment before the flight finished.
        s.on_segment_start(2);
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(!s.poll(&tx).await, "stale partial must be discarded");
        // Nothing reached the channel, and the new segment can decode again
        // once a full interval of new audio exists (10 probes = 1000 ms).
        let d2 = FakeDecoder {
            count: Arc::clone(&count),
            delay_ms: 0,
            text: "nuevo".into(),
        };
        assert!(s.maybe_spawn(2, true, false, &probe_buf(10), d2));
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(count.load(Ordering::SeqCst), 2);
        rx.close();
    }

    #[tokio::test]
    async fn finish_drops_stale_flight_and_decodes_fresh() {
        let mut s = streamer(10);
        s.on_segment_start(1);
        let (tx, mut rx) = mpsc::channel(16);

        let d = FakeDecoder {
            count: Arc::new(AtomicUsize::new(0)),
            delay_ms: 80,
            text: "viejo".into(),
        };
        let count = Arc::clone(&d.count);
        assert!(s.maybe_spawn(1, true, false, &probe_buf(20), d));
        // Segment 1 closed mid-flight (finish awaited in the real flow) — here
        // we simulate the rarer path: the provider moved on before awaiting.
        s.on_segment_start(2);

        s.finish_detached(
            2,
            probe_buf(10),
            16_000,
            FakeDecoder {
                count: Arc::clone(&count),
                delay_ms: 0,
                text: "final".into(),
            },
            tx,
        );
        match rx.recv().await.unwrap() {
            SpeechEvent::SpeechEnd { quality, .. } => {
                assert_eq!(quality.text, "final", "stale flight result must not leak");
            }
            other => panic!("expected SpeechEnd, got {other:?}"),
        }
        // Give the stale (detached) decode time to finish so the count is
        // stable before asserting.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }
}
