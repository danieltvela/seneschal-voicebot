//! ACP subprocess management — JSON-RPC 2.0 wire protocol + persistent writer.
//!
//! Extracted from `src/tools/run_agent.rs` to break the circular dependency
//! between the agents and tools modules during the crate carve-out.

use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow;
use chrono::Local;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::config::{Config, HermesSessionViewerMode};

// ── JSON-RPC 2.0 helpers ─────────────────────────────────────────────────────

pub fn jsonrpc_request(id: u64, method: &str, params: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    })
}

pub fn jsonrpc_notification(method: &str, params: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params
    })
}

/// A parsed JSON-RPC 2.0 message received from the ACP process.
#[derive(Debug, Clone)]
pub enum JsonRpcMessage {
    /// A response to a request we sent (matched by id).
    Response {
        id: u64,
        result: Option<Value>,
        error: Option<Value>,
    },
    /// A request from the server that expects a response (has id + method).
    Request {
        id: u64,
        method: String,
        params: Option<Value>,
    },
    /// A notification from the server (has method but no id).
    Notification {
        method: String,
        params: Option<Value>,
    },
}

pub fn parse_jsonrpc(v: &Value) -> Option<JsonRpcMessage> {
    let method = v.get("method").and_then(|m| m.as_str()).map(String::from);
    let id = v.get("id").and_then(|i| i.as_u64());

    match (method, id) {
        (Some(method), Some(id)) => Some(JsonRpcMessage::Request {
            id,
            method,
            params: v.get("params").cloned(),
        }),
        (Some(method), None) => Some(JsonRpcMessage::Notification {
            method,
            params: v.get("params").cloned(),
        }),
        (None, Some(id)) => Some(JsonRpcMessage::Response {
            id,
            result: v.get("result").cloned(),
            error: v.get("error").cloned(),
        }),
        _ => None,
    }
}

// ── AcpWriter ───────────────────────────────────────────────────────────

/// Write-side of a persistent ACP subprocess using JSON-RPC 2.0 over stdio.
///
/// Reads are served by a background reader task that forwards parsed
/// `JsonRpcMessage` messages on an `mpsc` channel returned from `spawn()`.
#[derive(Debug)]
pub struct AcpWriter {
    pub session_id: Option<String>,
    stdin: ChildStdin,
    #[allow(dead_code)]
    child: std::mem::ManuallyDrop<Child>,
    next_id: u64,
    /// When true, raw JSON-RPC messages are printed to stderr.
    pub verbose: Arc<AtomicBool>,
    /// Cached PID for synchronous SIGKILL in `Drop`.
    child_pid: Option<libc::pid_t>,
    /// Optional log file for ACP traffic (HermesSessionViewerMode::LogFile).
    pub log_file: Option<File>,
}

/// Backward-compat alias. Renamed from `HermesAcpWriter` → `AcpWriter`.
#[deprecated(since = "0.2.0", note = "Use AcpWriter instead")]
pub type HermesAcpWriter = AcpWriter;

impl AcpWriter {
    /// Create a dummy writer backed by a `/bin/cat` process.
    /// Used exclusively in unit tests to avoid requiring a real Hermes binary.
    pub fn dummy() -> Self {
        let child = Command::new("/bin/cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("failed to spawn /bin/cat for AcpWriter::dummy");
        let pid = child.id().unwrap_or(0);
        let mut man = std::mem::ManuallyDrop::new(child);
        Self {
            session_id: None,
            stdin: man.stdin.take().expect("no stdin"),
            child: man,
            next_id: 0,
            verbose: Arc::new(AtomicBool::new(false)),
            child_pid: Some(pid as libc::pid_t),
            log_file: None,
        }
    }

    /// Spawn the ACP process and start the reader task.
    ///
    /// Returns `(writer, inbound_rx)`. The caller owns `inbound_rx`; it should
    /// not be shared (single-consumer design).
    pub async fn spawn(command: &str) -> anyhow::Result<(Self, mpsc::Receiver<JsonRpcMessage>)> {
        let parts: Vec<&str> = command.split_whitespace().collect();
        let program = parts
            .first()
            .copied()
            .ok_or_else(|| anyhow::anyhow!("ACP: AGENT_ACP_COMMAND is empty"))?;
        let args = &parts[1..];

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
            .map_err(|e| anyhow::anyhow!("ACP: failed to spawn '{}': {}", command, e))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("ACP: no stdin handle"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("ACP: no stdout handle"))?;

        let (tx, rx) = mpsc::channel::<JsonRpcMessage>(64);
        let verbose = Arc::new(AtomicBool::new(false));
        let verbose_reader = Arc::clone(&verbose);

        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                if verbose_reader.load(Ordering::Relaxed) {
                    eprintln!("\x1b[2m← {line}\x1b[0m");
                }
                match serde_json::from_str::<Value>(&line) {
                    Ok(v) => {
                        if let Some(msg) = parse_jsonrpc(&v) {
                            if tx.send(msg).await.is_err() {
                                break;
                            }
                        } else {
                            warn!(target: "acp", "Unrecognized JSON-RPC message: {:?}", line);
                        }
                    }
                    Err(e) => {
                        warn!(target: "acp", "Unparseable ACP line: {} — raw: {:?}", e, line);
                    }
                }
            }
            debug!(target: "acp", "ACP reader task ended");
        });

        let pid = child.id().unwrap_or(0);
        Ok((
            Self {
                session_id: None,
                stdin,
                child: std::mem::ManuallyDrop::new(child),
                next_id: 0,
                verbose,
                child_pid: Some(pid as libc::pid_t),
                log_file: None,
            },
            rx,
        ))
    }

    /// Open an ACP traffic log file at `/tmp/seneschal_sessions/{session_id}.log`
    /// and launch a macOS Terminal window tailing it.
    pub fn open_log_file(&mut self, session_id: &str) {
        let dir = std::path::PathBuf::from("/tmp/seneschal_sessions");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            warn!(target: "agent", "Failed to create log dir: {e}");
            return;
        }

        let path = dir.join(format!("{session_id}.log"));
        let path_str = path.to_string_lossy().to_string();

        match File::create(&path) {
            Ok(file) => {
                info!(target: "agent", "ACP log file opened: {}", path_str);
                self.log_file = Some(file);
            }
            Err(e) => {
                warn!(target: "agent", "Failed to open ACP log file: {e}");
                return;
            }
        }

        // Launch Terminal.app with tail -f
        let osacmd = format!(
            r#"tell application "Terminal" to do script "clear && echo 'ACP Session: {session_id}' && tail -f {}""#,
            path_str.replace("\"", "\\\""),
        );

        match std::process::Command::new("osascript")
            .arg("-e")
            .arg(&osacmd)
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => {
                info!(target: "agent", "Opened Terminal tail -f for ACP log: {}", path_str);
            }
            Err(e) => {
                warn!(target: "agent", "Failed to open Terminal for ACP log: {e}");
            }
        }
    }

    /// Log a formatted ACP message line to the log file.
    pub fn log_acp_message(&mut self, direction: &str, msg: &str) {
        if let Some(ref mut file) = self.log_file {
            let ts = Local::now().format("%H:%M:%S%.3f");
            let line = format!("[{ts}] {direction} {msg}\n");
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }

    /// Write a raw JSON value as a newline-delimited line to the process stdin.
    pub async fn write_json(&mut self, msg: &Value) -> anyhow::Result<()> {
        let json = serde_json::to_string(msg)?;
        if self.verbose.load(Ordering::Relaxed) {
            eprintln!("\x1b[2m→ {json}\x1b[0m");
        }
        self.stdin.write_all(json.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        Ok(())
    }

    /// Send a JSON-RPC request and return the assigned request id.
    pub async fn send_request(&mut self, method: &str, params: Value) -> anyhow::Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = jsonrpc_request(id, method, params);
        let json_str = serde_json::to_string(&msg).unwrap_or_default();
        debug!(target: "acp", "→ {}", json_str);
        self.log_acp_message("→ REQUEST", &json_str);
        self.write_json(&msg).await?;
        Ok(id)
    }

    /// Send a JSON-RPC notification (no id, no response expected).
    pub async fn send_notification(&mut self, method: &str, params: Value) -> anyhow::Result<()> {
        let msg = jsonrpc_notification(method, params);
        let json_str = serde_json::to_string(&msg).unwrap_or_default();
        debug!(target: "acp", "→ {}", json_str);
        self.log_acp_message("→ NOTIFICATION", &json_str);
        self.write_json(&msg).await?;
        Ok(())
    }

    /// Send a JSON-RPC response to a request from the server.
    pub async fn send_response(&mut self, id: u64, result: Value) -> anyhow::Result<()> {
        let msg = serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result});
        let json_str = serde_json::to_string(&msg).unwrap_or_default();
        debug!(target: "acp", "→ {}", json_str);
        self.log_acp_message("→ RESPONSE", &json_str);
        self.write_json(&msg).await?;
        Ok(())
    }

    /// Perform the full ACP initialize + session/new handshake.
    /// Blocks until both responses arrive on `rx`.
    /// If `viewer_mode` is `LogFile`, opens an ACP traffic log and launches a Terminal.
    pub async fn initialize(
        &mut self,
        rx: &mut mpsc::Receiver<JsonRpcMessage>,
        cwd: &str,
        viewer_mode: HermesSessionViewerMode,
    ) -> anyhow::Result<String> {
        let init_id = self
            .send_request(
                "initialize",
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "clientInfo": {"name": "seneschal", "version": "0.1.0"}
                }),
            )
            .await?;

        loop {
            match rx.recv().await {
                Some(JsonRpcMessage::Response { id, error, .. }) if id == init_id => {
                    if let Some(err) = error {
                        anyhow::bail!("ACP initialize error: {}", err);
                    }
                    debug!(target: "acp", "initialize response received");
                    break;
                }
                Some(other) => debug!(target: "acp", "init: ignoring {:?}", other),
                None => anyhow::bail!("ACP process closed before initialize response"),
            }
        }

        let session_id = self
            .send_request(
                "session/new",
                serde_json::json!({
                    "cwd": cwd,
                    "mcpServers": []
                }),
            )
            .await?;

        let sid = loop {
            match rx.recv().await {
                Some(JsonRpcMessage::Response {
                    id, result, error, ..
                }) if id == session_id => {
                    if let Some(err) = error {
                        anyhow::bail!("ACP session/new error: {}", err);
                    }
                    let result = result.unwrap_or_default();
                    let sid = result["sessionId"]
                        .as_str()
                        .ok_or_else(|| {
                            anyhow::anyhow!("ACP session/new response missing sessionId")
                        })?
                        .to_string();
                    break sid;
                }
                Some(other) => debug!(target: "acp", "session/new: ignoring {:?}", other),
                None => anyhow::bail!("ACP process closed before session/new response"),
            }
        };

        self.session_id = Some(sid.clone());

        if viewer_mode == HermesSessionViewerMode::LogFile {
            self.open_log_file(&sid);
        }

        info!(target: "acp", "ACP initialized, sessionId={}", sid);
        Ok(sid)
    }

    /// Send a session/prompt request and return the request id.
    pub async fn send_prompt(&mut self, session_id: &str, text: &str) -> anyhow::Result<u64> {
        self.send_request(
            "session/prompt",
            serde_json::json!({
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": text}]
            }),
        )
        .await
    }

    /// Send a session/cancel notification for a running prompt request.
    pub async fn send_cancel(&mut self, request_id: u64) -> anyhow::Result<()> {
        self.send_notification(
            "session/cancel",
            serde_json::json!({
                "requestId": request_id
            }),
        )
        .await
    }

    /// Create a new session (without re-initializing the process).
    #[allow(dead_code)]
    pub async fn send_new_session(&mut self, cwd: &str) -> anyhow::Result<u64> {
        self.send_request(
            "session/new",
            serde_json::json!({
                "cwd": cwd,
                "mcpServers": []
            }),
        )
        .await
    }

    /// Fork an existing session.
    #[allow(dead_code)]
    pub async fn send_fork_session(&mut self, session_id: &str, cwd: &str) -> anyhow::Result<u64> {
        self.send_request(
            "session/fork",
            serde_json::json!({
                "sessionId": session_id,
                "cwd": cwd
            }),
        )
        .await
    }

    /// Load a previous session by ID.
    #[allow(dead_code)]
    pub async fn send_load_session(&mut self, session_id: &str, cwd: &str) -> anyhow::Result<u64> {
        self.send_request(
            "session/load",
            serde_json::json!({
                "sessionId": session_id,
                "cwd": cwd
            }),
        )
        .await
    }

    /// Resume a suspended session.
    #[allow(dead_code)]
    pub async fn send_resume_session(
        &mut self,
        session_id: &str,
        cwd: &str,
    ) -> anyhow::Result<u64> {
        self.send_request(
            "session/resume",
            serde_json::json!({
                "sessionId": session_id,
                "cwd": cwd
            }),
        )
        .await
    }

    /// List active sessions.
    #[allow(dead_code)]
    pub async fn send_list_sessions(&mut self, cwd: &str) -> anyhow::Result<u64> {
        self.send_request(
            "session/list",
            serde_json::json!({
                "cwd": cwd
            }),
        )
        .await
    }

    /// Check whether the underlying child process is still alive.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Send a minimal ready-ping prompt to verify the ACP session responds.
    pub async fn warm_up(&mut self, session_id: &str, timeout_secs: u64) -> anyhow::Result<()> {
        let text = "Hello, you are ready. Acknowledge with 'ready'.";
        let _id = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            self.send_prompt(session_id, text),
        )
        .await
        .map_err(|_| anyhow::anyhow!("ACP warm-up timed out after {timeout_secs}s"))??;
        Ok(())
    }

    /// Kill the subprocess (async).
    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
    }
}

impl Drop for AcpWriter {
    fn drop(&mut self) {
        if let Some(pid) = self.child_pid.take() {
            unsafe {
                let _ = libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}
