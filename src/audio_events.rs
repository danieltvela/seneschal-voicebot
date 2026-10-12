//! Audio-side `SpeechEvent` handler, extracted verbatim from the main audio
//! loop so the STT/VAD contract (`doc/stt-contract.md`) is testable end-to-end
//! with mock providers and no audio hardware or models.
//!
//! Contract implemented here:
//! - P2: `SpeechStart` fires BARGE_IN (cancelling LLM, SentenceSplitter, TTS
//!   and any in-flight STT) and records the segment seq; a `SpeechEnd` with a
//!   different seq is stale (a barge-in opened a newer utterance) and is
//!   dropped — its transcript never reaches the LLM.
//! - P4: the LLM launch is gated on the final `SpeechEnd` transcript, after
//!   the `NoSpeechGate` quality filter and the ambient/wake-word gating.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use seneschal_common::tools::ConversationMode;
use seneschal_core::audio::ambient_buffer::AmbientBuffer;
use seneschal_core::audio::buffer::AudioBuffer;
use seneschal_core::pipeline::{AnnouncementWindow, PipelineEvents, PipelineFrame};
use seneschal_core::stt::{NoSpeechGate, SpeechEvent, SttProvider};
use seneschal_extras::analysis::identity::IdentityAnalyzer;

use crate::config::Config;

/// Utterances shorter than this are only processed when they produced text
/// (the macOS speech provider's short-utterance path can close in <50 ms).
pub const MIN_SPEECH_DURATION_MS: u32 = 300;

/// Per-batch handler context. Holds mutable references to the audio loop's
/// locals and clones of the shared Arcs; construct one per event batch.
pub struct AudioEvents<'a> {
    pub t_speech_start: &'a mut Option<Instant>,
    pub active_segment_seq: &'a mut u64,
    pub last_cleared_commit: &'a mut u64,
    pub speech_buffer: &'a mut AudioBuffer,
    pub speech_buffer_start_offset: &'a mut usize,
    pub stt_provider: &'a mut dyn SttProvider,
    pub source_sample_rate: u32,
    pub config: &'a Config,
    pub events: Arc<PipelineEvents>,
    pub utterance_epoch: Arc<AtomicU64>,
    pub turn_commit_counter: Arc<AtomicU64>,
    pub t_vad_end: Arc<Mutex<Option<Instant>>>,
    pub last_speech_at: Arc<Mutex<Instant>>,
    pub non_user_streak: Arc<Mutex<u8>>,
    pub conv_mode: Arc<Mutex<ConversationMode>>,
    pub ambient_buffer: Arc<Mutex<AmbientBuffer>>,
    pub announcement_window: Arc<Mutex<AnnouncementWindow>>,
    pub no_speech_gate: NoSpeechGate,
    pub transcript_tx: mpsc::Sender<PipelineFrame>,
    pub identity_analyzer: Option<Arc<Mutex<IdentityAnalyzer>>>,
    #[cfg(feature = "tui")]
    pub tui_tx: tokio::sync::mpsc::UnboundedSender<seneschal_tui::events::TuiEvent>,
}

impl<'a> AudioEvents<'a> {
    #[cfg(feature = "tui")]
    fn tui_state(&self, state: seneschal_tui::events::PipelineState) {
        self.tui_tx
            .send(seneschal_tui::events::TuiEvent::StateChange(state))
            .ok();
    }

    /// Handle one `SpeechEvent` from the STT provider.
    pub async fn handle(&mut self, event: SpeechEvent, mono: &[f32]) {
        match event {
            SpeechEvent::SpeechStart { segment_seq } => {
                *self.active_segment_seq = segment_seq;
                *self.t_speech_start = Some(Instant::now());
                info!(target: "performance", "[+0ms] SpeechStart (seq {segment_seq})");
                #[cfg(feature = "tui")]
                self.tui_state(seneschal_tui::events::PipelineState::Listening);
                *self.last_speech_at.lock().unwrap() = Instant::now();

                info!(target: "pipeline", "SpeechStart — firing BARGE_IN");
                self.events
                    .barge_in_tx
                    .send(self.utterance_epoch.load(Ordering::SeqCst))
                    .ok();
                // Remote STT (issue #17) / live decodes: abort any in-flight
                // transcription now that the user barged in.
                self.stt_provider.cancel_inflight();
                self.announcement_window
                    .lock()
                    .unwrap()
                    .set_user_speaking(true);
                self.announcement_window.lock().unwrap().interrupt();
                info!(target: "pipeline", "SpeechStart — interrupted pending announcements");

                let current_commits = self.turn_commit_counter.load(Ordering::SeqCst);
                if current_commits > *self.last_cleared_commit {
                    self.speech_buffer.clear();
                    *self.last_cleared_commit = current_commits;
                }
                *self.speech_buffer_start_offset = self.speech_buffer.sample_count();
                self.speech_buffer.push(mono);

                self.utterance_epoch.fetch_add(1, Ordering::SeqCst);
            }
            SpeechEvent::Speech(partial_text) => {
                self.speech_buffer.push(mono);
                debug!(target: "stt", "Partial: {}", partial_text);
            }
            SpeechEvent::SpeechEnd {
                segment_seq,
                quality,
            } => {
                self.speech_buffer.push(mono);

                // Stale result: a newer SpeechStart (barge-in) opened a
                // different utterance, so this end belongs to superseded
                // speech. Drop it — its transcript must never reach the LLM
                // under the new utterance id.
                if segment_seq != *self.active_segment_seq {
                    debug!(
                        target: "pipeline",
                        "Stale SpeechEnd (seq {segment_seq}, active {}) — dropped after barge-in",
                        *self.active_segment_seq
                    );
                    return;
                }

                // NoSpeechGate: reject non-speech transcriptions
                if self.no_speech_gate.should_reject(&quality) {
                    debug!(target: "nospeechgate", "NoSpeechGate rejected transcription, skipping");
                    *self.t_speech_start = None;
                    #[cfg(feature = "tui")]
                    self.tui_state(seneschal_tui::events::PipelineState::Idle);
                    return;
                }

                let segment_duration_ms = self
                    .t_speech_start
                    .as_ref()
                    .map(|t| t.elapsed().as_millis() as u32)
                    .unwrap_or(0);

                // Wall-clock duration can be tiny on the short-utterance
                // path (SpeechStart fires at finalize, Apple returns in
                // ~50–200ms). NoSpeechGate already rejected empty/junk;
                // accept non-empty gated text even if wall duration is low.
                if segment_duration_ms < MIN_SPEECH_DURATION_MS && quality.text.trim().is_empty() {
                    debug!(target: "pipeline", "Too short ({}ms), skipping", segment_duration_ms);
                    *self.t_speech_start = None;
                    #[cfg(feature = "tui")]
                    self.tui_state(seneschal_tui::events::PipelineState::Idle);
                    return;
                }

                let current_commits = self.turn_commit_counter.load(Ordering::SeqCst);
                if current_commits > *self.last_cleared_commit {
                    *self.last_cleared_commit = current_commits;
                }
                let audio = self
                    .speech_buffer
                    .get_samples_from(*self.speech_buffer_start_offset);
                let duration_ms = audio.len() as u32 * 1000 / self.source_sample_rate;

                info!(target: "pipeline", "Speech: {}ms (segment {}ms)", duration_ms, segment_duration_ms);

                let mut segment_text = quality.text;

                #[cfg(feature = "tui")]
                self.tui_state(seneschal_tui::events::PipelineState::Transcribing);

                let vad_elapsed = self
                    .t_speech_start
                    .take()
                    .map(|t| t.elapsed().as_millis())
                    .unwrap_or(0);
                info!(target: "performance", "[+{}ms] VAD end ({}ms speech)", vad_elapsed, duration_ms);
                *self.t_vad_end.lock().unwrap() = Some(Instant::now());

                *self.last_speech_at.lock().unwrap() = Instant::now();

                // ── Speaker verification (synchronous, CPU-blocking) ──
                // issue #220: the verification is best-effort. A failed
                // JoinHandle used to `.unwrap()` — panicking the audio
                // loop and killing the whole pipeline (no state events
                // ever reach the TUI again). On failure, fall back to the
                // default speaker identity so the utterance still flows.
                let (is_main_speaker, speaker_label) =
                    if let Some(ref analyzer) = self.identity_analyzer {
                        let analyzer = Arc::clone(analyzer);
                        let audio_c = audio.clone();
                        let sample_rate = self.config.sample_rate;
                        match tokio::task::spawn_blocking(move || {
                            let mut a = analyzer.lock().unwrap();
                            let result = a.verify(sample_rate, &audio_c);
                            (result.is_main_speaker, result.speaker_label)
                        })
                        .await
                        {
                            Ok(result) => result,
                            Err(e) => {
                                warn!(
                                    target: "pipeline",
                                    "Speaker verification task failed ({e}) — \
                                     defaulting to main speaker",
                                );
                                (true, "Usuario".to_string())
                            }
                        }
                    } else {
                        (true, "Usuario".to_string())
                    };

                // ── Mode-change side effects (streak, ambient buffer) ──
                if !is_main_speaker {
                    let mut streak = self.non_user_streak.lock().unwrap();
                    *streak = streak.saturating_add(1);
                    if *streak >= self.config.speaker_ambient_trigger {
                        let mut mode = self.conv_mode.lock().unwrap();
                        if *mode == ConversationMode::Active {
                            *mode = ConversationMode::Ambient;
                            drop(mode);
                            info!(
                                target: "pipeline",
                                "Ambient mode: {} consecutive non-user voices",
                                *streak
                            );
                        }
                    }
                    drop(streak);
                    // Buffer non-main-speaker transcript regardless of mode.
                    self.ambient_buffer
                        .lock()
                        .unwrap()
                        .push(speaker_label.clone(), segment_text.clone());
                    debug!(
                        target: "pipeline",
                        "Ambient buffer ← {speaker_label}: {}",
                        segment_text
                    );
                } else {
                    *self.non_user_streak.lock().unwrap() = 0;
                }

                // ── Permission gate (issue #167) ────────────────────────
                // Previously this branch re-transcribed the audio and
                // mapped keywords (sí/no) to the ACP option. The voice
                // path no longer intercepts permissions: the user's
                // spoken answer flows through the normal transcript
                // pipeline and the frontend LLM resolves the pending
                // permission by calling `respond_agent_permission`. The
                // gate itself is still drained by `cancel_scope` at the
                // end of every ACP prompt and by the HTTP control API.

                let stt_elapsed_ms = segment_duration_ms as u128;
                info!(
                    target: "performance",
                    "[+{}ms] STT transcription complete (audio={}samples, {}chars)",
                    stt_elapsed_ms, audio.len(), segment_text.len()
                );
                debug!(target: "stt", "Segment final: {}", segment_text);

                if segment_text.trim().is_empty() {
                    debug!(target: "pipeline", "Empty transcription — skipping");
                    // issue #220: the TUI was set to Transcribing above;
                    // this rejection branch must return it to Idle or the
                    // status bar stays stuck on TRANSCRIBING.
                    #[cfg(feature = "tui")]
                    self.tui_state(seneschal_tui::events::PipelineState::Idle);
                    return;
                }

                // ── Ambient/Active mode gating with wake word ─────────
                let mode_snapshot = self.conv_mode.lock().unwrap().clone();
                let is_ambient = matches!(
                    mode_snapshot,
                    ConversationMode::Ambient | ConversationMode::AmbientLocked
                );
                // issue #228: WAKE_WORD may hold a comma-separated
                // list; any entry triggers a response.
                let has_wake_word = self.config.has_wake_word(&segment_text);

                if is_ambient {
                    if has_wake_word && is_main_speaker {
                        // Main user + wake word → respond AND switch to Active
                        *self.conv_mode.lock().unwrap() = ConversationMode::Active;
                        info!(target: "pipeline", "Main user wake word — switching to Active");
                        // Fall through → respond normally
                    } else if has_wake_word {
                        // Secondary voice + wake word → respond BUT stay Ambient
                        info!(target: "pipeline", "Secondary voice wake word — responding, staying Ambient");
                        // Fall through → respond normally (mode unchanged)
                    } else {
                        // No wake word → buffer (already buffered for
                        // non-main above) and skip LLM pipeline.
                        if is_main_speaker {
                            self.ambient_buffer
                                .lock()
                                .unwrap()
                                .push(speaker_label.clone(), segment_text.clone());
                            debug!(target: "pipeline", "Ambient: main user without wake word — buffered");
                        }
                        // issue #220: this branch leaves the LLM pipeline
                        // cold, so Transcribing is never resolved by llm_task
                        // — reset the TUI here.
                        #[cfg(feature = "tui")]
                        self.tui_state(seneschal_tui::events::PipelineState::Idle);
                        return;
                    }
                } else {
                    // Active mode
                    if !is_main_speaker {
                        // Non-main user in Active → discard (already buffered
                        // above in the mode-change side effects section).
                        // issue #220: same as the Ambient no-wake-word branch
                        // — the turn dies here, so return the TUI to Idle.
                        #[cfg(feature = "tui")]
                        self.tui_state(seneschal_tui::events::PipelineState::Idle);
                        return;
                    }
                    // Main user in Active → respond normally (fall through)
                }

                // ── Ambient context injection ──────────────────────────
                let mut final_text = segment_text;
                final_text = {
                    let buf = self.ambient_buffer.lock().unwrap();
                    if seneschal_core::audio::ambient_buffer::has_referential(&final_text) {
                        if let Some(ctx) = buf.format_context() {
                            format!("{ctx}\n---\n{final_text}")
                        } else {
                            final_text
                        }
                    } else {
                        final_text
                    }
                };

                if final_text.trim().is_empty() {
                    debug!(target: "pipeline", "Empty after context injection — skipping");
                    // issue #220: last rejection branch before the LLM
                    // hand-off — return the TUI to Idle so the bar
                    // doesn't stay stuck on TRANSCRIBING.
                    #[cfg(feature = "tui")]
                    self.tui_state(seneschal_tui::events::PipelineState::Idle);
                    return;
                }

                if let Some(t0) = self.t_vad_end.lock().unwrap().as_ref() {
                    info!(
                        target: "performance",
                        "[+{}ms] STT done → transcript channel",
                        t0.elapsed().as_millis()
                    );
                }
                let uid = self.utterance_epoch.load(Ordering::SeqCst);
                self.announcement_window
                    .lock()
                    .unwrap()
                    .set_user_speaking(false);
                self.announcement_window.lock().unwrap().begin_turn();
                self.transcript_tx
                    .send(PipelineFrame::TranscriptReady {
                        utterance_id: uid,
                        text: final_text,
                    })
                    .await
                    .ok();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use seneschal_core::stt::TranscriptionQuality;

    async fn recv_transcript(rx: &mut mpsc::Receiver<PipelineFrame>) -> Option<String> {
        match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
            Ok(Some(PipelineFrame::TranscriptReady { text, .. })) => Some(text),
            _ => None,
        }
    }

    /// Mock STT provider (no models): records cancel_inflight calls.
    struct MockStt {
        cancels: Arc<AtomicU64>,
    }

    #[async_trait::async_trait]
    impl SttProvider for MockStt {
        fn provider_name(&self) -> &'static str {
            "mock"
        }
        async fn process_audio(
            &mut self,
            _audio: &[f32],
            _tx: &mpsc::Sender<SpeechEvent>,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        fn transcribe_complete(&self, _audio: &[f32]) -> anyhow::Result<TranscriptionQuality> {
            Ok(TranscriptionQuality {
                text: String::new(),
                no_speech_prob: 1.0,
                avg_logprob: 0.0,
                compression_ratio: 0.0,
            })
        }
        fn cancel_inflight(&mut self) {
            self.cancels.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct Fixture {
        ctx_parts: CtxParts,
        transcript_rx: mpsc::Receiver<PipelineFrame>,
        barge_in_rx: tokio::sync::broadcast::Receiver<u64>,
        cancels: Arc<AtomicU64>,
    }

    struct CtxParts {
        t_speech_start: Option<Instant>,
        active_segment_seq: u64,
        last_cleared_commit: u64,
        speech_buffer: AudioBuffer,
        speech_buffer_start_offset: usize,
        stt: MockStt,
        events: Arc<PipelineEvents>,
        utterance_epoch: Arc<AtomicU64>,
        turn_commit_counter: Arc<AtomicU64>,
        t_vad_end: Arc<Mutex<Option<Instant>>>,
        last_speech_at: Arc<Mutex<Instant>>,
        non_user_streak: Arc<Mutex<u8>>,
        conv_mode: Arc<Mutex<ConversationMode>>,
        ambient_buffer: Arc<Mutex<AmbientBuffer>>,
        announcement_window: Arc<Mutex<AnnouncementWindow>>,
        no_speech_gate: NoSpeechGate,
        transcript_tx: mpsc::Sender<PipelineFrame>,
        #[cfg(feature = "tui")]
        tui_tx: tokio::sync::mpsc::UnboundedSender<seneschal_tui::events::TuiEvent>,
    }

    fn fixture() -> Fixture {
        let (transcript_tx, transcript_rx) = mpsc::channel(16);
        let events = Arc::new(PipelineEvents::new());
        let barge_in_rx = events.barge_in_tx.subscribe();
        let cancels = Arc::new(AtomicU64::new(0));
        #[cfg(feature = "tui")]
        let (tui_tx, _tui_rx) =
            tokio::sync::mpsc::unbounded_channel::<seneschal_tui::events::TuiEvent>();
        let parts = CtxParts {
            t_speech_start: None,
            active_segment_seq: 0,
            last_cleared_commit: 0,
            speech_buffer: AudioBuffer::new(16_000, 60),
            speech_buffer_start_offset: 0,
            stt: MockStt {
                cancels: Arc::clone(&cancels),
            },
            events,
            utterance_epoch: Arc::new(AtomicU64::new(0)),
            turn_commit_counter: Arc::new(AtomicU64::new(0)),
            t_vad_end: Arc::new(Mutex::new(None)),
            last_speech_at: Arc::new(Mutex::new(Instant::now())),
            non_user_streak: Arc::new(Mutex::new(0)),
            conv_mode: Arc::new(Mutex::new(ConversationMode::Active)),
            ambient_buffer: Arc::new(Mutex::new(AmbientBuffer::new(30, 3))),
            announcement_window: Arc::new(Mutex::new(AnnouncementWindow::new())),
            no_speech_gate: NoSpeechGate::default(),
            transcript_tx,
            #[cfg(feature = "tui")]
            tui_tx,
        };
        Fixture {
            ctx_parts: parts,
            transcript_rx,
            barge_in_rx,
            cancels,
        }
    }

    fn ctx(f: &mut Fixture) -> AudioEvents<'_> {
        let p = &mut f.ctx_parts;
        AudioEvents {
            t_speech_start: &mut p.t_speech_start,
            active_segment_seq: &mut p.active_segment_seq,
            last_cleared_commit: &mut p.last_cleared_commit,
            speech_buffer: &mut p.speech_buffer,
            speech_buffer_start_offset: &mut p.speech_buffer_start_offset,
            stt_provider: &mut p.stt,
            source_sample_rate: 16_000,
            config: config(),
            events: Arc::clone(&p.events),
            utterance_epoch: Arc::clone(&p.utterance_epoch),
            turn_commit_counter: Arc::clone(&p.turn_commit_counter),
            t_vad_end: Arc::clone(&p.t_vad_end),
            last_speech_at: Arc::clone(&p.last_speech_at),
            non_user_streak: Arc::clone(&p.non_user_streak),
            conv_mode: Arc::clone(&p.conv_mode),
            ambient_buffer: Arc::clone(&p.ambient_buffer),
            announcement_window: Arc::clone(&p.announcement_window),
            no_speech_gate: p.no_speech_gate.clone(),
            transcript_tx: p.transcript_tx.clone(),
            identity_analyzer: None,
            #[cfg(feature = "tui")]
            tui_tx: p.tui_tx.clone(),
        }
    }

    static CONFIG: std::sync::OnceLock<Config> = std::sync::OnceLock::new();

    fn config() -> &'static Config {
        CONFIG.get_or_init(|| Config::from_env().expect("Config::from_env() should succeed"))
    }

    fn quality(text: &str) -> TranscriptionQuality {
        TranscriptionQuality {
            text: text.to_string(),
            no_speech_prob: 0.0,
            avg_logprob: -0.5,
            compression_ratio: 1.2,
        }
    }

    /// P2: SpeechStart fires BARGE_IN, cancels in-flight STT, and opens a new
    /// utterance epoch.
    #[tokio::test]
    async fn speechstart_fires_barge_in_and_cancels_stt() {
        let mut f = fixture();
        {
            let mut c = ctx(&mut f);
            c.handle(SpeechEvent::SpeechStart { segment_seq: 7 }, &[0.1; 1600])
                .await;
        }
        assert_eq!(
            f.barge_in_rx.try_recv().ok(),
            Some(0),
            "barge-in must fire with the interrupted epoch"
        );
        assert_eq!(
            f.cancels.load(Ordering::SeqCst),
            1,
            "STT in-flight must be cancelled"
        );
        assert_eq!(f.ctx_parts.utterance_epoch.load(Ordering::SeqCst), 1);
        assert_eq!(f.ctx_parts.active_segment_seq, 7);
        assert!(f.ctx_parts.t_speech_start.is_some());
        assert!(f.transcript_rx.try_recv().is_err());
    }

    /// P4: a current SpeechEnd with valid text hands the transcript to the LLM.
    #[tokio::test]
    async fn speechend_launches_transcript_for_active_segment() {
        let mut f = fixture();
        {
            let mut c = ctx(&mut f);
            c.handle(SpeechEvent::SpeechStart { segment_seq: 3 }, &[0.1; 1600])
                .await;
            c.handle(
                SpeechEvent::SpeechEnd {
                    segment_seq: 3,
                    quality: quality("hola"),
                },
                &[0.1; 1600],
            )
            .await;
        }
        let text = recv_transcript(&mut f.transcript_rx)
            .await
            .expect("transcript must reach the LLM channel");
        assert_eq!(text, "hola");
    }

    /// P2: a SpeechEnd from a superseded segment (a barge-in opened a newer
    /// utterance) is dropped — no LLM launch, no stale text.
    #[tokio::test]
    async fn stale_speech_end_is_dropped_after_new_speechstart() {
        let mut f = fixture();
        {
            let mut c = ctx(&mut f);
            c.handle(SpeechEvent::SpeechStart { segment_seq: 1 }, &[0.1; 1600])
                .await;
            // Barge-in: the user starts a new utterance.
            c.handle(SpeechEvent::SpeechStart { segment_seq: 2 }, &[0.1; 1600])
                .await;
            // The old segment's decode finally completes.
            c.handle(
                SpeechEvent::SpeechEnd {
                    segment_seq: 1,
                    quality: quality("vieja"),
                },
                &[0.1; 1600],
            )
            .await;
            // The new utterance is still open: its start time must survive.
            assert!(
                f.ctx_parts.t_speech_start.is_some(),
                "stale end must not clear the new utterance timing"
            );
        }
        assert!(
            recv_transcript(&mut f.transcript_rx).await.is_none(),
            "stale transcript must never reach the LLM"
        );
    }

    /// NoSpeechGate: an empty/junk transcription is rejected and nothing is
    /// launched (FSM reset paths stay exercised via the TUI cfg when enabled).
    #[tokio::test]
    async fn no_speech_transcription_is_rejected() {
        let mut f = fixture();
        {
            let mut c = ctx(&mut f);
            c.handle(SpeechEvent::SpeechStart { segment_seq: 4 }, &[0.1; 1600])
                .await;
            c.handle(
                SpeechEvent::SpeechEnd {
                    segment_seq: 4,
                    quality: TranscriptionQuality {
                        text: String::new(),
                        no_speech_prob: 0.0,
                        avg_logprob: 0.0,
                        compression_ratio: 0.0,
                    },
                },
                &[0.1; 1600],
            )
            .await;
        }
        assert!(recv_transcript(&mut f.transcript_rx).await.is_none());
    }

    /// Ambient mode without the wake word: the transcript is buffered, the
    /// LLM stays cold.
    #[tokio::test]
    async fn ambient_mode_without_wake_word_skips_llm() {
        let mut f = fixture();
        *f.ctx_parts.conv_mode.lock().unwrap() = ConversationMode::Ambient;
        {
            let mut c = ctx(&mut f);
            c.handle(SpeechEvent::SpeechStart { segment_seq: 9 }, &[0.1; 1600])
                .await;
            c.handle(
                SpeechEvent::SpeechEnd {
                    segment_seq: 9,
                    quality: quality("qué hora es"),
                },
                &[0.1; 1600],
            )
            .await;
        }
        assert!(recv_transcript(&mut f.transcript_rx).await.is_none());
    }
}
