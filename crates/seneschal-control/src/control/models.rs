//! Model manager behind the Control API (issue #35).
//!
//! Wraps `seneschal-models` (catalog + resumable downloads) with
//! settings-backed activation, SSE broadcasting and NeMo docker service
//! cards. The desktop shell talks to this only through the Control REST/SSE
//! boundary; no core crate is linked into the GUI.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;

use uuid::Uuid;

use seneschal_common::config::Config;
use seneschal_common::settings::{FieldError, SettingsApplyResponse, SettingsPatch};
use seneschal_models::catalog::ModelSpec;
use seneschal_models::manager::{DownloadPhase, DownloadSnapshotWire, ManagerError, ModelManager};
use seneschal_models::nemo::{
    self, ComposeAction, ContainerState, NemoServiceSpec, NemoServiceStatus, WeightsState,
};
use seneschal_models::paths;

use super::broadcast::{ControlBroadcast, ControlEvent};
use super::settings::SettingsService;

const SILERO_VAD_ID: &str = "silero-vad-5.1.2";

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("unknown model or service '{0}'")]
    UnknownModel(String),
    #[error("a download is already running for '{0}'")]
    DownloadInProgress(String),
    #[error("no download is running for '{0}'")]
    DownloadNotRunning(String),
    #[error("model '{0}' is not fully installed")]
    NotInstalled(String),
    #[error("dependency '{1}' of model '{0}' is not installed")]
    DependencyMissing(String, String),
    #[error("model '{0}' is in use by the active configuration")]
    InUse(String),
    #[error("the NeMo weights are incomplete; download them before starting the service")]
    WeightsMissing,
    #[error("validation failed")]
    Validation(Vec<FieldError>),
    #[error("docker operation failed: {0}")]
    Docker(String),
    #[error("download failed: {0}")]
    Download(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl ModelError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownModel(_) => "unknown_model",
            Self::DownloadInProgress(_) => "download_in_progress",
            Self::DownloadNotRunning(_) => "download_not_running",
            Self::NotInstalled(_) => "model_not_installed",
            Self::DependencyMissing(..) => "dependency_missing",
            Self::InUse(_) => "model_in_use",
            Self::WeightsMissing => "weights_missing",
            Self::Validation(_) => "validation_failed",
            Self::Docker(_) => "docker_error",
            Self::Download(_) => "download_failed",
            Self::Io(_) => "model_io_error",
        }
    }

    pub fn status_code(&self) -> u16 {
        match self {
            Self::UnknownModel(_) => 404,
            Self::DownloadInProgress(_)
            | Self::DownloadNotRunning(_)
            | Self::NotInstalled(_)
            | Self::DependencyMissing(..)
            | Self::InUse(_)
            | Self::WeightsMissing => 409,
            Self::Validation(_) => 422,
            Self::Docker(_) => 502,
            Self::Download(_) | Self::Io(_) => 500,
        }
    }

    pub fn hint(&self) -> Option<String> {
        match self {
            Self::NotInstalled(_) => Some("Download the model first.".to_string()),
            Self::DependencyMissing(model, dep) => {
                Some(format!("Download '{dep}' first, then activate '{model}'."))
            }
            Self::InUse(_) => Some(
                "Switch the active model in Settings first, then delete this one.".to_string(),
            ),
            Self::WeightsMissing => Some(
                "Use the weights download action on the NeMo card, or run the docker download script."
                    .to_string(),
            ),
            _ => None,
        }
    }
}

fn phase_token(phase: DownloadPhase) -> &'static str {
    match phase {
        DownloadPhase::Downloading => "downloading",
        DownloadPhase::Verifying => "verifying",
        DownloadPhase::Completed => "completed",
        DownloadPhase::Failed => "failed",
        DownloadPhase::Cancelled => "cancelled",
    }
}

pub struct ModelControl {
    broadcast: ControlBroadcast,
    settings: Arc<SettingsService>,
    manager: ModelManager,
    workdir: PathBuf,
    weights_downloading: Arc<Mutex<HashSet<String>>>,
}

impl ModelControl {
    pub fn new(
        broadcast: ControlBroadcast,
        settings: Arc<SettingsService>,
        workdir: &Path,
    ) -> Self {
        let manager = ModelManager::new(workdir);
        Self::with_manager(broadcast, settings, manager, workdir)
    }

    pub fn with_manager(
        broadcast: ControlBroadcast,
        settings: Arc<SettingsService>,
        manager: ModelManager,
        workdir: &Path,
    ) -> Self {
        Self {
            broadcast,
            settings,
            manager,
            workdir: workdir.to_path_buf(),
            weights_downloading: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Catalog with per-file states, active flags and download snapshots.
    pub async fn catalog(&self) -> seneschal_models::ModelCatalogWire {
        let config = self.settings.config().await;
        let active = self.active_ids(&config);
        self.manager.catalog_wire(&active)
    }

    /// Models that are active (configured) or required by an active model.
    fn active_ids(&self, config: &Config) -> HashSet<String> {
        let mut active: HashSet<String> = HashSet::new();
        let stt_provider = config.stt_provider.to_lowercase();
        let tts_provider = config.tts_provider.to_lowercase();
        for spec in &self.manager_catalog() {
            let is_active = match spec.backend {
                seneschal_models::ModelBackend::Whisper => {
                    stt_provider == "whisper"
                        && self.configured_file_matches(&config.whisper_model, spec)
                }
                seneschal_models::ModelBackend::Parakeet => {
                    stt_provider == "parakeet"
                        && config
                            .parakeet_model_dir
                            .as_deref()
                            .is_some_and(|dir| self.configured_dir_matches(dir, spec))
                }
                seneschal_models::ModelBackend::Silero => {
                    self.configured_file_matches(&config.vad_model, spec)
                }
                seneschal_models::ModelBackend::Kokoro => {
                    tts_provider == "kokoro"
                        && self.configured_file_matches(&config.kokoro_model, spec)
                        && self.configured_file_matches(&config.kokoro_voices, spec)
                }
                seneschal_models::ModelBackend::NemotronAsr
                | seneschal_models::ModelBackend::MagpieTts => false,
            };
            if is_active {
                active.insert(spec.id.clone());
            }
        }
        self.manager.required_closure(&active)
    }

    fn manager_catalog(&self) -> Vec<ModelSpec> {
        // The manager owns the (possibly test-injected) catalog.
        self.manager.catalog_slice().to_vec()
    }

    fn resolve_config_path(&self, raw: &str) -> PathBuf {
        let candidate = Path::new(raw.trim());
        if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.workdir.join(candidate)
        }
    }

    /// True when the configured path points at this spec's managed file
    /// (managed root or any legacy root).
    fn configured_file_matches(&self, configured: &str, spec: &ModelSpec) -> bool {
        let Some(file) = spec.files.first() else {
            return false;
        };
        let resolved = self.resolve_config_path(configured);
        let managed = self.manager.root().join(&file.rel_path);
        if paths::same_path(&resolved, &managed) {
            return true;
        }
        paths::legacy_roots(&self.workdir)
            .iter()
            .any(|root| paths::same_path(&resolved, &root.join(&file.rel_path)))
    }

    fn configured_dir_matches(&self, configured: &str, spec: &ModelSpec) -> bool {
        let Some(dir) = spec.bundle_dir() else {
            return false;
        };
        let resolved = self.resolve_config_path(configured);
        let managed = self.manager.root().join(&dir);
        if paths::same_path(&resolved, &managed) {
            return true;
        }
        paths::legacy_roots(&self.workdir)
            .iter()
            .any(|root| paths::same_path(&resolved, &root.join(&dir)))
    }

    pub async fn start_download(&self, id: &str) -> Result<DownloadSnapshotWire, ModelError> {
        let broadcast = self.broadcast.clone();
        let snapshot = self
            .manager
            .start_download(id, move |event| {
                broadcast.send(ControlEvent::ModelDownloadProgress {
                    model_id: event.model_id,
                    download_id: event.download_id,
                    phase: phase_token(event.phase).to_string(),
                    downloaded_bytes: event.downloaded_bytes,
                    total_bytes: event.total_bytes,
                    current_file: event.current_file,
                    message: event.message,
                });
            })
            .map_err(ModelError::from)?;
        Ok(snapshot)
    }

    pub fn cancel_download(&self, id: &str) -> Result<(), ModelError> {
        self.manager.cancel_download(id).map_err(ModelError::from)
    }

    /// Activate an installed model: validates dependencies, then applies the
    /// corresponding settings patch through the existing Settings service
    /// (atomic TOML write, restart tier).
    pub async fn activate(&self, id: &str) -> Result<SettingsApplyResponse, ModelError> {
        let spec = self
            .manager
            .spec(id)
            .ok_or_else(|| ModelError::UnknownModel(id.to_string()))?
            .clone();
        let wire = self.manager.catalog_wire(&HashSet::new());
        let entry = wire
            .models
            .iter()
            .find(|model| model.id == id)
            .ok_or_else(|| ModelError::UnknownModel(id.to_string()))?;
        if entry.state != seneschal_models::ModelStateWire::Installed {
            return Err(ModelError::NotInstalled(id.to_string()));
        }
        for dep in &spec.requires {
            let installed = wire.models.iter().any(|model| {
                model.id == *dep && model.state == seneschal_models::ModelStateWire::Installed
            });
            if !installed {
                return Err(ModelError::DependencyMissing(id.to_string(), dep.clone()));
            }
        }
        let patch = self.activation_patch(&spec);
        self.settings
            .apply(&patch)
            .await
            .map_err(ModelError::Validation)
    }

    /// Settings patch that selects this model (absolute paths under the
    /// managed root), including its VAD dependency.
    fn activation_patch(&self, spec: &ModelSpec) -> SettingsPatch {
        let root = self.manager.root().to_path_buf();
        let abs = |rel: &str| root.join(rel).display().to_string();
        let mut patch = SettingsPatch::default();
        match spec.backend {
            seneschal_models::ModelBackend::Whisper => {
                patch.stt_provider = Some("whisper".to_string());
                if let Some(file) = spec.files.first() {
                    patch.whisper_model = Some(abs(&file.rel_path));
                }
            }
            seneschal_models::ModelBackend::Parakeet => {
                patch.stt_provider = Some("parakeet".to_string());
                if let Some(dir) = spec.bundle_dir() {
                    patch.parakeet_model_dir = Some(Some(abs(&dir)));
                }
            }
            seneschal_models::ModelBackend::Kokoro => {
                patch.tts_provider = Some("kokoro".to_string());
                for file in &spec.files {
                    if file.rel_path.ends_with(".onnx") {
                        patch.kokoro_model = Some(abs(&file.rel_path));
                    } else if file.rel_path.ends_with(".bin") {
                        patch.kokoro_voices = Some(abs(&file.rel_path));
                    }
                }
            }
            _ => {}
        }
        if spec.requires.iter().any(|dep| dep == SILERO_VAD_ID)
            && let Some(vad) = self
                .manager
                .spec(SILERO_VAD_ID)
                .and_then(|vad| vad.files.first())
        {
            patch.vad_model = Some(abs(&vad.rel_path));
        }
        patch
    }

    pub async fn delete(&self, id: &str) -> Result<(), ModelError> {
        let config = self.settings.config().await;
        let active = self.active_ids(&config);
        let in_use = active.contains(id);
        self.manager
            .delete_model(id, in_use)
            .map_err(ModelError::from)
    }

    // ── NeMo service cards ───────────────────────────────────────────────────

    /// Status of every NeMo service (one `docker compose ps` call plus HTTP
    /// probes per service).
    pub async fn nemo_services(&self) -> Vec<NemoServiceStatus> {
        let config = self.settings.config().await;
        let compose = nemo::compose_services(&self.workdir).await;
        let containers = compose.as_ref().ok();
        let mut statuses = Vec::new();
        for spec in nemo::NEMO_SERVICES {
            statuses.push(self.nemo_status(spec, containers, &config).await);
        }
        statuses
    }

    async fn nemo_status(
        &self,
        spec: &NemoServiceSpec,
        containers: Option<&HashMap<String, String>>,
        config: &Config,
    ) -> NemoServiceStatus {
        let (base_url, api_key, expected_model) = match spec.compose_service {
            "nemotron-asr" => (
                config.stt_url.clone(),
                config.stt_api_key.clone(),
                Some(config.stt_model.clone()),
            ),
            _ => (config.openai_tts_url.clone(), String::new(), None),
        };
        let model_dir = std::env::var(spec.model_dir_env)
            .ok()
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from);
        let downloading = self.weights_downloading.lock().unwrap().contains(spec.id);
        let (weights_state, weights) =
            nemo::weights_status(spec, model_dir.as_deref(), downloading);
        let probe = nemo::probe_http(&base_url, &api_key).await;
        let container = containers.and_then(|map| map.get(spec.compose_service).cloned());
        let mut status = nemo::service_status(
            spec,
            &base_url,
            container,
            probe,
            expected_model.as_deref(),
            weights_state,
            weights,
        );
        status.active = match spec.compose_service {
            "nemotron-asr" => config.stt_provider == "nemotron",
            _ => config.tts_provider == "openai",
        };
        status
    }

    pub async fn nemo_action(
        &self,
        id: &str,
        action: ComposeAction,
    ) -> Result<NemoServiceStatus, ModelError> {
        let spec = nemo::service(id).ok_or_else(|| ModelError::UnknownModel(id.to_string()))?;
        if action == ComposeAction::Up {
            let model_dir = std::env::var(spec.model_dir_env)
                .ok()
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from);
            let (weights_state, _) = nemo::weights_status(spec, model_dir.as_deref(), false);
            if matches!(weights_state, WeightsState::Missing | WeightsState::Partial) {
                return Err(ModelError::WeightsMissing);
            }
        }
        nemo::compose_action(&self.workdir, spec.compose_service, action)
            .await
            .map_err(ModelError::Docker)?;
        let config = self.settings.config().await;
        // One fresh compose call so the returned card reflects reality.
        let compose = nemo::compose_services(&self.workdir).await;
        Ok(self.nemo_status(spec, compose.as_ref().ok(), &config).await)
    }

    /// Kick off the docker weight download script for a NeMo service. Progress
    /// is indeterminate; the terminal phase arrives over SSE.
    pub async fn download_nemo_weights(&self, id: &str) -> Result<(), ModelError> {
        let spec = nemo::service(id).ok_or_else(|| ModelError::UnknownModel(id.to_string()))?;
        {
            let mut downloading = self.weights_downloading.lock().unwrap();
            if downloading.contains(id) {
                return Err(ModelError::DownloadInProgress(id.to_string()));
            }
            downloading.insert(id.to_string());
        }
        let workdir = self.workdir.clone();
        let script = spec.download_script.to_string();
        let service_id = id.to_string();
        let downloading = Arc::clone(&self.weights_downloading);
        let broadcast = self.broadcast.clone();
        let download_id = Uuid::new_v4().to_string();

        broadcast.send(ControlEvent::ModelDownloadProgress {
            model_id: service_id.clone(),
            download_id: download_id.clone(),
            phase: "downloading".to_string(),
            downloaded_bytes: 0,
            total_bytes: None,
            current_file: Some(script.clone()),
            message: None,
        });

        tokio::spawn(async move {
            let output = tokio::process::Command::new("bash")
                .arg(&script)
                .current_dir(&workdir)
                .output()
                .await;
            downloading.lock().unwrap().remove(&service_id);
            let (phase, message) = match output {
                Ok(output) if output.status.success() => (DownloadPhase::Completed, None),
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let detail = if stderr.trim().is_empty() {
                        stdout.trim().to_string()
                    } else {
                        stderr.trim().to_string()
                    };
                    (
                        DownloadPhase::Failed,
                        Some(if detail.is_empty() {
                            format!("script exited with {}", output.status)
                        } else {
                            detail
                        }),
                    )
                }
                Err(error) => (DownloadPhase::Failed, Some(error.to_string())),
            };
            broadcast.send(ControlEvent::ModelDownloadProgress {
                model_id: service_id,
                download_id,
                phase: phase_token(phase).to_string(),
                downloaded_bytes: 0,
                total_bytes: None,
                current_file: None,
                message,
            });
        });
        Ok(())
    }

    /// Container state for tests / single-card refresh.
    #[allow(dead_code)]
    pub async fn nemo_container_state(&self, id: &str) -> Option<ContainerState> {
        let spec = nemo::service(id)?;
        let compose = nemo::compose_services(&self.workdir).await.ok()?;
        let state = compose.get(spec.compose_service)?;
        Some(match state.as_str() {
            "running" => ContainerState::Ready,
            _ => ContainerState::Stopped,
        })
    }
}

impl From<ManagerError> for ModelError {
    fn from(error: ManagerError) -> Self {
        match error {
            ManagerError::UnknownModel(id) => Self::UnknownModel(id),
            ManagerError::DownloadInProgress(id) => Self::DownloadInProgress(id),
            ManagerError::DownloadNotRunning(id) => Self::DownloadNotRunning(id),
            ManagerError::InUse(id) => Self::InUse(id),
            ManagerError::Download(message) => Self::Download(message),
            ManagerError::Io(error) => Self::Io(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_and_hints_are_stable() {
        let error = ModelError::NotInstalled("whisper-tiny".to_string());
        assert_eq!(error.code(), "model_not_installed");
        assert_eq!(error.status_code(), 409);
        assert!(error.hint().is_some());

        let error = ModelError::UnknownModel("nope".to_string());
        assert_eq!(error.status_code(), 404);
    }
}
