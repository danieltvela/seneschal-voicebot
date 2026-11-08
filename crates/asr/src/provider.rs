//! Nemotron 3.5 ASR over the NeMo-Speech.cpp realtime WebSocket.
//!
//! The server owns the VAD-driven endpointing. A `.delta` event opens a turn
//! (there is no separate ASR speech-start event on this endpoint), `.completed`
//! closes it, and multiple utterances may arrive over one persistent session.
//! Only the transport changed: events keep the existing `SpeechEvent` /
//! `NoSpeechGate` contract.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{AUTHORIZATION, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tracing::{debug, info, warn};

use seneschal_common::config::Config;
use seneschal_core::stt::wav::encode_pcm16_mono_16k;
use seneschal_core::stt::{SpeechEvent, SttProvider, TranscriptionQuality};

use crate::config::{AsrConfig, SAMPLE_RATE};
use crate::pcm::encode_pcm16_le;
use crate::protocol;

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Audio queued from the capture loop to the WebSocket worker.
enum Command {
    Audio(Vec<u8>),
}

/// Per-connection turn state.
#[derive(Debug)]
struct TurnState {
    /// Next `segment_seq` handed to the pipeline. Starts at 1; 0 is the
    /// legacy "no committed segment" reset value.
    next_seq: u64,
    /// Sequence of the turn currently open, if any.
    current: Option<u64>,
    /// Accumulated partial transcript of the current turn.
    partial: String,
}

impl Default for TurnState {
    fn default() -> Self {
        Self {
            next_seq: 1,
            current: None,
            partial: String::new(),
        }
    }
}

impl TurnState {
    /// Open a new turn and emit its `SpeechStart`.
    async fn open(&mut self, event_tx: &mpsc::Sender<SpeechEvent>) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.current = Some(seq);
        emit(event_tx, SpeechEvent::SpeechStart { segment_seq: seq }).await;
        seq
    }

    /// Close an incomplete turn after a transport failure. The empty final is
    /// rejected by `NoSpeechGate` and only resets the pipeline FSM.
    async fn close(&mut self, event_tx: &mpsc::Sender<SpeechEvent>) {
        if let Some(seq) = self.current.take() {
            emit(
                event_tx,
                SpeechEvent::SpeechEnd {
                    segment_seq: seq,
                    quality: TranscriptionQuality::empty(),
                },
            )
            .await;
        }
        self.partial.clear();
    }
}

/// Forward one event, ignoring a closed receiver (the audio loop is gone).
async fn emit(event_tx: &mpsc::Sender<SpeechEvent>, event: SpeechEvent) {
    let _ = event_tx.send(event).await;
}

/// Provider backed by one persistent NeMo-Speech.cpp realtime session.
pub struct NemotronAsrProvider {
    cfg: AsrConfig,
    client: reqwest::Client,
    session: Option<mpsc::Sender<Command>>,
    /// 1 while the WebSocket is connected (observability).
    connected: Arc<AtomicUsize>,
}

impl NemotronAsrProvider {
    pub fn new(cfg: AsrConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(cfg.connect_timeout)
            .timeout(cfg.http_timeout)
            .build()
            .context("failed to build the Nemotron ASR HTTP client")?;

        info!(
            target: "asr",
            "NemotronAsrProvider ready (url: {}, model: {}, lang: {}, endpointing_ms: {}, sample_rate: {})",
            cfg.base_url,
            cfg.model,
            cfg.language,
            cfg.endpointing_ms,
            SAMPLE_RATE,
        );

        Ok(Self {
            cfg,
            client,
            session: None,
            connected: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Start the persistent WebSocket worker on the first real audio chunk.
    fn ensure_session(&mut self, event_tx: mpsc::Sender<SpeechEvent>) {
        if self.session.is_none() {
            let (command_tx, command_rx) = mpsc::channel::<Command>(64);
            let cfg = self.cfg.clone();
            let connected = Arc::clone(&self.connected);
            tokio::spawn(run_session_loop(cfg, command_rx, event_tx, connected));
            self.session = Some(command_tx);
        }
    }

    /// Best-effort startup check (`/ready` + `/v1/models`).
    pub async fn verify_server(&self) -> Result<()> {
        let base = self.cfg.base_url.trim_end_matches('/');
        let ready_url = format!("{base}/ready");
        let mut ready = self.client.get(&ready_url);
        if !self.cfg.api_key.is_empty() {
            ready = ready.bearer_auth(&self.cfg.api_key);
        }
        let response = ready.send().await.map_err(map_reqwest)?;
        let status = response.status();
        let body = response.bytes().await.map_err(map_reqwest)?;
        if !status.is_success() {
            anyhow::bail!(
                "HTTP {} from {ready_url}: {}",
                status.as_u16(),
                extract_error_message(&body)
            );
        }

        let models_url = format!("{base}/v1/models");
        let mut models_request = self.client.get(&models_url);
        if !self.cfg.api_key.is_empty() {
            models_request = models_request.bearer_auth(&self.cfg.api_key);
        }
        let models_response = models_request.send().await.map_err(map_reqwest)?;
        let models_status = models_response.status();
        let models_body = models_response.bytes().await.map_err(map_reqwest)?;
        if !models_status.is_success() {
            warn!(
                target: "asr",
                "Model inventory check failed (HTTP {}); continuing with '{}'",
                models_status.as_u16(),
                self.cfg.model
            );
            return Ok(());
        }

        let models: serde_json::Value =
            serde_json::from_slice(&models_body).context("malformed /v1/models response")?;
        let entries = models.get("data").and_then(serde_json::Value::as_array);
        match entries.and_then(|list| {
            list.iter().find(|entry| {
                entry.get("id").and_then(serde_json::Value::as_str) == Some(self.cfg.model.as_str())
            })
        }) {
            Some(entry) => {
                if entry.get("streaming").and_then(serde_json::Value::as_bool) == Some(false) {
                    warn!(
                        target: "asr",
                        "Model '{}' reports streaming=false; realtime transcription will fail",
                        self.cfg.model
                    );
                } else {
                    info!(
                        target: "asr",
                        "Nemotron ASR model '{}' available at {}",
                        self.cfg.model,
                        self.cfg.base_url
                    );
                }
            }
            None => {
                let available = entries
                    .map(|list| {
                        list.iter()
                            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
                            .take(10)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                warn!(
                    target: "asr",
                    "Model '{}' not listed by {} (available: {}). Requests may fail.",
                    self.cfg.model,
                    self.cfg.base_url,
                    if available.is_empty() { "<none>" } else { &available }
                );
            }
        }
        Ok(())
    }

    /// Blocking one-shot transcription for trait compatibility / tests. The
    /// voice pipeline itself always uses the streaming WebSocket.
    fn transcribe_remote_blocking(&self, audio: &[f32]) -> Result<TranscriptionQuality> {
        if audio.is_empty() {
            return Ok(TranscriptionQuality::empty());
        }
        let client = self.client.clone();
        let cfg = self.cfg.clone();
        let wav = encode_pcm16_mono_16k(audio);
        thread::scope(|scope| {
            scope
                .spawn(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .context("failed to build the one-shot transcription runtime")?;
                    runtime.block_on(transcribe_remote(&client, &cfg, &wav))
                })
                .join()
                .map_err(|_| anyhow!("remote Nemotron transcription thread panicked"))?
        })
    }
}

#[async_trait]
impl SttProvider for NemotronAsrProvider {
    fn provider_name(&self) -> &'static str {
        "nemotron"
    }

    async fn process_audio(&mut self, audio: &[f32], tx: &mpsc::Sender<SpeechEvent>) -> Result<()> {
        let pcm = encode_pcm16_le(audio);
        if pcm.is_empty() {
            return Ok(());
        }
        self.ensure_session(tx.clone());
        let session = self
            .session
            .as_ref()
            .expect("ensure_session just populated the command channel");
        session
            .send(Command::Audio(pcm))
            .await
            .context("Nemotron ASR WebSocket worker stopped")?;
        Ok(())
    }

    fn transcribe_complete(&self, audio: &[f32]) -> Result<TranscriptionQuality> {
        self.transcribe_remote_blocking(audio)
    }

    async fn verify_server(&mut self) -> Result<()> {
        NemotronAsrProvider::verify_server(self).await
    }

    fn inflight_transcriptions(&self) -> usize {
        self.connected.load(Ordering::SeqCst)
    }
}

/// Persistent worker: connect, exchange audio/events, reconnect on failure.
///
/// While disconnected, queued audio is discarded rather than replayed after the
/// reconnect; that avoids a transcript whose endpointing timestamp lags real
/// time. The next successfully transported chunk starts the next turn.
async fn run_session_loop(
    cfg: AsrConfig,
    mut commands: mpsc::Receiver<Command>,
    event_tx: mpsc::Sender<SpeechEvent>,
    connected: Arc<AtomicUsize>,
) {
    let mut state = TurnState::default();
    let mut attempt = 0u32;
    loop {
        connected.store(0, Ordering::SeqCst);
        if commands.is_closed() {
            return;
        }
        match connect_session(&cfg).await {
            Ok(mut ws) => {
                connected.store(1, Ordering::SeqCst);
                attempt = 0;
                debug!(target: "asr", "Realtime ASR WebSocket connected");
                match run_connected_session(&mut ws, &cfg, &mut commands, &event_tx, &mut state)
                    .await
                {
                    Ok(()) => return, // command channel closed: normal shutdown
                    Err(error) => {
                        warn!(target: "asr", "Realtime ASR session failed: {error:#}");
                    }
                }
            }
            Err(error) => {
                warn!(target: "asr", "Could not connect to Nemotron ASR: {error:#}");
            }
        }

        state.close(&event_tx).await;
        // Drop audio queued while there was no usable transport.
        while let Ok(command) = commands.try_recv() {
            let _ = command;
        }
        let backoff = reconnect_backoff(attempt);
        attempt = attempt.saturating_add(1);
        debug!(target: "asr", "Reconnecting Nemotron ASR in {backoff:?}");
        tokio::time::sleep(backoff).await;
    }
}

/// Exponential reconnect backoff, capped at 5 s.
fn reconnect_backoff(attempt: u32) -> Duration {
    const BASE_MS: u64 = 250;
    const MAX_MS: u64 = 5_000;
    let shift = attempt.min(5);
    Duration::from_millis(BASE_MS.saturating_mul(1 << shift).min(MAX_MS))
}

/// Open the realtime WebSocket with the optional bearer token.
async fn connect_session(cfg: &AsrConfig) -> Result<WsStream> {
    let url = cfg.websocket_url();
    let mut request = url
        .as_str()
        .into_client_request()
        .with_context(|| format!("invalid realtime WebSocket URL: {url}"))?;
    if !cfg.api_key.is_empty() {
        let value = HeaderValue::from_str(&format!("Bearer {}", cfg.api_key))
            .context("invalid STT_API_KEY")?;
        request.headers_mut().insert(AUTHORIZATION, value);
    }
    let (stream, _response) = tokio::time::timeout(cfg.connect_timeout, connect_async(request))
        .await
        .map_err(|_| anyhow!("timed out connecting to {url}"))?
        .with_context(|| format!("failed to connect to {url}"))?;
    Ok(stream)
}

/// Drive one connected WebSocket until the command channel closes or the
/// transport fails.
async fn run_connected_session(
    ws: &mut WsStream,
    cfg: &AsrConfig,
    commands: &mut mpsc::Receiver<Command>,
    event_tx: &mpsc::Sender<SpeechEvent>,
    state: &mut TurnState,
) -> Result<()> {
    let update = protocol::session_update(SAMPLE_RATE, &cfg.language, cfg.endpointing_ms);
    ws.send(Message::text(update))
        .await
        .context("failed to send session.update")?;

    loop {
        tokio::select! {
            command = commands.recv() => {
                match command {
                    None => return Ok(()),
                    Some(Command::Audio(pcm)) => {
                        ws.send(Message::binary(pcm))
                            .await
                            .context("failed to send audio to the realtime ASR session")?;
                    }
                }
            }
            message = ws.next() => {
                match message {
                    None => anyhow::bail!("realtime ASR WebSocket closed"),
                    Some(Ok(Message::Text(text))) => {
                        handle_event(&text, event_tx, state).await?;
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        ws.send(Message::Pong(payload))
                            .await
                            .context("failed to answer the ASR WebSocket ping")?;
                    }
                    Some(Ok(Message::Close(_))) => {
                        anyhow::bail!("realtime ASR WebSocket sent a close frame");
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Err(error).context("realtime ASR WebSocket error"),
                }
            }
        }
    }
}

/// Map one JSON server event onto the pipeline's `SpeechEvent` stream.
async fn handle_event(
    text: &str,
    event_tx: &mpsc::Sender<SpeechEvent>,
    state: &mut TurnState,
) -> Result<()> {
    let event = protocol::parse_event(text)?;
    let kind = protocol::event_type(&event).to_string();

    match kind.as_str() {
        "conversation.item.input_audio_transcription.delta" => {
            let delta = protocol::string_field(&event, "delta").unwrap_or_default();
            if delta.trim().is_empty() {
                return Ok(());
            }
            let seq = match state.current {
                Some(seq) => seq,
                None => state.open(event_tx).await,
            };
            state.partial.push_str(delta);
            let partial = state.partial.clone();
            let audio_processed = protocol::number_field(&event, "audio_processed").unwrap_or(0.0);
            debug!(
                target: "asr",
                seq,
                audio_processed,
                "Partial: {partial}"
            );
            emit(event_tx, SpeechEvent::Speech(partial)).await;
        }
        "conversation.item.input_audio_transcription.completed" => {
            let transcript = protocol::string_field(&event, "transcript").unwrap_or_default();
            if state.current.is_none() && transcript.trim().is_empty() {
                return Ok(());
            }
            let seq = match state.current {
                Some(seq) => seq,
                None => state.open(event_tx).await,
            };
            let audio_processed = protocol::number_field(&event, "audio_processed").unwrap_or(0.0);
            let event_id = protocol::string_field(&event, "event_id").unwrap_or_default();
            info!(
                target: "asr",
                seq,
                audio_processed,
                event_id,
                "Endpointing: {transcript}"
            );
            state.current = None;
            state.partial.clear();
            emit(
                event_tx,
                SpeechEvent::SpeechEnd {
                    segment_seq: seq,
                    quality: TranscriptionQuality {
                        text: transcript.trim().to_string(),
                        no_speech_prob: 0.0,
                        avg_logprob: 0.0,
                        compression_ratio: 0.0,
                    },
                },
            )
            .await;
        }
        "error" => {
            anyhow::bail!("{}", protocol::error_message(&event));
        }
        "session.created"
        | "session.updated"
        | "input_audio_buffer.committed"
        | "input_audio_buffer.cleared" => {
            debug!(target: "asr", "ASR realtime event: {text}");
        }
        other => {
            debug!(target: "asr", "Ignoring ASR realtime event '{other}'");
        }
    }
    Ok(())
}

/// POST one complete utterance to the OpenAI-compatible REST route.
async fn transcribe_remote(
    client: &reqwest::Client,
    cfg: &AsrConfig,
    wav: &[u8],
) -> Result<TranscriptionQuality> {
    let part = reqwest::multipart::Part::bytes(wav.to_vec())
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .context("invalid WAV content type")?;
    let mut form = reqwest::multipart::Form::new()
        .text("model", cfg.model.clone())
        .text("response_format", "json")
        .text("automatic_punctuation", "true")
        .part("file", part);
    if !cfg.language.is_empty() {
        form = form.text("language", cfg.language.clone());
    }

    let url = format!(
        "{}/v1/audio/transcriptions",
        cfg.base_url.trim_end_matches('/')
    );
    let mut request = client.post(&url).multipart(form);
    if !cfg.api_key.is_empty() {
        request = request.bearer_auth(&cfg.api_key);
    }
    let response = request.send().await.map_err(map_reqwest)?;
    let status = response.status();
    let body = response.bytes().await.map_err(map_reqwest)?;
    if !status.is_success() {
        anyhow::bail!(
            "HTTP {} from {url}: {}",
            status.as_u16(),
            extract_error_message(&body)
        );
    }

    let parsed: serde_json::Value =
        serde_json::from_slice(&body).context("malformed transcription response")?;
    let text = parsed
        .get("text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    Ok(TranscriptionQuality {
        text,
        no_speech_prob: 0.0,
        avg_logprob: 0.0,
        compression_ratio: 0.0,
    })
}

fn extract_error_message(body: &[u8]) -> String {
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
        if let Some(message) = value
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(serde_json::Value::as_str)
        {
            return message.to_string();
        }
        if let Some(message) = value.get("detail").and_then(serde_json::Value::as_str) {
            return message.to_string();
        }
    }
    let raw = String::from_utf8_lossy(body).trim().to_string();
    if raw.is_empty() {
        "<no body>".to_string()
    } else {
        raw
    }
}

fn map_reqwest(error: reqwest::Error) -> anyhow::Error {
    if error.is_timeout() {
        anyhow!("Nemotron ASR request timed out: {error}")
    } else if error.is_connect() {
        anyhow!("could not connect to the Nemotron ASR service: {error}")
    } else if error.is_decode() {
        anyhow!("could not decode the Nemotron ASR response: {error}")
    } else {
        anyhow!("Nemotron ASR request failed: {error}")
    }
}

/// Runtime constructor used by the main pipeline.
pub fn create_provider(config: &Config) -> Result<Box<dyn SttProvider>> {
    let cfg = AsrConfig::from_config(config)?;
    Ok(Box::new(NemotronAsrProvider::new(cfg)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn recv_event(rx: &mut mpsc::Receiver<SpeechEvent>) -> SpeechEvent {
        tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("event must arrive within the test timeout")
            .expect("event channel must stay open")
    }

    #[tokio::test]
    async fn first_delta_opens_turn_and_accumulates_partials() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut state = TurnState::default();

        handle_event(
            r#"{"type":"conversation.item.input_audio_transcription.delta","delta":"Hola","audio_processed":0.16}"#,
            &tx,
            &mut state,
        )
        .await
        .unwrap();
        handle_event(
            r#"{"type":"conversation.item.input_audio_transcription.delta","delta":" señor","audio_processed":0.32}"#,
            &tx,
            &mut state,
        )
        .await
        .unwrap();

        assert!(matches!(
            recv_event(&mut rx).await,
            SpeechEvent::SpeechStart { segment_seq: 1 }
        ));
        assert_eq!(
            match recv_event(&mut rx).await {
                SpeechEvent::Speech(text) => text,
                other => panic!("expected Speech, got {other:?}"),
            },
            "Hola"
        );
        assert_eq!(
            match recv_event(&mut rx).await {
                SpeechEvent::Speech(text) => text,
                other => panic!("expected Speech, got {other:?}"),
            },
            "Hola señor"
        );
        assert_eq!(state.current, Some(1));
    }

    #[tokio::test]
    async fn completed_closes_turn_and_next_delta_starts_a_new_one() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut state = TurnState::default();

        handle_event(
            r#"{"type":"conversation.item.input_audio_transcription.delta","delta":"uno","audio_processed":0.2}"#,
            &tx,
            &mut state,
        )
        .await
        .unwrap();
        handle_event(
            r#"{"type":"conversation.item.input_audio_transcription.completed","transcript":"Uno.","audio_processed":1.2,"event_id":"event_9"}"#,
            &tx,
            &mut state,
        )
        .await
        .unwrap();
        handle_event(
            r#"{"type":"conversation.item.input_audio_transcription.delta","delta":"dos","audio_processed":1.5}"#,
            &tx,
            &mut state,
        )
        .await
        .unwrap();

        assert!(matches!(
            recv_event(&mut rx).await,
            SpeechEvent::SpeechStart { segment_seq: 1 }
        ));
        assert!(matches!(recv_event(&mut rx).await, SpeechEvent::Speech(_)));
        match recv_event(&mut rx).await {
            SpeechEvent::SpeechEnd {
                segment_seq,
                quality,
            } => {
                assert_eq!(segment_seq, 1);
                assert_eq!(quality.text, "Uno.");
                assert_eq!(quality.no_speech_prob, 0.0);
            }
            other => panic!("expected SpeechEnd, got {other:?}"),
        }
        assert!(matches!(
            recv_event(&mut rx).await,
            SpeechEvent::SpeechStart { segment_seq: 2 }
        ));
        assert_eq!(state.current, Some(2));
        assert_eq!(state.partial, "dos");
    }

    #[tokio::test]
    async fn completed_without_partial_emits_a_paired_start() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut state = TurnState::default();

        handle_event(
            r#"{"type":"conversation.item.input_audio_transcription.completed","transcript":"Directo.","audio_processed":0.8}"#,
            &tx,
            &mut state,
        )
        .await
        .unwrap();

        assert!(matches!(
            recv_event(&mut rx).await,
            SpeechEvent::SpeechStart { segment_seq: 1 }
        ));
        match recv_event(&mut rx).await {
            SpeechEvent::SpeechEnd { segment_seq, .. } => assert_eq!(segment_seq, 1),
            other => panic!("expected SpeechEnd, got {other:?}"),
        }
        assert_eq!(state.current, None);
    }

    #[tokio::test]
    async fn empty_completed_without_turn_is_ignored() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut state = TurnState::default();

        handle_event(
            r#"{"type":"conversation.item.input_audio_transcription.completed","transcript":"","audio_processed":0.4}"#,
            &tx,
            &mut state,
        )
        .await
        .unwrap();

        assert!(rx.try_recv().is_err());
        assert_eq!(state.current, None);
    }

    #[tokio::test]
    async fn transport_failure_closes_the_open_turn_with_an_empty_end() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut state = TurnState::default();
        state.open(&tx).await;
        state.close(&tx).await;

        assert!(matches!(
            recv_event(&mut rx).await,
            SpeechEvent::SpeechStart { segment_seq: 1 }
        ));
        match recv_event(&mut rx).await {
            SpeechEvent::SpeechEnd {
                segment_seq,
                quality,
            } => {
                assert_eq!(segment_seq, 1);
                assert!(quality.text.is_empty());
            }
            other => panic!("expected empty SpeechEnd, got {other:?}"),
        }
    }

    #[test]
    fn reconnect_backoff_is_bounded() {
        assert_eq!(reconnect_backoff(0), Duration::from_millis(250));
        assert_eq!(reconnect_backoff(1), Duration::from_millis(500));
        assert_eq!(reconnect_backoff(5), Duration::from_millis(5_000));
        assert_eq!(reconnect_backoff(20), Duration::from_millis(5_000));
    }

    #[tokio::test]
    async fn provider_starts_a_session_for_audio_chunks() {
        let config = Config::from_env().unwrap();
        let cfg = AsrConfig::from_config(&config).unwrap();
        let mut provider = NemotronAsrProvider::new(cfg).unwrap();
        let (tx, _rx) = mpsc::channel(1);
        provider.ensure_session(tx);
        let session = provider.session.as_ref().unwrap().clone();

        session
            .send(Command::Audio(vec![1, 2, 3, 4]))
            .await
            .unwrap();
        assert!(!session.is_closed());
    }
}
