use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::Value;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::agents::hermes_events::{
    HermesMilestone, extract_milestone as extract_hermes_milestone, parse_hermes_event,
};
use crate::agents::opencode_events::{OpenCodeMilestone, extract_milestone, parse_opencode_event};

/// Represents an OpenCode session returned from `POST /session` (or Hermes `/v1/runs`).
#[derive(Debug, Clone)]
pub struct OpenCodeSession {
    pub session_id: String,
    pub directory: String,
    pub created_at: SystemTime,
}

/// HTTP transport for remote agent protocols (OpenCode, Hermes).
///
/// Manages session lifecycle and prompt submission over HTTP.  Sessions are
/// created lazily and reused across prompts.
///
/// # Path defaults
///
/// | Agent     | Session create          | Message submit               | Event stream       |
/// |-----------|------------------------|------------------------------|---------------------|
/// | OpenCode  | `/session`             | `/session/{id}/message`      | `/event`           |
/// | Hermes    | `/v1/runs`             | `/v1/runs/{id}/message`      | `/v1/runs/{id}/events` |
pub struct HttpAgentTransport {
    client: Client,
    base_url: String,
    directory: String,
    session: Arc<Mutex<Option<OpenCodeSession>>>,
    cancel_token: CancellationToken,
    /// Path for creating a session (e.g. `/session` or `/v1/runs`).
    session_create_path: String,
    /// Path template for submitting a message. `{id}` is replaced with session_id.
    message_submit_path: String,
    /// Path for the SSE event stream. `{id}` is replaced with session_id.
    event_stream_path: String,
    /// Path for cancelling a run. `{id}` is replaced with run_id.
    /// Default: `/v1/runs/{id}/stop` for Hermes.
    cancel_path: String,
    /// Optional Hermes API key. When set, `Authorization: Bearer <key>` is used
    /// instead of `x-opencode-directory`, and other Hermes-specific protocol
    /// behaviours are enabled.
    api_key: Option<String>,
    /// The last run_id returned by Hermes' `POST /v1/runs`, used for event
    /// subscription and cancellation.
    last_run_id: std::sync::Arc<tokio::sync::Mutex<Option<String>>>,
}

/// Backward-compatible alias for `HttpAgentTransport`.
pub type OpenCodeHttpTransport = HttpAgentTransport;

impl std::fmt::Debug for HttpAgentTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpAgentTransport")
            .field("base_url", &self.base_url)
            .field("directory", &self.directory)
            .field("session_create_path", &self.session_create_path)
            .field("message_submit_path", &self.message_submit_path)
            .field("event_stream_path", &self.event_stream_path)
            .finish_non_exhaustive()
    }
}

impl HttpAgentTransport {
    /// Create a new transport targeting the given server URL and directory,
    /// using **OpenCode default paths** (`/session`, `/session/{id}/message`, `/event`).
    ///
    /// The `base_url` should be the root URL (e.g. `"http://localhost:4096"`).
    /// The `directory` is sent via the `x-opencode-directory` header on every request.
    pub fn new(base_url: String, directory: String) -> Self {
        Self::with_paths(
            base_url,
            directory,
            "/session",
            "/session/{id}/message",
            "/event",
        )
    }

    /// Create a transport with explicit URL paths.
    ///
    /// Use this for Hermes or other agents that expose different endpoints.
    ///
    /// # Arguments
    ///
    /// * `session_create_path` — e.g. `/v1/runs`
    /// * `message_submit_path` — e.g. `/v1/runs/{id}/message` (the `{id}` placeholder is replaced)
    /// * `event_stream_path` — e.g. `/v1/runs/{id}/events`
    pub fn with_paths(
        base_url: String,
        directory: String,
        session_create_path: &str,
        message_submit_path: &str,
        event_stream_path: &str,
    ) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .expect("Failed to create reqwest Client for HttpAgentTransport");
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            directory,
            session: Arc::new(Mutex::new(None)),
            cancel_token: CancellationToken::new(),
            session_create_path: session_create_path.to_string(),
            message_submit_path: message_submit_path.to_string(),
            event_stream_path: event_stream_path.to_string(),
            cancel_path: "/v1/runs/{id}/stop".to_string(),
            api_key: None,
            last_run_id: Arc::new(Mutex::new(None)),
        }
    }

    /// Set the Hermes API key for Bearer token authentication.
    ///
    /// When set, the transport switches to Hermes protocol:
    /// - Uses `Authorization: Bearer <key>` instead of `x-opencode-directory`
    /// - Submits prompts via `POST /v1/runs` with `{"input": prompt}`
    /// - Reads `run_id` from session creation responses
    /// - Supports `cancel_run()` via `POST /v1/runs/{id}/stop`
    pub fn with_api_key(mut self, api_key: String) -> Self {
        self.api_key = Some(api_key);
        self
    }

    /// Set a custom cancel path template (default: `/v1/runs/{id}/stop`).
    /// `{id}` is replaced with the run/session id.
    pub fn with_cancel_path(mut self, path: &str) -> Self {
        self.cancel_path = path.to_string();
        self
    }

    /// Returns a child CancellationToken for cancellation.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel_token.child_token()
    }

    /// Cancel any in-flight request.
    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    /// Headers common to all HTTP API calls.
    ///
    /// When `api_key` is set (Hermes mode), uses `Authorization: Bearer <key>`
    /// instead of the `x-opencode-directory` header.
    fn headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("application/json"),
        );

        if let Some(ref key) = self.api_key {
            // Hermes mode: Bearer token auth
            let value = format!("Bearer {key}");
            if let Ok(hv) = reqwest::header::HeaderValue::from_str(&value) {
                headers.insert(reqwest::header::AUTHORIZATION, hv);
            }
        } else {
            // OpenCode mode: directory header
            headers.insert(
                reqwest::header::HeaderName::from_static("x-opencode-directory"),
                reqwest::header::HeaderValue::from_str(&self.directory)
                    .unwrap_or_else(|_| reqwest::header::HeaderValue::from_static(".")),
            );
        }

        headers
    }

    /// Get or create a remote session.
    ///
    /// If a session already exists, returns it. Otherwise, creates a new one
    /// via `POST {session_create_path}`.
    pub async fn get_or_create_session(&self) -> Result<OpenCodeSession> {
        let mut guard = self.session.lock().await;
        if let Some(ref session) = *guard {
            return Ok(session.clone());
        }

        info!(
            target: "opencode",
            url = %self.base_url,
            dir = %self.directory,
            path = %self.session_create_path,
            "Creating new remote agent session"
        );

        let url = format!("{}{}", self.base_url, self.session_create_path);
        let resp = self
            .client
            .post(&url)
            .headers(self.headers())
            .send()
            .await
            .context("Failed to send POST session creation request")?;

        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .context("Failed to parse POST session creation response")?;

        if !status.is_success() {
            anyhow::bail!(
                "Remote agent POST {} returned {}: {}",
                self.session_create_path,
                status,
                serde_json::to_string(&body).unwrap_or_default()
            );
        }

        let session_id = body["id"]
            .as_str()
            .or_else(|| body["sessionId"].as_str())
            .or_else(|| body["run_id"].as_str())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Remote agent POST {} response missing 'id', 'sessionId', or 'run_id': {body}",
                    self.session_create_path,
                )
            })?
            .to_string();

        // If this is a Hermes response, store the run_id separately
        if body["run_id"].as_str().is_some() {
            *self.last_run_id.lock().await = Some(session_id.clone());
        }

        let session = OpenCodeSession {
            session_id,
            directory: self.directory.clone(),
            created_at: SystemTime::now(),
        };

        info!(
            target: "opencode",
            session_id = %session.session_id,
            "Remote agent session created"
        );

        *guard = Some(session.clone());
        Ok(session)
    }

    /// Submit a prompt to the remote agent.
    ///
    /// In **OpenCode mode** (no api_key), sends via `POST {message_submit_path}`
    /// with the standard messages body.
    ///
    /// In **Hermes mode** (api_key set), sends via `POST {session_create_path}`
    /// with `{"input": prompt}` body, extracts and stores the `run_id` from the
    /// response.
    ///
    /// Cancellation is handled via the `CancellationToken`.
    pub async fn submit_prompt(
        &self,
        session_id: &str,
        prompt: &str,
        cancel: CancellationToken,
    ) -> Result<String> {
        let is_hermes = self.api_key.is_some();

        if is_hermes {
            // ── Hermes mode: POST /v1/runs with {"input": prompt} ────────
            let url = format!("{}{}", self.base_url, self.session_create_path);

            let body = serde_json::json!({
                "input": prompt
            });

            debug!(
                target: "hermes",
                url = %url,
                prompt_len = prompt.len(),
                "Submitting prompt to Hermes agent"
            );

            let req = self.client.post(&url).headers(self.headers()).json(&body);

            let resp = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    info!(target: "hermes", "Hermes prompt cancelled");
                    return Ok("[Tarea cancelada por el usuario.]".to_string());
                }
                result = req.send() => {
                    result.context("Failed to send Hermes POST run request")?
                }
            };

            let status = resp.status();
            let response_body: Value = resp
                .json()
                .await
                .context("Failed to parse Hermes POST run response")?;

            if !status.is_success() {
                anyhow::bail!(
                    "Hermes POST {} returned {}: {}",
                    self.session_create_path,
                    status,
                    serde_json::to_string(&response_body).unwrap_or_default()
                );
            }

            // Extract and store the run_id from Hermes response
            if let Some(run_id) = response_body["run_id"].as_str() {
                *self.last_run_id.lock().await = Some(run_id.to_string());
            }

            let result = response_body["content"]
                .as_str()
                .or_else(|| response_body["message"].as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| serde_json::to_string(&response_body).unwrap_or_default());

            debug!(
                target: "hermes",
                result_len = result.len(),
                "Hermes prompt response received"
            );

            return Ok(result);
        }

        // ── OpenCode mode: POST {message_submit_path} ────────────────────
        let url = format!(
            "{}{}",
            self.base_url,
            self.message_submit_path.replace("{id}", session_id)
        );

        let body = serde_json::json!({
            "messages": [
                {"role": "user", "content": prompt}
            ]
        });

        debug!(
            target: "opencode",
            session_id = %session_id,
            prompt_len = prompt.len(),
            url = %url,
            "Submitting prompt to remote agent"
        );

        let req = self.client.post(&url).headers(self.headers()).json(&body);

        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                info!(target: "opencode", session_id = %session_id, "Prompt cancelled");
                return Ok("[Tarea cancelada por el usuario.]".to_string());
            }
            result = req.send() => {
                result.context("Failed to send POST message request")?
            }
        };

        let status = resp.status();
        let response_body: Value = resp
            .json()
            .await
            .context("Failed to parse POST message response")?;

        if !status.is_success() {
            anyhow::bail!(
                "Remote agent POST message (session={}) returned {}: {}",
                session_id,
                status,
                serde_json::to_string(&response_body).unwrap_or_default()
            );
        }

        // Extract the assistant's response from the message array or single response.
        let result = response_body["content"]
            .as_str()
            .or_else(|| response_body["message"].as_str())
            .or_else(|| {
                response_body.as_array().and_then(|arr| {
                    arr.iter()
                        .rfind(|m| m["role"].as_str() == Some("assistant"))
                        .and_then(|m| m["content"].as_str())
                })
            })
            .map(|s| s.to_string())
            .unwrap_or_else(|| serde_json::to_string(&response_body).unwrap_or_default());

        debug!(
            target: "opencode",
            session_id = %session_id,
            result_len = result.len(),
            "Prompt response received"
        );

        Ok(result)
    }

    /// Cancel a running Hermes run via `POST {cancel_path}` (default:
    /// `/v1/runs/{id}/stop`).  In OpenCode mode this is a no-op (cancellation
    /// is handled via the `CancellationToken`).
    ///
    /// Returns an error if the HTTP request fails.
    pub async fn cancel_run(&self, run_id: &str) -> Result<()> {
        if self.api_key.is_none() {
            // OpenCode mode: cancellation is handled by the CancellationToken;
            // nothing to do over HTTP.
            return Ok(());
        }

        let url = format!(
            "{}{}",
            self.base_url,
            self.cancel_path.replace("{id}", run_id)
        );

        debug!(target: "hermes", url = %url, run_id = %run_id, "Cancelling Hermes run");

        let resp = self
            .client
            .post(&url)
            .headers(self.headers())
            .send()
            .await
            .context("Failed to send Hermes cancel request")?;

        let status = resp.status();
        if !status.is_success() {
            let body: Value = resp.json().await.unwrap_or_default();
            anyhow::bail!(
                "Hermes POST cancel (run={}) returned {}: {}",
                run_id,
                status,
                serde_json::to_string(&body).unwrap_or_default()
            );
        }

        info!(target: "hermes", run_id = %run_id, "Hermes run cancelled");
        Ok(())
    }

    /// Get the last stored `run_id` from Hermes mode, if any.
    ///
    /// Returns `None` when no run has been created yet, or in OpenCode mode.
    pub async fn get_last_run_id(&self) -> Option<String> {
        self.last_run_id.lock().await.clone()
    }

    /// Returns `true` when this transport is configured for Hermes protocol
    /// (i.e. an API key was provided).
    pub fn is_hermes(&self) -> bool {
        self.api_key.is_some()
    }

    /// Subscribe to the SSE event stream for a session (OpenCode format).
    ///
    /// Returns a receiver that delivers `OpenCodeMilestone` values (rate-limited
    /// to at most one milestone every 5 seconds), plus a `CancellationToken`
    /// that the caller can use to stop the subscriber.
    ///
    /// The subscriber uses `GET {event_stream_path}` with `{id}` replaced by
    /// `session_id`, and `Accept: text/event-stream`.
    pub fn subscribe_events(
        &self,
        session_id: &str,
    ) -> (mpsc::Receiver<OpenCodeMilestone>, CancellationToken) {
        let (tx, rx) = mpsc::channel::<OpenCodeMilestone>(32);
        let sid = session_id.to_string();
        let url = format!(
            "{}{}",
            self.base_url,
            self.event_stream_path.replace("{id}", &sid)
        );
        let client = self.client.clone();
        let cancel_token = self.cancellation_token();
        let cancel_inner = cancel_token.clone();

        tokio::spawn(async move {
            debug!(target: "opencode", url = %url, "Starting SSE event subscriber");

            let response = match client
                .get(&url)
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .send()
                .await
            {
                Ok(resp) => resp,
                Err(e) => {
                    warn!(target: "opencode", "SSE connection failed: {e}");
                    return;
                }
            };

            if !response.status().is_success() {
                warn!(
                    target: "opencode",
                    status = %response.status(),
                    "SSE connection returned non-success"
                );
                return;
            }

            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let min_interval = Duration::from_secs(5);
            let mut last_milestone_at: Option<std::time::Instant> = None;

            use futures_util::StreamExt;

            loop {
                tokio::select! {
                    biased;
                    _ = cancel_inner.cancelled() => {
                        debug!(target: "opencode", "SSE event subscriber cancelled");
                        break;
                    }
                    chunk = stream.next() => {
                        match chunk {
                            Some(Ok(bytes)) => {
                                buffer.push_str(&String::from_utf8_lossy(&bytes));

                                // Process complete SSE events (separated by \n\n)
                                while let Some(pos) = buffer.find("\n\n") {
                                    let event_text = buffer[..pos].to_string();
                                    buffer = buffer[pos + 2..].to_string();

                                    if let Some(event) = parse_opencode_event(&event_text)
                                        && let Some(milestone) = extract_milestone(&event)
                                    {
                                        // Rate-limit: max 1 per 5 seconds
                                        let should_send = match last_milestone_at {
                                            Some(t) => t.elapsed() >= min_interval,
                                            None => true,
                                        };

                                        if should_send {
                                            last_milestone_at = Some(std::time::Instant::now());
                                            let ms = OpenCodeMilestone {
                                                milestone,
                                                correlation_id: sid.clone(),
                                            };
                                            if tx.send(ms).await.is_err() {
                                                debug!(
                                                    target: "opencode",
                                                    "Milestone receiver dropped"
                                                );
                                                return;
                                            }
                                        }
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                warn!(target: "opencode", "SSE stream error: {e}");
                                break;
                            }
                            None => {
                                debug!(target: "opencode", "SSE stream ended");
                                break;
                            }
                        }
                    }
                }
            }
        });

        (rx, cancel_token)
    }

    /// Subscribe to the Hermes SSE event stream for a specific run.
    ///
    /// Like [`subscribe_events`](Self::subscribe_events) but uses Hermes event
    /// parsing (`HermesEvent` / `HermesMilestone`).  Returns a receiver that
    /// delivers `HermesMilestone` values, plus a `CancellationToken` that the
    /// caller can use to stop the subscriber.
    pub fn subscribe_hermes_events(
        &self,
        run_id: &str,
    ) -> (mpsc::Receiver<HermesMilestone>, CancellationToken) {
        let (tx, rx) = mpsc::channel::<HermesMilestone>(32);
        let rid = run_id.to_string();
        let url = format!(
            "{}{}",
            self.base_url,
            self.event_stream_path.replace("{id}", &rid)
        );
        let client = self.client.clone();
        let cancel_token = self.cancellation_token();
        let cancel_inner = cancel_token.clone();

        tokio::spawn(async move {
            debug!(target: "hermes", url = %url, "Starting Hermes SSE event subscriber");

            let response = match client
                .get(&url)
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .send()
                .await
            {
                Ok(resp) => resp,
                Err(e) => {
                    warn!(target: "hermes", "Hermes SSE connection failed: {e}");
                    return;
                }
            };

            if !response.status().is_success() {
                warn!(
                    target: "hermes",
                    status = %response.status(),
                    "Hermes SSE connection returned non-success"
                );
                return;
            }

            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let min_interval = Duration::from_secs(5);
            let mut last_milestone_at: Option<std::time::Instant> = None;

            use futures_util::StreamExt;

            loop {
                tokio::select! {
                    biased;
                    _ = cancel_inner.cancelled() => {
                        debug!(target: "hermes", "Hermes SSE event subscriber cancelled");
                        break;
                    }
                    chunk = stream.next() => {
                        match chunk {
                            Some(Ok(bytes)) => {
                                buffer.push_str(&String::from_utf8_lossy(&bytes));

                                // Process complete SSE events (separated by \n\n)
                                while let Some(pos) = buffer.find("\n\n") {
                                    let event_text = buffer[..pos].to_string();
                                    buffer = buffer[pos + 2..].to_string();

                                    if let Some(event) = parse_hermes_event(&event_text)
                                        && let Some(milestone) = extract_hermes_milestone(&event)
                                    {
                                        // Rate-limit: max 1 per 5 seconds
                                        let should_send = match last_milestone_at {
                                            Some(t) => t.elapsed() >= min_interval,
                                            None => true,
                                        };

                                        if should_send {
                                            last_milestone_at = Some(std::time::Instant::now());
                                            let ms = HermesMilestone {
                                                milestone,
                                                correlation_id: rid.clone(),
                                            };
                                            if tx.send(ms).await.is_err() {
                                                debug!(
                                                    target: "hermes",
                                                    "Hermes milestone receiver dropped"
                                                );
                                                return;
                                            }
                                        }
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                warn!(target: "hermes", "Hermes SSE stream error: {e}");
                                break;
                            }
                            None => {
                                debug!(target: "hermes", "Hermes SSE stream ended");
                                break;
                            }
                        }
                    }
                }
            }
        });

        (rx, cancel_token)
    }

    /// List messages for a session via `GET {message_submit_path}`.
    #[allow(dead_code)]
    pub async fn list_messages(&self, session_id: &str) -> Result<Value> {
        let url = format!(
            "{}{}",
            self.base_url,
            self.message_submit_path.replace("{id}", session_id)
        );

        let resp = self
            .client
            .get(&url)
            .headers(self.headers())
            .send()
            .await
            .context("Failed to send GET message request")?;

        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .context("Failed to parse GET message response")?;

        if !status.is_success() {
            anyhow::bail!(
                "Remote agent GET message (session={}) returned {}: {}",
                session_id,
                status,
                serde_json::to_string(&body).unwrap_or_default()
            );
        }

        Ok(body)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Helper: create a transport pointed at a wiremock server.
    async fn mock_transport(server: &MockServer) -> OpenCodeHttpTransport {
        OpenCodeHttpTransport {
            client: Client::new(),
            base_url: server.uri().trim_end_matches('/').to_string(),
            directory: "/tmp/test".to_string(),
            session: Arc::new(Mutex::new(None)),
            cancel_token: CancellationToken::new(),
            session_create_path: "/session".to_string(),
            message_submit_path: "/session/{id}/message".to_string(),
            event_stream_path: "/event".to_string(),
            cancel_path: "/v1/runs/{id}/stop".to_string(),
            api_key: None,
            last_run_id: Arc::new(Mutex::new(None)),
        }
    }

    /// Helper: create a Hermes-mode transport pointed at a wiremock server.
    #[allow(dead_code)]
    async fn hermes_mock_transport(server: &MockServer) -> OpenCodeHttpTransport {
        OpenCodeHttpTransport {
            client: Client::new(),
            base_url: server.uri().trim_end_matches('/').to_string(),
            directory: String::new(),
            session: Arc::new(Mutex::new(None)),
            cancel_token: CancellationToken::new(),
            session_create_path: "/v1/runs".to_string(),
            message_submit_path: "/v1/runs/{id}/message".to_string(),
            event_stream_path: "/v1/runs/{id}/events".to_string(),
            cancel_path: "/v1/runs/{id}/stop".to_string(),
            api_key: Some("test-api-key".to_string()),
            last_run_id: Arc::new(Mutex::new(None)),
        }
    }

    #[tokio::test]
    async fn session_creation_success() {
        let mock = MockServer::start().await;
        let transport = mock_transport(&mock).await;

        Mock::given(method("POST"))
            .and(path("/session"))
            .and(header("accept", "application/json"))
            .and(header("x-opencode-directory", "/tmp/test"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"id": "session-123", "status": "ok"})),
            )
            .mount(&mock)
            .await;

        let session = transport.get_or_create_session().await.unwrap();
        assert_eq!(session.session_id, "session-123");
        assert_eq!(session.directory, "/tmp/test");
    }

    #[tokio::test]
    async fn session_creation_reuses_existing() {
        let mock = MockServer::start().await;
        let transport = mock_transport(&mock).await;

        // First call — should hit the server
        Mock::given(method("POST"))
            .and(path("/session"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"id": "session-123", "status": "ok"})),
            )
            .expect(1) // Only one call expected
            .mount(&mock)
            .await;

        let s1 = transport.get_or_create_session().await.unwrap();
        let s2 = transport.get_or_create_session().await.unwrap();
        assert_eq!(s1.session_id, s2.session_id);
    }

    #[tokio::test]
    async fn session_creation_handles_server_error() {
        let mock = MockServer::start().await;
        let transport = mock_transport(&mock).await;

        Mock::given(method("POST"))
            .and(path("/session"))
            .respond_with(
                ResponseTemplate::new(500)
                    .set_body_json(serde_json::json!({"error": "Internal Server Error"})),
            )
            .mount(&mock)
            .await;

        let result = transport.get_or_create_session().await;
        assert!(
            result.is_err(),
            "Expected error on 500 response, got: {:?}",
            result
        );
        let err = result.unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("500"), "Error should mention 500: {msg}");
    }

    #[tokio::test]
    async fn prompt_submission_success() {
        let mock = MockServer::start().await;
        let transport = mock_transport(&mock).await;

        // First create a session
        Mock::given(method("POST"))
            .and(path("/session"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "session-456"})),
            )
            .mount(&mock)
            .await;

        // Then submit a prompt
        Mock::given(method("POST"))
            .and(path_regex(r"^/session/session-456/message$"))
            .and(header("accept", "application/json"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"content": "Hello from OpenCode!"})),
            )
            .mount(&mock)
            .await;

        let session = transport.get_or_create_session().await.unwrap();
        let cancel = transport.cancellation_token();
        let result = transport
            .submit_prompt(&session.session_id, "Say hello", cancel)
            .await
            .unwrap();

        assert_eq!(result, "Hello from OpenCode!");
    }

    #[tokio::test]
    async fn prompt_submission_handles_error() {
        let mock = MockServer::start().await;
        let transport = mock_transport(&mock).await;

        Mock::given(method("POST"))
            .and(path("/session"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "session-err"})),
            )
            .mount(&mock)
            .await;

        Mock::given(method("POST"))
            .and(path_regex(r"^/session/session-err/message$"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_json(serde_json::json!({"error": "bad request"})),
            )
            .mount(&mock)
            .await;

        let session = transport.get_or_create_session().await.unwrap();
        let cancel = transport.cancellation_token();
        let result = transport
            .submit_prompt(&session.session_id, "fail", cancel)
            .await;

        assert!(result.is_err(), "Expected error on 400, got: {:?}", result);
    }

    #[tokio::test]
    async fn cancellation_works() {
        let mock = MockServer::start().await;
        let transport = mock_transport(&mock).await;

        Mock::given(method("POST"))
            .and(path("/session"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"id": "session-cancel"})),
            )
            .mount(&mock)
            .await;

        let session = transport.get_or_create_session().await.unwrap();

        // Mount a handler that delays long enough for cancellation to fire
        Mock::given(method("POST"))
            .and(path_regex(r"^/session/session-cancel/message$"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_secs(60))
                    .set_body_json(serde_json::json!({"content": "should never arrive"})),
            )
            .expect(0..=1) // may or may not receive the request before cancellation
            .mount(&mock)
            .await;

        let cancel = transport.cancellation_token();

        let transport_arc = std::sync::Arc::new(transport);
        let session_id = session.session_id.clone();
        let transport_for_task = Arc::clone(&transport_arc);

        let handle = tokio::spawn(async move {
            transport_for_task
                .submit_prompt(&session_id, "long task", cancel)
                .await
        });

        // Give it a moment to start the request
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Cancel from another task
        let transport_for_cancel = Arc::clone(&transport_arc);
        tokio::spawn(async move {
            transport_for_cancel.cancel();
        });

        let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle)
            .await
            .expect("timed out waiting for cancellation")
            .expect("join error");

        assert_eq!(
            result.unwrap(),
            "[Tarea cancelada por el usuario.]",
            "Cancellation should return cancel message"
        );
    }

    // ── Hermes mode tests ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn hermes_session_creation_uses_run_id_field() {
        let mock = MockServer::start().await;
        let transport = hermes_mock_transport(&mock).await;

        // Hermes returns {"run_id": "run-123"} instead of {"id": "..."}
        Mock::given(method("POST"))
            .and(path("/v1/runs"))
            .and(header("accept", "application/json"))
            .and(header("authorization", "Bearer test-api-key"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"run_id": "run-123", "status": "ok"})),
            )
            .mount(&mock)
            .await;

        let session = transport.get_or_create_session().await.unwrap();
        assert_eq!(session.session_id, "run-123");
        let stored = transport.get_last_run_id().await;
        assert_eq!(stored, Some("run-123".to_string()));
    }

    #[tokio::test]
    async fn hermes_prompt_submission_sends_input_field() {
        let mock = MockServer::start().await;
        let transport = hermes_mock_transport(&mock).await;

        // Hermes POST /v1/runs with {"input": "hello"}
        Mock::given(method("POST"))
            .and(path("/v1/runs"))
            .and(header("authorization", "Bearer test-api-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"run_id": "run-456", "content": "Hello from Hermes!"}),
            ))
            .mount(&mock)
            .await;

        let cancel = transport.cancellation_token();
        let result = transport.submit_prompt("", "hello", cancel).await.unwrap();

        assert_eq!(result, "Hello from Hermes!");
        let stored = transport.get_last_run_id().await;
        assert_eq!(stored, Some("run-456".to_string()));
    }

    #[tokio::test]
    async fn hermes_cancel_run_sends_stop_request() {
        let mock = MockServer::start().await;
        let transport = hermes_mock_transport(&mock).await;

        Mock::given(method("POST"))
            .and(path("/v1/runs/run-cancel/stop"))
            .and(header("authorization", "Bearer test-api-key"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"status": "stopped"})),
            )
            .mount(&mock)
            .await;

        let result = transport.cancel_run("run-cancel").await;
        assert!(result.is_ok(), "cancel_run should succeed: {:?}", result);
    }

    #[tokio::test]
    async fn hermes_cancel_run_returns_error_on_failure() {
        let mock = MockServer::start().await;
        let transport = hermes_mock_transport(&mock).await;

        Mock::given(method("POST"))
            .and(path("/v1/runs/run-fail/stop"))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(serde_json::json!({"error": "not found"})),
            )
            .mount(&mock)
            .await;

        let result = transport.cancel_run("run-fail").await;
        assert!(result.is_err(), "cancel_run should fail on 404");
    }

    #[tokio::test]
    async fn hermes_is_hermes_returns_true() {
        let mock = MockServer::start().await;
        let transport = hermes_mock_transport(&mock).await;
        assert!(transport.is_hermes());
    }

    #[tokio::test]
    async fn opencode_is_hermes_returns_false() {
        let mock = MockServer::start().await;
        let transport = mock_transport(&mock).await;
        assert!(!transport.is_hermes());
    }
}
