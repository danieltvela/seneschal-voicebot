//! Model-manager bridge for issue #35: a latest-progress hub plus thin REST
//! clients for the daemon's model catalog and NeMo service endpoints.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast;

pub const MODEL_PROGRESS_EVENT: &str = "desktop-model-progress";

const QUICK_TIMEOUT: Duration = Duration::from_secs(10);
/// Starting a download only kicks off the background task.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// `docker compose up` can block on image pulls on cold starts.
const COMPOSE_TIMEOUT: Duration = Duration::from_secs(180);

/// One download progress update from Control SSE (models and NeMo weights).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProgress {
    pub model_id: String,
    pub download_id: String,
    pub phase: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub current_file: Option<String>,
    pub message: Option<String>,
}

/// Latest-value bus: one progress snapshot per model id.
pub struct ModelHub {
    current: Mutex<HashMap<String, ModelProgress>>,
    tx: broadcast::Sender<ModelProgress>,
}

impl ModelHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            current: Mutex::new(HashMap::new()),
            tx,
        }
    }

    pub fn apply(&self, progress: ModelProgress) {
        self.current
            .lock()
            .expect("model progress mutex poisoned")
            .insert(progress.model_id.clone(), progress.clone());
        let _ = self.tx.send(progress);
    }

    #[cfg(test)]
    pub fn latest(&self, model_id: &str) -> Option<ModelProgress> {
        self.current
            .lock()
            .expect("model progress mutex poisoned")
            .get(model_id)
            .cloned()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ModelProgress> {
        self.tx.subscribe()
    }
}

impl Default for ModelHub {
    fn default() -> Self {
        Self::new()
    }
}

pub type SharedModelHub = Arc<ModelHub>;

/// Forward progress updates as a dedicated Tauri event.
pub fn spawn_model_progress_emitter(app: AppHandle, hub: SharedModelHub) {
    let mut rx = hub.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(progress) => {
                    let _ = app.emit(MODEL_PROGRESS_EVENT, progress);
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
        .expect("failed to build model HTTP client")
}

async fn get_json(base_url: &str, path: &str, timeout: Duration) -> anyhow::Result<Value> {
    let response = client(timeout)
        .get(format!("{base_url}{path}"))
        .send()
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    if !status.is_success() {
        return Err(anyhow::anyhow!(model_error_message(&body, status)));
    }
    Ok(body)
}

async fn post_json(base_url: &str, path: &str, timeout: Duration) -> anyhow::Result<Value> {
    let response = client(timeout)
        .post(format!("{base_url}{path}"))
        .json(&serde_json::json!({}))
        .send()
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    if !status.is_success() {
        return Err(anyhow::anyhow!(model_error_message(&body, status)));
    }
    Ok(body)
}

async fn delete_json(base_url: &str, path: &str, timeout: Duration) -> anyhow::Result<Value> {
    let response = client(timeout)
        .delete(format!("{base_url}{path}"))
        .send()
        .await?;
    let status = response.status();
    if status == reqwest::StatusCode::NO_CONTENT {
        return Ok(serde_json::json!({ "deleted": true }));
    }
    let body: Value = response.json().await?;
    if !status.is_success() {
        return Err(anyhow::anyhow!(model_error_message(&body, status)));
    }
    Ok(body)
}

/// Structured `{ error: { code, message, hint } }` or Settings-style
/// `{ ok: false, errors: [...] }` failures become readable strings.
fn model_error_message(body: &Value, status: reqwest::StatusCode) -> String {
    if let Some(errors) = body.get("errors").and_then(Value::as_array) {
        let joined = errors
            .iter()
            .filter_map(|error| {
                let field = error.get("field")?.as_str()?;
                let message = error.get("message")?.as_str()?;
                Some(format!("{field}: {message}"))
            })
            .collect::<Vec<_>>()
            .join("; ");
        if !joined.is_empty() {
            return joined;
        }
    }
    let error = body.get("error");
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("model request failed");
    let hint = error
        .and_then(|error| error.get("hint"))
        .and_then(Value::as_str);
    match (
        error
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str),
        hint,
    ) {
        (Some(code), Some(hint)) => {
            format!("{code}: {message} — {hint} (HTTP {})", status.as_u16())
        }
        (Some(code), None) => format!("{code}: {message} (HTTP {})", status.as_u16()),
        (None, Some(hint)) => format!("{message} — {hint} (HTTP {})", status.as_u16()),
        (None, None) => format!("{message} (HTTP {})", status.as_u16()),
    }
}

pub async fn catalog(base_url: &str) -> anyhow::Result<Value> {
    get_json(base_url, "/control/models", QUICK_TIMEOUT).await
}

pub async fn download_model(base_url: &str, id: &str) -> anyhow::Result<Value> {
    post_json(
        base_url,
        &format!("/control/models/{id}/download"),
        DOWNLOAD_TIMEOUT,
    )
    .await
}

pub async fn cancel_model_download(base_url: &str, id: &str) -> anyhow::Result<Value> {
    post_json(
        base_url,
        &format!("/control/models/{id}/cancel"),
        QUICK_TIMEOUT,
    )
    .await
}

pub async fn delete_model(base_url: &str, id: &str) -> anyhow::Result<Value> {
    delete_json(base_url, &format!("/control/models/{id}"), QUICK_TIMEOUT).await
}

pub async fn activate_model(base_url: &str, id: &str) -> anyhow::Result<Value> {
    post_json(
        base_url,
        &format!("/control/models/{id}/activate"),
        QUICK_TIMEOUT,
    )
    .await
}

pub async fn nemo_services(base_url: &str) -> anyhow::Result<Value> {
    get_json(base_url, "/control/nemo/services", QUICK_TIMEOUT).await
}

pub async fn nemo_start(base_url: &str, id: &str) -> anyhow::Result<Value> {
    post_json(
        base_url,
        &format!("/control/nemo/services/{id}/start"),
        COMPOSE_TIMEOUT,
    )
    .await
}

pub async fn nemo_stop(base_url: &str, id: &str) -> anyhow::Result<Value> {
    post_json(
        base_url,
        &format!("/control/nemo/services/{id}/stop"),
        COMPOSE_TIMEOUT,
    )
    .await
}

pub async fn nemo_weights(base_url: &str, id: &str) -> anyhow::Result<Value> {
    post_json(
        base_url,
        &format!("/control/nemo/services/{id}/download-weights"),
        DOWNLOAD_TIMEOUT,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_hub_keeps_latest_progress_per_model() {
        let hub = ModelHub::new();
        let mut rx = hub.subscribe();
        hub.apply(ModelProgress {
            model_id: "whisper-tiny".into(),
            download_id: "d1".into(),
            phase: "downloading".into(),
            downloaded_bytes: 10,
            total_bytes: Some(100),
            current_file: Some("ggml-tiny.bin".into()),
            message: None,
        });
        hub.apply(ModelProgress {
            model_id: "magpie-tts".into(),
            download_id: "d2".into(),
            phase: "downloading".into(),
            downloaded_bytes: 0,
            total_bytes: None,
            current_file: None,
            message: None,
        });
        hub.apply(ModelProgress {
            model_id: "whisper-tiny".into(),
            download_id: "d1".into(),
            phase: "completed".into(),
            downloaded_bytes: 100,
            total_bytes: Some(100),
            current_file: None,
            message: None,
        });

        assert_eq!(hub.latest("whisper-tiny").unwrap().phase, "completed");
        assert_eq!(hub.latest("magpie-tts").unwrap().total_bytes, None);
        assert_eq!(rx.try_recv().unwrap().model_id, "whisper-tiny");
    }

    #[test]
    fn structured_model_errors_are_readable() {
        let body = serde_json::json!({
            "error": {
                "code": "model_in_use",
                "message": "model 'kokoro-v1.0' is in use by the active configuration",
                "hint": "Switch the active model in Settings first."
            }
        });
        let text = model_error_message(&body, reqwest::StatusCode::CONFLICT);
        assert!(text.starts_with("model_in_use: model 'kokoro-v1.0' is in use"));
        assert!(text.contains("Switch the active model"));

        let validation = serde_json::json!({
            "ok": false,
            "errors": [
                { "field": "whisper_model", "message": "Whisper model path cannot be empty" }
            ]
        });
        assert_eq!(
            model_error_message(&validation, reqwest::StatusCode::UNPROCESSABLE_ENTITY),
            "whisper_model: Whisper model path cannot be empty"
        );
    }
}
