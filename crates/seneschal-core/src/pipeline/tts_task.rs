use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::error;

use super::fsm::PipelineState;
use super::state::PipelineEvents;
use crate::audio::output::AudioOutput;
use crate::pipeline::frames::PipelineFrame;
use crate::tts::TtsEngine;
use seneschal_common::TtsAudioPacket;
use seneschal_common::tui_events::{TuiEvent, TuiEventTx};
use tracing::info;

/// Shared handle for optional remote TTS routing.
///
/// When a remote WebSocket client connects, the remote server installs a sender here.
/// `tts_task` then forwards synthesized audio to that client instead of local CPAL.
pub type RemoteTtsTx = Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Sender<TtsAudioPacket>>>>;

/// TTS task: receives sentences from sen_task (and llm_task error paths) via typed channel,
/// synthesizes and plays each one.
///
/// Owns the FSM transitions into and out of `PipelineState::Speaking`:
/// - Sets `Speaking { utterance_id }` on the first sentence of each utterance.
/// - Sets `Idle` when the LLM's end-of-turn frame arrives: because frames are
///   FIFO and playback is serialised in this loop, that frame is always observed
///   after every sentence for the turn has finished playing. Mid-turn flushes
///   (`end_of_turn: false`, e.g. a foreground tool's narration) do not idle.
/// - Sets `Idle` when barge-in cancels the in-flight response. Issue #34.
///
/// Playback is incremental (issue #16): the provider streams PCM chunks and
/// they are written to the speaker as they arrive.
#[allow(clippy::too_many_arguments)]
pub async fn tts_task(
    events: Arc<PipelineEvents>,
    pipeline_state_tx: Arc<watch::Sender<PipelineState>>,
    t_vad_end: Arc<Mutex<Option<Instant>>>,
    mut sentences_rx: mpsc::Receiver<PipelineFrame>,
    tts: Arc<TtsEngine>,
    audio_output: Arc<AudioOutput>,
    tts_sample_rate: u32,
    play_cancel: Arc<AtomicBool>,
    tts_muted: Arc<AtomicBool>,
    has_audio_device: bool,
    tui_tx: Option<TuiEventTx>,
    announcement_window: Arc<Mutex<crate::pipeline::announcement_window::AnnouncementWindow>>,
    // When Some(sender) is installed (remote client connected), TTS is routed there
    // instead of the local speaker. Always present so core does not need a `remote` feature.
    remote_tts_tx: RemoteTtsTx,
) {
    let mut cancel_rx = events.barge_in_tx.subscribe();
    let mut first_sentence = true;
    let mut last_utterance_id: Option<u64> = None;

    loop {
        // Barge-in is polled on every iteration — including while sentences are
        // queued — so a barge-in stops playback and drops the whole remaining
        // queue immediately, never after the response has drained.
        let next = tokio::select! {
            biased;
            _ = cancel_rx.recv() => {
                handle_barge_in(
                    &play_cancel,
                    &pipeline_state_tx,
                    &mut sentences_rx,
                    &mut cancel_rx,
                    &mut first_sentence,
                    &tui_tx,
                    &announcement_window,
                ).await;
                continue;
            }
            frame = sentences_rx.recv() => frame,
        };

        match next {
            Some(PipelineFrame::SentenceReady {
                sentence,
                utterance_id,
            }) => {
                if tts_muted.load(Ordering::SeqCst) {
                    continue;
                }
                announcement_window.lock().unwrap().queue_audio();

                if has_audio_device {
                    if last_utterance_id != Some(utterance_id) {
                        let _ = pipeline_state_tx.send(PipelineState::Speaking { utterance_id });
                        last_utterance_id = Some(utterance_id);
                    }

                    if let Some(ref tx) = tui_tx {
                        tx.send(TuiEvent::StateChange(
                            seneschal_common::tui_events::PipelineState::Speaking,
                        ))
                        .ok();
                    }
                }

                // Move this response from "queued" to "playing". Without the
                // matching `start_playback`, `queue_audio` accumulated forever
                // and `AnnouncementWindow::is_blocked()` stayed true for the
                // rest of the session — so background results (deep worker,
                // external agents) were never announced.
                announcement_window.lock().unwrap().start_playback();

                let outcome = exec_sentence(
                    &tts,
                    &audio_output,
                    &remote_tts_tx,
                    &events,
                    &TtsEngineStreamArgs {
                        sentence: &sentence,
                        sample_rate: tts_sample_rate,
                        cancel: &play_cancel,
                        has_audio_device,
                    },
                )
                .await;

                let playback_completed = match outcome {
                    SentenceOutcome::Completed => true,
                    SentenceOutcome::Cancelled => false,
                    SentenceOutcome::Error(e) => {
                        error!(target: "tts", "TTS synthesis error: {}", e);
                        if let Some(ref tx) = tui_tx {
                            tx.send(TuiEvent::Error(format!("TTS synthesis error: {e}")))
                                .ok();
                        }
                        true
                    }
                };

                if !playback_completed {
                    // Cancelled mid-playback: barge-in path drains and resets.
                    handle_barge_in(
                        &play_cancel,
                        &pipeline_state_tx,
                        &mut sentences_rx,
                        &mut cancel_rx,
                        &mut first_sentence,
                        &tui_tx,
                        &announcement_window,
                    )
                    .await;
                    continue;
                }

                if first_sentence && has_audio_device {
                    first_sentence = false;
                    if let Some(t0) = t_vad_end.lock().unwrap().as_ref() {
                        tracing::info!(target: "performance", "[+{}ms] SpeechStart → FirstAudioPlayback", t0.elapsed().as_millis());
                    }
                }

                announcement_window.lock().unwrap().finish_playback();
            }
            Some(PipelineFrame::LLMResponseDone { end_of_turn, .. }) => {
                // Frames are FIFO and playback is serialised in this loop, so an
                // end-of-turn frame is always observed after every `SentenceReady`
                // for that turn has finished playing. Only a real end-of-turn may
                // return the FSM to `Idle`; mid-turn flushes (`end_of_turn: false`,
                // e.g. a foreground tool's narration) keep the current state so
                // the follow-up answer does not briefly flash `Idle`.
                if end_of_turn {
                    try_set_idle(&pipeline_state_tx, &tui_tx, &announcement_window);
                }
            }
            Some(_) => continue,
            None => {
                // Channel closed — settle the FSM before exiting.
                try_set_idle(&pipeline_state_tx, &tui_tx, &announcement_window);
                break;
            }
        }
    }
}

/// Result of synthesising + playing one sentence.
enum SentenceOutcome {
    Completed,
    /// Barge-in interrupted playback (the caller must run the barge-in path).
    Cancelled,
    Error(anyhow::Error),
}

/// Synthesise `sentence` and stream it to the speaker (or remote sink).
#[allow(clippy::too_many_arguments)]
async fn exec_sentence(
    tts: &Arc<TtsEngine>,
    audio_output: &Arc<AudioOutput>,
    remote_tts_tx: &RemoteTtsTx,
    events: &Arc<PipelineEvents>,
    args: &TtsEngineStreamArgs<'_>,
) -> SentenceOutcome {
    // Subscribe *before* synthesis so a barge-in that lands while the model is
    // synthesizing is never lost (the subscription must exist when the
    // broadcast is sent). Synchronous backends cannot be preempted mid-synth,
    // so a cancel that arrives during synthesis is drained right below, before
    // any audio is played.
    let mut cancel_rx = events.barge_in_tx.subscribe();

    let synth = tts.synthesize_stream(args.sentence);
    tokio::pin!(synth);
    let mut stream = tokio::select! {
        biased;
        _ = cancel_rx.recv() => return SentenceOutcome::Cancelled,
        r = &mut synth => match r {
            Ok(s) => s,
            Err(e) => return SentenceOutcome::Error(e),
        },
    };

    // A cancel that arrived while a non-streaming backend was synthesizing.
    if cancel_rx.try_recv().is_ok() {
        return SentenceOutcome::Cancelled;
    }

    // Remote sink owns playback: accumulate chunks and send one packet.
    let remote_active = remote_tts_tx.lock().await.clone();
    if let Some(tx) = remote_active {
        let mut all: Vec<f32> = Vec::new();
        loop {
            tokio::select! {
                _ = cancel_rx.recv() => return SentenceOutcome::Cancelled,
                chunk = stream.rx.recv() => match chunk {
                    Some(chunk) => all.extend_from_slice(&chunk),
                    None => break,
                },
            }
        }
        let n = all.len();
        let packet = TtsAudioPacket {
            samples: all,
            sample_rate: args.sample_rate,
        };
        info!(
            target: "remote",
            "Routing TTS to remote client ({n} samples @ {} Hz)",
            args.sample_rate
        );
        if tx.send(packet).await.is_err() {
            tracing::warn!(target: "remote", "Remote TTS channel closed");
        }
        return SentenceOutcome::Completed;
    }

    if !args.has_audio_device {
        // Headless: drain the stream so the producer task finishes cleanly.
        while stream.rx.recv().await.is_some() {}
        return SentenceOutcome::Completed;
    }

    let session = match audio_output.start_stream(stream.sample_rate, Arc::clone(args.cancel)) {
        Ok(s) => s,
        Err(e) => return SentenceOutcome::Error(e),
    };

    loop {
        tokio::select! {
            _ = cancel_rx.recv() => {
                session.finish();
                drop(stream);
                return SentenceOutcome::Cancelled;
            }
            chunk = stream.rx.recv() => {
                match chunk {
                    Some(samples) => {
                        if args.cancel.load(Ordering::Relaxed) {
                            session.finish();
                            drop(stream);
                            return SentenceOutcome::Cancelled;
                        }
                        session.push(&samples);
                    }
                    None => break,
                }
            }
        }
    }

    session.finish();
    session.wait();
    SentenceOutcome::Completed
}

/// Bundle of args for [`exec_sentence`] (avoids a long parameter list).
struct TtsEngineStreamArgs<'a> {
    sentence: &'a str,
    sample_rate: u32,
    cancel: &'a Arc<AtomicBool>,
    has_audio_device: bool,
}

/// Handle a barge-in: cancel playback, drain queues, reset per-turn state.
async fn handle_barge_in(
    play_cancel: &Arc<AtomicBool>,
    pipeline_state_tx: &Arc<watch::Sender<PipelineState>>,
    sentences_rx: &mut mpsc::Receiver<PipelineFrame>,
    cancel_rx: &mut broadcast::Receiver<u64>,
    first_sentence: &mut bool,
    tui_tx: &Option<TuiEventTx>,
    announcement_window: &Arc<Mutex<crate::pipeline::announcement_window::AnnouncementWindow>>,
) {
    // Single ownership of play_cancel in this task avoids cross-writer races.
    play_cancel.store(true, Ordering::SeqCst);
    announcement_window.lock().unwrap().finish_playback();

    while sentences_rx.try_recv().is_ok() {}
    while cancel_rx.try_recv().is_ok() {}

    *first_sentence = true;
    play_cancel.store(false, Ordering::SeqCst);

    try_set_idle(pipeline_state_tx, tui_tx, announcement_window);
}

fn try_set_idle(
    pipeline_state_tx: &Arc<watch::Sender<PipelineState>>,
    tui_tx: &Option<TuiEventTx>,
    announcement_window: &Arc<Mutex<crate::pipeline::announcement_window::AnnouncementWindow>>,
) {
    if matches!(
        *pipeline_state_tx.borrow(),
        PipelineState::Thinking { .. } | PipelineState::Speaking { .. }
    ) {
        announcement_window.lock().unwrap().end_turn();
        let _ = pipeline_state_tx.send(PipelineState::Idle);
        if let Some(tx) = tui_tx {
            tx.send(TuiEvent::StateChange(
                seneschal_common::tui_events::PipelineState::Idle,
            ))
            .ok();
        }
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;
    use crate::pipeline::announcement_window::AnnouncementWindow;
    use crate::pipeline::fsm::PauseReason;
    use crate::tts::mock_tts::MockTts;
    use std::time::Duration;

    type Harness = (
        mpsc::Sender<PipelineFrame>,
        watch::Receiver<PipelineState>,
        tokio::task::JoinHandle<()>,
        Arc<Mutex<AnnouncementWindow>>,
    );

    /// Spawn a `tts_task` on a null audio output but with the `has_audio_device`
    /// gate enabled, so the FSM actually exercises `Speaking` (the real device
    /// check is irrelevant for state-machine coverage).
    ///
    /// The `AnnouncementWindow` the task owns is returned so tests can assert
    /// the audio-queued accounting (which gates result announcements).
    fn spawn(initial: PipelineState) -> Harness {
        let events = Arc::new(PipelineEvents::new());
        let (state_tx, state_rx) = watch::channel(initial);
        let (tx, rx) = mpsc::channel::<PipelineFrame>(16);
        let (mock, _captured) = MockTts::new();
        let announcement_window = Arc::new(Mutex::new(AnnouncementWindow::new()));
        let handle = tokio::spawn(tts_task(
            events,
            Arc::new(state_tx),
            Arc::new(Mutex::new(None)),
            rx,
            Arc::new(TtsEngine::Mock(mock)),
            Arc::new(AudioOutput::null()),
            22050,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            true,
            None,
            Arc::clone(&announcement_window),
            Arc::new(tokio::sync::Mutex::new(None)),
        ));
        (tx, state_rx, handle, announcement_window)
    }

    async fn send(tx: &mpsc::Sender<PipelineFrame>, frame: PipelineFrame) {
        tx.send(frame).await.expect("tts channel closed");
    }

    async fn wait_for_wire(rx: &mut watch::Receiver<PipelineState>, target: &str) {
        let reached = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if rx.borrow().control_wire_state() == target {
                    return;
                }
                if rx.changed().await.is_err() {
                    return;
                }
            }
        })
        .await;
        assert!(
            reached.is_ok(),
            "FSM did not reach `{target}` within timeout (now `{}`)",
            rx.borrow().control_wire_state()
        );
    }

    /// Regression for the observed bug: the end-of-turn frame is only consumed
    /// after the last sentence has already finished playing (FIFO), so the FSM
    /// used to stay stuck in `Speaking` forever. It must return to `Idle`.
    #[tokio::test]
    async fn returns_to_idle_when_end_of_turn_arrives_after_playback() {
        let (tx, mut rx, handle, _aw) = spawn(PipelineState::Thinking { utterance_id: 1 });

        send(
            &tx,
            PipelineFrame::SentenceReady {
                utterance_id: 1,
                sentence: "Hola.".into(),
            },
        )
        .await;
        wait_for_wire(&mut rx, "speaking").await;

        send(
            &tx,
            PipelineFrame::LLMResponseDone {
                utterance_id: 1,
                full_text: "Hola.".into(),
                end_of_turn: true,
            },
        )
        .await;
        wait_for_wire(&mut rx, "idle").await;

        handle.abort();
    }

    /// A mid-turn flush (foreground tool narration) must NOT idle: the follow-up
    /// answer continues the same spoken turn.
    #[tokio::test]
    async fn stays_speaking_on_mid_turn_flush_then_idles_on_end_of_turn() {
        let (tx, mut rx, handle, _aw) = spawn(PipelineState::Thinking { utterance_id: 1 });

        send(
            &tx,
            PipelineFrame::SentenceReady {
                utterance_id: 1,
                sentence: "Te lo busco.".into(),
            },
        )
        .await;
        wait_for_wire(&mut rx, "speaking").await;

        send(
            &tx,
            PipelineFrame::LLMResponseDone {
                utterance_id: 1,
                full_text: String::new(),
                end_of_turn: false,
            },
        )
        .await;
        // Let the task process the flush; state must remain `Speaking`.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(rx.borrow().control_wire_state(), "speaking");

        send(
            &tx,
            PipelineFrame::SentenceReady {
                utterance_id: 1,
                sentence: "Aquí está.".into(),
            },
        )
        .await;
        send(
            &tx,
            PipelineFrame::LLMResponseDone {
                utterance_id: 1,
                full_text: "Aquí está.".into(),
                end_of_turn: true,
            },
        )
        .await;
        wait_for_wire(&mut rx, "idle").await;

        handle.abort();
    }

    /// Empty response (no sentence ever queued): the end-of-turn frame is the
    /// only signal, and it must still return the FSM to `Idle`.
    #[tokio::test]
    async fn returns_to_idle_on_empty_response() {
        let (tx, mut rx, handle, _aw) = spawn(PipelineState::Thinking { utterance_id: 1 });

        send(
            &tx,
            PipelineFrame::LLMResponseDone {
                utterance_id: 1,
                full_text: String::new(),
                end_of_turn: true,
            },
        )
        .await;
        wait_for_wire(&mut rx, "idle").await;

        handle.abort();
    }

    /// Regression for the deep-worker "result never announced" bug: every
    /// `queue_audio` must be balanced by a `start_playback`, otherwise
    /// `audio_queued_count` grows forever and `pop_announcement` returns `None`
    /// for the rest of the session.
    #[tokio::test]
    async fn playback_start_balances_queue_audio_and_unblocks_announcements() {
        let (tx, mut rx, handle, aw) = spawn(PipelineState::Thinking { utterance_id: 1 });

        // A background result (deep worker) is queued while the turn speaks.
        aw.lock()
            .unwrap()
            .queue_announcement("tarea".into(), "resultado".into());

        send(
            &tx,
            PipelineFrame::SentenceReady {
                utterance_id: 1,
                sentence: "Procesando en segundo plano.".into(),
            },
        )
        .await;
        wait_for_wire(&mut rx, "speaking").await;

        send(
            &tx,
            PipelineFrame::LLMResponseDone {
                utterance_id: 1,
                full_text: "Procesando en segundo plano.".into(),
                end_of_turn: true,
            },
        )
        .await;
        wait_for_wire(&mut rx, "idle").await;

        // After playback the window must be open and the result deliverable.
        let announced = aw.lock().unwrap().pop_announcement();
        assert!(
            announced.is_some(),
            "announcement blocked after playback — start_playback accounting broken"
        );

        handle.abort();
    }

    /// `try_set_idle` must only reset active states — never `Idle` (no-op),
    /// `Listening` (still accumulating speech) or `Paused` (consolidation owns
    /// its own resume).
    #[test]
    fn try_set_idle_only_resets_active_states() {
        let aw = Arc::new(Mutex::new(AnnouncementWindow::new()));
        let cases = [
            (PipelineState::Idle, false),
            (PipelineState::Listening { utterance_id: 1 }, false),
            (PipelineState::Thinking { utterance_id: 1 }, true),
            (PipelineState::Speaking { utterance_id: 1 }, true),
            (
                PipelineState::Paused {
                    reason: PauseReason::Consolidation,
                },
                false,
            ),
        ];

        for (state, should_idle) in cases {
            let (tx, rx) = watch::channel(state.clone());
            try_set_idle(&Arc::new(tx), &None, &aw);
            let before = state.control_wire_state();
            let after = rx.borrow().control_wire_state();
            if should_idle {
                assert_eq!(after, "idle", "state {state:?} should reset to Idle");
            } else {
                assert_eq!(
                    after, before,
                    "state {state:?} must stay unchanged after try_set_idle"
                );
            }
        }
    }

    /// Regression: barge-in must cancel immediately, even mid-synthesis and
    /// with sentences still queued. Before the fix the loop drained the queue
    /// via `try_recv` without ever polling `cancel_rx`, so a whole response
    /// played before the barge-in was honored. Held synthesis forces the
    /// cancel to land while the task is inside `exec_sentence`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn barge_in_cancels_during_synthesis_and_drops_queue() {
        let events = Arc::new(PipelineEvents::new());
        let (state_tx, mut state_rx) = watch::channel(PipelineState::Thinking { utterance_id: 1 });
        let (tx, rx) = mpsc::channel::<PipelineFrame>(16);
        let (mock, captured, gate) = MockTts::gated();
        let announcement_window = Arc::new(Mutex::new(AnnouncementWindow::new()));

        let handle = tokio::spawn(tts_task(
            Arc::clone(&events),
            Arc::new(state_tx),
            Arc::new(Mutex::new(None)),
            rx,
            Arc::new(TtsEngine::Mock(mock)),
            Arc::new(AudioOutput::null()),
            22050,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            true,
            None,
            Arc::clone(&announcement_window),
            Arc::new(tokio::sync::Mutex::new(None)),
        ));

        send(
            &tx,
            PipelineFrame::SentenceReady {
                utterance_id: 1,
                sentence: "Primera.".into(),
            },
        )
        .await;
        send(
            &tx,
            PipelineFrame::SentenceReady {
                utterance_id: 1,
                sentence: "Segunda.".into(),
            },
        )
        .await;

        // Wait until the first sentence entered (held) synthesis.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while captured.lock().unwrap().is_empty() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "synthesis never started"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // Barge-in while synthesis is held, then release it.
        events.barge_in_tx.send(0).ok();
        gate.store(true, Ordering::SeqCst);

        wait_for_wire(&mut state_rx, "idle").await;
        // Give the (dropped) second sentence a chance to (wrongly) surface.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            *captured.lock().unwrap(),
            vec!["Primera.".to_string()],
            "the queued second sentence must be dropped on barge-in"
        );

        handle.abort();
    }
}
