//! Visible agent session — a PTY-based subagent session that the user can watch
//! in a terminal window.
//!
//! Provides [`VisibleSession`] for direct PTY I/O and [`VisibleSessionManager`]
//! for managing multiple concurrent visible sessions.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use tracing::{info, warn};
use uuid::Uuid;

use dashmap::DashMap;

// ── VisibleSession ─────────────────────────────────────────────────────────────

/// A single PTY-based agent session that the user can watch in real time.
///
/// The session spawns a command (e.g. `hermes chat`) inside a pseudo-terminal,
/// duplicates all I/O to a log file, and opens a macOS Terminal window tailing
/// that file. Text can be sent to the agent via [`send`] and received via [`receive`].
pub struct VisibleSession {
    /// Unique session identifier (UUID v4).
    session_id: String,
    /// Agent name (e.g. "hermes").
    agent_name: String,
    /// PTY writer (obtained via `take_writer`).
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    /// Child process handle, if the process is still alive.
    child: Mutex<Option<Box<dyn portable_pty::Child + Send + Sync>>>,
    /// Append-only log file duplicating all PTY I/O.
    log_file: Mutex<Option<File>>,
    /// Path to the log file (shared with the reader thread).
    log_path: String,
    /// Ring buffer of complete output lines received from the agent.
    /// Shared via Arc with the background reader thread.
    output_lines: Arc<tokio::sync::Mutex<VecDeque<String>>>,
    /// When the session was created.
    created_at: Instant,
    /// When the session was last used (via send/receive/close).
    last_used: Mutex<Instant>,
    /// Whether the session has been closed.
    closed: AtomicBool,
    /// Reader thread handle — kept alive for the session lifetime.
    _reader_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl VisibleSession {
    /// Spawn a new visible agent session.
    ///
    /// `command` is the full command line (e.g. `"hermes chat"`).
    /// `agent_name` is used for display in the Terminal window.
    /// `session_dir` is the directory for log files.
    pub fn spawn(command: &str, agent_name: &str, session_dir: &str) -> Result<Arc<Self>> {
        let session_id = Uuid::new_v4().to_string();
        let now = Instant::now();

        // ── Ensure session directory exists ──────────────────────────────────
        fs::create_dir_all(session_dir)
            .with_context(|| format!("Failed to create session directory: {session_dir}"))?;

        // ── Create log file ──────────────────────────────────────────────────
        let log_path = PathBuf::from(session_dir).join(format!("{session_id}.log"));
        let log_path_str = log_path.to_string_lossy().to_string();
        let log_file = OpenOptions::new()
            .create(true)
            .write(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("Failed to create log file at {log_path_str}"))?;

        // ── Parse command into program + args ─────────────────────────────────
        let parts: Vec<&str> = command.split_whitespace().collect();
        let program = parts.first().copied().ok_or_else(|| {
            anyhow::anyhow!("Visible agent command is empty")
        })?;
        let args = &parts[1..];

        // ── Open PTY ─────────────────────────────────────────────────────────
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize::default())
            .context("Failed to open PTY")?;

        // ── Build command ─────────────────────────────────────────────────────
        let mut cmd_builder = CommandBuilder::new(program);
        for arg in args {
            cmd_builder.arg(arg);
        }
        cmd_builder.env("TERM", "xterm-256color");

        // ── Spawn command in PTY slave ────────────────────────────────────────
        let child = pair
            .slave
            .spawn_command(cmd_builder)
            .context("Failed to spawn agent command in PTY")?;

        // ── Obtain writer and reader from master ──────────────────────────────
        let writer: Box<dyn Write + Send> = pair
            .master
            .take_writer()
            .context("Failed to take PTY writer")?;
        let mut reader: Box<dyn Read + Send> = pair
            .master
            .try_clone_reader()
            .context("Failed to clone PTY reader")?;

        let agent_name_owned = agent_name.to_string();
        let log_path_for_reader = log_path_str.clone();

        // Shared state between reader thread and the session
        let output_lines_ref: Arc<tokio::sync::Mutex<VecDeque<String>>> =
            Arc::new(tokio::sync::Mutex::new(VecDeque::new()));

        // ── Spawn background reader thread ────────────────────────────────────
        let reader_session_id = session_id.clone();
        let reader_output = Arc::clone(&output_lines_ref);

        let reader_handle = std::thread::Builder::new()
            .name(format!("pty-reader-{session_id}"))
            .spawn(move || {
                let mut partial: Vec<u8> = Vec::new();
                let mut buf = [0u8; 4096];

                // Reader opens its own log file handle
                let mut f = match OpenOptions::new()
                    .create(true)
                    .write(true)
                    .append(true)
                    .open(&log_path_for_reader)
                {
                    Ok(f) => f,
                    Err(e) => {
                        warn!(
                            target: "agent_session",
                            session_id = %reader_session_id,
                            "Failed to open log for reader thread: {e}"
                        );
                        return;
                    }
                };

                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => {
                            info!(
                                target: "agent_session",
                                session_id = %reader_session_id,
                                "PTY reader EOF — agent process exited"
                            );
                            break;
                        }
                        Ok(n) => {
                            let chunk = &buf[..n];

                            // Write raw bytes to log file
                            if let Err(e) = f.write_all(chunk) {
                                warn!(
                                    target: "agent_session",
                                    session_id = %reader_session_id,
                                    "Failed to write log: {e}"
                                );
                                break;
                            }
                            if let Err(e) = f.flush() {
                                warn!(
                                    target: "agent_session",
                                    "Failed to flush log: {e}"
                                );
                            }

                            // Split chunk into lines, push complete lines
                            for &byte in chunk {
                                if byte == b'\n' {
                                    let line = String::from_utf8_lossy(&partial).to_string();
                                    partial.clear();
                                    let mut guard = reader_output.blocking_lock();
                                    guard.push_back(line);
                                    // Keep buffer bounded (max 1000 lines)
                                    while guard.len() > 1000 {
                                        guard.pop_front();
                                    }
                                } else {
                                    partial.push(byte);
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                target: "agent_session",
                                session_id = %reader_session_id,
                                "PTY reader error: {e}"
                            );
                            break;
                        }
                    }
                }

                // Push any remaining partial line
                if !partial.is_empty() {
                    let line = String::from_utf8_lossy(&partial).to_string();
                    let mut guard = reader_output.blocking_lock();
                    guard.push_back(line);
                }
            })
            .context("Failed to spawn PTY reader thread")?;

        // ── Launch Terminal.app (macOS only) ─────────────────────────────────
        #[cfg(target_os = "macos")]
        {
            let escaped = log_path_str.replace('"', "\\\"");
            let osa = format!(
                r#"tell application "Terminal" to do script "clear && echo 'Visible Agent: {}' && tail -f {}""#,
                agent_name,
                escaped,
            );

            if let Err(e) = std::process::Command::new("osascript")
                .arg("-e")
                .arg(&osa)
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                warn!(target: "agent_session", "Failed to launch Terminal window: {e}");
            } else {
                info!(target: "agent_session", %session_id, "Terminal.app launched for visible agent");
            }
        }

        Ok(Arc::new(Self {
            session_id,
            agent_name: agent_name_owned,
            writer: Mutex::new(Some(writer)),
            child: Mutex::new(Some(child)),
            log_file: Mutex::new(Some(log_file)),
            log_path: log_path_str,
            output_lines: output_lines_ref,
            created_at: now,
            last_used: Mutex::new(now),
            closed: AtomicBool::new(false),
            _reader_handle: Mutex::new(Some(reader_handle)),
        }))
    }

    /// Send text to the agent by writing it to the PTY (followed by `\n`).
    ///
    /// The text is also recorded in the session log file with a `[IN]` prefix.
    pub fn send(&self, text: &str) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            anyhow::bail!("Session is closed");
        }

        let mut data = text.as_bytes().to_vec();
        data.push(b'\n');

        // Write to PTY
        {
            let mut guard = self.writer.lock().unwrap();
            if let Some(w) = guard.as_mut() {
                w.write_all(&data)
                    .context("Failed to write to PTY")?;
                w.flush().context("Failed to flush PTY writer")?;
            }
        }

        // Log the input
        {
            let mut guard = self.log_file.lock().unwrap();
            if let Some(f) = guard.as_mut() {
                let ts = chrono::Local::now().format("%H:%M:%S%.3f");
                let line = format!("[{ts}] [IN] {text}\n");
                let _ = f.write_all(line.as_bytes());
                let _ = f.flush();
            }
        }

        // Update last_used
        if let Ok(mut last) = self.last_used.lock() {
            *last = Instant::now();
        }

        Ok(())
    }

    /// Receive any new output lines from the agent since the last call.
    ///
    /// Returns `None` if there are no new lines. Lines are drained from the
    /// internal buffer so each line is returned only once.
    pub fn receive(&self) -> Option<String> {
        if self.closed.load(Ordering::SeqCst) {
            return None;
        }

        let mut guard = self.output_lines.blocking_lock();
        if guard.is_empty() {
            return None;
        }

        let mut lines = Vec::new();
        while let Some(line) = guard.pop_front() {
            lines.push(line);
        }

        // Update last_used
        if let Ok(mut last) = self.last_used.lock() {
            *last = Instant::now();
        }

        Some(lines.join("\n"))
    }

    /// Close the session: kill the child process and clean up resources.
    pub fn close(&self) {
        if self
            .closed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return; // already closed
        }

        let session_id = self.session_id.clone();
        let agent = self.agent_name.clone();

        // Kill child process
        {
            let mut guard = self.child.lock().unwrap();
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                info!(
                    target: "agent_session",
                    %session_id,
                    agent = %agent,
                    "Killed visible agent process"
                );
            }
        }

        // Drop writer (sends EOF)
        {
            let mut guard = self.writer.lock().unwrap();
            guard.take();
        }

        // Flush and close log file
        {
            let mut guard = self.log_file.lock().unwrap();
            if let Some(mut f) = guard.take() {
                let _ = f.flush();
                drop(f);
                info!(
                    target: "agent_session",
                    %session_id,
                    "Closed log file"
                );
            }
        }

        // Update last_used
        if let Ok(mut last) = self.last_used.lock() {
            *last = Instant::now();
        }

        #[cfg(target_os = "macos")]
        {
            let osa = format!(
                r#"tell application "Terminal" to close (every window whose name contains "{}")"#,
                session_id,
            );
            let _ = std::process::Command::new("osascript")
                .arg("-e")
                .arg(&osa)
                .stderr(std::process::Stdio::null())
                .spawn();
        }

        info!(
            target: "agent_session",
            %session_id,
            agent = %agent,
            "Visible agent session closed"
        );
    }

    /// Check whether the child process is still alive.
    pub fn is_alive(&self) -> bool {
        if self.closed.load(Ordering::SeqCst) {
            return false;
        }
        let mut guard = self.child.lock().unwrap();
        if let Some(child) = guard.as_mut() {
            matches!(child.try_wait(), Ok(None))
        } else {
            false
        }
    }

    // ── Accessors ────────────────────────────────────────────────────────────

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn agent_name(&self) -> &str {
        &self.agent_name
    }

    pub fn log_path(&self) -> &str {
        &self.log_path
    }

    pub fn created_at(&self) -> Instant {
        self.created_at
    }

    pub fn last_used(&self) -> Instant {
        self.last_used.lock().map(|g| *g).unwrap_or(self.created_at)
    }
}

unsafe impl Send for VisibleSession {}
unsafe impl Sync for VisibleSession {}

impl std::fmt::Debug for VisibleSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VisibleSession")
            .field("session_id", &self.session_id)
            .field("agent_name", &self.agent_name)
            .field("log_path", &self.log_path)
            .field("created_at", &self.created_at)
            .field("closed", &self.closed.load(Ordering::SeqCst))
            .finish()
    }
}

// ── VisibleSessionManager ──────────────────────────────────────────────────────

/// Display-friendly session summary.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub agent_name: String,
    pub created_at: Instant,
    pub last_used: Instant,
}

/// Manages multiple concurrent visible agent sessions keyed by agent name.
///
/// Each agent can have at most one visible session at a time. Sessions are
/// automatically reused when the agent name matches an existing alive session.
#[derive(Debug, Default)]
pub struct VisibleSessionManager {
    sessions: DashMap<String, Arc<VisibleSession>>,
}

impl VisibleSessionManager {
    /// Create a new, empty manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get or create a visible session for the given `agent_name`.
    ///
    /// If a session already exists for this agent and is alive, it is returned
    /// (reused). Otherwise a new session is spawned.
    pub fn get_or_create(
        &self,
        agent_name: &str,
        command: &str,
        session_dir: &str,
    ) -> Result<Arc<VisibleSession>> {
        // Try to reuse an existing alive session
        if let Some(entry) = self.sessions.get(agent_name) {
            if entry.is_alive() {
                return Ok(Arc::clone(&entry));
            }
            // Session is dead — remove it and spawn a new one
            // Drop the entry before removing to avoid deadlock
            drop(entry);
            self.sessions.remove(agent_name);
        }

        let session = VisibleSession::spawn(command, agent_name, session_dir)?;
        self.sessions.insert(agent_name.to_string(), Arc::clone(&session));
        Ok(session)
    }

    /// Send text to a specific agent's session.
    pub fn send_to(&self, agent_name: &str, text: &str) -> Result<()> {
        let entry = self
            .sessions
            .get(agent_name)
            .ok_or_else(|| anyhow::anyhow!("No visible session for agent '{agent_name}'"))?;
        entry.send(text)
    }

    /// Receive new output from a specific agent's session.
    pub fn receive_from(&self, agent_name: &str) -> Option<String> {
        let entry = self.sessions.get(agent_name)?;
        entry.receive()
    }

    /// Close a specific agent's session and remove it from the manager.
    pub fn close_session(&self, agent_name: &str) {
        if let Some((_, session)) = self.sessions.remove(agent_name) {
            session.close();
        }
    }

    /// List all active sessions.
    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .iter()
            .map(|e| SessionInfo {
                session_id: e.session_id().to_string(),
                agent_name: e.agent_name().to_string(),
                created_at: e.created_at(),
                last_used: e.last_used(),
            })
            .collect()
    }

    /// Close and remove sessions that have been idle longer than `timeout`.
    /// Returns the number of sessions removed.
    pub fn cleanup_idle(&self, timeout: Duration) -> usize {
        let cutoff = Instant::now()
            .checked_sub(timeout)
            .unwrap_or(Instant::now());

        let mut to_remove = Vec::new();

        for entry in self.sessions.iter() {
            if entry.last_used() < cutoff {
                to_remove.push(entry.agent_name().to_string());
            }
        }

        let count = to_remove.len();
        for name in to_remove {
            self.close_session(&name);
        }
        count
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Helper: create a temp dir for session logs
    fn temp_session_dir() -> String {
        let dir = std::env::temp_dir()
            .join(format!("seneschal-test-{}", Uuid::new_v4()));
        dir.to_string_lossy().to_string()
    }

    #[test]
    fn test_spawn_cat_and_send_receive() {
        let dir = temp_session_dir();
        let session = VisibleSession::spawn("/bin/cat", "test-cat", &dir)
            .expect("Failed to spawn cat");

        // Wait a moment for the process to start
        std::thread::sleep(Duration::from_millis(200));

        session.send("hello world").unwrap();

        // Allow some time for the PTY to echo
        std::thread::sleep(Duration::from_millis(300));

        let output = session.receive();
        assert!(output.is_some(), "Expected output, got None");
        let text = output.unwrap();
        assert!(
            text.contains("hello world"),
            "Output should contain 'hello world', got: {text:?}"
        );

        session.close();
    }

    #[test]
    fn test_spawn_cat_multiline() {
        let dir = temp_session_dir();
        let session = VisibleSession::spawn("/bin/cat", "test-cat-2", &dir)
            .expect("Failed to spawn cat");

        std::thread::sleep(Duration::from_millis(200));

        session.send("line1").unwrap();
        session.send("line2").unwrap();

        std::thread::sleep(Duration::from_millis(300));

        let output = session.receive();
        assert!(output.is_some(), "Expected output, got None");
        let text = output.unwrap();
        assert!(
            text.contains("line1") && text.contains("line2"),
            "Output should contain both lines, got: {text:?}"
        );

        session.close();
    }

    #[test]
    fn test_close_terminates_process() {
        let dir = temp_session_dir();
        let session = VisibleSession::spawn("sleep 30", "test-sleep", &dir)
            .expect("Failed to spawn sleep");

        assert!(session.is_alive(), "Process should be alive");

        session.close();

        std::thread::sleep(Duration::from_millis(100));
        assert!(!session.is_alive(), "Process should not be alive after close");
    }

    #[test]
    fn test_send_empty_string() {
        let dir = temp_session_dir();
        let session = VisibleSession::spawn("/bin/cat", "test-empty", &dir)
            .expect("Failed to spawn cat");

        std::thread::sleep(Duration::from_millis(200));

        // Sending empty string should write a newline
        session.send("").unwrap();

        std::thread::sleep(Duration::from_millis(200));

        // empty send just sends "\n" — cat should echo a blank line
        let output = session.receive();
        // This might or might not return something depending on timing
        // At minimum, no error should occur
        drop(output);

        session.close();
    }

    #[test]
    fn test_log_file_created() {
        let dir = temp_session_dir();
        let session = VisibleSession::spawn("/bin/cat", "test-log", &dir)
            .expect("Failed to spawn cat");

        let log_path = session.log_path();
        assert!(
            std::path::Path::new(log_path).exists(),
            "Log file should exist at {log_path}"
        );

        session.close();
    }

    #[test]
    fn test_double_close_is_safe() {
        let dir = temp_session_dir();
        let session = VisibleSession::spawn("/bin/cat", "test-double-close", &dir)
            .expect("Failed to spawn cat");

        session.close();
        session.close(); // should not panic
    }

    #[test]
    fn test_session_id_assigned() {
        let dir = temp_session_dir();
        let session = VisibleSession::spawn("/bin/cat", "test-id", &dir)
            .expect("Failed to spawn cat");

        assert!(!session.session_id().is_empty());
        assert_eq!(session.agent_name(), "test-id");

        session.close();
    }

    // ── VisibleSessionManager tests ─────────────────────────────────────────

    #[test]
    fn test_manager_get_or_create_reuses_session() {
        let mgr = VisibleSessionManager::new();
        let dir = temp_session_dir();

        let s1 = mgr
            .get_or_create("hermes", "/bin/cat", &dir)
            .expect("First create");
        let s2 = mgr
            .get_or_create("hermes", "/bin/cat", &dir)
            .expect("Second get");

        // Both Arcs should point to the same session
        assert_eq!(s1.session_id(), s2.session_id());

        s1.close();
    }

    #[test]
    fn test_manager_get_or_create_different_agents() {
        let mgr = VisibleSessionManager::new();
        let dir = temp_session_dir();

        let s1 = mgr
            .get_or_create("hermes", "/bin/cat", &dir)
            .expect("hermes");
        let s2 = mgr
            .get_or_create("opencode", "/bin/cat", &dir)
            .expect("opencode");

        assert_ne!(s1.session_id(), s2.session_id());

        s1.close();
        s2.close();
    }

    #[test]
    fn test_manager_close_session_removes_from_map() {
        let mgr = VisibleSessionManager::new();
        let dir = temp_session_dir();

        let s = mgr
            .get_or_create("test-agent", "/bin/cat", &dir)
            .expect("create");
        let _sid = s.session_id().to_string();
        drop(s);

        assert_eq!(mgr.list_sessions().len(), 1);

        mgr.close_session("test-agent");
        assert_eq!(mgr.list_sessions().len(), 0);
    }

    #[test]
    fn test_manager_send_to_nonexistent() {
        let mgr = VisibleSessionManager::new();
        let result = mgr.send_to("unknown", "hello");
        assert!(result.is_err(), "Sending to unknown agent should error");
    }

    #[test]
    fn test_manager_cleanup_idle_safe() {
        let mgr = VisibleSessionManager::new();
        let dir = temp_session_dir();

        let _s = mgr
            .get_or_create("no-cleanup", "/bin/cat", &dir)
            .expect("create");

        // This session was just created, so it shouldn't be cleaned up
        // (sessions are idle for <1ms, so a 1-hour timeout keeps them)
        let _count = mgr.cleanup_idle(Duration::from_secs(3600));
        assert_eq!(mgr.list_sessions().len(), 1, "Fresh session should not be cleaned up");

        mgr.close_session("no-cleanup");
    }
}
