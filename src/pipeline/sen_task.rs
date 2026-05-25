use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::info;

use super::state::PipelineEvents;
use crate::pipeline::frames::PipelineFrame;
use crate::tts::SentenceSplitter;

/// SEN task: receives LLM tokens via typed channel, splits them into sentences,
/// and forwards complete sentences to tts_task via a second typed channel.
pub async fn sen_task(
    events: Arc<PipelineEvents>,
    mut llm_rx: mpsc::Receiver<PipelineFrame>,
    sentences_tx: mpsc::Sender<PipelineFrame>,
    t_vad_end: Arc<Mutex<Option<Instant>>>,
    t_llm_post_send: Arc<Mutex<Option<Instant>>>,
) {
    let mut cancel_rx = events.barge_in_tx.subscribe();
    let mut splitter = SentenceSplitter::new();
    let mut first_sentence_logged = false;

    loop {
        tokio::select! {
            frame = llm_rx.recv() => {
                match frame {
                    Some(PipelineFrame::LLMToken { utterance_id, token }) => {
                        if !first_sentence_logged {
                            // Log latency on first token reaching sen_task.
                            if let Some(t0) = t_vad_end.lock().unwrap().as_ref() {
                                let ms = t0.elapsed().as_millis();
                                info!(target: "performance", "[+{}ms] first token → sentence splitter", ms);
                            }
                        }
                        if let Some(sentence) = splitter.push(&token) {
                            if !first_sentence_logged {
                                first_sentence_logged = true;
                                if let Some(t0) = t_vad_end.lock().unwrap().as_ref() {
                                    let tts_queue_ms = t0.elapsed().as_millis();
                                    info!(target: "performance", "[+{}ms] first sentence → TTS queue", tts_queue_ms);
                                    if let Some(t_llm_sent) = t_llm_post_send.lock().unwrap().as_ref() {
                                        info!(target: "performance", "  └─ LLM processing: {}ms", t_llm_sent.elapsed().as_millis());
                                    }
                                }
                            }
                            let _ = sentences_tx.send(PipelineFrame::SentenceReady { utterance_id, sentence }).await;
                        }
                    }
                    Some(PipelineFrame::LLMResponseDone { utterance_id, .. }) => {
                        if let Some(sentence) = splitter.flush() {
                            let _ = sentences_tx.send(PipelineFrame::SentenceReady { utterance_id, sentence }).await;
                        }
                        first_sentence_logged = false;
                    }
                    Some(_) => {}
                    None => {} // channel closed — exit
                }
            }
            _ = cancel_rx.recv() => {
                // Drain buffered tokens from the cancelled turn.
                while llm_rx.try_recv().is_ok() {}
                splitter = SentenceSplitter::new();
                first_sentence_logged = false;
                while cancel_rx.try_recv().is_ok() {}
            }
        }
    }
}

#[cfg(test)]
mod latency_tests {
    use std::time::Instant;

    /// Instant::now() overhead must stay below 1ms micro-benchmark.
    #[test]
    fn instant_overhead_baseline() {
        const ITERATIONS: usize = 100_000;
        let start = Instant::now();
        for _ in 0..ITERATIONS {
            let _ = Instant::now();
        }
        let total_us = start.elapsed().as_micros();
        let avg_ns = total_us as f64 / ITERATIONS as f64;
        let avg_ms = avg_ns / 1_000.0;
        assert!(
            avg_ms < 1.0,
            "Instant::now() avg overhead {:.4}ms (avg {:.*}ns) — expected < 1ms",
            avg_ms,
            2,
            avg_ns
        );
    }

    /// Elapsed calculation correctly captures wall-clock duration.
    #[test]
    fn elapsed_logs_correctly() {
        let t0 = Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let elapsed_ms = t0.elapsed().as_millis();
        assert!(
            elapsed_ms >= 50,
            "logged latency {}ms should be ≥ 50ms",
            elapsed_ms
        );
    }
}
