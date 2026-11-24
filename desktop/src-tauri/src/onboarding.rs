//! Durable first-run state for the desktop onboarding wizard (issue #36).
//!
//! The marker lives in the Tauri shell, not in the daemon config: the wizard
//! records skips and completion for the *app setup*, while the TOML remains the
//! pipeline configuration. Secrets are never persisted here.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const ONBOARDING_STEPS: [&str; 6] =
    ["language", "llm", "backends", "models", "audio", "finish"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingState {
    pub version: u32,
    pub completed: bool,
    pub in_progress: bool,
    pub current_step: Option<String>,
    pub skipped_steps: Vec<String>,
}

impl Default for OnboardingState {
    fn default() -> Self {
        Self {
            version: 1,
            completed: false,
            in_progress: false,
            current_step: None,
            skipped_steps: Vec::new(),
        }
    }
}

impl OnboardingState {
    pub fn completed(&self) -> bool {
        self.completed
    }

    pub fn in_progress(&self) -> bool {
        self.in_progress
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!(
                "unsupported onboarding state version {}",
                self.version
            ));
        }
        if let Some(step) = &self.current_step
            && !ONBOARDING_STEPS.contains(&step.as_str())
        {
            return Err(format!("unknown onboarding step '{step}'"));
        }
        for step in &self.skipped_steps {
            if !ONBOARDING_STEPS.contains(&step.as_str()) {
                return Err(format!("unknown skipped step '{step}'"));
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct OnboardingStore {
    path: PathBuf,
}

impl OnboardingStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Missing state means "not completed yet"; a corrupt file is reset rather
    /// than trapping the app on the setup screen forever.
    pub fn load(&self) -> OnboardingState {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => match serde_json::from_str::<OnboardingState>(&text) {
                Ok(state) => match state.validate() {
                    Ok(()) => state,
                    Err(error) => {
                        tracing::warn!(target: "onboarding", "resetting invalid state: {error}");
                        OnboardingState::default()
                    }
                },
                Err(error) => {
                    tracing::warn!(target: "onboarding", "resetting unreadable state: {error}");
                    OnboardingState::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                OnboardingState::default()
            }
            Err(error) => {
                tracing::warn!(target: "onboarding", "could not read state: {error}");
                OnboardingState::default()
            }
        }
    }

    pub fn save(&self, state: &OnboardingState) -> Result<(), String> {
        state
            .validate()
            .map_err(|error| format!("invalid onboarding state: {error}"))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(state)
            .map_err(|error| format!("failed to serialize onboarding state: {error}"))?;
        let temp = self
            .path
            .with_extension(format!("tmp.{}", std::process::id()));
        std::fs::write(&temp, text)
            .map_err(|error| format!("failed to write {}: {error}", temp.display()))?;
        std::fs::rename(&temp, &self.path)
            .map_err(|error| format!("failed to replace {}: {error}", self.path.display()))?;
        Ok(())
    }
}

/// Whether the app should show the wizard automatically.
///
/// Existing installs that predate the marker are treated as completed so an
/// upgrade never forces every user through setup again.
pub fn should_open_wizard(state: &OnboardingState, config_exists: bool) -> bool {
    !state.completed() && (state.in_progress() || !config_exists)
}

/// Write the smallest config that lets Control start before models exist.
///
/// On Linux the local defaults (`whisper` + `kokoro`) are the eventual target,
/// but they require downloads; `nemotron` + `openai` start without local
/// artifacts. On macOS the native pair is genuinely zero-download.
pub fn write_bootstrap_config(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let contents = if std::env::consts::OS == "macos" {
        "# Created by desktop onboarding (issue #36)\nstt_provider = \"speech\"\ntts_provider = \"avspeech\"\n"
    } else {
        "# Created by desktop onboarding (issue #36)\nstt_provider = \"nemotron\"\ntts_provider = \"openai\"\n"
    };
    let temp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&temp, contents)
        .map_err(|error| format!("failed to write {}: {error}", temp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&temp, path)
        .map_err(|error| format!("failed to replace {}: {error}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_store() -> (PathBuf, OnboardingStore) {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "seneschal-onboarding-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = OnboardingStore::new(dir.join("onboarding.json"));
        (dir, store)
    }

    #[test]
    fn missing_state_is_first_run_and_survives_save_reload() {
        let (_dir, store) = temp_store();
        assert_eq!(store.load(), OnboardingState::default());

        let mut state = store.load();
        state.in_progress = true;
        state.current_step = Some("models".to_string());
        state.skipped_steps = vec!["language".to_string()];
        store.save(&state).unwrap();

        let loaded = store.load();
        assert!(loaded.in_progress());
        assert_eq!(loaded.current_step.as_deref(), Some("models"));
        assert_eq!(loaded.skipped_steps, ["language"]);
    }

    #[test]
    fn invalid_state_is_reset_instead_of_blocking_setup() {
        let (_dir, store) = temp_store();
        std::fs::write(&store.path, "{not json").unwrap();
        assert_eq!(store.load(), OnboardingState::default());

        std::fs::write(&store.path, r#"{"version":2}"#).unwrap();
        assert_eq!(store.load(), OnboardingState::default());
    }

    #[test]
    fn wizard_opens_for_first_run_and_in_progress_but_not_legacy_installs() {
        let mut state = OnboardingState::default();
        assert!(should_open_wizard(&state, false));
        assert!(!should_open_wizard(&state, true), "legacy install");

        state.in_progress = true;
        assert!(should_open_wizard(&state, true), "resumable first run");

        state.completed = true;
        state.in_progress = false;
        assert!(!should_open_wizard(&state, true), "completed marker");
    }

    fn temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "seneschal-onboarding-config-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn bootstrap_config_matches_the_platform_and_is_idempotent() {
        let dir = temp_dir();
        let path = dir.join("config.toml");
        write_bootstrap_config(&path).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        if std::env::consts::OS == "macos" {
            assert!(contents.contains("stt_provider = \"speech\""));
            assert!(contents.contains("tts_provider = \"avspeech\""));
        } else {
            assert!(contents.contains("stt_provider = \"nemotron\""));
            assert!(contents.contains("tts_provider = \"openai\""));
        }

        std::fs::write(&path, "# user config\n").unwrap();
        write_bootstrap_config(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# user config\n");
    }
}
