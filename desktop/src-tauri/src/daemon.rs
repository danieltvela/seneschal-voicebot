use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tauri::{AppHandle, Emitter};
use tauri_plugin_shell::ShellExt;
use tauri_plugin_shell::process::CommandEvent;
use tokio::sync::{Mutex, watch};

use crate::status::{DaemonStatus, SharedStatusHub};

pub const STATUS_EVENT: &str = "desktop-status";

/// Owns the Seneschal sidecar and coordinates restarts with the Control bridge.
pub struct AppRuntime {
    app: AppHandle,
    pub(crate) status: SharedStatusHub,
    control_port: u16,
    restart_tx: watch::Sender<u64>,
    restart_lock: Mutex<()>,
    child: Mutex<Option<tauri_plugin_shell::process::CommandChild>>,
    generation: Arc<AtomicU64>,
    exit_tx: watch::Sender<u64>,
    workdir: PathBuf,
    config_path: PathBuf,
    onboarding_active: AtomicBool,
}

impl AppRuntime {
    pub fn new(
        app: AppHandle,
        status: SharedStatusHub,
        control_port: u16,
        restart_tx: watch::Sender<u64>,
        workdir: PathBuf,
        config_path: PathBuf,
        onboarding_active: bool,
    ) -> Self {
        let (exit_tx, _) = watch::channel(0u64);
        Self {
            app,
            status,
            control_port,
            restart_tx,
            restart_lock: Mutex::new(()),
            child: Mutex::new(None),
            generation: Arc::new(AtomicU64::new(0)),
            exit_tx,
            workdir,
            config_path,
            onboarding_active: AtomicBool::new(onboarding_active),
        }
    }

    pub fn set_onboarding_active(&self, active: bool) {
        self.onboarding_active.store(active, Ordering::SeqCst);
    }

    pub(crate) fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.control_port)
    }

    pub async fn start(&self) -> Result<(), String> {
        self.next_generation();
        self.spawn_daemon().await
    }

    pub async fn restart(&self) -> Result<(), String> {
        let _guard = self.restart_lock.lock().await;
        self.status.begin_reconnect();
        self.status.restart();
        self.next_generation();
        self.stop_daemon().await?;
        self.spawn_daemon().await
    }

    pub async fn shutdown(&self) {
        let _guard = self.restart_lock.lock().await;
        let _ = self.stop_daemon().await;
    }

    fn next_generation(&self) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.restart_tx.send(generation);
        generation
    }

    fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn sidecar_command(&self) -> Result<tauri_plugin_shell::process::Command, String> {
        let mut command = self
            .app
            .shell()
            .sidecar("seneschal")
            .map_err(|error| format!("failed to resolve Seneschal sidecar: {error}"))?;
        command = command
            .env("CONTROL_PORT", self.control_port.to_string())
            .env(
                "SENECHAL_CONFIG_FILE",
                self.config_path.to_string_lossy().into_owned(),
            );
        if self.onboarding_active.load(Ordering::SeqCst) {
            command = command.env("SENESCHAL_DEFER_STARTUP_GREETING", "1");
        }
        command = command
            .current_dir(&self.workdir)
            .set_process_group(true)
            .kill_on_drop(true);
        Ok(command)
    }

    async fn spawn_daemon(&self) -> Result<(), String> {
        let generation = self.current_generation();
        tracing::debug!(
            target: "desktop_daemon",
            "spawning sidecar generation {generation} on port {}",
            self.control_port
        );
        self.status.set_daemon(DaemonStatus::Starting, None);
        let command = self.sidecar_command()?;
        let (rx, child) = match command.spawn() {
            Ok(spawned) => spawned,
            Err(error) => {
                tracing::error!(target: "desktop_daemon", "failed to spawn sidecar: {error}");
                return Err(format!("failed to spawn Seneschal sidecar: {error}"));
            }
        };
        *self.child.lock().await = Some(child);

        let runtime = Arc::new(self.clone_runtime());
        tauri::async_runtime::spawn(async move {
            supervise_sidecar(runtime, generation, rx).await;
        });
        Ok(())
    }

    /// The sidecar output task shares the generation counter so events from an
    /// old process cannot overwrite the status of a newer one.
    fn clone_runtime(&self) -> AppRuntimeHandle {
        AppRuntimeHandle {
            status: Arc::clone(&self.status),
            generation: Arc::clone(&self.generation),
            exit_tx: self.exit_tx.clone(),
        }
    }

    async fn stop_daemon(&self) -> Result<(), String> {
        let child = self.child.lock().await.take();
        if let Some(child) = child {
            // Subscribe before signaling so a fast graceful exit is not missed.
            let mut exit_rx = self.exit_tx.subscribe();

            #[cfg(unix)]
            {
                let pid = child.pid() as libc::pid_t;
                let result = unsafe { libc::killpg(pid, libc::SIGTERM) };
                if result != 0 {
                    let error = std::io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        tracing::warn!(
                            target: "desktop_daemon",
                            "failed to send SIGTERM to sidecar {pid}: {error}"
                        );
                    }
                }
            }

            let stopped = tokio::time::timeout(GRACEFUL_STOP_TIMEOUT, exit_rx.changed())
                .await
                .is_ok();
            if !stopped {
                tracing::warn!(
                    target: "desktop_daemon",
                    "sidecar did not exit within {:?}; sending SIGKILL",
                    GRACEFUL_STOP_TIMEOUT
                );
            }

            let result = tokio::task::spawn_blocking(move || child.kill())
                .await
                .map_err(|error| format!("failed to join sidecar kill task: {error}"))?;
            result.map_err(|error| format!("failed to stop Seneschal sidecar: {error}"))?;
        }
        Ok(())
    }
}

/// How long to wait after SIGTERM before the existing SIGKILL fallback.
const GRACEFUL_STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Minimal handle for the sidecar output task. It only needs the status bus
/// and an immutable snapshot of the generation it belongs to.
struct AppRuntimeHandle {
    status: SharedStatusHub,
    generation: Arc<AtomicU64>,
    exit_tx: watch::Sender<u64>,
}

async fn supervise_sidecar(
    runtime: Arc<AppRuntimeHandle>,
    generation: u64,
    mut rx: tauri::async_runtime::Receiver<CommandEvent>,
) {
    while let Some(event) = rx.recv().await {
        if runtime.generation.load(Ordering::SeqCst) != generation {
            continue;
        }
        match event {
            CommandEvent::Stdout(bytes) => {
                let line = String::from_utf8_lossy(&bytes);
                tracing::trace!(target: "desktop_sidecar", stdout = %line.trim_end());
            }
            CommandEvent::Stderr(bytes) => {
                let line = String::from_utf8_lossy(&bytes);
                tracing::debug!(target: "desktop_sidecar", stderr = %line.trim_end());
            }
            CommandEvent::Error(error) => {
                runtime
                    .status
                    .daemon_exited(DaemonStatus::Failed, Some(error));
            }
            CommandEvent::Terminated(payload) => {
                let message = match payload.code {
                    Some(code) => format!("daemon exited (code {code})"),
                    None => "daemon exited".to_string(),
                };
                tracing::warn!(target: "desktop_daemon", "{message}");
                runtime
                    .status
                    .daemon_exited(DaemonStatus::Stopped, Some(message));
                let _ = runtime.exit_tx.send(generation);
            }
            _ => {}
        }
    }
}

/// Reserve a fresh loopback port so the desktop shell never collides with a
/// manually started daemon or the iOS companion on the default port.
pub fn select_control_port() -> Result<u16, String> {
    if let Ok(value) = std::env::var("SENESCHAL_DESKTOP_CONTROL_PORT") {
        return value
            .parse()
            .map_err(|error| format!("invalid SENESCHAL_DESKTOP_CONTROL_PORT: {error}"));
    }

    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("failed to reserve a control port: {error}"))?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|error| format!("failed to read the reserved control port: {error}"))
}

/// Resolve the daemon workdir: explicit QA override, the repository in dev, or
/// Tauri's per-user data directory in packaged builds. The daemon writes TOML,
/// SQLite and relative data paths under this directory.
pub fn daemon_workdir(app_data_dir: Option<PathBuf>) -> PathBuf {
    if let Ok(path) = std::env::var("SENESCHAL_DESKTOP_WORKDIR") {
        return PathBuf::from(path);
    }
    if cfg!(debug_assertions) {
        // `tauri dev` runs from `desktop/src-tauri`; the daemon expects repo
        // root for `.env`, embedded TOML fallback and relative data paths.
        return PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../");
    }
    app_data_dir.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// The single TOML path used for first-run detection and every sidecar spawn.
/// An explicit override keeps existing behavior; otherwise the env-specific
/// file lives in the daemon workdir.
pub fn daemon_config_path(workdir: &Path) -> PathBuf {
    if let Ok(path) = std::env::var("SENECHAL_CONFIG_FILE") {
        return PathBuf::from(path);
    }
    let env = match std::env::var("SENECHAL_ENV").as_deref() {
        Ok("dev") | Ok("development") => "dev",
        _ => "pro",
    };
    workdir.join(format!("seneschal.{env}.toml"))
}

/// Forward every status change to the webview.
pub fn spawn_status_emitter(app: AppHandle, status: SharedStatusHub) {
    let mut rx = status.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(snapshot) => {
                    let _ = app.emit(STATUS_EVENT, snapshot);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_port_can_be_pinned_for_manual_qa() {
        // SAFETY: test-only single-threaded mutation.
        unsafe {
            std::env::set_var("SENESCHAL_DESKTOP_CONTROL_PORT", "19123");
        }
        assert_eq!(select_control_port(), Ok(19123));
        unsafe {
            std::env::remove_var("SENESCHAL_DESKTOP_CONTROL_PORT");
        }
    }

    #[test]
    fn workdir_override_wins_and_config_follows_the_environment() {
        // SAFETY: test-only single-threaded mutation.
        unsafe {
            std::env::set_var("SENESCHAL_DESKTOP_WORKDIR", "/tmp/seneschal-desktop-qa");
        }
        let workdir = daemon_workdir(None);
        assert_eq!(workdir, PathBuf::from("/tmp/seneschal-desktop-qa"));
        assert_eq!(
            daemon_config_path(&workdir),
            PathBuf::from("/tmp/seneschal-desktop-qa/seneschal.pro.toml")
        );
        unsafe {
            std::env::remove_var("SENESCHAL_DESKTOP_WORKDIR");
            std::env::set_var("SENECHAL_ENV", "dev");
        }
        assert_eq!(
            daemon_config_path(Path::new("/tmp/other")),
            PathBuf::from("/tmp/other/seneschal.dev.toml")
        );
        unsafe {
            std::env::remove_var("SENECHAL_ENV");
        }
    }
}
