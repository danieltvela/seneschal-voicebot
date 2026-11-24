//! NeMo docker services as first-class backends (issue #35).
//!
//! The daemon does not manage containers with a Docker API client; it shells
//! out to `docker compose` with an allow-listed service name and the compose
//! file already present in the repository, and probes the services' HTTP
//! health endpoints (`/ready` + `/v1/models`) for the card status.

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;
use tokio::time::timeout;

const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const COMPOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NemoServiceSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// `docker compose` service name.
    pub compose_service: &'static str,
    pub port: u16,
    /// Environment variable holding the weights directory.
    pub model_dir_env: &'static str,
    /// Stable artifacts that must exist inside the weights directory.
    pub weights: &'static [&'static str],
    /// Download script relative to the daemon workdir.
    pub download_script: &'static str,
}

pub const NEMO_SERVICES: &[NemoServiceSpec] = &[
    NemoServiceSpec {
        id: "nemo-asr",
        label: "Nemotron 3.5 ASR",
        compose_service: "nemotron-asr",
        port: 8026,
        model_dir_env: "NEMOTRON_ASR_MODEL_DIR",
        weights: &["asr.gguf", "silero-v6.2.0.gguf"],
        download_script: "docker/nemotron-asr/download-weights.sh",
    },
    NemoServiceSpec {
        id: "magpie-tts",
        label: "Magpie TTS",
        compose_service: "magpie-tts",
        port: 8025,
        model_dir_env: "MAGPIE_TTS_MODEL_DIR",
        weights: &["magpie.gguf", "codec.gguf", "tokenizer"],
        download_script: "docker/magpie-tts/download-weights.sh",
    },
];

pub fn service(id: &str) -> Option<&'static NemoServiceSpec> {
    NEMO_SERVICES.iter().find(|candidate| candidate.id == id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerState {
    /// `docker` is not installed or the compose file is missing.
    DockerUnavailable,
    Stopped,
    Starting,
    Ready,
    /// Container answers but `/v1/models` does not list the expected model.
    ModelMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WeightsState {
    Missing,
    Partial,
    Ready,
    Downloading,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct WeightFileWire {
    pub name: String,
    pub present: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NemoServiceStatus {
    pub id: String,
    pub label: String,
    pub base_url: String,
    pub container_state: ContainerState,
    pub ready: Option<bool>,
    pub served_models: Vec<String>,
    pub expected_model: Option<String>,
    pub model_matches: Option<bool>,
    pub weights_state: WeightsState,
    pub weights: Vec<WeightFileWire>,
    /// Set by the Control layer: this backend is the active STT/TTS provider.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Result of probing `{base}/ready` and `{base}/v1/models`.
#[derive(Debug, Default)]
pub struct HttpProbe {
    pub ready: Option<bool>,
    pub served_models: Vec<String>,
}

pub async fn probe_http(base_url: &str, api_key: &str) -> HttpProbe {
    let client = match reqwest::Client::builder().timeout(PROBE_TIMEOUT).build() {
        Ok(client) => client,
        Err(_) => return HttpProbe::default(),
    };
    let base = base_url.trim_end_matches('/');
    let mut ready_request = client.get(format!("{base}/ready"));
    if !api_key.is_empty() {
        ready_request = ready_request.bearer_auth(api_key);
    }
    let ready = match ready_request.send().await {
        Ok(response) => Some(response.status().is_success()),
        Err(_) => None,
    };

    let mut models_request = client.get(format!("{base}/v1/models"));
    if !api_key.is_empty() {
        models_request = models_request.bearer_auth(api_key);
    }
    let served_models = match models_request.send().await {
        Ok(response) if response.status().is_success() => response
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|value| {
                value
                    .get("data")
                    .and_then(serde_json::Value::as_array)
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    HttpProbe {
        ready,
        served_models,
    }
}

/// `docker compose ps --format json`, mapped service name → state.
/// `Err` means docker itself is unavailable; `Ok(map)` may still be empty
/// (all services stopped).
pub async fn compose_services(workdir: &Path) -> Result<HashMap<String, String>, String> {
    let output = timeout(
        COMPOSE_TIMEOUT,
        tokio::process::Command::new("docker")
            .args(["compose", "ps", "--format", "json"])
            .current_dir(compose_dir(workdir))
            .output(),
    )
    .await
    .map_err(|_| "docker compose ps timed out".to_string())?
    .map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "docker is not installed".to_string()
        } else {
            format!("docker compose ps failed: {error}")
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("docker compose ps failed: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut states = HashMap::new();
    let parsed = serde_json::from_str::<Vec<serde_json::Value>>(&stdout)
        .ok()
        .map(|entries| entries.into_iter())
        .unwrap_or_else(|| {
            // Some docker compose versions emit NDJSON lines instead of an array.
            stdout
                .lines()
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .collect::<Vec<_>>()
                .into_iter()
        });
    for entry in parsed {
        if let (Some(service), Some(state)) = (
            entry.get("Service").and_then(serde_json::Value::as_str),
            entry.get("State").and_then(serde_json::Value::as_str),
        ) {
            states.insert(service.to_string(), state.to_string());
        }
    }
    Ok(states)
}

pub async fn compose_action(
    workdir: &Path,
    compose_service: &str,
    action: ComposeAction,
) -> Result<(), String> {
    let mut command = tokio::process::Command::new("docker");
    command.args(["compose", "--profile", "nvidia"]);
    match action {
        ComposeAction::Up => command.args(["up", "-d", compose_service]),
        ComposeAction::Stop => command.args(["stop", compose_service]),
    };
    let output = timeout(
        COMPOSE_TIMEOUT,
        command.current_dir(compose_dir(workdir)).output(),
    )
    .await
    .map_err(|_| "docker compose timed out".to_string())?
    .map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "docker is not installed".to_string()
        } else {
            format!("docker compose failed: {error}")
        }
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "docker compose {} {} failed: {}",
            match action {
                ComposeAction::Up => "up -d",
                ComposeAction::Stop => "stop",
            },
            compose_service,
            stderr.trim()
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeAction {
    Up,
    Stop,
}

fn compose_dir(workdir: &Path) -> std::path::PathBuf {
    workdir.join("docker")
}

/// Check the stable weight artifacts for a service.
pub fn weights_status(
    spec: &NemoServiceSpec,
    model_dir: Option<&Path>,
    downloading: bool,
) -> (WeightsState, Vec<WeightFileWire>) {
    let Some(model_dir) = model_dir else {
        return (WeightsState::Unknown, Vec::new());
    };
    let files: Vec<WeightFileWire> = spec
        .weights
        .iter()
        .map(|name| WeightFileWire {
            name: (*name).to_string(),
            present: model_dir.join(name).exists(),
        })
        .collect();
    let present = files.iter().filter(|file| file.present).count();
    let state = if downloading {
        WeightsState::Downloading
    } else if present == files.len() {
        WeightsState::Ready
    } else if present == 0 {
        WeightsState::Missing
    } else {
        WeightsState::Partial
    };
    (state, files)
}

/// Assemble the card status from the container inventory + HTTP probe.
#[allow(clippy::too_many_arguments)]
pub fn service_status(
    spec: &NemoServiceSpec,
    base_url: &str,
    container_state: Option<String>,
    probe: HttpProbe,
    expected_model: Option<&str>,
    weights_state: WeightsState,
    weights: Vec<WeightFileWire>,
) -> NemoServiceStatus {
    let container_is_running = container_state.as_deref() == Some("running");
    let (container_state, model_matches, hint) = match container_state.as_deref() {
        None => (
            ContainerState::DockerUnavailable,
            None,
            Some(
                "Docker is not available from the daemon; manage the NeMo services manually."
                    .to_string(),
            ),
        ),
        Some(_) if !container_is_running => (ContainerState::Stopped, None, None),
        Some(_) => match probe.ready {
            Some(true) => match expected_model {
                Some(expected) => {
                    if probe.served_models.iter().any(|id| id == expected) {
                        (ContainerState::Ready, Some(true), None)
                    } else {
                        (
                            ContainerState::ModelMismatch,
                            Some(false),
                            Some(format!(
                                "Service is ready but does not serve '{expected}' (found: {}).",
                                if probe.served_models.is_empty() {
                                    "<none>".to_string()
                                } else {
                                    probe.served_models.join(", ")
                                }
                            )),
                        )
                    }
                }
                None => (ContainerState::Ready, None, None),
            },
            // Running but `/ready` not answering yet (cold start can take minutes).
            Some(false) | None => (ContainerState::Starting, None, None),
        },
    };

    NemoServiceStatus {
        id: spec.id.to_string(),
        label: spec.label.to_string(),
        base_url: base_url.to_string(),
        container_state,
        ready: probe.ready,
        served_models: probe.served_models,
        expected_model: expected_model.map(str::to_string),
        model_matches,
        weights_state,
        weights,
        active: false,
        hint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn spec() -> &'static NemoServiceSpec {
        service("nemo-asr").unwrap()
    }

    #[test]
    fn weights_status_classifies_missing_partial_ready() {
        let dir = tempfile::TempDir::new().unwrap();
        let (state, files) = weights_status(spec(), Some(dir.path()), false);
        assert_eq!(state, WeightsState::Missing);
        assert_eq!(files.len(), 2);

        std::fs::write(dir.path().join("asr.gguf"), b"x").unwrap();
        let (state, _) = weights_status(spec(), Some(dir.path()), false);
        assert_eq!(state, WeightsState::Partial);

        std::fs::write(dir.path().join("silero-v6.2.0.gguf"), b"x").unwrap();
        let (state, _) = weights_status(spec(), Some(dir.path()), false);
        assert_eq!(state, WeightsState::Ready);

        let (state, _) = weights_status(spec(), Some(dir.path()), true);
        assert_eq!(state, WeightsState::Downloading);

        let (state, files) = weights_status(spec(), None, false);
        assert_eq!(state, WeightsState::Unknown);
        assert!(files.is_empty());
    }

    #[test]
    fn service_status_maps_container_and_probe_to_card_state() {
        // Docker unavailable.
        let status = service_status(
            spec(),
            "http://localhost:8026",
            None,
            HttpProbe::default(),
            Some("nemotron-3.5"),
            WeightsState::Ready,
            Vec::new(),
        );
        assert_eq!(status.container_state, ContainerState::DockerUnavailable);

        // Running but not ready yet.
        let status = service_status(
            spec(),
            "http://localhost:8026",
            Some("running".to_string()),
            HttpProbe {
                ready: Some(false),
                served_models: Vec::new(),
            },
            Some("nemotron-3.5"),
            WeightsState::Ready,
            Vec::new(),
        );
        assert_eq!(status.container_state, ContainerState::Starting);

        // Ready with the expected model.
        let status = service_status(
            spec(),
            "http://localhost:8026",
            Some("running".to_string()),
            HttpProbe {
                ready: Some(true),
                served_models: vec!["nemotron-3.5".to_string()],
            },
            Some("nemotron-3.5"),
            WeightsState::Ready,
            Vec::new(),
        );
        assert_eq!(status.container_state, ContainerState::Ready);
        assert_eq!(status.model_matches, Some(true));

        // Ready but wrong model id.
        let status = service_status(
            spec(),
            "http://localhost:8026",
            Some("running".to_string()),
            HttpProbe {
                ready: Some(true),
                served_models: vec!["other-model".to_string()],
            },
            Some("nemotron-3.5"),
            WeightsState::Ready,
            Vec::new(),
        );
        assert_eq!(status.container_state, ContainerState::ModelMismatch);
        assert_eq!(status.model_matches, Some(false));
        assert!(status.hint.is_some());

        // Stopped container (present in ps but not running).
        let status = service_status(
            spec(),
            "http://localhost:8026",
            Some("exited".to_string()),
            HttpProbe::default(),
            Some("nemotron-3.5"),
            WeightsState::Ready,
            Vec::new(),
        );
        assert_eq!(status.container_state, ContainerState::Stopped);
    }

    #[test]
    fn compose_service_lookup_is_exact() {
        assert!(service("nemo-asr").is_some());
        assert!(service("magpie-tts").is_some());
        assert!(service("nope").is_none());
    }

    #[test]
    fn weights_paths_resolve_under_the_model_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = PathBuf::from(dir.path()).join("asr.gguf");
        std::fs::write(&path, b"x").unwrap();
        let (_, files) = weights_status(spec(), Some(dir.path()), false);
        assert!(files[0].present);
        assert!(!files[1].present);
    }
}
