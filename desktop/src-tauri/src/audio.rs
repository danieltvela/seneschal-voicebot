//! Audio-panel bridge for issue #34: a low-frequency-isolated VU bus plus thin
//! REST clients for the daemon's audio diagnostics endpoints.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast;

pub const AUDIO_LEVEL_EVENT: &str = "desktop-audio-level";

const QUICK_TIMEOUT: Duration = Duration::from_secs(10);
const PLAYBACK_TIMEOUT: Duration = Duration::from_secs(30);
/// Covers a 10 s recording plus a cold local/remote STT one-shot request.
const CAPTURE_TEST_TIMEOUT: Duration = Duration::from_secs(45);
/// Some OpenAI-compatible backends report up to a 120 s cold-start timeout.
const TTS_PREVIEW_TIMEOUT: Duration = Duration::from_secs(130);

/// One RMS/peak sample from Control SSE. Kept out of `AppStatus` so a 10 Hz
/// meter does not re-render the daemon/pipeline status view.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioLevel {
    pub rms: f32,
    pub peak: f32,
}

/// Latest-value bus for VU samples.
pub struct AudioHub {
    current: Mutex<AudioLevel>,
    tx: broadcast::Sender<AudioLevel>,
}

impl AudioHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            current: Mutex::new(AudioLevel {
                rms: 0.0,
                peak: 0.0,
            }),
            tx,
        }
    }

    pub fn apply(&self, level: AudioLevel) {
        *self.current.lock().expect("audio level mutex poisoned") = level;
        let _ = self.tx.send(level);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AudioLevel> {
        self.tx.subscribe()
    }
}

impl Default for AudioHub {
    fn default() -> Self {
        Self::new()
    }
}

pub type SharedAudioHub = Arc<AudioHub>;

/// Forward VU samples as a dedicated Tauri event.
pub fn spawn_audio_level_emitter(app: AppHandle, audio: SharedAudioHub) {
    let mut rx = audio.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(level) => {
                    let _ = app.emit(AUDIO_LEVEL_EVENT, level);
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn client(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .expect("failed to build audio HTTP client")
}

async fn get_json(base_url: &str, path: &str, timeout: Duration) -> anyhow::Result<Value> {
    let response = client(timeout)
        .get(format!("{base_url}{path}"))
        .send()
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    if !status.is_success() {
        return Err(anyhow::anyhow!(audio_error_message(&body, status)));
    }
    Ok(body)
}

async fn post_json(
    base_url: &str,
    path: &str,
    body: Value,
    timeout: Duration,
) -> anyhow::Result<Value> {
    let response = client(timeout)
        .post(format!("{base_url}{path}"))
        .json(&body)
        .send()
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    if !status.is_success() {
        return Err(anyhow::anyhow!(audio_error_message(&body, status)));
    }
    Ok(body)
}

fn audio_error_message(body: &Value, status: reqwest::StatusCode) -> String {
    let error = body.get("error");
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("audio request failed");
    match error
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
    {
        Some(code) => format!("{code}: {message} (HTTP {})", status.as_u16()),
        None => format!("{message} (HTTP {})", status.as_u16()),
    }
}

pub async fn devices(base_url: &str) -> anyhow::Result<Value> {
    get_json(base_url, "/control/audio/devices", QUICK_TIMEOUT).await
}

pub async fn status(base_url: &str) -> anyhow::Result<Value> {
    get_json(base_url, "/control/audio/status", QUICK_TIMEOUT).await
}

pub async fn request_permission(base_url: &str) -> anyhow::Result<Value> {
    post_json(
        base_url,
        "/control/audio/permission",
        serde_json::json!({}),
        QUICK_TIMEOUT,
    )
    .await
}

pub async fn tts_preview(base_url: &str) -> anyhow::Result<Value> {
    post_json(
        base_url,
        "/control/audio/preview-tts",
        serde_json::json!({}),
        TTS_PREVIEW_TIMEOUT,
    )
    .await
}

pub async fn speaker_test(base_url: &str) -> anyhow::Result<Value> {
    post_json(
        base_url,
        "/control/audio/speaker-test",
        serde_json::json!({}),
        PLAYBACK_TIMEOUT,
    )
    .await
}

pub async fn mic_test(base_url: &str, duration_secs: Option<u64>) -> anyhow::Result<Value> {
    post_json(
        base_url,
        "/control/audio/mic-test",
        serde_json::json!({ "duration_secs": duration_secs }),
        CAPTURE_TEST_TIMEOUT,
    )
    .await
}

pub async fn loopback_test(base_url: &str, duration_secs: Option<u64>) -> anyhow::Result<Value> {
    post_json(
        base_url,
        "/control/audio/loopback",
        serde_json::json!({ "duration_secs": duration_secs }),
        CAPTURE_TEST_TIMEOUT,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_hub_keeps_latest_level_and_notifies() {
        let hub = AudioHub::new();
        let mut rx = hub.subscribe();
        hub.apply(AudioLevel {
            rms: 0.1,
            peak: 0.2,
        });
        hub.apply(AudioLevel {
            rms: 0.3,
            peak: 0.4,
        });
        assert_eq!(
            rx.try_recv().expect("first sample"),
            AudioLevel {
                rms: 0.1,
                peak: 0.2
            }
        );
        assert_eq!(
            rx.try_recv().expect("second sample"),
            AudioLevel {
                rms: 0.3,
                peak: 0.4
            }
        );
    }

    #[test]
    fn structured_audio_errors_are_readable() {
        let body = serde_json::json!({
            "error": {
                "code": "capture_unavailable",
                "message": "Audio capture is not active"
            }
        });
        assert!(
            audio_error_message(&body, reqwest::StatusCode::SERVICE_UNAVAILABLE)
                .starts_with("capture_unavailable: Audio capture is not active")
        );
    }
}
