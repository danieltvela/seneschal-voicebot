//! OpenAI-compatible remote STT provider (issue #17).
//!
//! VAD, utterance segmentation and the `SpeechStart` / `SpeechEnd` event flow
//! stay local (shared [`VadSegmenter`](super::segmenter::VadSegmenter)); only a
//! finalized utterance is POSTed as a batch request to
//! `POST /v1/audio/transcriptions`. The reference deployment is `seneschal-asr`
//! (vLLM serving `Qwen3-ASR-1.7B` on port 8024), but the provider targets the
//! OpenAI transcription API shape, so whisper.cpp server, Speaches or any other
//! compatible endpoint work too.
//!
//! Verified against `seneschal-asr` (`qwen3-asr`):
//!   * default `response_format=json` → `{"text": "...", "usage": {...}}`;
//!   * `verbose_json` is rejected (HTTP 400) — so we never request it;
//!   * `language=auto` is rejected (HTTP 400) — so we omit `language` for
//!     auto-detection and only send a concrete ISO-639-1 code otherwise;
//!   * silence/noise return an empty `text` (no hallucinated speech).
//!
//! ## NoSpeechGate behavior without Whisper metrics
//!
//! `qwen3-asr` does not return `no_speech_prob` / `avg_logprob` /
//! `compression_ratio`. We request the plain `json` response and map whatever
//! quality fields the server happens to return (top-level or per-segment); when
//! they are absent we fall back to `0.0` and compute `compression_ratio`
//! locally from the text — exactly the pass-through behavior Parakeet already
//! uses. The gate therefore still rejects empty or gibberish transcriptions;
//! for this server the primary protection is the empty-text case, which the
//! model returns for genuine non-speech.

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use whisper_cpp_plus::enhanced::fallback::calculate_compression_ratio;

use super::SpeechEvent;
use super::no_speech_gate::TranscriptionQuality;
use super::provider::SttProvider;
use super::segmenter::{SAMPLE_RATE, SegmenterEvent, SileroDetector, SpeechDetector, VadSegmenter};
use super::streaming::LiveStreamConfig;
use super::wav;
use super::whisper::{WhisperSTTVADConfig, load_vad, segmenter_config};

/// TCP connect timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Runtime configuration for [`OpenAiSttProvider`].
#[derive(Debug, Clone)]
pub struct OpenAiSttConfig {
    /// Service base URL, e.g. `http://localhost:8024`.
    pub base_url: String,
    /// Bearer token; empty = no `Authorization` header.
    pub api_key: String,
    /// Model name sent in the multipart `model` field.
    pub model: String,
    /// Language hint: `"auto"`/empty omits the field (server auto-detects);
    /// a concrete code is normalized to its primary ISO-639-1 subtag.
    pub language: String,
    /// Per-request timeout in milliseconds (covers the whole round trip).
    pub timeout_ms: u64,
}

/// Body of a successful `POST /v1/audio/transcriptions` (`json`, and the
/// optional quality fields some servers include).
#[derive(Debug, serde::Deserialize)]
struct TranscriptionResponse {
    #[serde(default)]
    text: String,
    #[serde(default)]
    no_speech_prob: Option<f32>,
    #[serde(default)]
    avg_logprob: Option<f32>,
    #[serde(default)]
    compression_ratio: Option<f32>,
    /// `verbose_json` style per-segment metrics, used when present.
    #[serde(default)]
    segments: Option<Vec<Segment>>,
}

#[derive(Debug, serde::Deserialize)]
struct Segment {
    #[serde(default)]
    no_speech_prob: Option<f32>,
    #[serde(default)]
    avg_logprob: Option<f32>,
    #[serde(default)]
    compression_ratio: Option<f32>,
}

#[derive(Debug, serde::Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Option<Vec<ModelEntry>>,
}

#[derive(Debug, serde::Deserialize)]
struct ModelEntry {
    #[serde(default)]
    id: String,
}

/// STT provider that transcribes finalized utterances against an
/// OpenAI-compatible `/v1/audio/transcriptions` endpoint.
///
/// Live partials (contract P3): while the segment is open, the buffered
/// prefix is POSTed every `live.interval_ms`; partial texts arrive as
/// `SpeechEvent::Speech`. At close a fresh request covering the complete
/// utterance produces the final `SpeechEnd` (contract P4). Requests run
/// detached — the audio loop is never blocked by the network.
///
/// Generic over the speech detector so the full VAD + segmentation + HTTP
/// pipeline can be exercised in tests without loading Silero.
pub struct OpenAiSttProvider<D: SpeechDetector = SileroDetector> {
    client: reqwest::Client,
    cfg: OpenAiSttConfig,
    segmenter: VadSegmenter<D>,
    live_cfg: LiveStreamConfig,
    /// Live-partial state for the segment currently open.
    live: Option<LiveRemote>,
    /// In-flight transcription token (final request), cancelled on barge-in.
    pending: Option<CancellationToken>,
    /// Number of transcriptions currently in flight (observability + tests).
    inflight: Arc<AtomicUsize>,
}

/// Live-prefix request in flight for the open segment.
struct PendingLive {
    token: CancellationToken,
}

/// Per-segment live state.
struct LiveRemote {
    seq: u64,
    /// Segment samples already dispatched (cadence is measured against this).
    covered_upto: usize,
    pending: Vec<PendingLive>,
}

impl OpenAiSttProvider<SileroDetector> {
    /// Production constructor: loads the local Silero VAD for segmentation.
    pub fn new(base: WhisperSTTVADConfig, cfg: OpenAiSttConfig) -> Result<Self> {
        let vad = load_vad(&base.vad_model, base.vad_gpu)?;
        Self::with_detector(SileroDetector::new(vad), &base, cfg)
    }
}

impl<D: SpeechDetector + 'static> OpenAiSttProvider<D> {
    /// Build a provider around an injected speech detector (used by tests and
    /// by [`OpenAiSttProvider::new`]).
    pub fn with_detector(
        detector: D,
        base: &WhisperSTTVADConfig,
        cfg: OpenAiSttConfig,
    ) -> Result<Self> {
        if cfg.base_url.trim().is_empty() {
            bail!("STT_URL must not be empty when STT_PROVIDER=openai");
        }
        if cfg.model.trim().is_empty() {
            bail!("STT_MODEL must not be empty when STT_PROVIDER=openai");
        }

        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(Duration::from_millis(cfg.timeout_ms.max(1_000)))
            .build()
            .map_err(map_reqwest)?;

        let segmenter = VadSegmenter::new(detector, segmenter_config(base))?;

        tracing::info!(
            target: "sttvad",
            "OpenAiSttProvider ready (url: {}, model: {}, lang: {}, timeout_ms: {}, vad: {}, silence_ms: {}, start_thr: {:.2}, end_thr: {:.2}, confirm_probes: {}, probe_ms: {}, preroll_ms: {}, post_roll_ms: {}, live: {}, vad_gpu: {})",
            cfg.base_url,
            cfg.model,
            cfg.language,
            cfg.timeout_ms,
            base.vad_model,
            base.silence_ms,
            base.vad_start_threshold,
            base.vad_end_threshold,
            base.vad_confirm_probes.max(1),
            base.probe_ms,
            base.preroll_ms,
            base.post_roll_ms,
            base.live.enabled,
            base.vad_gpu.map(|i| i.to_string()).unwrap_or_else(|| "cpu".into()),
        );

        Ok(Self {
            client,
            cfg,
            segmenter,
            live_cfg: base.live.clone(),
            live: None,
            pending: None,
            inflight: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Surface a bad URL/model early at startup (issue #17, requirement 6).
    ///
    /// Best-effort: a transport failure is returned to the caller (which may
    /// log and continue, since the service can still be starting), but an
    /// unknown model only produces a warning — some gateways accept aliases.
    pub async fn verify_server(&self) -> Result<()> {
        let url = format!("{}/v1/models", self.cfg.base_url.trim_end_matches('/'));
        let mut req = self.client.get(&url);
        if !self.cfg.api_key.trim().is_empty() {
            req = req.bearer_auth(self.cfg.api_key.trim());
        }
        let resp = req.send().await.map_err(map_reqwest)?;
        let status = resp.status();
        let body = resp.bytes().await.map_err(map_reqwest)?;
        if !status.is_success() {
            bail!(
                "HTTP {} from {url}: {}",
                status.as_u16(),
                extract_error_message(&body)
            );
        }

        let models: ModelsResponse =
            serde_json::from_slice(&body).context("malformed /v1/models response")?;
        match models.data {
            Some(list) if list.iter().any(|m| m.id == self.cfg.model) => {
                tracing::info!(
                    target: "stt",
                    "Remote STT model '{}' available at {}",
                    self.cfg.model,
                    self.cfg.base_url
                );
            }
            Some(list) => {
                let available: Vec<&str> = list.iter().take(10).map(|m| m.id.as_str()).collect();
                tracing::warn!(
                    target: "stt",
                    "Remote STT model '{}' not listed by {} (available: {}). Requests may fail.",
                    self.cfg.model,
                    self.cfg.base_url,
                    if available.is_empty() {
                        "<none>".to_string()
                    } else {
                        available.join(", ")
                    }
                );
            }
            None => {
                tracing::debug!(
                    target: "stt",
                    "{} returned no model list; skipping model check",
                    self.cfg.base_url
                );
            }
        }
        Ok(())
    }

    /// Feed a chunk of 16 kHz mono f32 audio.
    ///
    /// While the segment is open, live prefix requests are dispatched every
    /// `live.interval_ms` (contract P3). A finalized utterance dispatches one
    /// final request covering the complete audio (contract P4); both kinds run
    /// detached so the pipeline is never blocked by the HTTP round trip. A
    /// barge-in aborts pending requests via [`OpenAiSttProvider::cancel_inflight`].
    pub async fn process_audio(
        &mut self,
        audio: &[f32],
        tx: &mpsc::Sender<SpeechEvent>,
    ) -> Result<()> {
        // Reap cancelled live requests (their partials were sent directly).
        if let Some(live) = &mut self.live {
            live.pending.retain(|p| !p.token.is_cancelled());
        }

        for event in self.segmenter.feed(audio) {
            match event {
                SegmenterEvent::SpeechStart { segment_seq } => {
                    // A new segment supersedes the previous one: abort its live
                    // prefix requests before attaching to the new one.
                    if let Some(mut superseded) = self.live.take() {
                        for p in superseded.pending.drain(..) {
                            p.token.cancel();
                        }
                    }
                    self.live = Some(LiveRemote {
                        seq: segment_seq,
                        covered_upto: 0,
                        pending: Vec::new(),
                    });
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

                    // The final request covers the complete utterance; any
                    // live prefix request is superseded by it.
                    if let Some(mut live) = self.live.take() {
                        for p in live.pending.drain(..) {
                            p.token.cancel();
                        }
                    }
                    let speech_end_offset = segment.len().saturating_sub(trailing_silence);
                    tracing::debug!(
                        target: "stt",
                        "Final request covers {speech_end_offset} spoken samples of {}",
                        segment.len()
                    );

                    let token = CancellationToken::new();
                    self.pending = Some(token.clone());
                    self.inflight.fetch_add(1, Ordering::SeqCst);
                    let client = self.client.clone();
                    let cfg = self.cfg.clone();
                    let tx = tx.clone();
                    let inflight = Arc::clone(&self.inflight);
                    tokio::spawn(async move {
                        dispatch_transcription(
                            client,
                            cfg,
                            segment,
                            segment_seq,
                            token,
                            tx,
                            inflight,
                        )
                        .await;
                    });
                }
            }
        }

        // Live cadence: dispatch a prefix request when the interval elapsed
        // (runs detached; never blocks the audio loop).
        if self.live_cfg.enabled
            && let Some(live) = &mut self.live
            && self.segmenter.in_speech()
        {
            let buf = self.segmenter.speech_buf();
            let seq = live.seq;
            if buf.len().saturating_sub(live.covered_upto)
                >= SAMPLE_RATE * self.live_cfg.interval_ms.max(1) as usize / 1000
            {
                let window =
                    (SAMPLE_RATE * self.live_cfg.window_ms.max(1) as usize / 1000).min(buf.len());
                let snapshot = buf[buf.len() - window..].to_vec();
                live.covered_upto = buf.len();
                let token = CancellationToken::new();
                live.pending.push(PendingLive {
                    token: token.clone(),
                });
                self.inflight.fetch_add(1, Ordering::SeqCst);
                let client = self.client.clone();
                let cfg = self.cfg.clone();
                let tx = tx.clone();
                let inflight = Arc::clone(&self.inflight);
                tokio::spawn(async move {
                    dispatch_live_transcription(client, cfg, snapshot, seq, token, tx, inflight)
                        .await;
                });
            }
        }

        Ok(())
    }

    /// Cancel the in-flight *final* transcription request (barge-in).
    ///
    /// The live prefix state is deliberately kept: the pipeline calls this
    /// from its `SpeechStart` handler, i.e. immediately after the segmenter
    /// opened a new segment, so dropping `self.live` here would disable live
    /// transcription (contract P3) for the utterance the user just started.
    /// Superseded live requests are cancelled when the next segment opens.
    pub fn cancel_inflight(&mut self) {
        if let Some(token) = self.pending.take() {
            token.cancel();
            tracing::debug!(
                target: "stt",
                "Cancelled in-flight remote transcription (barge-in)"
            );
        }
    }

    /// Number of transcriptions currently in flight.
    pub fn inflight_transcriptions(&self) -> usize {
        self.inflight.load(Ordering::SeqCst)
    }

    /// Blocking one-shot transcription.
    ///
    /// The trait method is synchronous, so this runs the async HTTP call on a
    /// dedicated thread with its own current-thread runtime (safe from both
    /// sync and async callers). Used by audit/test harnesses.
    pub fn transcribe_complete(&self, audio: &[f32]) -> Result<TranscriptionQuality> {
        let client = self.client.clone();
        let cfg = self.cfg.clone();
        let audio = audio.to_vec();
        std::thread::scope(|scope| {
            scope
                .spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .context("failed to build transcription runtime")?;
                    rt.block_on(transcribe_remote(
                        &client,
                        &cfg,
                        &audio,
                        &CancellationToken::new(),
                    ))
                })
                .join()
                .map_err(|_| anyhow!("remote transcription thread panicked"))?
        })
    }
}

#[async_trait]
impl<D: SpeechDetector + Sync + 'static> SttProvider for OpenAiSttProvider<D> {
    fn provider_name(&self) -> &'static str {
        "openai"
    }

    async fn process_audio(&mut self, audio: &[f32], tx: &mpsc::Sender<SpeechEvent>) -> Result<()> {
        OpenAiSttProvider::process_audio(self, audio, tx).await
    }

    fn transcribe_complete(&self, audio: &[f32]) -> Result<TranscriptionQuality> {
        OpenAiSttProvider::transcribe_complete(self, audio)
    }

    async fn verify_server(&mut self) -> Result<()> {
        OpenAiSttProvider::verify_server(self).await
    }

    fn cancel_inflight(&mut self) {
        OpenAiSttProvider::cancel_inflight(self);
    }

    fn inflight_transcriptions(&self) -> usize {
        OpenAiSttProvider::inflight_transcriptions(self)
    }
}

impl<D: SpeechDetector> Drop for OpenAiSttProvider<D> {
    fn drop(&mut self) {
        // Best-effort: stop detached requests when the provider goes away
        // (pipeline shutdown) instead of letting them run to their timeout.
        if let Some(token) = self.pending.take() {
            token.cancel();
        }
        if let Some(mut live) = self.live.take() {
            for p in live.pending.drain(..) {
                p.token.cancel();
            }
        }
    }
}

/// Decrements the in-flight counter when the task finishes (any path).
struct InflightGuard(Arc<AtomicUsize>);

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Run the remote transcription and forward the outcome through `tx`.
#[allow(clippy::too_many_arguments)]
async fn dispatch_transcription(
    client: reqwest::Client,
    cfg: OpenAiSttConfig,
    segment: Vec<f32>,
    segment_seq: u64,
    token: CancellationToken,
    tx: mpsc::Sender<SpeechEvent>,
    inflight: Arc<AtomicUsize>,
) {
    let _guard = InflightGuard(inflight);
    match transcribe_remote(&client, &cfg, &segment, &token).await {
        Ok(quality) => {
            if token.is_cancelled() {
                tracing::debug!(target: "stt", "Remote transcription finished but was cancelled");
                return;
            }
            if quality.text.trim().is_empty() {
                tracing::debug!(target: "stt", "Remote transcription returned empty text");
            } else {
                tracing::info!(target: "stt", "SpeechEnd: {}", quality.text);
            }
            let _ = tx
                .send(SpeechEvent::SpeechEnd {
                    segment_seq,
                    quality,
                })
                .await;
        }
        Err(e) => {
            if token.is_cancelled() {
                tracing::debug!(target: "stt", "Remote transcription aborted on barge-in");
            } else {
                tracing::warn!(target: "stt", "Remote transcription failed: {e:#}");
            }
        }
    }
}

/// Run a live prefix transcription and forward the partial text through `tx`
/// (contract P3). Results are cosmetic: superseded requests simply vanish.
async fn dispatch_live_transcription(
    client: reqwest::Client,
    cfg: OpenAiSttConfig,
    segment: Vec<f32>,
    segment_seq: u64,
    token: CancellationToken,
    tx: mpsc::Sender<SpeechEvent>,
    inflight: Arc<AtomicUsize>,
) {
    let _guard = InflightGuard(inflight);
    match transcribe_remote(&client, &cfg, &segment, &token).await {
        Ok(quality) => {
            if token.is_cancelled() || quality.text.trim().is_empty() {
                return;
            }
            tracing::debug!(target: "stt", "Partial (live remote, seq {segment_seq}): {}", quality.text);
            let _ = tx.send(SpeechEvent::Speech(quality.text)).await;
        }
        Err(e) => {
            if !token.is_cancelled() {
                tracing::debug!(target: "stt", "Live remote transcription failed: {e:#}");
            }
        }
    }
}

/// POST one utterance and map the response into [`TranscriptionQuality`].
async fn transcribe_remote(
    client: &reqwest::Client,
    cfg: &OpenAiSttConfig,
    audio: &[f32],
    token: &CancellationToken,
) -> Result<TranscriptionQuality> {
    if audio.is_empty() {
        return Ok(TranscriptionQuality {
            text: String::new(),
            no_speech_prob: 0.0,
            avg_logprob: 0.0,
            compression_ratio: 0.0,
        });
    }

    let wav = wav::encode_pcm16_mono_16k(audio);
    let part = reqwest::multipart::Part::bytes(wav)
        .file_name("segment.wav")
        .mime_str("audio/wav")
        .context("invalid WAV content type")?;
    let mut form = reqwest::multipart::Form::new()
        .text("model", cfg.model.clone())
        .text("response_format", "json")
        .part("file", part);
    if let Some(language) = wire_language(&cfg.language) {
        form = form.text("language", language);
    }

    let url = format!(
        "{}/v1/audio/transcriptions",
        cfg.base_url.trim_end_matches('/')
    );
    let mut req = client.post(&url).multipart(form);
    if !cfg.api_key.trim().is_empty() {
        req = req.bearer_auth(cfg.api_key.trim());
    }

    let resp = tokio::select! {
        _ = token.cancelled() => bail!("transcription cancelled"),
        r = req.send() => r.map_err(map_reqwest)?,
    };

    let status = resp.status();
    let body = tokio::select! {
        _ = token.cancelled() => bail!("transcription cancelled"),
        b = resp.bytes() => b.map_err(map_reqwest)?,
    };

    if !status.is_success() {
        bail!(
            "HTTP {} from {url}: {}",
            status.as_u16(),
            extract_error_message(&body)
        );
    }

    parse_transcription_response(&body)
}

/// Map the response body into quality metrics and text.
fn parse_transcription_response(body: &[u8]) -> Result<TranscriptionQuality> {
    let parsed: TranscriptionResponse =
        serde_json::from_slice(body).context("malformed transcription response")?;

    let (no_speech_prob, avg_logprob, compression_ratio) = quality_metrics(&parsed);
    let text = parsed.text.trim().to_string();
    let compression_ratio = compression_ratio.unwrap_or_else(|| {
        if text.is_empty() {
            0.0
        } else {
            calculate_compression_ratio(&text)
        }
    });

    Ok(TranscriptionQuality {
        text,
        no_speech_prob: no_speech_prob.unwrap_or(0.0),
        avg_logprob: avg_logprob.unwrap_or(0.0),
        compression_ratio,
    })
}

/// Extract Whisper-style metrics, preferring the top-level fields and falling
/// back to the first segment when a server returns them per segment.
fn quality_metrics(resp: &TranscriptionResponse) -> (Option<f32>, Option<f32>, Option<f32>) {
    if resp.no_speech_prob.is_some()
        || resp.avg_logprob.is_some()
        || resp.compression_ratio.is_some()
    {
        return (
            resp.no_speech_prob,
            resp.avg_logprob,
            resp.compression_ratio,
        );
    }
    if let Some(seg) = resp.segments.as_ref().and_then(|s| s.first()) {
        return (seg.no_speech_prob, seg.avg_logprob, seg.compression_ratio);
    }
    (None, None, None)
}

/// The wire value for the `language` field.
///
/// `auto`/empty means auto-detection: the field is omitted because the OpenAI
/// transcription API (and `qwen3-asr`) rejects the literal `"auto"`. A concrete
/// code is normalized to its primary ISO-639-1 subtag (`es-ES` -> `es`), which
/// is what these servers expect.
fn wire_language(language: &str) -> Option<String> {
    let language = language.trim();
    if language.is_empty() || language.eq_ignore_ascii_case("auto") {
        return None;
    }
    let primary = language.split(['-', '_']).next().unwrap_or(language);
    Some(primary.to_ascii_lowercase())
}

/// Human-readable detail from an OpenAI-style error body.
fn extract_error_message(body: &[u8]) -> String {
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) {
        if let Some(msg) = v
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            return msg.to_string();
        }
        if let Some(msg) = v.get("detail").and_then(|d| d.as_str()) {
            return msg.to_string();
        }
    }
    let raw = String::from_utf8_lossy(body);
    let raw = raw.trim();
    if raw.is_empty() {
        "<no body>".to_string()
    } else {
        raw.to_string()
    }
}

fn map_reqwest(e: reqwest::Error) -> anyhow::Error {
    if e.is_timeout() {
        anyhow!("remote STT request timed out: {e}")
    } else if e.is_connect() {
        anyhow!("could not connect to remote STT service: {e}")
    } else if e.is_decode() {
        anyhow!("could not decode remote STT response: {e}")
    } else {
        anyhow!("remote STT request failed: {e}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(text: &str) -> TranscriptionResponse {
        TranscriptionResponse {
            text: text.to_string(),
            no_speech_prob: None,
            avg_logprob: None,
            compression_ratio: None,
            segments: None,
        }
    }

    #[test]
    fn wire_language_omits_auto() {
        assert_eq!(wire_language("auto"), None);
        assert_eq!(wire_language("AUTO"), None);
        assert_eq!(wire_language(""), None);
        assert_eq!(wire_language("   "), None);
    }

    #[test]
    fn wire_language_normalizes_regions() {
        assert_eq!(wire_language("es"), Some("es".to_string()));
        assert_eq!(wire_language("ES"), Some("es".to_string()));
        assert_eq!(wire_language("es-ES"), Some("es".to_string()));
        assert_eq!(wire_language("en_US"), Some("en".to_string()));
    }

    #[test]
    fn parses_json_without_metrics() {
        let q = parse_transcription_response(br#"{"text":" Hola mundo ","usage":{"seconds":2}}"#)
            .unwrap();
        assert_eq!(q.text, "Hola mundo");
        assert_eq!(q.no_speech_prob, 0.0);
        assert_eq!(q.avg_logprob, 0.0);
        assert!(q.compression_ratio > 0.0);
    }

    #[test]
    fn parses_top_level_metrics() {
        let q = parse_transcription_response(
            br#"{"text":"hello","no_speech_prob":0.9,"avg_logprob":-1.5,"compression_ratio":3.0}"#,
        )
        .unwrap();
        assert_eq!(q.no_speech_prob, 0.9);
        assert_eq!(q.avg_logprob, -1.5);
        assert_eq!(q.compression_ratio, 3.0);
    }

    #[test]
    fn parses_segment_metrics() {
        let q = parse_transcription_response(
            br#"{"text":"hello","segments":[{"no_speech_prob":0.8,"avg_logprob":-1.2}]}"#,
        )
        .unwrap();
        assert_eq!(q.no_speech_prob, 0.8);
        assert_eq!(q.avg_logprob, -1.2);
    }

    #[test]
    fn empty_text_stays_empty() {
        let q = parse_transcription_response(br#"{"text":"   "}"#).unwrap();
        assert_eq!(q.text, "");
        assert_eq!(q.compression_ratio, 0.0);
    }

    #[test]
    fn malformed_body_is_an_error() {
        assert!(parse_transcription_response(b"not json").is_err());
        assert!(parse_transcription_response(b"").is_err());
    }

    #[test]
    fn extracts_openai_error_message() {
        let body =
            br#"{"error":{"message":"The model `nope` does not exist.","type":"NotFoundError"}}"#;
        assert_eq!(
            extract_error_message(body),
            "The model `nope` does not exist."
        );
        assert_eq!(
            extract_error_message(b"<html>oops</html>"),
            "<html>oops</html>"
        );
        assert_eq!(extract_error_message(b""), "<no body>");
    }

    #[test]
    fn quality_metrics_prefers_top_level() {
        let mut r = resp("hi");
        r.no_speech_prob = Some(0.1);
        r.segments = Some(vec![Segment {
            no_speech_prob: Some(0.9),
            avg_logprob: None,
            compression_ratio: None,
        }]);
        assert_eq!(quality_metrics(&r).0, Some(0.1));
    }
}
