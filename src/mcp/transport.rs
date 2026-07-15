//! MCP transport abstraction and stdio implementation.
//!
//! Defines the [`McpTransport`] trait that abstracts over different MCP transport
//! mechanisms (stdio, HTTP). The [`StdioTransport`] implements the existing
//! subprocess-based stdio transport.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::StatusCode;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, mpsc};
use tracing::debug;

use crate::config::Config;

// ── McpTransport trait ────────────────────────────────────────────────────────

/// Abstract MCP transport layer.
///
/// Implementations handle the low-level I/O: sending JSON-RPC messages to the
/// server and receiving parsed JSON-RPC messages from it.
///
/// - [`send`](McpTransport::send): Write a complete JSON-RPC message to the
///   server.
/// - [`subscribe`](McpTransport::subscribe): Return a channel receiver that
///   yields parsed JSON values from the server. Should be called exactly once.
/// - [`close`](McpTransport::close): Cleanly shut down the transport (send exit,
///   close handles, wait for child process to exit).
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a complete JSON-RPC message to the server.
    async fn send(&self, msg: Value) -> Result<()>;

    /// Subscribe to a stream of parsed JSON-RPC values from the server.
    ///
    /// Returns a receiver channel that yields parsed [`Value`] objects from the
    /// server's stdout (for stdio transport) or HTTP SSE stream (for HTTP
    /// transport, future).
    async fn subscribe(&self) -> Result<mpsc::Receiver<Value>>;

    /// Close the transport cleanly.
    ///
    /// Sends an exit notification (if applicable), closes the write handle,
    /// and waits for the server to terminate.
    async fn close(&self);
}

// ── StdioTransport ────────────────────────────────────────────────────────────

/// Stdio-based MCP transport.
///
/// Spawns a child process and communicates over stdin/stdout using
/// newline-delimited JSON-RPC messages.
pub struct StdioTransport {
    /// Stdin handle (wrapped in `Option` so we can drop it in `close`).
    stdin: Mutex<Option<ChildStdin>>,
    /// Child process handle (wrapped in `Option` so we can take it in `close`).
    child: Mutex<Option<Child>>,
}

impl StdioTransport {
    /// Spawn a child process for the given command and capture its stdin/stdout.
    ///
    /// The caller must call [`subscribe()`](Self::subscribe) to start reading
    /// stdout before sending requests.
    pub async fn spawn(command: &str) -> Result<Self> {
        let parts: Vec<&str> = command.split_whitespace().collect();
        let program = parts
            .first()
            .copied()
            .ok_or_else(|| anyhow::anyhow!("MCP_COMMAND is empty"))?;
        let args = &parts[1..];

        // Redirect server stderr to seneschal.log so it doesn't clutter TUI output.
        let log_path = Config::log_file_path();
        let stderr_sink = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map(std::process::Stdio::from)
            .unwrap_or_else(|_| std::process::Stdio::null());

        let mut child = Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(stderr_sink)
            .spawn()
            .map_err(|e| anyhow::anyhow!("MCP: failed to spawn '{}': {}", command, e))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("MCP: no stdin handle"))?;

        Ok(Self {
            stdin: Mutex::new(Some(stdin)),
            child: Mutex::new(Some(child)),
        })
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send(&self, msg: Value) -> Result<()> {
        let json = serde_json::to_string(&msg)?;
        debug!(target: "mcp", "→ {json}");
        let mut guard = self.stdin.lock().await;
        let stdin = guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("MCP: stdin closed"))?;
        stdin.write_all(json.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok(())
    }

    async fn subscribe(&self) -> Result<mpsc::Receiver<Value>> {
        let mut guard = self.child.lock().await;
        let child = guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("MCP: child process already taken"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("MCP: no stdout handle"))?;
        // Release lock before spawning the reader task.
        drop(guard);

        let (tx, rx) = mpsc::channel::<Value>(256);

        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                debug!(target: "mcp", "← {line}");
                match serde_json::from_str::<Value>(&line) {
                    Ok(v) => {
                        if tx.send(v).await.is_err() {
                            // Receiver dropped — stop reading.
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(target: "mcp", "Unparseable line: {e} — raw: {line:?}");
                    }
                }
            }
            tracing::debug!(target: "mcp", "MCP stdout reader task ended (server exited?)");
        });

        Ok(rx)
    }

    async fn close(&self) {
        // Send exit notification.
        let exit_msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "exit",
            "params": {}
        });
        let _ = self.send(exit_msg).await;

        // Drop stdin to signal EOF to the child process.
        self.stdin.lock().await.take();

        // Wait for the child process to exit.
        if let Some(mut child) = self.child.lock().await.take() {
            match child.wait().await {
                Ok(status) => {
                    debug!(target: "mcp", "MCP server exited with status: {}", status);
                }
                Err(e) => {
                    tracing::warn!(target: "mcp", "MCP server wait error: {e}");
                }
            }
        }
    }
}

// ── HttpTransport ────────────────────────────────────────────────────────────

/// HTTP-based MCP transport (MCP Streamable HTTP).
///
/// Communicates via HTTP POST for requests and SSE for streaming responses.
/// Session IDs are tracked per the MCP Streamable HTTP spec.
pub struct HttpTransport {
    /// Base URL of the MCP HTTP server.
    url: String,
    /// Reusable reqwest HTTP client.
    client: reqwest::Client,
    /// Session ID captured from `Mcp-Session-Id` response header during
    /// initialize, included in all subsequent requests.
    session_id: Arc<Mutex<Option<String>>>,
    /// Sender side of the subscribe channel — send() pushes POST responses
    /// (JSON or SSE) through this channel to the router task.
    subscribe_tx: Mutex<Option<mpsc::Sender<Value>>>,
    /// Background SSE task handle for cleanup.
    #[allow(dead_code)]
    sse_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl HttpTransport {
    /// Create a new HTTP transport targeting the given base URL.
    pub fn new(url: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self {
            url: url.to_string(),
            client,
            session_id: Arc::new(Mutex::new(None)),
            subscribe_tx: Mutex::new(None),
            sse_handle: Mutex::new(None),
        })
    }

    /// Build a request with common headers (Accept, Content-Type, session ID).
    async fn build_request(
        &self,
        method: reqwest::Method,
        accept: &str,
    ) -> reqwest::RequestBuilder {
        let mut req = self
            .client
            .request(method, &self.url)
            .header("Accept", accept);
        let session_id = self.session_id.lock().await.clone();
        if let Some(sid) = session_id {
            req = req.header("Mcp-Session-Id", sid);
        }
        req
    }

    /// Capture `Mcp-Session-Id` from response headers, if present.
    fn capture_session_id(&self, headers: &reqwest::header::HeaderMap) {
        if let Some(sid) = headers.get("Mcp-Session-Id").and_then(|v| v.to_str().ok())
            && let Ok(mut guard) = self.session_id.try_lock()
        {
            *guard = Some(sid.to_string());
        }
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn send(&self, msg: Value) -> Result<()> {
        let json = serde_json::to_string(&msg)?;
        debug!(target: "mcp", "→ {json}");

        let request = self
            .build_request(reqwest::Method::POST, "application/json, text/event-stream")
            .await;
        let response = request
            .header("Content-Type", "application/json")
            .body(json.clone())
            .send()
            .await?;

        let status = response.status();
        self.capture_session_id(response.headers());

        match status {
            StatusCode::OK => {
                let content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();

                if content_type.contains("application/json") {
                    // Single JSON-RPC response in the body.
                    let value: Value = response.json().await?;
                    debug!(target: "mcp", "← {value}");
                    let maybe_tx = self.subscribe_tx.lock().await.clone();
                    if let Some(tx) = maybe_tx {
                        let _ = tx.send(value).await;
                    }
                } else if content_type.contains("text/event-stream") {
                    // SSE response — parse data: lines and push each to channel.
                    let bytes = response.bytes().await?;
                    let text = String::from_utf8_lossy(&bytes);
                    let maybe_tx = self.subscribe_tx.lock().await.clone();
                    if let Some(tx) = maybe_tx {
                        for line in text.lines() {
                            if let Some(data) = line.strip_prefix("data: ")
                                && let Ok(value) = serde_json::from_str::<Value>(data)
                            {
                                debug!(target: "mcp", "← {value}");
                                let _ = tx.send(value).await;
                            }
                        }
                    }
                } else {
                    // Unknown content type — try JSON anyway, but warn.
                    tracing::warn!(
                        target: "mcp",
                        "Unexpected Content-Type: {content_type} — attempting JSON parse"
                    );
                    let value: Value = response.json().await?;
                    let maybe_tx = self.subscribe_tx.lock().await.clone();
                    if let Some(tx) = maybe_tx {
                        let _ = tx.send(value).await;
                    }
                }
            }
            StatusCode::ACCEPTED => {
                // 202 Accepted: response will come via SSE stream — wait for it.
                debug!(target: "mcp", "202 Accepted — awaiting SSE response");
            }
            other => {
                let body = response.text().await.unwrap_or_default();
                anyhow::bail!("MCP HTTP error {other}: {body}");
            }
        }

        Ok(())
    }

    async fn subscribe(&self) -> Result<mpsc::Receiver<Value>> {
        let (tx, rx) = mpsc::channel::<Value>(256);
        *self.subscribe_tx.lock().await = Some(tx.clone());

        let url = self.url.clone();
        let client = self.client.clone();
        let session_id = Arc::clone(&self.session_id);

        let handle = tokio::spawn(async move {
            // Reconnection loop with exponential backoff
            const MAX_BACKOFF: u64 = 30;
            let mut backoff: u64 = 1;

            loop {
                // Build GET request for SSE subscription with current session ID.
                let mut req = client.get(&url).header("Accept", "text/event-stream");
                let sid = session_id.lock().await.clone();
                if let Some(ref s) = sid {
                    req = req.header("Mcp-Session-Id", s.clone());
                }

                let response = match req.send().await {
                    Ok(r) => {
                        // Reset backoff on successful connection.
                        backoff = 1;
                        r
                    }
                    Err(e) => {
                        tracing::error!(target: "mcp", "HTTP SSE subscribe GET failed: {e}");
                        if tx.is_closed() {
                            return;
                        }
                        debug!(
                            target: "mcp",
                            "SSE reconnecting in {backoff}s (backoff up to {MAX_BACKOFF}s)",
                        );
                        tokio::time::sleep(Duration::from_secs(backoff)).await;
                        backoff = std::cmp::min(backoff * 2, MAX_BACKOFF);
                        continue;
                    }
                };

                // Capture session ID from response headers.
                if let Some(sid) = response
                    .headers()
                    .get("Mcp-Session-Id")
                    .and_then(|v| v.to_str().ok())
                {
                    *session_id.lock().await = Some(sid.to_string());
                }

                let mut stream = response.bytes_stream();
                let mut buf = String::new();

                while let Some(chunk) = stream.next().await {
                    let bytes = match chunk {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::error!(target: "mcp", "SSE stream error: {e}");
                            break;
                        }
                    };

                    buf.push_str(&String::from_utf8_lossy(&bytes));

                    // Process complete lines from the buffer.
                    while let Some(newline) = buf.find('\n') {
                        let line = buf[..newline].trim().to_string();
                        buf = buf[newline + 1..].to_string();

                        if line.is_empty() {
                            continue;
                        }

                        if let Some(data) = line.strip_prefix("data: ")
                            && let Ok(value) = serde_json::from_str::<Value>(data)
                        {
                            debug!(target: "mcp", "← {value}");
                            if tx.send(value).await.is_err() {
                                // Receiver dropped — stop.
                                return;
                            }
                        }
                    }
                }

                // Stream ended (server closed connection or error).
                debug!(target: "mcp", "SSE stream ended — attempting reconnection");

                if tx.is_closed() {
                    debug!(target: "mcp", "MCP transport channel closed, stopping reconnection");
                    return;
                }

                debug!(
                    target: "mcp",
                    "SSE reconnecting in {backoff}s (backoff up to {MAX_BACKOFF}s)",
                );
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = std::cmp::min(backoff * 2, MAX_BACKOFF);
            }
        });

        *self.sse_handle.lock().await = Some(handle);

        Ok(rx)
    }

    async fn close(&self) {
        // Cancel the SSE background task.
        if let Some(handle) = self.sse_handle.lock().await.take() {
            handle.abort();
        }
        // Drop the subscribe channel sender so the router task can end.
        self.subscribe_tx.lock().await.take();
        tracing::debug!(target: "mcp", "HTTP transport closed");
    }
}

// ── Integration tests (HTTP transport with wiremock) ─────────────────────────

#[cfg(test)]
mod http_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Helper that returns a dynamic responder for POST requests to the MCP
    /// endpoint. It inspects the JSON-RPC method in the request body and returns
    /// an appropriate response.
    fn mcp_post_responder() -> impl wiremock::Respond {
        |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap_or_default();
            let method_name = body["method"].as_str().unwrap_or("");
            // Copy the request id into the response (or null for notifications).
            let id = body.get("id").cloned().unwrap_or(Value::Null);

            match method_name {
                "initialize" => ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "protocolVersion": "2024-11-05",
                            "capabilities": {},
                            "serverInfo": {"name": "test-server", "version": "1.0.0"}
                        }
                    }))
                    .insert_header("Content-Type", "application/json"),
                "notifications/initialized" => ResponseTemplate::new(202),
                "tools/list" => ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "tools": [
                                {
                                    "name": "echo",
                                    "description": "Echo input back",
                                    "inputSchema": {
                                        "type": "object",
                                        "properties": {
                                            "text": {"type": "string"}
                                        }
                                    }
                                }
                            ]
                        }
                    }))
                    .insert_header("Content-Type", "application/json"),
                "tools/call" => ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [
                                {"type": "text", "text": "Echo: Hello"}
                            ]
                        }
                    }))
                    .insert_header("Content-Type", "application/json"),
                _ => ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {}
                    }))
                    .insert_header("Content-Type", "application/json"),
            }
        }
    }

    /// Test that the initialize handshake works over HTTP transport with a
    /// wiremock server that returns valid JSON-RPC responses.
    #[tokio::test]
    #[ignore]
    async fn http_transport_initialize() -> Result<()> {
        let server = MockServer::start().await;

        // Mock SSE subscription endpoint (GET) — returns an empty SSE stream.
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "text/event-stream")
                    .set_body_string(
                        "data: {\"jsonrpc\":\"2.0\",\"method\":\"end\",\"params\":{}}\n\n",
                    ),
            )
            .mount(&server)
            .await;

        // Mock JSON-RPC POST endpoint.
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(mcp_post_responder())
            .mount(&server)
            .await;

        let transport = HttpTransport::new(&server.uri())?;
        let mut rx = transport.subscribe().await?;

        // Send initialize request.
        transport
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "method": "initialize",
                "id": 0,
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "seneschal", "version": "0.1.0"}
                }
            }))
            .await?;

        // Wait for the initialize response on the subscribe channel.
        let resp = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for initialize response")
            .expect("channel closed before initialize response");

        assert_eq!(resp["id"], 0);
        assert!(resp["result"].is_object(), "expected result object");
        assert_eq!(
            resp["result"]["serverInfo"]["name"], "test-server",
            "unexpected server name",
        );

        transport.close().await;
        Ok(())
    }

    /// Test that tools/list returns the expected tool definitions over HTTP.
    #[tokio::test]
    #[ignore]
    async fn http_transport_tools_list() -> Result<()> {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "text/event-stream")
                    .set_body_string(
                        "data: {\"jsonrpc\":\"2.0\",\"method\":\"end\",\"params\":{}}\n\n",
                    ),
            )
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(mcp_post_responder())
            .mount(&server)
            .await;

        let transport = HttpTransport::new(&server.uri())?;
        let mut rx = transport.subscribe().await?;

        // Send tools/list request.
        transport
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "method": "tools/list",
                "id": 1,
                "params": {}
            }))
            .await?;

        let resp = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for tools/list response")
            .expect("channel closed before tools/list response");

        assert_eq!(resp["id"], 1);
        let tools = resp["result"]["tools"]
            .as_array()
            .expect("expected tools array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "echo");

        transport.close().await;
        Ok(())
    }

    /// Test that tools/call returns the expected text content over HTTP.
    #[tokio::test]
    #[ignore]
    async fn http_transport_tools_call() -> Result<()> {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "text/event-stream")
                    .set_body_string(
                        "data: {\"jsonrpc\":\"2.0\",\"method\":\"end\",\"params\":{}}\n\n",
                    ),
            )
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(mcp_post_responder())
            .mount(&server)
            .await;

        let transport = HttpTransport::new(&server.uri())?;
        let mut rx = transport.subscribe().await?;

        // Send tools/call request.
        transport
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "method": "tools/call",
                "id": 2,
                "params": {
                    "name": "echo",
                    "arguments": {"text": "Hello"}
                }
            }))
            .await?;

        let resp = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for tools/call response")
            .expect("channel closed before tools/call response");

        assert_eq!(resp["id"], 2);
        let content = resp["result"]["content"]
            .as_array()
            .expect("expected content array");
        assert_eq!(content[0]["text"], "Echo: Hello");

        transport.close().await;
        Ok(())
    }
}
