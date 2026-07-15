//! MCP (Model Context Protocol) client — JSON-RPC 2.0 over stdio (or, in
//! future, HTTP).
//!
//! Transport is abstracted behind the [`McpTransport`] trait, allowing the
//! same protocol code to work over stdio subprocesses, HTTP SSE connections,
//! or any future transport.
//!
//! Concurrent `call_tool()` calls are safe: each request registers a oneshot
//! channel keyed on its JSON-RPC request id; the router task routes responses
//! to the correct waiter.
//!
//! Since Gap 1 (architecture redesign, see `doc/ARCHITECTURE-MCP-LAYER.md`),
//! the router task also routes **server→client notifications** (JSON-RPC
//! messages with a `method` but no `id`) to an optional `McpNotificationHandler`.
//! When the handler returns a `ProactiveEvent`, the client forwards it to the
//! main loop via the `proactive_tx` channel. Without a handler, the behavior is
//! the legacy one: notifications are silently ignored.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{debug, info, warn};

use crate::agents::ProactiveEvent;

pub mod config;
pub mod transport;

#[allow(unused_imports)]
pub use config::{McpConfig, McpRegistry, McpServerTomlConfig, McpTransportKind};
pub use transport::McpTransport;

// ── Notification handler trait ───────────────────────────────────────────────

/// Handler for server→client MCP notifications.
///
/// Implementations receive the JSON-RPC `method` and `params` of an inbound
/// notification that has no associated `id` (i.e., a one-way message from the
/// MCP server). The handler may inspect them and return a `ProactiveEvent` to
/// inject into the seneschal main loop; returning `None` means "ignore".
///
/// The handler is `Send + Sync` so it can be shared across the reader task and
/// the `McpClient` struct via `Arc<dyn …>`.
pub trait McpNotificationHandler: Send + Sync {
    fn handle(&self, method: &str, params: Value) -> Option<ProactiveEvent>;
}

/// No-op handler — preserves the legacy behavior of silently ignoring
/// notifications. Used by `spawn_and_init` when callers do not supply one.
#[allow(dead_code)]
struct NoopNotificationHandler;

impl McpNotificationHandler for NoopNotificationHandler {
    fn handle(&self, _method: &str, _params: Value) -> Option<ProactiveEvent> {
        None
    }
}

/// Default handler that forwards **every** server notification to the
/// seneschal main loop as a `ProactiveEvent::McpNotification`.
///
/// Use this when you want the pipeline (and the Control API, via
/// `ControlEvent::McpNotification`) to see all server→client messages and
/// decide what to do with them.
///
/// Server-name tagging is performed by the caller (they know which server
/// spawned the reader task that invokes this handler).
pub struct ForwardingNotificationHandler {
    pub server_name: String,
}

impl McpNotificationHandler for ForwardingNotificationHandler {
    fn handle(&self, method: &str, params: Value) -> Option<ProactiveEvent> {
        // Ignore the standard `notifications/initialized` handshake message —
        // it has no semantic value for the pipeline.
        if method == "notifications/initialized" {
            return None;
        }
        Some(ProactiveEvent::McpNotification {
            server_name: self.server_name.clone(),
            method: method.to_string(),
            params,
        })
    }
}

// ── Response / notification parsing ──────────────────────────────────────────

/// Parsed inbound JSON-RPC response (id already matched and removed from pending map).
#[derive(Debug)]
struct RpcResponse {
    result: Option<Value>,
    error: Option<Value>,
}

/// Inbound notification: method + params, with no `id`.
#[derive(Debug)]
struct RpcNotification {
    method: String,
    params: Value,
}

/// Classified inbound JSON-RPC message.
#[derive(Debug)]
enum Inbound {
    Response(u64, RpcResponse),
    Notification(RpcNotification),
    /// Server request (has `id` AND `method`) — not currently supported; ignored.
    ServerRequest,
}

/// Parse an inbound JSON value into one of three categories.
fn classify_inbound(v: &Value) -> Option<Inbound> {
    let has_method = v.get("method").is_some();
    let id = v.get("id").and_then(|i| i.as_u64());

    if has_method && id.is_some() {
        return Some(Inbound::ServerRequest);
    }

    if has_method {
        let method = v.get("method")?.as_str()?.to_string();
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        return Some(Inbound::Notification(RpcNotification { method, params }));
    }

    let id = id?;
    Some(Inbound::Response(
        id,
        RpcResponse {
            result: v.get("result").cloned(),
            error: v.get("error").cloned(),
        },
    ))
}

/// Backwards-compatible parser equivalent to `classify_inbound(...).into_response()`.
/// Returns None for notifications or unrecognized messages.
///
/// Kept for backwards compatibility with existing unit tests that assert the
/// legacy parser contract. New code should use [`classify_inbound`] which also
/// distinguishes notifications and server-initiated requests.
#[allow(dead_code)]
fn parse_response(v: &Value) -> Option<(u64, RpcResponse)> {
    match classify_inbound(v)? {
        Inbound::Response(id, resp) => Some((id, resp)),
        _ => None,
    }
}

// ── Tool definition ──────────────────────────────────────────────────────────

/// A tool exposed by the MCP server (from `tools/list`).
#[derive(Debug, Clone)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool input (`inputSchema` field).
    pub input_schema: Value,
}

// ── McpClient ────────────────────────────────────────────────────────────────

/// Persistent MCP server client (transport-agnostic).
///
/// Uses a [`Box<dyn McpTransport>`] for I/O, allowing the same protocol logic
/// to work over stdio subprocesses, HTTP SSE connections, or any future
/// transport.
pub struct McpClient {
    /// The underlying transport (stdio, HTTP, etc.).
    transport: Box<dyn McpTransport>,
    /// Monotonically increasing JSON-RPC request id.
    next_id: AtomicU64,
    /// In-flight request map: id → response channel.
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<RpcResponse>>>>,
    /// Hard timeout for each tool call (seconds).
    tool_timeout_secs: u64,
    /// Optional handler for server→client notifications. When `None`,
    /// inbound notifications are silently ignored (legacy behavior).
    /// Stored on the client for future introspection / reconnection logic;
    /// the router task keeps its own `Arc` clone for routing.
    #[allow(dead_code)]
    notification_handler: Option<Arc<dyn McpNotificationHandler>>,
    /// Optional proactive event channel where notification-derived events
    /// are forwarded by the router task.
    #[allow(dead_code)]
    proactive_tx: Option<mpsc::Sender<ProactiveEvent>>,
}

impl McpClient {
    /// Spawn the MCP server process, perform the initialize handshake, query
    /// `tools/list`, and return `(client, tool_defs)`.
    ///
    /// Backwards-compatible entry point: notifications are ignored (legacy
    /// behavior). Use [`McpClient::spawn_and_init_with_handler`] to route
    /// server→client notifications.
    pub async fn spawn_and_init(
        command: &str,
        tool_timeout_secs: u64,
    ) -> Result<(Self, Vec<McpToolDef>)> {
        Self::spawn_and_init_with_handler(command, tool_timeout_secs, None, None).await
    }

    /// Spawn the MCP server process and wire optional notification handling.
    ///
    /// When `notification_handler` is `Some`, the router task classifies each
    /// inbound JSON-RPC message:
    /// - **Response** (id + no method): routed to the oneshot waiter as before.
    /// - **Notification** (method + no id): passed to the handler; if the
    ///   handler returns `Some(ProactiveEvent)` and `proactive_tx` is `Some`,
    ///   the event is forwarded to the seneschal main loop.
    /// - **Server request** (id + method): logged and ignored — seneschal does
    ///   not currently act as an MCP server.
    pub async fn spawn_and_init_with_handler(
        command: &str,
        tool_timeout_secs: u64,
        notification_handler: Option<Arc<dyn McpNotificationHandler>>,
        proactive_tx: Option<mpsc::Sender<ProactiveEvent>>,
    ) -> Result<(Self, Vec<McpToolDef>)> {
        let transport = transport::StdioTransport::spawn(command).await?;
        Self::init_from_transport(
            Box::new(transport),
            tool_timeout_secs,
            notification_handler,
            proactive_tx,
        )
        .await
    }

    /// Connect to an MCP server over HTTP (MCP Streamable HTTP transport).
    ///
    /// Performs the same initialize handshake and `tools/list` query as the
    /// stdio variant, but communicates over HTTP POST + SSE instead of a
    /// subprocess.
    pub async fn connect_http(
        url: &str,
        tool_timeout_secs: u64,
        notification_handler: Option<Arc<dyn McpNotificationHandler>>,
        proactive_tx: Option<mpsc::Sender<ProactiveEvent>>,
    ) -> Result<(Self, Vec<McpToolDef>)> {
        let transport = transport::HttpTransport::new(url)?;
        Self::init_from_transport(
            Box::new(transport),
            tool_timeout_secs,
            notification_handler,
            proactive_tx,
        )
        .await
    }

    /// Shared initialisation logic for any transport.
    ///
    /// Subscribes to the transport's message stream, spawns the router task,
    /// runs the MCP initialize handshake, and queries `tools/list`.
    async fn init_from_transport(
        transport: Box<dyn McpTransport>,
        tool_timeout_secs: u64,
        notification_handler: Option<Arc<dyn McpNotificationHandler>>,
        proactive_tx: Option<mpsc::Sender<ProactiveEvent>>,
    ) -> Result<(Self, Vec<McpToolDef>)> {
        // Start reading messages from the transport.
        let mut rx = transport.subscribe().await?;

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<RpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_reader = Arc::clone(&pending);

        let handler_clone = notification_handler.clone();
        let proactive_tx_clone = proactive_tx.clone();

        // Router task: consume parsed JSON values from the transport, classify
        // inbound messages, and route responses / notifications accordingly.
        tokio::spawn(async move {
            use tokio::sync::mpsc;

            while let Some(v) = rx.recv().await {
                match classify_inbound(&v) {
                    Some(Inbound::Response(id, resp)) => {
                        let tx = pending_reader.lock().await.remove(&id);
                        if let Some(tx) = tx {
                            let _ = tx.send(resp);
                        } else {
                            warn!(target: "mcp", "Unexpected response for id={id}");
                        }
                    }
                    Some(Inbound::Notification(notif)) => {
                        if let (Some(handler), Some(tx)) =
                            (handler_clone.as_ref(), proactive_tx_clone.as_ref())
                        {
                            if let Some(event) = handler.handle(&notif.method, notif.params) {
                                // Non-blocking: if the channel is full
                                // (slow consumer) we drop the event
                                // and log, rather than blocking the
                                // router task and stalling responses.
                                match tx.try_send(event) {
                                    Ok(()) => {}
                                    Err(mpsc::error::TrySendError::Full(ev)) => {
                                        warn!(
                                            target: "mcp",
                                            "Proactive channel full — dropped MCP notification: {ev:?}"
                                        );
                                    }
                                    Err(mpsc::error::TrySendError::Closed(ev)) => {
                                        warn!(
                                            target: "mcp",
                                            "Proactive channel closed — dropping MCP notification: {ev:?}"
                                        );
                                    }
                                }
                            }
                        } else {
                            // Legacy path: handler not configured.
                            // `notifications/initialized` among others
                            // falls through silently here.
                            debug!(
                                target: "mcp",
                                "Ignoring server notification: {}",
                                notif.method
                            );
                        }
                    }
                    Some(Inbound::ServerRequest) => {
                        debug!(
                            target: "mcp",
                            "Ignoring server-initiated request \
                             (seneschal is not an MCP server)"
                        );
                    }
                    None => {
                        warn!(target: "mcp", "Unrecognizable JSON-RPC message");
                    }
                }
            }
            debug!(target: "mcp", "MCP router task ended (transport closed?)");
        });

        let client = Self {
            transport,
            next_id: AtomicU64::new(0),
            pending,
            tool_timeout_secs,
            notification_handler,
            proactive_tx,
        };

        // ── MCP handshake ────────────────────────────────────────────────────
        client.initialize().await?;
        let tools = client.list_tools().await?;

        info!(
            target: "mcp",
            "MCP server ready — {} tool(s): {:?}",
            tools.len(),
            tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        );

        Ok((client, tools))
    }

    // ── Transport helpers ─────────────────────────────────────────────────────

    /// Build a JSON-RPC request with a fresh id and send it via the transport.
    async fn send_request(&self, method: &str, params: Value) -> Result<u64> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.transport.send(msg).await?;
        Ok(id)
    }

    /// Build a JSON-RPC notification (no id) and send it via the transport.
    async fn send_notification(&self, method: &str, params: Value) -> Result<()> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.transport.send(msg).await
    }

    // ── Protocol methods ─────────────────────────────────────────────────────

    /// Send `initialize` and `notifications/initialized`.
    async fn initialize(&self) -> Result<()> {
        // Send initialize request.
        let init_id = self
            .send_request(
                "initialize",
                serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "seneschal", "version": "0.1.0"},
                }),
            )
            .await?;

        // Wait for initialize response.
        let resp = self.wait_for_response(init_id).await?;
        if let Some(err) = resp.error {
            anyhow::bail!("MCP initialize error: {err}");
        }
        debug!(target: "mcp", "initialize OK");

        // Send initialized notification (no response expected).
        self.send_notification("notifications/initialized", serde_json::json!({}))
            .await?;

        Ok(())
    }

    /// Call `tools/list` and return the tool definitions.
    async fn list_tools(&self) -> Result<Vec<McpToolDef>> {
        let id = self
            .send_request("tools/list", serde_json::json!({}))
            .await?;

        let resp = self.wait_for_response(id).await?;
        if let Some(err) = resp.error {
            anyhow::bail!("MCP tools/list error: {err}");
        }

        let result = resp.result.unwrap_or_default();
        let tools_arr = result["tools"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("MCP tools/list: missing 'tools' array"))?;

        let defs = tools_arr
            .iter()
            .filter_map(|t| {
                let name = t["name"].as_str()?.to_string();
                let description = t["description"].as_str().unwrap_or("").to_string();
                let input_schema = t
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}));
                Some(McpToolDef {
                    name,
                    description,
                    input_schema,
                })
            })
            .collect();

        Ok(defs)
    }

    /// Call `tools/call` and return the text content of the response.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<String> {
        let id = self
            .send_request(
                "tools/call",
                serde_json::json!({
                    "name": name,
                    "arguments": arguments,
                }),
            )
            .await?;

        let resp = tokio::time::timeout(
            Duration::from_secs(self.tool_timeout_secs),
            self.wait_for_response(id),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "MCP tool '{}' timed out after {}s",
                name,
                self.tool_timeout_secs
            )
        })??;

        if let Some(err) = resp.error {
            anyhow::bail!("MCP tool '{}' error: {err}", name);
        }

        let result = resp.result.unwrap_or_default();
        Ok(extract_text_content(&result))
    }

    /// Send exit notification, close stdin, and wait for child to exit.
    pub async fn disconnect(self) {
        self.transport.close().await;
        self.pending.lock().await.clear();
    }

    // ── Internal ─────────────────────────────────────────────────────────────

    /// Register a oneshot channel for request `id` and wait for the response.
    async fn wait_for_response(&self, id: u64) -> Result<RpcResponse> {
        let (tx, rx) = oneshot::channel::<RpcResponse>();
        self.pending.lock().await.insert(id, tx);
        rx.await
            .map_err(|_| anyhow::anyhow!("MCP: response channel closed for id={id}"))
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Extract text content from a `tools/call` result.
///
/// MCP result format: `{"content": [{"type": "text", "text": "..."}], "isError": false}`
/// All text parts are joined with newlines.
fn extract_text_content(result: &Value) -> String {
    let content = match result["content"].as_array() {
        Some(arr) => arr,
        None => return result.to_string(),
    };

    let parts: Vec<&str> = content
        .iter()
        .filter(|item| item["type"].as_str() == Some("text"))
        .filter_map(|item| item["text"].as_str())
        .collect();

    if parts.is_empty() {
        result.to_string()
    } else {
        parts.join("\n")
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_response_ignores_notifications() {
        let notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        });
        assert!(parse_response(&notif).is_none());
    }

    #[test]
    fn parse_response_matches_success() {
        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "result": {"tools": []}
        });
        let (id, r) = parse_response(&resp).unwrap();
        assert_eq!(id, 3);
        assert!(r.error.is_none());
        assert!(r.result.is_some());
    }

    #[test]
    fn parse_response_matches_error() {
        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "error": {"code": -32601, "message": "Method not found"}
        });
        let (id, r) = parse_response(&resp).unwrap();
        assert_eq!(id, 5);
        assert!(r.error.is_some());
    }

    #[test]
    fn extract_text_content_single_part() {
        let result = serde_json::json!({
            "content": [{"type": "text", "text": "Hola mundo"}],
            "isError": false
        });
        assert_eq!(extract_text_content(&result), "Hola mundo");
    }

    #[test]
    fn extract_text_content_multiple_parts() {
        let result = serde_json::json!({
            "content": [
                {"type": "text", "text": "Parte 1"},
                {"type": "text", "text": "Parte 2"},
            ],
            "isError": false
        });
        assert_eq!(extract_text_content(&result), "Parte 1\nParte 2");
    }

    #[test]
    fn extract_text_content_skips_non_text() {
        let result = serde_json::json!({
            "content": [
                {"type": "image", "data": "base64..."},
                {"type": "text", "text": "Solo este"},
            ],
        });
        assert_eq!(extract_text_content(&result), "Solo este");
    }

    #[test]
    fn extract_text_content_empty_falls_back_to_json() {
        let result = serde_json::json!({"content": []});
        // Empty content → JSON serialization of the whole result.
        assert!(!extract_text_content(&result).is_empty());
    }

    #[test]
    fn mcp_registry_from_env_empty() {
        temp_env::with_var("MCPS", None::<&str>, || {
            temp_env::with_var("MCP_COMMAND", None::<&str>, || {
                let registry = McpRegistry::from_env();
                assert!(registry.servers.is_empty());
            });
        });
    }

    #[test]
    fn mcp_registry_from_env_legacy() {
        temp_env::with_var("MCPS", None::<&str>, || {
            temp_env::with_var("MCP_COMMAND", Some("bunx apple-mcp@latest"), || {
                temp_env::with_var("MCP_TOOL_TIMEOUT_SECS", Some("60"), || {
                    let registry = McpRegistry::from_env();
                    assert_eq!(registry.servers.len(), 1);
                    assert_eq!(registry.servers[0].name, "default");
                    assert_eq!(registry.servers[0].command, "bunx apple-mcp@latest");
                    assert_eq!(registry.servers[0].tool_timeout_secs, 60);
                });
            });
        });
    }

    #[test]
    fn mcp_registry_from_env_multi() {
        temp_env::with_var("MCP_COMMAND", None::<&str>, || {
            temp_env::with_var("MCPS", Some("apple,filesystem"), || {
                temp_env::with_var("MCP_APPLE_COMMAND", Some("bunx apple-mcp@latest"), || {
                    temp_env::with_var("MCP_APPLE_TIMEOUT_SECS", Some("120"), || {
                        temp_env::with_var(
                            "MCP_FILESYSTEM_COMMAND",
                            Some("npx @mcp/server-filesystem /tmp"),
                            || {
                                let registry = McpRegistry::from_env();
                                assert_eq!(registry.servers.len(), 2);
                                assert_eq!(registry.servers[0].name, "apple");
                                assert_eq!(registry.servers[0].command, "bunx apple-mcp@latest");
                                assert_eq!(registry.servers[0].tool_timeout_secs, 120);
                                assert_eq!(registry.servers[1].name, "filesystem");
                                assert_eq!(
                                    registry.servers[1].command,
                                    "npx @mcp/server-filesystem /tmp"
                                );
                                assert_eq!(registry.servers[1].tool_timeout_secs, 30);
                            },
                        );
                    });
                });
            });
        });
    }

    #[test]
    fn mcp_registry_from_env_skips_missing_command() {
        temp_env::with_var("MCP_COMMAND", None::<&str>, || {
            temp_env::with_var("MCPS", Some("exists,missing"), || {
                temp_env::with_var("MCP_EXISTS_COMMAND", Some("echo exists"), || {
                    let registry = McpRegistry::from_env();
                    assert_eq!(registry.servers.len(), 1);
                    assert_eq!(registry.servers[0].name, "exists");
                });
            });
        });
    }

    // ── classify_inbound ───────────────────────────────────────────────────

    #[test]
    fn classify_inbound_notification_with_method_no_id() {
        let v = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/document_changed",
            "params": {"doc_id": "abc", "content": "hi"}
        });
        match classify_inbound(&v) {
            Some(Inbound::Notification(notif)) => {
                assert_eq!(notif.method, "notifications/document_changed");
                assert_eq!(notif.params["doc_id"], "abc");
                assert_eq!(notif.params["content"], "hi");
            }
            other => panic!("expected Notification, got {other:?}"),
        }
    }

    #[test]
    fn classify_inbound_notification_missing_params_defaults_null() {
        let v = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/something"
        });
        match classify_inbound(&v) {
            Some(Inbound::Notification(notif)) => {
                assert_eq!(notif.method, "notifications/something");
                assert!(notif.params.is_null());
            }
            other => panic!("expected Notification, got {other:?}"),
        }
    }

    #[test]
    fn classify_inbound_response_with_id_no_method() {
        let v = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 42,
            "result": {"tools": []}
        });
        match classify_inbound(&v) {
            Some(Inbound::Response(id, resp)) => {
                assert_eq!(id, 42);
                assert!(resp.error.is_none());
                assert!(resp.result.is_some());
            }
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[test]
    fn classify_inbound_response_with_error() {
        let v = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "error": {"code": -32601, "message": "Method not found"}
        });
        match classify_inbound(&v) {
            Some(Inbound::Response(id, resp)) => {
                assert_eq!(id, 7);
                assert!(resp.error.is_some());
                assert!(resp.result.is_none());
            }
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[test]
    fn classify_inbound_server_request_with_id_and_method() {
        // A server-initiated request (id + method). Seneschal does not act as
        // an MCP server, so the reader task ignores these — but classify_inbound
        // must still recognize them so they are not mistaken for responses.
        let v = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 99,
            "method": "some/server_request",
            "params": {}
        });
        assert!(matches!(classify_inbound(&v), Some(Inbound::ServerRequest)));
    }

    #[test]
    fn classify_inbound_unrecognizable_returns_none() {
        // Missing both `id` and `method` — cannot classify.
        let v = serde_json::json!({"jsonrpc": "2.0", "params": {}});
        assert!(classify_inbound(&v).is_none());
    }

    #[test]
    fn parse_response_still_ignores_notifications_after_refactor() {
        // Sanity: the legacy parser keeps its contract after introducing
        // classify_inbound. It must return None for any message that has a
        // `method` (notifications, server requests).
        let notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        });
        assert!(parse_response(&notif).is_none());

        let server_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "some/request",
            "params": {}
        });
        // Server-initiated requests are not "responses" either.
        assert!(parse_response(&server_req).is_none());
    }

    // ── ForwardingNotificationHandler ──────────────────────────────────────

    #[test]
    fn forwarding_handler_emits_event_for_arbitrary_notification() {
        let handler = ForwardingNotificationHandler {
            server_name: "editor".to_string(),
        };
        let event = handler.handle(
            "notifications/document_changed",
            serde_json::json!({"doc_id": "x"}),
        );
        match event {
            Some(ProactiveEvent::McpNotification {
                server_name,
                method,
                params,
            }) => {
                assert_eq!(server_name, "editor");
                assert_eq!(method, "notifications/document_changed");
                assert_eq!(params["doc_id"], "x");
            }
            other => panic!("expected McpNotification, got {other:?}"),
        }
    }

    #[test]
    fn forwarding_handler_skips_initialized_handshake() {
        // `notifications/initialized` is part of the MCP handshake and has no
        // semantic value for the pipeline — the handler must drop it.
        let handler = ForwardingNotificationHandler {
            server_name: "any".to_string(),
        };
        let event = handler.handle("notifications/initialized", serde_json::json!({}));
        assert!(event.is_none());
    }

    #[test]
    fn forwarding_handler_skips_unknown_method_with_empty_params() {
        // A contrived notification with no payload should still forward —
        // the pipeline (not the handler) decides what to do with it.
        let handler = ForwardingNotificationHandler {
            server_name: "any".to_string(),
        };
        let event = handler.handle("notifications/something_weird", serde_json::json!({}));
        assert!(matches!(
            event,
            Some(ProactiveEvent::McpNotification { .. })
        ));
    }
}
