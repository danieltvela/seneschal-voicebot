//! Settings contract for the desktop Settings UI (issue #33).
//!
//! The Control API deliberately exposes a small, secret-free subset of `Config`
//! instead of serializing the full struct. Every exposed field carries its
//! effective origin (`default` / `file` / `environment`) and apply tier so the
//! UI can render readable validation errors and explain why a value cannot be
//! changed while an env override is active.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::config::{Config, SeneschalEnv};

/// Runtime-switchable conversation settings shared with the main audio loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotSettings {
    pub language: String,
    pub wake_word: String,
}

/// A sparse settings update. `None` means "leave unchanged"; the nested
/// `Option` fields use `Some(None)` to clear an optional value.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_thinking: Option<bool>,
    /// Write-only credential for the OpenAI-compatible LLM endpoint (issue #36).
    /// Never serialized in the Settings snapshot. `Some("")` clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_api_key: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stt_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stt_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stt_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whisper_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parakeet_model_dir: Option<Option<String>>,
    /// Path to the Silero VAD model used by the local STT providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vad_model: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tts_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avspeech_voice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avspeech_rate: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kokoro_voice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kokoro_speed: Option<f32>,
    /// Path to the Kokoro ONNX model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kokoro_model: Option<String>,
    /// Path to the Kokoro voices bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kokoro_voices: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_tts_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_tts_voice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_tts_speed: Option<f32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_word: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_volume: Option<f32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_input_device: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_output_device: Option<Option<String>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_port: Option<Option<u16>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ws_port: Option<Option<u16>>,
}

/// One Settings field on the wire.
#[derive(Debug, Clone, Serialize)]
pub struct SettingsFieldWire {
    pub key: &'static str,
    pub label: &'static str,
    pub category: &'static str,
    pub value: serde_json::Value,
    pub origin: SettingOrigin,
    pub tier: SettingTier,
    pub editable: bool,
    /// Stable values for select fields; empty for free-form fields.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingOrigin {
    Default,
    File,
    Environment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingTier {
    Hot,
    Restart,
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingsProviders {
    pub stt: Vec<&'static str>,
    pub tts: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingsSnapshot {
    pub revision: u64,
    pub config_file: Option<String>,
    pub fields: Vec<SettingsFieldWire>,
    pub providers: SettingsProviders,
}

/// Readable field-level validation failure.
#[derive(Debug, Clone, Serialize)]
pub struct FieldError {
    pub field: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidationResponse {
    pub ok: bool,
    pub errors: Vec<FieldError>,
    pub tier: Option<SettingTier>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingsApplyResponse {
    pub tier: SettingTier,
    pub revision: u64,
    pub snapshot: SettingsSnapshot,
}

#[derive(Debug, Clone, Copy)]
pub struct SettingSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub category: &'static str,
    pub env_var: &'static str,
    pub tier: SettingTier,
}

pub const SETTING_SPECS: &[SettingSpec] = &[
    SettingSpec {
        key: "llm_url",
        label: "LLM URL",
        category: "llm",
        env_var: "LLM_URL",
        tier: SettingTier::Restart,
    },
    SettingSpec {
        key: "llm_model",
        label: "LLM model",
        category: "llm",
        env_var: "LLM_MODEL",
        tier: SettingTier::Restart,
    },
    SettingSpec {
        key: "llm_temperature",
        label: "Temperature",
        category: "llm",
        env_var: "LLM_TEMPERATURE",
        tier: SettingTier::Restart,
    },
    SettingSpec {
        key: "llm_max_tokens",
        label: "Max tokens",
        category: "llm",
        env_var: "LLM_MAX_TOKENS",
        tier: SettingTier::Restart,
    },
    SettingSpec {
        key: "llm_thinking",
        label: "Thinking",
        category: "llm",
        env_var: "LLM_THINKING",
        tier: SettingTier::Restart,
    },
    SettingSpec {
        key: "stt_provider",
        label: "STT backend",
        category: "stt",
        env_var: "STT_PROVIDER",
        tier: SettingTier::Restart,
    },
    SettingSpec {
        key: "stt_url",
        label: "STT URL",
        category: "stt",
        env_var: "STT_URL",
        tier: SettingTier::Restart,
    },
    SettingSpec {
        key: "stt_model",
        label: "STT model",
        category: "stt",
        env_var: "STT_MODEL",
        tier: SettingTier::Restart,
    },
    SettingSpec {
        key: "whisper_model",
        label: "Whisper model",
        category: "stt",
        env_var: "WHISPER_MODEL",
        tier: SettingTier::Restart,
    },
    SettingSpec {
        key: "vad_model",
        label: "VAD model",
        category: "stt",
        env_var: "VAD_MODEL",
        tier: SettingTier::Restart,
    },
    SettingSpec {
        key: "parakeet_model_dir",
        label: "Parakeet model dir",
        category: "stt",
        env_var: "PARAKEET_MODEL_DIR",
        tier: SettingTier::Restart,
    },
    SettingSpec {
        key: "tts_provider",
        label: "TTS backend",
        category: "tts",
        env_var: "TTS_PROVIDER",
        tier: SettingTier::Restart,
    },
    SettingSpec {
        key: "avspeech_voice",
        label: "AVSpeech voice",
        category: "tts",
        env_var: "AVSPEECH_VOICE",
        tier: SettingTier::Hot,
    },
    SettingSpec {
        key: "avspeech_rate",
        label: "AVSpeech rate",
        category: "tts",
        env_var: "AVSPEECH_RATE",
        tier: SettingTier::Hot,
    },
    SettingSpec {
        key: "kokoro_voice",
        label: "Kokoro voice",
        category: "tts",
        env_var: "KOKORO_VOICE",
        tier: SettingTier::Hot,
    },
    SettingSpec {
        key: "kokoro_model",
        label: "Kokoro model",
        category: "tts",
        env_var: "KOKORO_MODEL",
        tier: SettingTier::Restart,
    },
    SettingSpec {
        key: "kokoro_voices",
        label: "Kokoro voices",
        category: "tts",
        env_var: "KOKORO_VOICES",
        tier: SettingTier::Restart,
    },
    SettingSpec {
        key: "kokoro_speed",
        label: "Kokoro speed",
        category: "tts",
        env_var: "KOKORO_SPEED",
        tier: SettingTier::Hot,
    },
    SettingSpec {
        key: "openai_tts_url",
        label: "OpenAI TTS URL",
        category: "tts",
        env_var: "OPENAI_TTS_URL",
        tier: SettingTier::Restart,
    },
    SettingSpec {
        key: "openai_tts_voice",
        label: "OpenAI TTS voice",
        category: "tts",
        env_var: "OPENAI_TTS_VOICE",
        tier: SettingTier::Hot,
    },
    SettingSpec {
        key: "openai_tts_speed",
        label: "OpenAI TTS speed",
        category: "tts",
        env_var: "OPENAI_TTS_SPEED",
        tier: SettingTier::Hot,
    },
    SettingSpec {
        key: "language",
        label: "Language",
        category: "general",
        env_var: "SENECHAL_LANGUAGE",
        tier: SettingTier::Hot,
    },
    SettingSpec {
        key: "wake_word",
        label: "Wake word",
        category: "general",
        env_var: "WAKE_WORD",
        tier: SettingTier::Hot,
    },
    SettingSpec {
        key: "output_volume",
        label: "Output volume",
        category: "general",
        env_var: "AUDIO_OUTPUT_VOLUME",
        tier: SettingTier::Hot,
    },
    SettingSpec {
        key: "audio_input_device",
        label: "Input device",
        category: "audio",
        env_var: "AUDIO_INPUT_DEVICE",
        tier: SettingTier::Restart,
    },
    SettingSpec {
        key: "audio_output_device",
        label: "Output device",
        category: "audio",
        env_var: "AUDIO_OUTPUT_DEVICE",
        tier: SettingTier::Restart,
    },
    SettingSpec {
        key: "control_port",
        label: "Control port",
        category: "network",
        env_var: "CONTROL_PORT",
        tier: SettingTier::Restart,
    },
    SettingSpec {
        key: "ws_port",
        label: "WebSocket port",
        category: "network",
        env_var: "WS_PORT",
        tier: SettingTier::Restart,
    },
];

pub fn setting_spec(key: &str) -> Option<&'static SettingSpec> {
    SETTING_SPECS.iter().find(|spec| spec.key == key)
}

/// Resolve the TOML file Settings writes to, respecting `SENESCHAL_CONFIG_FILE`.
/// When no file exists yet, the env-specific path in the daemon workdir is used.
pub fn active_config_path() -> Result<PathBuf, anyhow::Error> {
    if let Some(path) = Config::resolve_config_path()? {
        return Ok(path);
    }
    let env = SeneschalEnv::from_env_var();
    Ok(PathBuf::from(format!("seneschal.{}.toml", env.as_str())))
}
