//! Config read / validate / apply service behind the Control API (issue #33).
//!
//! The service owns the current effective `Config` for the exposed Settings
//! subset. It writes the active TOML atomically (preserving unrelated keys and
//! comments with `toml_edit`), applies hot fields directly, and asks the main
//! audio loop to reload conversation-level hot fields.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Context;
use tokio::sync::{RwLock, mpsc};

use seneschal_common::settings::{
    FieldError, HotSettings, SettingOrigin, SettingTier, SettingsApplyResponse, SettingsFieldWire,
    SettingsPatch, SettingsProviders, SettingsSnapshot, ValidationResponse, active_config_path,
    setting_spec,
};
use seneschal_core::audio::output::AudioOutput;
use seneschal_core::stt;
use seneschal_core::tts::TtsEngine;

/// Shared TTS engine handle, also used by the pipeline task.
pub type SharedTtsEngine = Arc<RwLock<TtsEngine>>;

pub struct SettingsService {
    config: RwLock<seneschal_common::config::Config>,
    path: PathBuf,
    revision: AtomicU64,
    tts: SharedTtsEngine,
    audio_output: Arc<AudioOutput>,
    hot_settings: Arc<std::sync::Mutex<HotSettings>>,
    hot_tx: mpsc::Sender<HotSettings>,
}

impl SettingsService {
    pub fn new(
        config: seneschal_common::config::Config,
        tts: SharedTtsEngine,
        audio_output: Arc<AudioOutput>,
        hot_settings: Arc<std::sync::Mutex<HotSettings>>,
        hot_tx: mpsc::Sender<HotSettings>,
    ) -> anyhow::Result<Self> {
        let path = active_config_path()?;
        Ok(Self {
            config: RwLock::new(config),
            path,
            revision: AtomicU64::new(1),
            tts,
            audio_output,
            hot_settings,
            hot_tx,
        })
    }

    pub async fn snapshot(&self) -> SettingsSnapshot {
        let config = self.config.read().await.clone();
        let file_keys = self.file_keys();
        let revision = self.revision.load(Ordering::SeqCst);
        let fields = seneschal_common::settings::SETTING_SPECS
            .iter()
            .map(|spec| Self::field(&config, spec, &file_keys))
            .collect();
        SettingsSnapshot {
            revision,
            config_file: Some(self.path.display().to_string()),
            fields,
            providers: SettingsProviders {
                stt: stt::supported_providers(),
                tts: TtsEngine::supported_providers(),
            },
        }
    }

    /// Effective `Config` (embedded defaults + file + env overrides). The
    /// model manager uses it for active-model detection and NeMo health
    /// targets, including values that are not part of the Settings allow-list.
    pub async fn config(&self) -> seneschal_common::config::Config {
        self.config.read().await.clone()
    }

    pub async fn validate(&self, patch: &SettingsPatch) -> ValidationResponse {
        let config = self.config.read().await.clone();
        let (tier, errors) = Self::validate_patch(&config, patch);
        ValidationResponse {
            ok: errors.is_empty(),
            errors,
            tier,
        }
    }

    pub async fn apply(
        &self,
        patch: &SettingsPatch,
    ) -> Result<SettingsApplyResponse, Vec<FieldError>> {
        let mut config = self.config.write().await;
        let old = config.clone();
        apply_patch_to_config(&mut config, patch);
        let (candidate_tier, errors) = Self::validate_patch(&config, patch);
        if !errors.is_empty() {
            *config = old;
            return Err(errors);
        }

        let changed = Self::changed_keys(&old, &config);
        if changed.is_empty() {
            drop(config);
            let snapshot = self.snapshot().await;
            return Ok(SettingsApplyResponse {
                tier: SettingTier::Hot,
                revision: self.revision.load(Ordering::SeqCst),
                snapshot,
            });
        }

        let tier = candidate_tier.unwrap_or(SettingTier::Restart);
        let previous_tts = Self::tts_values(&old);
        let previous_volume = old.output_volume;

        // Preflight runtime hot values before the file is replaced. If this
        // fails (for example an AVSpeech voice that is not installed), neither
        // TOML nor the running pipeline changes.
        if tier == SettingTier::Hot
            && let Err(error) = self.apply_hot_values(&config, patch).await
        {
            *config = old;
            return Err(vec![error]);
        }

        if let Err(error) = write_patch(&self.path, patch, &changed) {
            *config = old;
            if tier == SettingTier::Hot {
                self.revert_hot_values(previous_tts, previous_volume).await;
            }
            return Err(vec![FieldError {
                field: "config",
                message: format!("Failed to write {}: {error:#}", self.path.display()),
            }]);
        }

        if tier == SettingTier::Hot {
            self.publish_hot_settings(&config).await;
        }
        drop(config);
        self.revision.fetch_add(1, Ordering::SeqCst);
        let snapshot = self.snapshot().await;
        Ok(SettingsApplyResponse {
            tier,
            revision: self.revision.load(Ordering::SeqCst),
            snapshot,
        })
    }

    fn tts_values(config: &seneschal_common::config::Config) -> (Option<String>, Option<f32>) {
        match config.tts_provider.as_str() {
            "avspeech" => (
                Some(config.avspeech_voice.clone()),
                Some(config.avspeech_rate),
            ),
            "kokoro" => (Some(config.kokoro_voice.clone()), Some(config.kokoro_speed)),
            "openai" => (
                Some(config.openai_tts_voice.clone()),
                Some(config.openai_tts_speed),
            ),
            _ => (None, None),
        }
    }

    async fn apply_hot_values(
        &self,
        config: &seneschal_common::config::Config,
        patch: &SettingsPatch,
    ) -> Result<(), FieldError> {
        let (voice, rate) = match config.tts_provider.as_str() {
            "avspeech" => (
                patch.avspeech_voice.as_deref().map(str::trim),
                patch.avspeech_rate,
            ),
            "kokoro" => (
                patch.kokoro_voice.as_deref().map(str::trim),
                patch.kokoro_speed,
            ),
            "openai" => (
                patch.openai_tts_voice.as_deref().map(str::trim),
                patch.openai_tts_speed,
            ),
            _ => (None, None),
        };
        if voice.is_some() || rate.is_some() {
            let mut tts = self.tts.write().await;
            tts.apply_voice_and_rate(voice, rate).map_err(|error| {
                tracing::warn!(target: "settings", "TTS hot preflight failed: {error:#}");
                FieldError {
                    field: "tts",
                    message: format!("{error:#}"),
                }
            })?;
        }
        if let Some(volume) = patch.output_volume {
            self.audio_output.set_volume(volume);
        }
        Ok(())
    }

    async fn revert_hot_values(&self, tts: (Option<String>, Option<f32>), volume: f32) {
        let (voice, rate) = tts;
        if voice.is_some() || rate.is_some() {
            let mut engine = self.tts.write().await;
            if let Err(error) = engine.apply_voice_and_rate(voice.as_deref(), rate) {
                tracing::warn!(target: "settings", "TTS hot revert failed: {error:#}");
            }
        }
        self.audio_output.set_volume(volume);
    }

    async fn publish_hot_settings(&self, config: &seneschal_common::config::Config) {
        let hot = HotSettings {
            language: config.language.clone(),
            wake_word: config.primary_wake_word().to_string(),
        };
        *self.hot_settings.lock().unwrap() = hot.clone();
        if self.hot_tx.send(hot).await.is_err() {
            tracing::debug!(target: "settings", "No main-loop receiver for hot settings");
        }
    }

    fn field(
        config: &seneschal_common::config::Config,
        spec: &seneschal_common::settings::SettingSpec,
        file_keys: &HashSet<String>,
    ) -> SettingsFieldWire {
        let origin = if std::env::var(spec.env_var).is_ok() {
            SettingOrigin::Environment
        } else if file_keys.contains(spec.key) {
            SettingOrigin::File
        } else {
            SettingOrigin::Default
        };
        let options = match spec.key {
            "stt_provider" => stt::supported_providers(),
            "tts_provider" => TtsEngine::supported_providers(),
            "language" => vec!["es", "en"],
            _ => Vec::new(),
        };
        SettingsFieldWire {
            key: spec.key,
            label: spec.label,
            category: spec.category,
            value: config_value(config, spec.key),
            origin,
            tier: spec.tier,
            editable: origin != SettingOrigin::Environment,
            options,
        }
    }

    fn file_keys(&self) -> HashSet<String> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return HashSet::new();
        };
        let Ok(value) = toml::from_str::<toml::Value>(&text) else {
            return HashSet::new();
        };
        value
            .as_table()
            .map(|table| table.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn validate_patch(
        config: &seneschal_common::config::Config,
        patch: &SettingsPatch,
    ) -> (Option<SettingTier>, Vec<FieldError>) {
        let mut errors = Vec::new();
        let mut tier = None;

        let mut check = |field: &'static str, result: Result<(), String>| {
            if let Err(message) = result {
                errors.push(FieldError { field, message });
            }
        };

        let mut consider = |field: &'static str, spec_tier: SettingTier| {
            tier = Some(match tier {
                Some(SettingTier::Restart) => SettingTier::Restart,
                _ => spec_tier,
            });
            let _ = field;
        };

        if let Some(value) = &patch.llm_url {
            consider("llm_url", SettingTier::Restart);
            check("llm_url", validate_url(value));
        }
        if let Some(value) = &patch.llm_model {
            consider("llm_model", SettingTier::Restart);
            check("llm_model", non_empty(value, "LLM model cannot be empty"));
        }
        if let Some(value) = patch.llm_temperature {
            consider("llm_temperature", SettingTier::Restart);
            check(
                "llm_temperature",
                validate_range(value, 0.0, 2.0, "Temperature must be between 0.0 and 2.0"),
            );
        }
        if let Some(value) = patch.llm_max_tokens {
            consider("llm_max_tokens", SettingTier::Restart);
            check(
                "llm_max_tokens",
                validate_min(value as f32, 1.0, "Max tokens must be at least 1"),
            );
        }
        if patch.llm_thinking.is_some() {
            consider("llm_thinking", SettingTier::Restart);
        }

        if let Some(value) = &patch.stt_provider {
            consider("stt_provider", SettingTier::Restart);
            let provider = value.trim().to_lowercase();
            check(
                "stt_provider",
                if stt::supported_providers().contains(&provider.as_str()) {
                    Ok(())
                } else {
                    Err(format!(
                        "STT backend '{provider}' is not available in this build"
                    ))
                },
            );
            if provider == "parakeet" && config.parakeet_model_dir.is_none() {
                check(
                    "parakeet_model_dir",
                    Err("STT backend 'parakeet' requires a model directory".to_string()),
                );
            }
        }
        if let Some(value) = &patch.stt_url {
            consider("stt_url", SettingTier::Restart);
            check("stt_url", validate_url(value));
        }
        if let Some(value) = &patch.stt_model {
            consider("stt_model", SettingTier::Restart);
            check("stt_model", non_empty(value, "STT model cannot be empty"));
        }
        if let Some(value) = &patch.whisper_model {
            consider("whisper_model", SettingTier::Restart);
            check(
                "whisper_model",
                non_empty(value, "Whisper model path cannot be empty"),
            );
        }
        if let Some(value) = &patch.vad_model {
            consider("vad_model", SettingTier::Restart);
            check(
                "vad_model",
                non_empty(value, "VAD model path cannot be empty"),
            );
        }
        if patch.parakeet_model_dir.is_some() {
            consider("parakeet_model_dir", SettingTier::Restart);
            if config.stt_provider == "parakeet" && config.parakeet_model_dir.is_none() {
                check(
                    "parakeet_model_dir",
                    Err("Parakeet requires a model directory".to_string()),
                );
            }
        }

        if let Some(value) = &patch.tts_provider {
            consider("tts_provider", SettingTier::Restart);
            let provider = value.trim().to_lowercase();
            check(
                "tts_provider",
                if TtsEngine::supported_providers().contains(&provider.as_str()) {
                    Ok(())
                } else {
                    Err(format!(
                        "TTS backend '{provider}' is not available in this build"
                    ))
                },
            );
        }
        if let Some(value) = patch.avspeech_rate {
            consider("avspeech_rate", SettingTier::Hot);
            check(
                "avspeech_rate",
                validate_range(value, 0.0, 1.0, "AVSpeech rate must be between 0.0 and 1.0"),
            );
        }
        if let Some(value) = patch.kokoro_speed {
            consider("kokoro_speed", SettingTier::Hot);
            check(
                "kokoro_speed",
                validate_range(value, 0.1, 3.0, "Kokoro speed must be between 0.1 and 3.0"),
            );
        }
        if let Some(value) = patch.openai_tts_speed {
            consider("openai_tts_speed", SettingTier::Hot);
            check(
                "openai_tts_speed",
                validate_range(
                    value,
                    0.0,
                    3.0,
                    "OpenAI TTS speed must be between 0.0 and 3.0",
                ),
            );
        }
        if let Some(value) = &patch.kokoro_voice {
            consider("kokoro_voice", SettingTier::Hot);
            check(
                "kokoro_voice",
                non_empty(value, "Kokoro voice cannot be empty"),
            );
        }
        if let Some(value) = &patch.kokoro_model {
            consider("kokoro_model", SettingTier::Restart);
            check(
                "kokoro_model",
                non_empty(value, "Kokoro model path cannot be empty"),
            );
        }
        if let Some(value) = &patch.kokoro_voices {
            consider("kokoro_voices", SettingTier::Restart);
            check(
                "kokoro_voices",
                non_empty(value, "Kokoro voices path cannot be empty"),
            );
        }
        if let Some(value) = &patch.openai_tts_url {
            consider("openai_tts_url", SettingTier::Restart);
            check("openai_tts_url", validate_url(value));
        }
        if let Some(value) = &patch.openai_tts_voice {
            consider("openai_tts_voice", SettingTier::Hot);
            check(
                "openai_tts_voice",
                non_empty(value, "OpenAI TTS voice cannot be empty"),
            );
        }

        if let Some(value) = &patch.language {
            consider("language", SettingTier::Hot);
            check(
                "language",
                if matches!(value.as_str(), "es" | "en") {
                    Ok(())
                } else {
                    Err("Language must be 'es' or 'en'".to_string())
                },
            );
        }
        if let Some(value) = &patch.wake_word {
            consider("wake_word", SettingTier::Hot);
            if value.trim().is_empty() {
                // Empty is normalized to the historical `seneschal` default.
            }
        }
        if let Some(value) = patch.output_volume {
            consider("output_volume", SettingTier::Hot);
            check(
                "output_volume",
                validate_range(value, 0.0, 1.0, "Volume must be between 0.0 and 1.0"),
            );
        }

        if patch.audio_input_device.is_some() {
            consider("audio_input_device", SettingTier::Restart);
        }
        if patch.audio_output_device.is_some() {
            consider("audio_output_device", SettingTier::Restart);
        }
        if let Some(value) = patch.control_port {
            consider("control_port", SettingTier::Restart);
            if let Some(port) = value {
                check(
                    "control_port",
                    validate_port(port, "Control port must be between 1 and 65535"),
                );
            }
        }
        if let Some(value) = patch.ws_port {
            consider("ws_port", SettingTier::Restart);
            if let Some(port) = value {
                check(
                    "ws_port",
                    validate_port(port, "WebSocket port must be between 1 and 65535"),
                );
            }
        }

        // Also run the full config validation so a patch cannot persist a
        // combination the daemon would refuse at startup.
        if let Err(error) = config.validate() {
            errors.push(FieldError {
                field: "config",
                message: format!("{error:#}"),
            });
        }

        (tier, errors)
    }

    fn changed_keys(
        old: &seneschal_common::config::Config,
        new: &seneschal_common::config::Config,
    ) -> Vec<&'static str> {
        seneschal_common::settings::SETTING_SPECS
            .iter()
            .filter(|spec| config_value(old, spec.key) != config_value(new, spec.key))
            .map(|spec| spec.key)
            .collect()
    }
}

fn apply_patch_to_config(config: &mut seneschal_common::config::Config, patch: &SettingsPatch) {
    if let Some(value) = &patch.llm_url {
        config.llm_url = value.trim().to_string();
    }
    if let Some(value) = &patch.llm_model {
        config.llm_model = value.trim().to_string();
    }
    if let Some(value) = patch.llm_temperature {
        config.llm_temperature = value;
    }
    if let Some(value) = patch.llm_max_tokens {
        config.llm_max_tokens = value;
    }
    if let Some(value) = patch.llm_thinking {
        config.llm_thinking = value;
    }
    if let Some(value) = &patch.stt_provider {
        config.stt_provider = value.trim().to_lowercase();
    }
    if let Some(value) = &patch.stt_url {
        config.stt_url = value.trim().to_string();
    }
    if let Some(value) = &patch.stt_model {
        config.stt_model = value.trim().to_string();
    }
    if let Some(value) = &patch.whisper_model {
        config.whisper_model = value.trim().to_string();
    }
    if let Some(value) = &patch.vad_model {
        config.vad_model = value.trim().to_string();
    }
    if let Some(value) = &patch.parakeet_model_dir {
        config.parakeet_model_dir = value
            .clone()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
    }
    if let Some(value) = &patch.tts_provider {
        config.tts_provider = value.trim().to_lowercase();
    }
    if let Some(value) = &patch.avspeech_voice {
        config.avspeech_voice = value.trim().to_string();
    }
    if let Some(value) = patch.avspeech_rate {
        config.avspeech_rate = value;
    }
    if let Some(value) = &patch.kokoro_voice {
        config.kokoro_voice = value.trim().to_string();
    }
    if let Some(value) = &patch.kokoro_model {
        config.kokoro_model = value.trim().to_string();
    }
    if let Some(value) = &patch.kokoro_voices {
        config.kokoro_voices = value.trim().to_string();
    }
    if let Some(value) = patch.kokoro_speed {
        config.kokoro_speed = value;
    }
    if let Some(value) = &patch.openai_tts_url {
        config.openai_tts_url = value.trim().to_string();
    }
    if let Some(value) = &patch.openai_tts_voice {
        config.openai_tts_voice = value.trim().to_string();
    }
    if let Some(value) = patch.openai_tts_speed {
        config.openai_tts_speed = value;
    }
    if let Some(value) = &patch.language {
        config.language = value.trim().to_string();
    }
    if let Some(value) = &patch.wake_word {
        let value = value.trim();
        config.wake_word = if value.is_empty() {
            "seneschal".to_string()
        } else {
            value.to_string()
        };
    }
    if let Some(value) = patch.output_volume {
        config.output_volume = value;
    }
    if let Some(value) = &patch.audio_input_device {
        config.audio_input_device = value
            .clone()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
    }
    if let Some(value) = &patch.audio_output_device {
        config.audio_output_device = value
            .clone()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
    }
    if let Some(value) = patch.control_port {
        config.control_port = value;
    }
    if let Some(value) = patch.ws_port {
        config.ws_port = value;
    }
}

fn config_value(config: &seneschal_common::config::Config, key: &str) -> serde_json::Value {
    match key {
        "llm_url" => serde_json::json!(config.llm_url),
        "llm_model" => serde_json::json!(config.llm_model),
        "llm_temperature" => serde_json::json!(config.llm_temperature),
        "llm_max_tokens" => serde_json::json!(config.llm_max_tokens),
        "llm_thinking" => serde_json::json!(config.llm_thinking),
        "stt_provider" => serde_json::json!(config.stt_provider),
        "stt_url" => serde_json::json!(config.stt_url),
        "stt_model" => serde_json::json!(config.stt_model),
        "whisper_model" => serde_json::json!(config.whisper_model),
        "vad_model" => serde_json::json!(config.vad_model),
        "parakeet_model_dir" => serde_json::json!(config.parakeet_model_dir),
        "tts_provider" => serde_json::json!(config.tts_provider),
        "avspeech_voice" => serde_json::json!(config.avspeech_voice),
        "avspeech_rate" => serde_json::json!(config.avspeech_rate),
        "kokoro_voice" => serde_json::json!(config.kokoro_voice),
        "kokoro_model" => serde_json::json!(config.kokoro_model),
        "kokoro_voices" => serde_json::json!(config.kokoro_voices),
        "kokoro_speed" => serde_json::json!(config.kokoro_speed),
        "openai_tts_url" => serde_json::json!(config.openai_tts_url),
        "openai_tts_voice" => serde_json::json!(config.openai_tts_voice),
        "openai_tts_speed" => serde_json::json!(config.openai_tts_speed),
        "language" => serde_json::json!(config.language),
        "wake_word" => serde_json::json!(config.wake_word),
        "output_volume" => serde_json::json!(config.output_volume),
        "audio_input_device" => serde_json::json!(config.audio_input_device),
        "audio_output_device" => serde_json::json!(config.audio_output_device),
        "control_port" => serde_json::json!(config.control_port),
        "ws_port" => serde_json::json!(config.ws_port),
        _ => serde_json::Value::Null,
    }
}

fn validate_url(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("URL cannot be empty".to_string());
    }
    if value.starts_with("http://") || value.starts_with("https://") {
        Ok(())
    } else {
        Err("URL must start with http:// or https://".to_string())
    }
}

fn non_empty(value: &str, message: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(message.to_string())
    } else {
        Ok(())
    }
}

fn validate_range(value: f32, min: f32, max: f32, message: &str) -> Result<(), String> {
    if value.is_finite() && value >= min && value <= max {
        Ok(())
    } else {
        Err(message.to_string())
    }
}

fn validate_min(value: f32, min: f32, message: &str) -> Result<(), String> {
    if value.is_finite() && value >= min {
        Ok(())
    } else {
        Err(message.to_string())
    }
}

fn validate_port(value: u16, message: &str) -> Result<(), String> {
    if value > 0 {
        Ok(())
    } else {
        Err(message.to_string())
    }
}

fn toml_f32_value(value: f32) -> toml_edit::Item {
    // `f32 -> f64` would otherwise emit noisy binary conversions such as
    // `1.399999976158142`; TOML is user-facing, so preserve the decimal input.
    let value = format!("{value}").parse::<f64>().unwrap_or(value as f64);
    toml_edit::value(value)
}

fn write_patch(path: &Path, patch: &SettingsPatch, changed: &[&'static str]) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut document = text
        .parse::<toml_edit::DocumentMut>()
        .context("Failed to parse the active TOML config")?;

    for key in changed {
        if setting_spec(key).is_none() {
            continue;
        }
        match *key {
            "llm_url" => document["llm_url"] = toml_edit::value(patch.llm_url.clone().unwrap()),
            "llm_model" => {
                document["llm_model"] = toml_edit::value(patch.llm_model.clone().unwrap())
            }
            "llm_temperature" => {
                document["llm_temperature"] = toml_f32_value(patch.llm_temperature.unwrap())
            }
            "llm_max_tokens" => {
                document["llm_max_tokens"] = toml_edit::value(patch.llm_max_tokens.unwrap() as i64)
            }
            "llm_thinking" => {
                document["llm_thinking"] = toml_edit::value(patch.llm_thinking.unwrap())
            }
            "stt_provider" => {
                document["stt_provider"] = toml_edit::value(patch.stt_provider.clone().unwrap())
            }
            "stt_url" => document["stt_url"] = toml_edit::value(patch.stt_url.clone().unwrap()),
            "stt_model" => {
                document["stt_model"] = toml_edit::value(patch.stt_model.clone().unwrap())
            }
            "whisper_model" => {
                document["whisper_model"] = toml_edit::value(patch.whisper_model.clone().unwrap())
            }
            "parakeet_model_dir" => set_optional_string(
                &mut document,
                "parakeet_model_dir",
                patch.parakeet_model_dir.clone(),
            ),
            "tts_provider" => {
                document["tts_provider"] = toml_edit::value(patch.tts_provider.clone().unwrap())
            }
            "avspeech_voice" => {
                document["avspeech_voice"] = toml_edit::value(patch.avspeech_voice.clone().unwrap())
            }
            "avspeech_rate" => {
                document["avspeech_rate"] = toml_f32_value(patch.avspeech_rate.unwrap())
            }
            "kokoro_voice" => {
                document["kokoro_voice"] = toml_edit::value(patch.kokoro_voice.clone().unwrap())
            }
            "kokoro_speed" => {
                document["kokoro_speed"] = toml_f32_value(patch.kokoro_speed.unwrap())
            }
            "openai_tts_url" => {
                document["openai_tts_url"] = toml_edit::value(patch.openai_tts_url.clone().unwrap())
            }
            "openai_tts_voice" => {
                document["openai_tts_voice"] =
                    toml_edit::value(patch.openai_tts_voice.clone().unwrap())
            }
            "openai_tts_speed" => {
                document["openai_tts_speed"] = toml_f32_value(patch.openai_tts_speed.unwrap())
            }
            "language" => document["language"] = toml_edit::value(patch.language.clone().unwrap()),
            "wake_word" => {
                document["wake_word"] = toml_edit::value(patch.wake_word.clone().unwrap())
            }
            "output_volume" => {
                document["output_volume"] = toml_f32_value(patch.output_volume.unwrap())
            }
            "audio_input_device" => set_optional_string(
                &mut document,
                "audio_input_device",
                patch.audio_input_device.clone(),
            ),
            "audio_output_device" => set_optional_string(
                &mut document,
                "audio_output_device",
                patch.audio_output_device.clone(),
            ),
            "control_port" => set_optional_u16(&mut document, "control_port", patch.control_port),
            "ws_port" => set_optional_u16(&mut document, "ws_port", patch.ws_port),
            _ => {}
        }
    }

    write_atomic(path, &document.to_string())
}

fn set_optional_string(
    document: &mut toml_edit::DocumentMut,
    key: &'static str,
    value: Option<Option<String>>,
) {
    match value {
        Some(Some(value)) => document[key] = toml_edit::value(value),
        Some(None) => {
            document.as_table_mut().remove(key);
        }
        None => {}
    }
}

fn set_optional_u16(
    document: &mut toml_edit::DocumentMut,
    key: &'static str,
    value: Option<Option<u16>>,
) {
    match value {
        Some(Some(value)) => document[key] = toml_edit::value(value as i64),
        Some(None) => {
            document.as_table_mut().remove(key);
        }
        None => {}
    }
}

fn write_atomic(path: &Path, contents: &str) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_string());
    let temp = parent.join(format!(".{file_name}.tmp.{}", std::process::id()));
    std::fs::write(&temp, contents)
        .with_context(|| format!("Failed to write {}", temp.display()))?;
    if let Ok(metadata) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&temp, metadata.permissions());
    }
    std::fs::rename(&temp, path)
        .with_context(|| format!("Failed to replace {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use seneschal_common::settings::SettingsPatch;
    use seneschal_core::tts::mock_tts::MockTts;
    use serde_json::json;

    /// Tests that touch `SENESCHAL_CONFIG_FILE` must serialize: temp-env is
    /// not thread-safe across parallel tests.
    pub(crate) fn config_env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    type ServiceHandles = (
        Arc<SettingsService>,
        SharedTtsEngine,
        Arc<AudioOutput>,
        Arc<std::sync::Mutex<HotSettings>>,
        mpsc::Receiver<HotSettings>,
    );

    fn service_with_file(path: &Path) -> ServiceHandles {
        service_with_config(path, seneschal_common::config::Config::embedded_defaults())
    }

    fn service_with_config(
        _path: &Path,
        config: seneschal_common::config::Config,
    ) -> ServiceHandles {
        let (hot_tx, hot_rx) = mpsc::channel(4);
        let tts: SharedTtsEngine = Arc::new(RwLock::new(TtsEngine::Mock(MockTts::new().0)));
        let output = Arc::new(AudioOutput::null());
        let hot = Arc::new(std::sync::Mutex::new(HotSettings {
            language: "es".to_string(),
            wake_word: "seneschal".to_string(),
        }));
        let service = SettingsService::new(
            config,
            Arc::clone(&tts),
            Arc::clone(&output),
            Arc::clone(&hot),
            hot_tx,
        )
        .unwrap();
        (Arc::new(service), tts, output, hot, hot_rx)
    }

    #[test]
    fn hot_apply_persists_updates_engine_volume_and_main_loop_without_restart() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(
            &path,
            "# user comment\nunknown_key = true\nkokoro_voice = \"af_bella\"\n",
        )
        .unwrap();
        let path_str = path.to_string_lossy().into_owned();
        let _guard = config_env_lock().lock().unwrap();
        temp_env::with_vars([("SENECHAL_CONFIG_FILE", Some(path_str.as_str()))], || {
            let runtime = runtime();
            runtime.block_on(async {
                let (service, _tts, output, hot, mut hot_rx) = service_with_file(&path);
                let patch: SettingsPatch = serde_json::from_value(json!({
                    "kokoro_voice": "ef_dora",
                    "kokoro_speed": 1.4,
                    "language": "en",
                    "wake_word": "jardis",
                    "output_volume": 0.25
                }))
                .unwrap();

                let response = service.apply(&patch).await.unwrap();
                assert_eq!(response.tier, SettingTier::Hot);

                let config = service.config.read().await;
                assert_eq!(config.kokoro_voice, "ef_dora");
                assert_eq!(config.kokoro_speed, 1.4);
                assert_eq!(config.language, "en");
                assert_eq!(config.wake_word, "jardis");
                drop(config);

                assert_eq!(output.volume(), 0.25);
                let applied = hot.lock().unwrap().clone();
                assert_eq!(applied.language, "en");
                assert_eq!(applied.wake_word, "jardis");
                assert_eq!(hot_rx.recv().await.unwrap().wake_word, "jardis");

                let persisted = std::fs::read_to_string(&path).unwrap();
                assert!(persisted.contains("# user comment"));
                assert!(persisted.contains("unknown_key = true"));
                assert!(persisted.contains("kokoro_voice = \"ef_dora\""));
                assert!(
                    persisted.contains("kokoro_speed = 1.4"),
                    "persisted TOML was:\n{persisted}"
                );
                assert!(persisted.contains("language = \"en\""));
            });
        });
    }

    #[test]
    fn invalid_patch_is_rejected_without_writing_the_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("settings.toml");
        let original = "# unchanged\noutput_volume = 0.5\n";
        std::fs::write(&path, original).unwrap();
        let path_str = path.to_string_lossy().into_owned();
        let _guard = config_env_lock().lock().unwrap();
        temp_env::with_vars([("SENECHAL_CONFIG_FILE", Some(path_str.as_str()))], || {
            let runtime = runtime();
            runtime.block_on(async {
                let (service, _tts, _output, _hot, _rx) = service_with_file(&path);
                let patch: SettingsPatch =
                    serde_json::from_value(json!({ "output_volume": 2.0 })).unwrap();
                let errors = service.apply(&patch).await.unwrap_err();
                assert!(errors.iter().any(|error| error.field == "output_volume"));
                assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            });
        });
    }

    #[test]
    fn environment_override_is_reported_and_not_editable() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "kokoro_voice = \"af_bella\"\n").unwrap();
        let path_str = path.to_string_lossy().into_owned();
        let _guard = config_env_lock().lock().unwrap();
        temp_env::with_vars(
            [
                ("SENECHAL_CONFIG_FILE", Some(path_str.as_str())),
                ("KOKORO_VOICE", Some("ef_dora")),
            ],
            || {
                let runtime = runtime();
                runtime.block_on(async {
                    let mut config = seneschal_common::config::Config::embedded_defaults();
                    config.kokoro_voice = "ef_dora".to_string();
                    let (service, _tts, _output, _hot, _rx) = service_with_config(&path, config);
                    let snapshot = service.snapshot().await;
                    let field = snapshot
                        .fields
                        .iter()
                        .find(|field| field.key == "kokoro_voice")
                        .unwrap();
                    assert_eq!(field.origin, SettingOrigin::Environment);
                    assert_eq!(field.value, json!("ef_dora"));
                    assert!(!field.editable);
                });
            },
        );
    }
}
