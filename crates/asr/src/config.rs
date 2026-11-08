//! Runtime configuration for the Nemotron ASR WebSocket provider.
//!
//! The provider deliberately reuses the remote-STT keys (`STT_URL`,
//! `STT_MODEL`, `STT_API_KEY`, `STT_LANGUAGE`) so existing deployments only
//! change the URL/model. `VAD_SILENCE_MS` is forwarded as the server's
//! endpointing threshold, preserving the old turn-close timing knob.

use anyhow::Result;
use std::time::Duration;

use seneschal_common::config::Config;

/// Wire sample rate accepted by the shipped Nemotron models.
pub const SAMPLE_RATE: usize = 16_000;

#[derive(Debug, Clone)]
pub struct AsrConfig {
    /// Service base URL, e.g. `http://localhost:8026`. Converted to
    /// `ws://.../v1/audio/transcriptions/realtime`.
    pub base_url: String,
    /// Bearer token; empty = no `Authorization` header.
    pub api_key: String,
    /// Model id from `GET /v1/models` (e.g. `nemotron-3.5`).
    pub model: String,
    /// Language prompt: `auto`, a locale (`es-ES`) or a bare code (`es`).
    pub language: String,
    /// Trailing silence that closes a turn (server-side endpointing).
    pub endpointing_ms: u32,
    /// TCP/WebSocket connect timeout.
    pub connect_timeout: Duration,
    /// HTTP timeout for `verify_server` / one-shot REST transcription.
    pub http_timeout: Duration,
}

impl AsrConfig {
    /// Build from the shared runtime configuration.
    pub fn from_config(config: &Config) -> Result<Self> {
        let base_url = config.stt_url.trim().to_string();
        if base_url.is_empty() {
            anyhow::bail!("STT_URL must not be empty for the Nemotron ASR provider");
        }
        let model = config.stt_model.trim().to_string();
        if model.is_empty() {
            anyhow::bail!("STT_MODEL must not be empty for the Nemotron ASR provider");
        }

        Ok(Self {
            base_url,
            api_key: config.stt_api_key.trim().to_string(),
            model,
            language: if config.stt_language.trim().is_empty() {
                "auto".to_string()
            } else {
                config.stt_language.trim().to_string()
            },
            endpointing_ms: config.vad_silence_ms,
            connect_timeout: Duration::from_secs(5),
            http_timeout: Duration::from_millis(config.stt_timeout_ms.max(1_000)),
        })
    }

    /// URL of the realtime transcription WebSocket.
    pub fn websocket_url(&self) -> String {
        websocket_url(&self.base_url)
    }
}

/// Convert a service base URL into the realtime transcription WebSocket URL.
///
/// `http` becomes `ws`, `https` becomes `wss`; an already-ws/wss URL is
/// accepted unchanged. A trailing slash is normalized before appending the
/// documented realtime route.
pub fn websocket_url(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    let scheme = if let Some(rest) = base
        .strip_prefix("https://")
        .or_else(|| base.strip_prefix("wss://"))
    {
        format!("wss://{rest}")
    } else if let Some(rest) = base
        .strip_prefix("http://")
        .or_else(|| base.strip_prefix("ws://"))
    {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{scheme}/v1/audio/transcriptions/realtime")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_service_base_urls_to_realtime_websockets() {
        assert_eq!(
            websocket_url("http://localhost:8026"),
            "ws://localhost:8026/v1/audio/transcriptions/realtime"
        );
        assert_eq!(
            websocket_url("http://localhost:8026/"),
            "ws://localhost:8026/v1/audio/transcriptions/realtime"
        );
        assert_eq!(
            websocket_url("https://asr.example.com"),
            "wss://asr.example.com/v1/audio/transcriptions/realtime"
        );
        assert_eq!(
            websocket_url("ws://localhost:8026"),
            "ws://localhost:8026/v1/audio/transcriptions/realtime"
        );
        assert_eq!(
            websocket_url("wss://asr.example.com/"),
            "wss://asr.example.com/v1/audio/transcriptions/realtime"
        );
    }

    #[test]
    fn config_uses_shared_stt_settings() {
        let config = Config::from_env().expect("default config must parse");
        let asr = AsrConfig::from_config(&config).expect("ASR config must build");
        assert_eq!(asr.model, config.stt_model);
        assert_eq!(asr.endpointing_ms, config.vad_silence_ms);
        assert_eq!(asr.language, config.stt_language);
        assert_eq!(asr.websocket_url(), websocket_url(&config.stt_url));
    }

    #[test]
    fn empty_url_or_model_is_rejected() {
        let mut config = Config::from_env().unwrap();
        config.stt_url = "  ".into();
        assert!(AsrConfig::from_config(&config).is_err());

        let mut config = Config::from_env().unwrap();
        config.stt_model = "".into();
        assert!(AsrConfig::from_config(&config).is_err());
    }
}
