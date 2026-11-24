use std::sync::Mutex;

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Feed used by stable builds. GitHub only resolves this to a non-prerelease.
const STABLE_UPDATER_ENDPOINT: &str =
    "https://github.com/danieltvela/seneschal-voicebot/releases/latest/download/latest.json";
/// Rolling technical release that carries the manifest for alpha/beta/rc builds.
const PREVIEW_UPDATER_ENDPOINT: &str = "https://github.com/danieltvela/seneschal-voicebot/releases/download/desktop-updates-preview/latest.json";

/// Holds the checked update until the user explicitly installs it.
pub struct PendingUpdate(Mutex<Option<Update>>);

impl PendingUpdate {
    pub fn new() -> Self {
        Self(Mutex::new(None))
    }
}

impl Default for PendingUpdate {
    fn default() -> Self {
        Self::new()
    }
}

/// Serializable update metadata shown in the shell.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current_version: String,
    pub version: String,
    pub notes: Option<String>,
    pub published_at: Option<String>,
}

fn update_info(update: &Update) -> UpdateInfo {
    UpdateInfo {
        current_version: update.current_version.clone(),
        version: update.version.clone(),
        notes: update.body.clone(),
        published_at: update.date.as_ref().map(ToString::to_string),
    }
}

/// Prerelease builds read the rolling preview feed; stable builds read GitHub
/// "latest". Keeping the channels separate prevents an alpha from becoming a
/// required update for stable installs.
pub fn updater_endpoint(prerelease: Option<&str>) -> &'static str {
    match prerelease {
        Some(prerelease) if !prerelease.is_empty() => PREVIEW_UPDATER_ENDPOINT,
        _ => STABLE_UPDATER_ENDPOINT,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum UpdateProgress {
    #[serde(rename_all = "camelCase")]
    Started {
        content_length: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    Progress {
        chunk_length: usize,
    },
    Finished,
}

#[tauri::command]
pub async fn check_for_update(
    app: AppHandle,
    pending_update: State<'_, PendingUpdate>,
) -> Result<Option<UpdateInfo>, String> {
    let current = &app.package_info().version;
    let endpoint = updater_endpoint(if current.pre.is_empty() {
        None
    } else {
        Some(current.pre.as_str())
    });
    let endpoint_url =
        url::Url::parse(endpoint).map_err(|error| format!("invalid updater endpoint: {error}"))?;

    let update = app
        .updater_builder()
        .endpoints(vec![endpoint_url])
        .map_err(|error| error.to_string())?
        .build()
        .map_err(|error| error.to_string())?
        .check()
        .await
        .map_err(|error| error.to_string())?;

    *pending_update.0.lock().unwrap() = update.clone();
    Ok(update.as_ref().map(update_info))
}

#[tauri::command]
pub async fn install_update(
    pending_update: State<'_, PendingUpdate>,
    on_event: Channel<UpdateProgress>,
) -> Result<(), String> {
    let Some(update) = pending_update.0.lock().unwrap().take() else {
        return Err("there is no pending update".to_string());
    };

    let mut started = false;
    update
        .download_and_install(
            |chunk_length, content_length| {
                if !started {
                    started = true;
                    let _ = on_event.send(UpdateProgress::Started { content_length });
                }
                let _ = on_event.send(UpdateProgress::Progress { chunk_length });
            },
            || {
                let _ = on_event.send(UpdateProgress::Finished);
            },
        )
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prerelease_builds_use_the_preview_feed() {
        assert_eq!(updater_endpoint(None), STABLE_UPDATER_ENDPOINT);
        assert_eq!(updater_endpoint(Some("")), STABLE_UPDATER_ENDPOINT);
        assert_eq!(updater_endpoint(Some("alpha.1")), PREVIEW_UPDATER_ENDPOINT);
        assert_eq!(updater_endpoint(Some("beta.2")), PREVIEW_UPDATER_ENDPOINT);
    }

    #[test]
    fn updater_endpoints_are_https_github_manifests() {
        for endpoint in [STABLE_UPDATER_ENDPOINT, PREVIEW_UPDATER_ENDPOINT] {
            let parsed = url::Url::parse(endpoint).expect("valid endpoint");
            assert_eq!(parsed.scheme(), "https");
            assert!(parsed.path().ends_with("latest.json"));
        }
    }
}
