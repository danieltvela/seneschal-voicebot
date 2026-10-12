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
/// - Sets `Idle` when playback finishes AND the LLM has signalled it is done
///   streaming (so no more sentences are coming), or when barge-in cancels
///   the in-flight response. Issue #34.
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
    // Set when the LLM signalled end-of-response; used to go idle as soon as the
    // final sentence finishes playing.
    let mut llm_done = false;

    loop {
        // Drain queue first for low latency; block only when empty.
        let next = match sentences_rx.try_recv() {
            Ok(frame) => Some(frame),
            Err(mpsc::error::TryRecvError::Empty) => {
                tokio::select! {
                    frame = sentences_rx.recv() => frame,
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
                }
            }
            Err(mpsc::error::TryRecvError::Disconnected) => {
                try_set_idle(&pipeline_state_tx, &tui_tx, &announcement_window);
                break;
            }
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

                // A new utterance id means a fresh response started.
                if last_utterance_id != Some(utterance_id) {
                    llm_done = false;
                }

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

                // Last sentence of the response already played → go idle.
                if llm_done {
                    try_set_idle(&pipeline_state_tx, &tui_tx, &announcement_window);
                    llm_done = false;
                }
            }
            Some(PipelineFrame::LLMResponseDone { .. }) => {
                llm_done = true;
                // No sentence was playing (e.g. empty response) → idle now.
                if last_utterance_id.is_none() {
                    try_set_idle(&pipeline_state_tx, &tui_tx, &announcement_window);
                    llm_done = false;
                }
            }
            Some(_) => continue,
            None => break,
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
    let mut stream = match tts.synthesize_stream(args.sentence).await {
        Ok(s) => s,
        Err(e) => return SentenceOutcome::Error(e),
    };

    // A dedicated cancel subscription lets us abort inside this helper without
    // stealing the main loop's receiver.
    let mut cancel_rx = events.barge_in_tx.subscribe();

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
