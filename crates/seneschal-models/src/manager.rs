//! Catalog status + download orchestration for the model manager (issue #35).
//!
//! `ModelManager` owns the models root: it reports per-file presence (managed
//! root or legacy locations), runs resumable checksum-verified downloads in
//! background tasks, and exposes cancellation + deletion. The Control layer
//! (`seneschal-control::control::models`) wraps it with settings-backed
//! activation, SSE broadcasting and the REST endpoints.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::catalog::{ModelBackend, ModelPurpose, ModelSpec};
use crate::download::{self, DownloadError};
use crate::paths;

/// Minimum gap between throttled progress events.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadPhase {
    Downloading,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}

/// SSE payload for download lifecycle events. `total_bytes` is `None` for
/// indeterminate downloads (NeMo weight scripts).
#[derive(Debug, Clone, Serialize)]
pub struct ModelEvent {
    pub model_id: String,
    pub download_id: String,
    pub phase: DownloadPhase,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelStateWire {
    Missing,
    Partial,
    Installed,
    Downloading,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelFileWire {
    pub path: String,
    pub url: String,
    pub size_bytes: u64,
    pub sha256: Option<String>,
    pub present: bool,
    /// `true` after a checksum-verified download; `null` for legacy files
    /// that have not been re-verified.
    pub verified: Option<bool>,
    /// Found outside the managed root (daemon workdir `models/` or
    /// `~/.seneschal/models`); never touched by download, only by delete.
    pub legacy: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadSnapshotWire {
    pub download_id: String,
    pub phase: DownloadPhase,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelWire {
    pub id: String,
    pub label: String,
    pub purpose: ModelPurpose,
    pub backend: ModelBackend,
    pub state: ModelStateWire,
    pub active: bool,
    pub total_bytes: u64,
    pub requires: Vec<String>,
    pub files: Vec<ModelFileWire>,
    pub download: Option<DownloadSnapshotWire>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelCatalogWire {
    pub root: String,
    pub models: Vec<ModelWire>,
}

#[derive(Debug, thiserror::Error)]
pub enum ManagerError {
    #[error("unknown model '{0}'")]
    UnknownModel(String),
    #[error("a download is already running for '{0}'")]
    DownloadInProgress(String),
    #[error("no download is running for '{0}'")]
    DownloadNotRunning(String),
    #[error("model '{0}' is in use by the active configuration")]
    InUse(String),
    #[error("download failed: {0}")]
    Download(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

struct ProgressState {
    phase: DownloadPhase,
    downloaded: u64,
    total: u64,
    current_file: Option<String>,
    message: Option<String>,
}

impl ProgressState {
    fn snapshot(&self, download_id: &str) -> DownloadSnapshotWire {
        let percent = if self.total > 0 {
            (self.downloaded as f64 / self.total as f64) * 100.0
        } else {
            0.0
        };
        DownloadSnapshotWire {
            download_id: download_id.to_string(),
            phase: self.phase,
            downloaded_bytes: self.downloaded,
            total_bytes: self.total,
            percent: (percent * 10.0).round() / 10.0,
            current_file: self.current_file.clone(),
            message: self.message.clone(),
        }
    }
}

struct DownloadEntry {
    download_id: String,
    cancel: CancellationToken,
    running: Arc<AtomicBool>,
    progress: Arc<Mutex<ProgressState>>,
}

pub struct ModelManager {
    root: PathBuf,
    workdir: PathBuf,
    catalog: Vec<ModelSpec>,
    downloads: Mutex<HashMap<String, DownloadEntry>>,
    /// Final paths whose bytes were verified this session (download path).
    verified: Arc<Mutex<HashSet<PathBuf>>>,
}

impl ModelManager {
    pub fn new(workdir: &Path) -> Self {
        Self::with_catalog(paths::models_root(), workdir, crate::catalog::catalog())
    }

    pub fn with_catalog(root: PathBuf, workdir: &Path, catalog: Vec<ModelSpec>) -> Self {
        Self {
            root,
            workdir: workdir.to_path_buf(),
            catalog,
            downloads: Mutex::new(HashMap::new()),
            verified: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Catalog slice (test-injected catalogs included).
    pub fn catalog_slice(&self) -> &[ModelSpec] {
        &self.catalog
    }

    pub fn spec(&self, id: &str) -> Option<&ModelSpec> {
        crate::catalog::spec(&self.catalog, id)
    }

    /// Transitive dependencies of the active set (the active models plus
    /// everything they require, e.g. Silero VAD).
    pub fn required_closure(&self, active: &HashSet<String>) -> HashSet<String> {
        let mut required: HashSet<String> = HashSet::new();
        let mut queue: Vec<&str> = active.iter().map(String::as_str).collect();
        while let Some(id) = queue.pop() {
            if !required.insert(id.to_string()) {
                continue;
            }
            if let Some(spec) = self.spec(id) {
                for dep in &spec.requires {
                    queue.push(dep.as_str());
                }
            }
        }
        required
    }

    fn candidate_paths(&self, rel_path: &str) -> Vec<PathBuf> {
        let mut candidates = vec![self.root.join(rel_path)];
        for root in paths::legacy_roots(&self.workdir) {
            candidates.push(root.join(rel_path));
        }
        candidates
    }

    fn file_states(&self, spec: &ModelSpec) -> Vec<ModelFileWire> {
        let verified = self.verified.lock().unwrap();
        spec.files
            .iter()
            .map(|file| {
                let mut present_path: Option<(PathBuf, bool)> = None;
                let managed = self.root.join(&file.rel_path);
                if managed.is_file() {
                    present_path = Some((managed, false));
                } else {
                    for root in paths::legacy_roots(&self.workdir) {
                        let candidate = root.join(&file.rel_path);
                        if candidate.is_file() {
                            present_path = Some((candidate, true));
                            break;
                        }
                    }
                }
                match present_path {
                    Some((path, legacy)) => ModelFileWire {
                        path: file.rel_path.clone(),
                        url: file.url.clone(),
                        size_bytes: file.size_bytes,
                        sha256: file.sha256.clone(),
                        present: true,
                        verified: if legacy {
                            None
                        } else {
                            Some(verified.contains(&path))
                        },
                        legacy,
                    },
                    None => ModelFileWire {
                        path: file.rel_path.clone(),
                        url: file.url.clone(),
                        size_bytes: file.size_bytes,
                        sha256: file.sha256.clone(),
                        present: false,
                        verified: Some(false),
                        legacy: false,
                    },
                }
            })
            .collect()
    }

    fn is_running(&self, id: &str) -> bool {
        self.downloads
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|entry| entry.running.load(Ordering::SeqCst))
    }

    pub fn download_snapshot(&self, id: &str) -> Option<DownloadSnapshotWire> {
        let downloads = self.downloads.lock().unwrap();
        downloads.get(id).map(|entry| {
            let progress = entry.progress.lock().unwrap();
            let mut snapshot = progress.snapshot(&entry.download_id);
            if !entry.running.load(Ordering::SeqCst)
                && matches!(
                    snapshot.phase,
                    DownloadPhase::Downloading | DownloadPhase::Verifying
                )
            {
                // The task died with the daemon; the partial file stays on
                // disk, so the download is resumable rather than in-flight.
                snapshot.phase = DownloadPhase::Failed;
                snapshot
                    .message
                    .get_or_insert_with(|| "interrupted".to_string());
            }
            snapshot
        })
    }

    pub fn catalog_wire(&self, active: &HashSet<String>) -> ModelCatalogWire {
        let models = self
            .catalog
            .iter()
            .map(|spec| {
                let files = self.file_states(spec);
                let present = files.iter().filter(|file| file.present).count();
                let downloading = self.is_running(&spec.id);
                let state = if downloading {
                    ModelStateWire::Downloading
                } else if present == files.len() {
                    ModelStateWire::Installed
                } else if present > 0 {
                    ModelStateWire::Partial
                } else {
                    ModelStateWire::Missing
                };
                ModelWire {
                    id: spec.id.clone(),
                    label: spec.label.clone(),
                    purpose: spec.purpose,
                    backend: spec.backend,
                    state,
                    active: active.contains(&spec.id),
                    total_bytes: spec.total_size(),
                    requires: spec.requires.clone(),
                    download: self.download_snapshot(&spec.id),
                    files,
                }
            })
            .collect();
        ModelCatalogWire {
            root: self.root.display().to_string(),
            models,
        }
    }

    /// Start (or resume) the download for `id`. Returns the initial snapshot.
    pub fn start_download<F>(
        &self,
        id: &str,
        on_event: F,
    ) -> Result<DownloadSnapshotWire, ManagerError>
    where
        F: Fn(ModelEvent) + Send + Sync + 'static,
    {
        let spec = self
            .spec(id)
            .ok_or_else(|| ManagerError::UnknownModel(id.to_string()))?
            .clone();

        let mut downloads = self.downloads.lock().unwrap();
        if let Some(entry) = downloads.get(id)
            && entry.running.load(Ordering::SeqCst)
        {
            return Err(ManagerError::DownloadInProgress(id.to_string()));
        }

        let download_id = Uuid::new_v4().to_string();
        let entry = DownloadEntry {
            download_id: download_id.clone(),
            cancel: CancellationToken::new(),
            running: Arc::new(AtomicBool::new(true)),
            progress: Arc::new(Mutex::new(ProgressState {
                phase: DownloadPhase::Downloading,
                downloaded: 0,
                total: spec.total_size(),
                current_file: None,
                message: None,
            })),
        };
        let snapshot = entry.progress.lock().unwrap().snapshot(&entry.download_id);
        let cancel = entry.cancel.clone();
        let running = Arc::clone(&entry.running);
        let progress = Arc::clone(&entry.progress);
        let verified = Arc::clone(&self.verified);
        downloads.insert(id.to_string(), entry);
        drop(downloads);

        let total = spec.total_size();
        let files = spec.files.clone();
        let root = self.root.clone();
        let manager_id = id.to_string();
        let on_event = Arc::new(on_event);

        tokio::spawn(async move {
            let client = download::download_client();
            let last_emit = Arc::new(Mutex::new(Instant::now() - PROGRESS_INTERVAL));
            let mut counted: u64 = 0;
            let mut outcome: Result<(), DownloadError> = Ok(());

            let emit = |phase: DownloadPhase,
                        downloaded: u64,
                        current_file: Option<String>,
                        message: Option<String>| {
                on_event(ModelEvent {
                    model_id: manager_id.clone(),
                    download_id: download_id.clone(),
                    phase,
                    downloaded_bytes: downloaded,
                    total_bytes: Some(total),
                    current_file,
                    message,
                });
            };

            for file in &files {
                if cancel.is_cancelled() {
                    outcome = Err(DownloadError::Cancelled);
                    break;
                }
                let file_offset = counted;
                let on_chunk = {
                    let on_event = Arc::clone(&on_event);
                    let progress_for_chunk = Arc::clone(&progress);
                    let last_emit_for_chunk = Arc::clone(&last_emit);
                    let model_id = manager_id.clone();
                    let download_id = download_id.clone();
                    move |rel: &str, downloaded_in_file: u64, _total_in_file: u64| {
                        let overall = file_offset + downloaded_in_file;
                        {
                            let mut state = progress_for_chunk.lock().unwrap();
                            state.downloaded = overall;
                            state.current_file = Some(rel.to_string());
                        }
                        let should_emit = {
                            let mut last = last_emit_for_chunk.lock().unwrap();
                            if last.elapsed() >= PROGRESS_INTERVAL {
                                *last = Instant::now();
                                true
                            } else {
                                false
                            }
                        };
                        if should_emit {
                            let state = progress_for_chunk.lock().unwrap();
                            on_event(ModelEvent {
                                model_id: model_id.clone(),
                                download_id: download_id.clone(),
                                phase: state.phase,
                                downloaded_bytes: overall,
                                total_bytes: Some(total),
                                current_file: Some(rel.to_string()),
                                message: None,
                            });
                        }
                    }
                };

                match download::download_file(&client, &root, file, &cancel, &on_chunk).await {
                    Ok(()) => {
                        if let Some(expected) = &file.sha256 {
                            let final_path = root.join(&file.rel_path);
                            if download::verify_file(&final_path, expected)
                                .await
                                .unwrap_or(false)
                            {
                                verified.lock().unwrap().insert(final_path);
                            }
                        }
                        counted += file.size_bytes;
                        {
                            let mut state = progress.lock().unwrap();
                            state.downloaded = counted;
                            state.current_file = Some(file.rel_path.clone());
                        }
                    }
                    Err(error) => {
                        outcome = Err(error);
                        break;
                    }
                }
            }

            let (phase, message) = match &outcome {
                Ok(()) => (DownloadPhase::Completed, None),
                Err(DownloadError::Cancelled) => (
                    DownloadPhase::Cancelled,
                    Some("Download cancelled".to_string()),
                ),
                Err(error) => (DownloadPhase::Failed, Some(error.to_string())),
            };
            {
                let mut state = progress.lock().unwrap();
                state.phase = phase;
                state.message = message.clone();
                if phase == DownloadPhase::Completed {
                    state.downloaded = total;
                    state.current_file = None;
                }
            }
            running.store(false, Ordering::SeqCst);
            emit(phase, counted, None, message);
        });

        Ok(snapshot)
    }

    pub fn cancel_download(&self, id: &str) -> Result<(), ManagerError> {
        let downloads = self.downloads.lock().unwrap();
        match downloads.get(id) {
            Some(entry) if entry.running.load(Ordering::SeqCst) => {
                entry.cancel.cancel();
                Ok(())
            }
            Some(_) => Err(ManagerError::DownloadNotRunning(id.to_string())),
            None => Err(ManagerError::UnknownModel(id.to_string())),
        }
    }

    /// Delete the managed files of a model (final + `.part`) wherever they
    /// were found (root or legacy locations).
    pub fn delete_model(&self, id: &str, in_use: bool) -> Result<(), ManagerError> {
        let spec = self
            .spec(id)
            .ok_or_else(|| ManagerError::UnknownModel(id.to_string()))?
            .clone();
        if self.is_running(id) {
            return Err(ManagerError::DownloadInProgress(id.to_string()));
        }
        if in_use {
            return Err(ManagerError::InUse(id.to_string()));
        }
        for file in &spec.files {
            for candidate in self.candidate_paths(&file.rel_path) {
                let part = download::part_file(&candidate);
                if let Err(error) = std::fs::remove_file(&part)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(error.into());
                }
                if let Err(error) = std::fs::remove_file(&candidate)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(error.into());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::ModelFile;
    use sha2::{Digest, Sha256};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const PAYLOAD: &[u8] = b"manager payload bytes";

    fn tiny_spec(id: &str, rel_path: &str, url: &str) -> ModelSpec {
        ModelSpec {
            id: id.to_string(),
            label: "Test model".to_string(),
            purpose: ModelPurpose::Stt,
            backend: ModelBackend::Whisper,
            files: vec![ModelFile {
                rel_path: rel_path.to_string(),
                url: url.to_string(),
                size_bytes: PAYLOAD.len() as u64,
                sha256: Some(hex::encode(Sha256::digest(PAYLOAD))),
            }],
            requires: Vec::new(),
        }
    }

    fn events() -> (
        Arc<Mutex<Vec<ModelEvent>>>,
        impl Fn(ModelEvent) + Send + Sync + 'static,
    ) {
        let events: Arc<Mutex<Vec<ModelEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        (events, move |event| sink.lock().unwrap().push(event))
    }

    async fn wait_for_phase(
        manager: &ModelManager,
        id: &str,
        phase: DownloadPhase,
    ) -> DownloadSnapshotWire {
        for _ in 0..200 {
            if let Some(snapshot) = manager.download_snapshot(id)
                && snapshot.phase == phase
            {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("download never reached {phase:?}");
    }

    #[tokio::test]
    async fn download_completes_and_marks_installed() {
        let dir = tempfile::TempDir::new().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/model.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(PAYLOAD))
            .mount(&server)
            .await;
        let url = format!("{}/model.bin", server.uri());
        let manager = ModelManager::with_catalog(
            dir.path().to_path_buf(),
            dir.path(),
            vec![tiny_spec("test-model", "test-model.bin", &url)],
        );

        let (events, on_event) = events();
        manager.start_download("test-model", on_event).unwrap();
        let snapshot = wait_for_phase(&manager, "test-model", DownloadPhase::Completed).await;
        assert_eq!(snapshot.downloaded_bytes, PAYLOAD.len() as u64);
        assert!(snapshot.percent >= 99.9);

        assert!(dir.path().join("test-model.bin").is_file());
        let wire = manager.catalog_wire(&HashSet::new());
        let model = &wire.models[0];
        assert_eq!(model.state, ModelStateWire::Installed);
        assert_eq!(model.files[0].verified, Some(true));

        let events = events.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.phase == DownloadPhase::Completed)
        );
    }

    #[tokio::test]
    async fn failed_checksum_reports_failed_phase_and_no_installed_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/model.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"wrong bytes"))
            .mount(&server)
            .await;
        let url = format!("{}/model.bin", server.uri());
        let manager = ModelManager::with_catalog(
            dir.path().to_path_buf(),
            dir.path(),
            vec![tiny_spec("test-model", "test-model.bin", &url)],
        );

        let (_events, on_event) = events();
        manager.start_download("test-model", on_event).unwrap();
        let snapshot = wait_for_phase(&manager, "test-model", DownloadPhase::Failed).await;
        assert!(snapshot.message.as_deref().unwrap().contains("checksum"));

        assert!(!dir.path().join("test-model.bin").is_file());
        assert_eq!(
            manager.catalog_wire(&HashSet::new()).models[0].state,
            ModelStateWire::Missing
        );
    }

    #[tokio::test]
    async fn second_download_while_running_is_rejected() {
        let dir = tempfile::TempDir::new().unwrap();
        let server = MockServer::start().await;
        // Delayed response keeps the download observably "running".
        Mock::given(method("GET"))
            .and(path("/model.bin"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(PAYLOAD)
                    .set_delay(Duration::from_millis(300)),
            )
            .mount(&server)
            .await;
        let url = format!("{}/model.bin", server.uri());
        let manager = ModelManager::with_catalog(
            dir.path().to_path_buf(),
            dir.path(),
            vec![tiny_spec("test-model", "test-model.bin", &url)],
        );

        let (_events, on_event) = events();
        manager.start_download("test-model", on_event).unwrap();
        assert!(matches!(
            manager.start_download("test-model", |_| {}),
            Err(ManagerError::DownloadInProgress(_))
        ));
        manager.cancel_download("test-model").unwrap();
        wait_for_phase(&manager, "test-model", DownloadPhase::Cancelled).await;
    }

    #[tokio::test]
    async fn delete_removes_files_and_refuses_in_use() {
        let dir = tempfile::TempDir::new().unwrap();
        let url = "https://example.invalid/model.bin";
        let manager = ModelManager::with_catalog(
            dir.path().to_path_buf(),
            dir.path(),
            vec![tiny_spec("test-model", "test-model.bin", url)],
        );
        std::fs::write(dir.path().join("test-model.bin"), PAYLOAD).unwrap();

        assert!(matches!(
            manager.delete_model("test-model", true),
            Err(ManagerError::InUse(_))
        ));
        manager.delete_model("test-model", false).unwrap();
        assert!(!dir.path().join("test-model.bin").exists());
        // Deleting again is idempotent while the catalog entry exists.
        manager.delete_model("test-model", false).unwrap();
        assert!(matches!(
            manager.delete_model("unknown-model", false),
            Err(ManagerError::UnknownModel(_))
        ));
    }

    #[tokio::test]
    async fn required_closure_includes_dependencies() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut catalog: Vec<ModelSpec> = crate::catalog::catalog();
        // Trim the catalog to vad + whisper for a deterministic closure test.
        catalog.retain(|spec| spec.id == "silero-vad-5.1.2" || spec.id == "whisper-large-v3-turbo");
        let manager = ModelManager::with_catalog(dir.path().to_path_buf(), dir.path(), catalog);

        let mut active = HashSet::new();
        active.insert("whisper-large-v3-turbo".to_string());
        let closure = manager.required_closure(&active);
        assert!(closure.contains("whisper-large-v3-turbo"));
        assert!(closure.contains("silero-vad-5.1.2"));
    }

    #[tokio::test]
    async fn partial_and_legacy_files_are_reported() {
        let dir = tempfile::TempDir::new().unwrap();
        let legacy = dir.path().join("legacy");
        std::fs::create_dir_all(legacy.join("models")).unwrap();
        let url = "https://example.invalid/model.bin";
        let manager = ModelManager::with_catalog(
            dir.path().join("root"),
            &legacy,
            vec![
                tiny_spec("test-model", "sub/model.bin", url),
                tiny_spec("second", "second.bin", url),
            ],
        );
        std::fs::create_dir_all(manager.root().join("sub")).unwrap();
        std::fs::write(manager.root().join("sub/model.bin"), PAYLOAD).unwrap();
        std::fs::write(legacy.join("models/second.bin"), PAYLOAD).unwrap();

        let wire = manager.catalog_wire(&HashSet::new());
        let first = wire.models.iter().find(|m| m.id == "test-model").unwrap();
        assert_eq!(first.state, ModelStateWire::Installed);
        assert_eq!(first.files[0].verified, Some(false));
        assert!(!first.files[0].legacy);

        let second = wire.models.iter().find(|m| m.id == "second").unwrap();
        assert!(second.files[0].legacy);
        assert_eq!(second.files[0].verified, None);
        assert_eq!(second.state, ModelStateWire::Installed);
    }
}
