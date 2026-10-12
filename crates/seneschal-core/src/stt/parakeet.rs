use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use parakeet_rs::{ParakeetTDT, Transcriber};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use whisper_cpp_plus::enhanced::fallback::calculate_compression_ratio;

use super::SpeechEvent;
use super::no_speech_gate::TranscriptionQuality;
use super::provider::SttProvider;
use super::segmenter::{SAMPLE_RATE, SegmenterEvent, SileroDetector, VadSegmenter};
use super::streaming::{LiveDecoder, LiveStreamer};
use super::whisper::{WhisperSTTVADConfig, load_vad, segmenter_config};

/// Load the Parakeet TDT model, placing the ONNX Runtime session on `gpu`
/// when requested (issue #14).
#[cfg(feature = "cuda")]
fn load_parakeet_model(model_dir: &str, gpu: Option<i32>) -> parakeet_rs::Result<ParakeetTDT> {
    use parakeet_rs::ExecutionConfig;

    let config = match gpu {
        Some(device) => Some(
            ExecutionConfig::new().with_custom_configure(move |builder| {
                use ort::ep::{CPU, CUDA};
                // Register a single CUDA EP with the requested device id plus a CPU
                // fallback. We intentionally keep parakeet-rs' `execution_provider`
                // at its default (CPU) so it does not register a second, device-0
                // CUDA EP before this custom configure runs.
                builder
                    .with_execution_providers([
                        CUDA::default().with_device_id(device).build(),
                        CPU::default().build().error_on_failure(),
                    ])
                    .map_err(Into::into)
            }),
        ),
        None => None,
    };

    ParakeetTDT::from_pretrained(model_dir, config)
}

#[cfg(not(feature = "cuda"))]
fn load_parakeet_model(model_dir: &str, gpu: Option<i32>) -> parakeet_rs::Result<ParakeetTDT> {
    if let Some(device) = gpu {
        tracing::warn!(
            target: "sttvad",
            "Parakeet assigned to GPU {device} but this build lacks the 'cuda' feature; running on CPU (rebuild with --features cuda, or set GPU_AUTO=0)"
        );
    }
    ParakeetTDT::from_pretrained(model_dir, None)
}

pub struct ParakeetSttProvider {
    model: Arc<Mutex<ParakeetTDT>>,
    segmenter: VadSegmenter<SileroDetector>,
    streamer: LiveStreamer,
}

/// Blocking Parakeet decoder over one snapshot of the open segment (live).
struct ParakeetDecoder {
    model: Arc<Mutex<ParakeetTDT>>,
}

impl LiveDecoder for ParakeetDecoder {
    fn decode(&self, audio: Vec<f32>) -> Result<TranscriptionQuality> {
        transcribe(&self.model, &audio)
    }
}

impl ParakeetSttProvider {
    pub fn new(base: WhisperSTTVADConfig, parakeet_model_dir: Option<&str>) -> Result<Self> {
        let model_dir = parakeet_model_dir
            .ok_or_else(|| anyhow!("PARAKEET_MODEL_DIR must be set when STT_PROVIDER=parakeet"))?;
        let model = load_parakeet_model(model_dir, base.parakeet_gpu)
            .with_context(|| format!("Failed to load Parakeet TDT model from: {}\n\nThe model directory must contain ONNX Runtime files (encoder-model.onnx, decoder_joint-model.onnx, vocab.txt).\nYou may have downloaded the wrong format — parakeet-rs requires the ONNX export, not the HuggingFace Transformers model (.safetensors) or NeMo checkpoint (.nemo).\n\nSolution: download the correct ONNX model from HuggingFace:\n  mkdir -p {}\n  cd {}\n  wget https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/encoder-model.onnx\n  wget https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/decoder_joint-model.onnx\n  wget https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/vocab.txt\n  # Optional external data (if present):\n  wget https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/encoder-model.onnx.data\n", model_dir, model_dir, model_dir))?;

        let vad = load_vad(&base.vad_model, base.vad_gpu)?;
        let segmenter = VadSegmenter::new(SileroDetector::new(vad), segmenter_config(&base))?;
        let streamer = LiveStreamer::new(base.live.clone());

        tracing::info!(
            target: "sttvad",
            "ParakeetSttProvider ready (parakeet: {}, vad: {}, lang: {}, silence_ms: {}, start_thr: {:.2}, end_thr: {:.2}, confirm_probes: {}, probe_ms: {}, preroll_ms: {}, post_roll_ms: {}, live: {}, parakeet_gpu: {}, vad_gpu: {})",
            model_dir,
            base.vad_model,
            base.language,
            base.silence_ms,
            base.vad_start_threshold,
            base.vad_end_threshold,
            base.vad_confirm_probes.max(1),
            base.probe_ms,
            base.preroll_ms,
            base.post_roll_ms,
            base.live.enabled,
            base.parakeet_gpu
                .map(|i| i.to_string())
                .unwrap_or_else(|| "cpu".into()),
            base.vad_gpu
                .map(|i| i.to_string())
                .unwrap_or_else(|| "cpu".into()),
        );

        Ok(Self {
            model: Arc::new(Mutex::new(model)),
            segmenter,
            streamer,
        })
    }

    /// Feed a chunk of 16 kHz mono f32 audio. Live partial decodes run while
    /// the segment is open (contract P3); the close completes via a detached
    /// task (contract P4) — decoding never blocks the VAD feed.
    pub async fn process_audio(
        &mut self,
        audio: &[f32],
        tx: &mpsc::Sender<SpeechEvent>,
    ) -> Result<()> {
        self.streamer.poll(&tx).await;

        for event in self.segmenter.feed(audio) {
            match event {
                SegmenterEvent::SpeechStart { segment_seq } => {
                    self.streamer.on_segment_start(segment_seq);
                    let _ = tx.send(SpeechEvent::SpeechStart { segment_seq }).await;
                }
                SegmenterEvent::SegmentReady {
                    segment_seq,
                    audio: segment,
                    trailing_silence,
                } => {
                    tracing::debug!(
                        target: "sttvad",
                        "Finalizing segment {segment_seq}: {:.2}s",
                        segment.len() as f32 / SAMPLE_RATE as f32
                    );

                    let speech_end_offset = segment.len().saturating_sub(trailing_silence);
                    let decoder = ParakeetDecoder {
                        model: Arc::clone(&self.model),
                    };
                    self.streamer.finish_detached(
                        segment_seq,
                        segment,
                        speech_end_offset,
                        decoder,
                        tx.clone(),
                    );
                }
            }
        }

        let seq = self.segmenter.current_segment_seq();
        self.streamer.maybe_spawn(
            seq,
            self.segmenter.in_speech(),
            self.segmenter.take_closing_trigger(),
            self.segmenter.speech_buf(),
            ParakeetDecoder {
                model: Arc::clone(&self.model),
            },
        );

        Ok(())
    }

    pub fn transcribe_complete(&self, audio: &[f32]) -> Result<TranscriptionQuality> {
        transcribe(&self.model, audio)
    }
}

#[async_trait]
impl SttProvider for ParakeetSttProvider {
    fn provider_name(&self) -> &'static str {
        "parakeet"
    }

    async fn process_audio(&mut self, audio: &[f32], tx: &mpsc::Sender<SpeechEvent>) -> Result<()> {
        ParakeetSttProvider::process_audio(self, audio, tx).await
    }

    fn transcribe_complete(&self, audio: &[f32]) -> Result<TranscriptionQuality> {
        ParakeetSttProvider::transcribe_complete(self, audio)
    }

    fn inflight_transcriptions(&self) -> usize {
        self.streamer.inflight()
    }
}

fn transcribe(model: &Arc<Mutex<ParakeetTDT>>, audio: &[f32]) -> Result<TranscriptionQuality> {
    if audio.is_empty() {
        return Ok(TranscriptionQuality {
            text: String::new(),
            no_speech_prob: 0.0,
            avg_logprob: 0.0,
            compression_ratio: 0.0,
        });
    }

    let mut model = model
        .lock()
        .map_err(|_| anyhow!("Parakeet model lock poisoned"))?;

    let result = model
        .transcribe_samples(audio.to_vec(), SAMPLE_RATE as u32, 1, None)
        .context("Parakeet transcription failed")?;

    let text = result.text.trim().to_string();

    let compression_ratio = if !text.is_empty() {
        calculate_compression_ratio(&text)
    } else {
        0.0
    };

    Ok(TranscriptionQuality {
        text,
        no_speech_prob: 0.0,
        avg_logprob: 0.0,
        compression_ratio,
    })
}
