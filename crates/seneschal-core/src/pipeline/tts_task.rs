use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::{Notify, broadcast, mpsc, watch};
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
    let mut play_handle: Option<tokio::task::JoinHandle<anyhow::Result<()>>> = None;
    let mut first_sentence = true;
    let mut last_utterance_id: Option<u64> = None;
    let mut no_more_sentences = false;
    let playback_done = Arc::new(Notify::new());

    loop {
        if no_more_sentences && play_handle.is_none() {
            try_set_idle(&pipeline_state_tx, &tui_tx, &announcement_window);
            no_more_sentences = false;
        }

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
                            &mut play_handle,
                            &mut sentences_rx,
                            &mut cancel_rx,
                            &mut first_sentence,
                            &mut no_more_sentences,
                            &tui_tx,
                            &announcement_window,
                        ).await;
                        continue;
                    }
                    _ = playback_done.notified() => {
                        if let Some(h) = play_handle.take() {
                            match h.await {
                                Ok(Ok(())) => {}
                                Ok(Err(e)) => error!(target: "audio", "Playback error: {}", e),
                                Err(e) => error!(target: "audio", "Playback task join failed: {}", e),
                            }
                        }
                        announcement_window.lock().unwrap().finish_playback();
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

                // Ensure previous playback fully stops before starting next sentence.
                if let Some(mut h) = play_handle.take() {
                    let mut cancelled = false;
                    loop {
                        tokio::select! {
                            result = &mut h => {
                                if let Ok(Err(e)) = result {
                                    error!(target: "audio", "Playback error: {}", e);
                                }
                                break;
                            },
                            _ = cancel_rx.recv() => {
                                play_cancel.store(true, Ordering::SeqCst);
                                cancelled = true;
                                // Keep awaiting the playback task until CPAL callback sees
                                // play_cancel and the task actually finishes.
                            }
                        }
                    }
                    if cancelled {
                        handle_barge_in(
                            &play_cancel,
                            &pipeline_state_tx,
                            &mut play_handle,
                            &mut sentences_rx,
                            &mut cancel_rx,
                            &mut first_sentence,
                            &mut no_more_sentences,
                            &tui_tx,
                            &announcement_window,
                        )
                        .await;
                        continue;
                    }
                }

                let tts_c = Arc::clone(&tts);
                let sentence_c = sentence.clone();
                let mut synth_handle =
                    tokio::task::spawn_blocking(move || tts_c.synthesize(&sentence_c));

                let samples = tokio::select! {
                    _ = cancel_rx.recv() => {
                        synth_handle.abort();
                        handle_barge_in(
                            &play_cancel,
                            &pipeline_state_tx,
                            &mut play_handle,
                            &mut sentences_rx,
                            &mut cancel_rx,
                            &mut first_sentence,
                            &mut no_more_sentences,
                            &tui_tx,
                            &announcement_window,
                        ).await;
                        continue;
                    }
                    result = &mut synth_handle => {
                        match result {
                            Ok(Ok(s)) => s,
                            Ok(Err(e)) => {
                                error!(target: "tts", "TTS synthesis error: {}", e);
                                if let Some(ref tx) = tui_tx {
                                    tx.send(TuiEvent::Error(format!("TTS synthesis error: {e}"))).ok();
                                }
                                continue;
                            }
                            Err(e) => {
                                error!(target: "tts", "TTS task panicked: {e}");
                                if let Some(ref tx) = tui_tx {
                                    tx.send(TuiEvent::Error(format!("TTS task panicked: {e}"))).ok();
                                }
                                continue;
                            }
                        }
                    }
                };

                if first_sentence {
                    first_sentence = false;
                    if let Some(t0) = t_vad_end.lock().unwrap().as_ref() {
                        tracing::info!(target: "performance", "[+{}ms] SpeechStart → FirstAudioPlayback", t0.elapsed().as_millis());
                    }
                }

                // Prefer remote client when one is connected (exclusive TTS routing).
                {
                    let maybe_tx = remote_tts_tx.lock().await.clone();
                    if let Some(tx) = maybe_tx {
                        let n = samples.len();
                        let packet = TtsAudioPacket {
                            samples,
                            sample_rate: tts_sample_rate,
                        };
                        info!(
                            target: "remote",
                            "Routing TTS to remote client ({n} samples @ {tts_sample_rate} Hz)"
                        );
                        if tx.send(packet).await.is_err() {
                            tracing::warn!(target: "remote", "Remote TTS channel closed");
                        }
                        // Remote sink owns playback duration; don't block on local CPAL.
                        announcement_window.lock().unwrap().start_playback();
                        announcement_window.lock().unwrap().finish_playback();
                        continue;
                    }
                }

                let out_c = Arc::clone(&audio_output);
                let cancel_c = Arc::clone(&play_cancel);
                let notify_c = Arc::clone(&playback_done);
                let rate = tts_sample_rate;
                play_handle = Some(tokio::task::spawn(async move {
                    let r = match tokio::task::spawn_blocking(move || {
                        out_c.play_blocking(&samples, rate, &cancel_c)
                    })
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => Err(anyhow::anyhow!("playback task join failed: {e}")),
                    };
                    notify_c.notify_one();
                    r
                }));
                announcement_window.lock().unwrap().start_playback();
            }
            Some(PipelineFrame::LLMResponseDone { .. }) => {
                no_more_sentences = true;
                if play_handle.is_none() {
                    try_set_idle(&pipeline_state_tx, &tui_tx, &announcement_window);
                    no_more_sentences = false;
                }
            }
            Some(_) => continue,
            None => break,
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_barge_in(
    play_cancel: &Arc<AtomicBool>,
    pipeline_state_tx: &Arc<watch::Sender<PipelineState>>,
    play_handle: &mut Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    sentences_rx: &mut mpsc::Receiver<PipelineFrame>,
    cancel_rx: &mut broadcast::Receiver<u64>,
    first_sentence: &mut bool,
    no_more_sentences: &mut bool,
    tui_tx: &Option<TuiEventTx>,
    announcement_window: &Arc<Mutex<crate::pipeline::announcement_window::AnnouncementWindow>>,
) {
    // Single ownership of play_cancel in this task avoids cross-writer races.
    play_cancel.store(true, Ordering::SeqCst);

    if let Some(handle) = play_handle.take() {
        match handle.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => error!(target: "audio", "Playback error during barge-in: {}", e),
            Err(e) => error!(target: "audio", "Playback task join failed during barge-in: {}", e),
        }
    }
    announcement_window.lock().unwrap().finish_playback();

    while sentences_rx.try_recv().is_ok() {}
    while cancel_rx.try_recv().is_ok() {}

    *first_sentence = true;
    *no_more_sentences = false;
    play_cancel.store(false, Ordering::SeqCst);

    try_set_idle(pipeline_state_tx, tui_tx, announcement_window);
}

fn try_set_idle(
    pipeline_state_tx: &watch::Sender<PipelineState>,
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
