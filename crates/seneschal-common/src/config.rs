use anyhow::{Context, Result};
use serde::Deserialize;
use std::env;
use std::path::PathBuf;
use tracing::warn;

/// Embedded default configuration. Keep `seneschal.pro.toml` in sync with this constant.
const DEFAULT_CONFIG_TOML: &str = include_str!("../../../seneschal.pro.toml");

// ── Shared config sub-structs (moved from agents::config and mcp::config) ──────

/// Agent configuration loaded from TOML.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentTomlConfig {
    pub name: String,
    pub mode: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub acp_command: Option<String>,
    #[serde(default)]
    pub acp_warmup: bool,
    #[serde(default)]
    pub remote_url: String,
    #[serde(default)]
    pub remote_dir: String,
    #[serde(default)]
    pub remote_session_path: String,
    #[serde(default)]
    pub remote_message_path: String,
    #[serde(default)]
    pub remote_event_path: String,
    #[serde(default)]
    pub remote_api_key: String,
    #[serde(default)]
    pub when_to_use: String,
    #[serde(default)]
    pub instructions: String,
}

/// MCP server configuration loaded from TOML.
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerTomlConfig {
    pub name: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default = "default_mcp_tool_timeout")]
    pub tool_timeout_secs: u64,
}

fn default_mcp_tool_timeout() -> u64 {
    30
}

/// Mode for the Hermes ACP session log viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HermesSessionViewerMode {
    #[default]
    Off,
    LogFile,
}

impl std::str::FromStr for HermesSessionViewerMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "off" | "0" | "false" => Ok(Self::Off),
            "logfile" | "log-file" | "log" => Ok(Self::LogFile),
            _ => Err(format!("Invalid HermesSessionViewerMode: {s}")),
        }
    }
}

/// Active Seneschal runtime environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeneschalEnv {
    #[default]
    Pro,
    Dev,
}

impl SeneschalEnv {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pro => "pro",
            Self::Dev => "dev",
        }
    }

    pub fn from_env_var() -> Self {
        match env::var("SENECHAL_ENV") {
            Ok(v) => v.parse().unwrap_or_else(|e| {
                warn!(
                    "Invalid SENECHAL_ENV value '{}': {}; defaulting to pro",
                    v, e
                );
                Self::Pro
            }),
            Err(_) => Self::Pro,
        }
    }
}

impl std::str::FromStr for SeneschalEnv {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pro" | "production" => Ok(Self::Pro),
            "dev" | "development" => Ok(Self::Dev),
            _ => Err(format!(
                "Invalid SeneschalEnv: {s}. Expected 'pro' or 'dev'"
            )),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Config {
    // ── Audio input ──────────────────────────────────────────────────────────
    /// Microphone sample rate (default 16000 — required by Silero VAD)
    pub sample_rate: u32,
    pub channels: u16,
    pub chunk_ms: u32,
    pub audio_input_device: Option<String>,
    pub audio_output_device: Option<String>,
    pub list_devices: bool,
    pub list_voices: bool,
    pub device_monitor_enabled: bool,
    pub device_monitor_poll_secs: u64,

    // ── VAD ───────────────────────────────────────────────────────────────────
    /// Milliseconds of continuous silence before SpeechEnd fires.
    /// Lower = faster response; higher = fewer false cuts mid-sentence.
    /// With STT_PROVIDER=speech this also controls the short-utterance silence window:
    /// unconfirmed speech followed by this much silence is still fed to STT.
    pub vad_silence_ms: u32,
    /// Speech probability threshold to start a segment (silence -> speech).
    pub vad_start_threshold: f32,
    /// Speech probability threshold to keep a segment open (speech -> silence below this).
    pub vad_end_threshold: f32,
    /// Path to Silero VAD model file (.bin) used by whisper-cpp-plus
    pub vad_model: String,
    /// Minimum consecutive speech probes required before VAD *commits* to full STT.
    /// With STT_PROVIDER=speech, probes are 100ms each; SpeechStart (barge-in / LISTENING)
    /// fires at commit (or short-utterance finalize), not on the first noisy probe.
    /// Utterances below this count are still transcribed via the short-utterance fallback
    /// (so short answers like "yes"/"Vale" work). Coughs/noise are rejected by NoSpeechGate.
    /// Default: 2 (~200ms sustained speech-like signal).
    pub vad_confirm_probes: usize,

    // ── Language ─────────────────────────────────────────────────────────────
    /// "es" (default) or "en"
    pub language: String,

    // ── STT ──────────────────────────────────────────────────────────────────
    /// STT backend provider: "speech" (default on macOS), "whisper", or "parakeet".
    pub stt_provider: String,
    /// Path to whisper.cpp GGML model file (.bin)
    pub whisper_model: String,
    /// Path to Parakeet model directory (required when STT_PROVIDER=parakeet).
    pub parakeet_model_dir: Option<String>,
    /// Number of CPU threads for Whisper decoding (0 = auto).
    /// Set to physical core count for best throughput.
    pub whisper_threads: u32,
    /// A1 Conservative Early Reuse: enable checking for ready speculative results
    /// before waiting for the final complete decode. Default: false (disabled).
    pub stt_early_reuse_enabled: bool,
    /// A1 Conservative Early Reuse: minimum tokens to consider an early result valid.
    /// Results shorter than this are ignored and we wait for the final decode.
    /// Default: 6 tokens.
    pub stt_early_min_tokens: usize,
    /// A1 Conservative Early Reuse: require terminal punctuation (. ! ?) on early
    /// results. If true, incomplete sentences are not reused even if they meet
    /// the token threshold. Default: true.
    pub stt_early_require_punctuation: bool,
    /// NoSpeechGate: no_speech_prob threshold (default 0.6).
    /// Above this + low logprob → reject. (STT_NO_SPEECH_THRESHOLD)
    pub stt_no_speech_threshold: f32,
    /// NoSpeechGate: avg_logprob threshold (default -1.0).
    /// Below this + high no_speech → reject. (STT_LOGPROB_THRESHOLD)
    pub stt_logprob_threshold: f32,
    /// NoSpeechGate: compression_ratio threshold (default 2.4).
    /// Above this → gibberish → reject. (STT_COMPRESSION_THRESHOLD)
    pub stt_compression_threshold: f32,

    // ── LLM ──────────────────────────────────────────────────────────────────
    /// LLM backend provider: "openai" (default).
    pub llm_provider: String,
    /// LLM server base URL (OpenAI-compatible, default http://127.0.0.1:8000 for mlx-lm)
    pub llm_url: String,
    /// API key sent as `Authorization: Bearer <key>`. Empty = no auth header.
    pub llm_api_key: String,
    /// Model name sent in the `model` field of API requests
    pub llm_model: String,
    /// Max tokens per response
    pub llm_max_tokens: u32,
    pub llm_system_prompt: String,
    pub llm_temperature: f32,
    /// Enable Qwen3 thinking mode on the main LLM (LLM_THINKING, default false).
    /// When true, `chat_template_kwargs: {"enable_thinking": true}` is sent and
    /// `<think>…</think>` blocks are stripped from the output.
    pub llm_thinking: bool,
    /// SIMPLE-mode temperature (LLM_TEMPERATURE_SIMPLE, default 0.8).
    pub llm_temperature_simple: f32,
    /// COMPLEX-mode temperature (LLM_TEMPERATURE_COMPLEX, default = llm_temperature).
    pub llm_temperature_complex: f32,
    /// SIMPLE-mode thinking (LLM_THINKING_SIMPLE, default false).
    pub llm_thinking_simple: bool,
    /// COMPLEX-mode thinking (LLM_THINKING_COMPLEX, default = llm_thinking).
    pub llm_thinking_complex: bool,
    /// Keywords (CSV) that mark a request as COMPLEX.
    /// Empty → use DEFAULT_COMPLEX_KEYWORDS from the classifier module.
    pub llm_complex_keywords: Vec<String>,

    // ── Intent Classifier (cascada C01) ───────────────────────────────
    /// Umbral de confianza [0.0,1.0]: si un nivel baja de éste, la cascada continúa.
    pub classifier_confidence_threshold: f32,
    /// Activar Niveles 2/3 (embeddings) — requiere feature `classifier-embedding`.
    pub classifier_enable_embedding: bool,
    /// Ruta al modelo de embeddings ONNX (dir o .onnx). Vacío → skip Nivel 2.
    pub classifier_model_path: String,
    /// Ruta al fichero JSON con centroides de referencia por categoría.
    pub classifier_centroids_path: String,
    /// Ruta al fichero JSON con pesos de la regresión logística (Nivel 3).
    pub classifier_weights_path: String,
    /// Activar Nivel 4 (SLM fallback). Si false, la cascada termina en último nivel.
    pub classifier_enable_fallback: bool,
    /// URL base del segundo endpoint LLM (SLM en vLLM) para el fallback.
    pub classifier_fallback_url: String,
    /// Modelo del SLM fallback (campo `model` del payload).
    pub classifier_fallback_model: String,
    /// API key del SLM fallback (Bearer; vacío = sin auth).
    pub classifier_fallback_api_key: String,
    /// Timeout en ms del fallback SLM.
    pub classifier_fallback_timeout_ms: u64,
    /// "Tools Strict": en SIMPLE fuerza `tool_choice: "none"` explícito.
    pub llm_tools_strict: bool,

    // ── TTS ──────────────────────────────────────────────────────────────────
    /// TTS backend: "avspeech" (default, native AVSpeechSynthesizer, --features avspeech)
    /// or "kokoro" (--features kokoro).
    pub tts_provider: String,
    /// AVSpeechSynthesizer voice display name (AVSPEECH_VOICE, default "Jorge (Enhanced)").
    pub avspeech_voice: String,
    /// AVSpeechSynthesizer normalized speech rate 0.0–1.0 (AVSPEECH_RATE, default 0.55).
    /// AVSpeechUtteranceDefaultSpeechRate (0.5) ≈ 180 wpm; 0.55 ≈ 215 wpm.
    pub avspeech_rate: f32,
    /// Path to kokoro-v1.0.onnx model file (KOKORO_MODEL)
    pub kokoro_model: String,
    /// Path to voices-v1.0.bin embeddings file (KOKORO_VOICES)
    pub kokoro_voices: String,
    /// Kokoro voice style name, e.g. "af_bella" or "es_*" (KOKORO_VOICE)
    pub kokoro_voice: String,
    /// BCP-47 language code for espeak-ng, e.g. "en-us" or "es" (KOKORO_LANGUAGE)
    pub kokoro_language: String,

    // ── Context consolidation ────────────────────────────────────────────────
    /// Approximate context window of the LLM model in tokens.
    /// Context consolidation triggers when the prompt exceeds the configured
    /// threshold percentage of this limit.
    pub llm_context_tokens: usize,
    /// Number of most-recent (role, content) turns to keep verbatim after consolidation.
    pub llm_summary_keep_turns: usize,
    /// Percentage of the context window that triggers consolidation (default 80).
    pub llm_consolidation_threshold_pct: usize,
    /// Seconds of user inactivity after which a silent consolidation is triggered
    /// (if context needs it). 0 = disabled. Default: 900 (15 minutes).
    pub llm_idle_consolidation_secs: u64,
    /// Minimum context fill percentage required for an idle-triggered consolidation to run.
    /// If the current context is below this threshold, idle consolidation is skipped.
    /// Default: 20. Set to 0 to disable the minimum check.
    pub llm_idle_min_context_pct: usize,
    /// Maximum number of messages loaded from the DB on startup (0 = unlimited).
    /// Older messages beyond this count are skipped — the session summary covers them.
    /// Default: 0. Recommended: 40–60 to prevent restart compaction. (LLM_HISTORY_LOAD_LIMIT)
    pub llm_history_load_limit: usize,

    // ── Agent delegation ──────────────────────────────────────────────────────
    /// CLI command used to invoke the agent (e.g. "hermes chat"). May include arguments.
    /// None = agent tools disabled. Used in "cli" mode only.
    pub agent_command: Option<String>,
    /// Hard timeout in seconds for synchronous agent calls (AGENT_TIMEOUT_SECS).
    pub agent_timeout_secs: u64,
    /// Agent communication mode: "cli" (default, fire-and-forget subprocess) or
    /// "acp" (persistent ACP JSON-RPC stdio process with bidirectional communication).
    pub agent_mode: String,
    /// Command to start the ACP process (AGENT_ACP_COMMAND, default "hermes acp").
    /// Only used when agent_mode = "acp".
    pub agent_acp_command: String,
    /// When true, send a warmup prompt to Hermes at startup to force model load.
    /// AGENT_ACP_WARMUP=1. Only applies when agent_mode = "acp".
    pub agent_acp_warmup: bool,
    /// When true, periodically ping the ACP process to keep it alive.
    /// Default: true when AGENT_ACP_WARMUP=1, otherwise false.
    pub agent_acp_keepalive_enabled: bool,
    /// Seconds between keep-alive pings (AGENT_ACP_KEEPALIVE_INTERVAL_SECS, default 300).
    pub agent_acp_keepalive_interval_secs: u64,
    /// Seconds to wait for warmup response before giving up (AGENT_ACP_WARMUP_TIMEOUT_SECS, default 10).
    pub agent_acp_warmup_timeout_secs: u64,
    /// Initial backoff in seconds after an ACP restart (AGENT_ACP_RESTART_BACKOFF_SECS, default 2).
    pub agent_acp_restart_backoff_secs: u64,
    /// Maximum backoff cap in seconds (AGENT_ACP_RESTART_MAX_BACKOFF_SECS, default 60).
    pub agent_acp_restart_max_backoff_secs: u64,

    // ── Visible agent sessions (PTY) ──────────────────────────────────────────
    /// Directory for visible session log files (SENECHAL_SESSION_DIR, default "/tmp/seneschal_sessions").
    pub session_dir: String,
    /// Enable visible agent mode (VISIBLE_AGENT_ENABLED, default false).
    /// When true, agents with mode="visible" are accepted.
    pub visible_agent_enabled: bool,

    // ── Agent delegation (TOML array) ────────────────────────────────────────
    /// External agents defined in `[[agents]]` TOML array.
    /// Loaded by `AgentRegistry::from_config_and_env()` when no env vars are set.
    #[serde(default)]
    pub agents: Vec<AgentTomlConfig>,

    // ── MCP servers (TOML array) ─────────────────────────────────────────────
    /// MCP servers defined in `[[mcp_servers]]` TOML array.
    /// Loaded by `McpRegistry::from_config_and_env()` when no env vars are set.
    #[serde(default)]
    pub mcp_servers: Vec<McpServerTomlConfig>,

    // ── Inference daemon ──────────────────────────────────────────────────────
    /// Enable the background "is there anything worth saying?" loop.
    pub daemon_enabled: bool,
    /// Seconds between daemon checks (DAEMON_INTERVAL_SECS, default 300).
    pub daemon_interval_secs: u64,

    // ── EYES (visual awareness) ───────────────────────────────────────────────
    /// Seconds between screen-capture checks for EYES (EYES_INTERVAL_SECS).
    /// 0 = disabled (default). Requires SECONDARY_LLM_URL to be set.
    pub eyes_interval_secs: u64,

    // ── Secondary LLM (vision + background tasks) ────────────────────────────
    /// Base URL of the secondary LLM provider (SECONDARY_LLM_URL). None = disabled.
    /// When set, enables the vision tool and routes summarization + profile
    /// extraction to this model instead of the primary.
    pub secondary_llm_url: Option<String>,
    /// Model name for secondary LLM requests (SECONDARY_LLM_MODEL).
    pub secondary_llm_model: String,
    /// Max tokens for secondary LLM responses (SECONDARY_LLM_MAX_TOKENS, default 512).
    pub secondary_llm_max_tokens: u32,
    /// Bearer token for secondary LLM API (SECONDARY_LLM_API_KEY, default empty).
    pub secondary_llm_api_key: String,
    /// Enable Qwen3 thinking mode on the secondary LLM (SECONDARY_LLM_THINKING, default false).
    /// When true, `chat_template_kwargs: {"enable_thinking": true}` is sent in requests and
    /// `<think>…</think>` blocks are stripped from the returned text.
    pub secondary_llm_thinking: bool,

    // ── Shell tool ────────────────────────────────────────────────────────────
    /// Enable the `run_shell` tool (SHELL_ENABLED=1). Off by default.
    pub shell_enabled: bool,
    /// Hard timeout per shell command in seconds (SHELL_TIMEOUT_SECS).
    pub shell_timeout_secs: u64,

    // ── NOOP tool ──────────────────────────────────────────────────────────────
    /// Instructions for the LLM about when to call the NOOP (silent) tool.
    /// The NOOP tool stops the current query without any response to the user.
    /// Default: instructs the LLM to call it when the user asks something to
    /// Siri or Alexa. (NOOP_TOOL_INSTRUCTIONS)
    pub noop_tool_instructions: String,

    // ── Web Search (Native API providers) ─────────────────────────────────────
    /// Brave public search scraper enabled (BRAVE_PUBLIC_SEARCH, default true).
    /// When true, `quick_search` uses the public search.brave.com endpoint with
    /// no API key.  Disable to fall through to the other providers.
    pub brave_public_search_enabled: bool,
    /// Tavily Search API key (TAVILY_API_KEY). When set, enables the
    /// `quick_search` tool with Tavily backend (fast path, preferred).
    pub tavily_api_key: Option<String>,
    /// Max tokens for Tavily's AI-generated answer (TAVILY_MAX_TOKENS, default 512).
    /// Set to 0 to disable the AI-generated answer and return raw results only.
    pub tavily_max_tokens: usize,
    /// Exa API key (EXA_API_KEY). When set, enables `quick_search` with Exa
    /// backend. Only used when TAVILY_API_KEY is not set.
    pub exa_api_key: Option<String>,

    // ── Web Search (SearXNG fallback) ─────────────────────────────────────────
    /// Base URL of the SearXNG instance (SEARXNG_URL). Used as fallback when
    /// no native API key (TAVILY_API_KEY / EXA_API_KEY) is configured.
    pub searxng_url: Option<String>,
    /// Bearer token for SearXNG authentication (SEARXNG_SECRET).
    pub searxng_secret: String,
    /// Enable the web_search tool (WEB_SEARCH_ENABLED, default true).
    /// Set to 0 to disable without removing SEARXNG_URL.
    pub web_search_enabled: bool,

    // ── Speaker verification ──────────────────────────────────────────────────
    /// Path to sherpa-onnx speaker embedding ONNX model (SPEAKER_MODEL).
    /// None = auto-detect from models/speaker_embedding.onnx; disabled if absent.
    pub speaker_model: Option<String>,
    /// Path where the enrolled speaker embedding is persisted (SPEAKER_ENROLLMENT_PATH).
    pub speaker_enrollment_path: String,
    /// Cosine similarity threshold [0..1] (SPEAKER_SIMILARITY_MIN, default 0.45).
    pub speaker_similarity_min: f32,

    // ── Conversation mode (ambient state machine) ─────────────────────────────
    /// Wake word that triggers a response in Ambient mode (WAKE_WORD, default "seneschal").
    /// Case-insensitive substring match against the STT transcript.
    pub wake_word: String,
    /// Seconds in Ambient mode with no speech before auto-returning to Active
    /// (AMBIENT_CLEAR_SECS, default 300).
    pub ambient_clear_secs: u64,
    /// Consecutive non-enrolled-speaker VAD segments before auto-switching to
    /// Ambient mode (SPEAKER_AMBIENT_TRIGGER, default 3). Only applies when
    /// speaker verification is enabled.
    pub speaker_ambient_trigger: u8,

    // ── Ambient context buffer ────────────────────────────────────────────────
    /// Maximum number of speaker profiles to auto-enroll (SPEAKER_MAX_PROFILES, default 5).
    /// The first enrolled speaker is always the "main user" (id=0).
    pub speaker_max_profiles: u8,
    /// Rolling window duration for the ambient context buffer in minutes
    /// (AMBIENT_BUFFER_MINUTES, default 3).
    pub ambient_buffer_minutes: u64,
    /// Maximum number of utterances to keep in the ambient context buffer
    /// (AMBIENT_BUFFER_MAX_ENTRIES, default 30).
    pub ambient_buffer_max_entries: usize,

    // ── Remote device (WebSocket) ──────────────────────────────────────────────
    /// WebSocket server port. None = disabled (WS_PORT).
    pub ws_port: Option<u16>,

    // ── Control API (HTTP + SSE) ──────────────────────────────────────────────
    /// HTTP control/SSE API port. None = disabled (CONTROL_PORT).
    pub control_port: Option<u16>,

    // ── Self-managed LLM process ──────────────────────────────────────────────
    /// When true, seneschal launches and supervises the LLM server process.
    /// Requires LLM_COMMAND to be set. (LLM_SELF_MANAGED, default false)
    #[serde(default)]
    pub llm_self_managed: bool,
    /// Full shell command to launch the LLM server (LLM_COMMAND).
    /// Required when LLM_SELF_MANAGED=1.
    /// Example: `mlx_lm.server --model google/gemma-4-26b-a4b --host 0.0.0.0 --port 8080 --max-tokens 32768`
    pub llm_command: Option<String>,

    // ── Persistence ───────────────────────────────────────────────────────────
    pub db_path: String,

    // ── Hermes ACP session log viewer ───────────────────────────────────────────
    pub hermes_session_viewer: HermesSessionViewerMode,

    // ── Cold Path Memory (S-DREAM) ─────────────────────────────────────────────
    /// Interval in seconds between S-DREAM consolidation cycles.
    /// 0 = disabled. Default: 3600 (1 hour). (S_DREAM_INTERVAL_SECS)
    pub s_dream_interval_secs: u64,
    /// Whether S-DREAM should trigger consolidation when the user is idle.
    /// Default: true. (S_DREAM_ON_IDLE)
    pub s_dream_on_idle: bool,
    /// Seconds of user inactivity before idle consolidation triggers.
    /// Default: 600 (10 minutes). (S_DREAM_IDLE_THRESHOLD_SECS)
    pub s_dream_idle_threshold_secs: u64,
    /// Scheduled hour (0-23) for daily consolidation.
    /// None = no scheduled consolidation. Default: Some(3) (3 AM).
    /// (S_DREAM_SCHEDULED_HOUR)
    pub s_dream_scheduled_hour: Option<u8>,
    /// Minimum number of L2 messages before consolidation triggers.
    /// Default: 50. (S_DREAM_L2_MIN_MESSAGES)
    pub s_dream_l2_min_messages: usize,
    /// Directory path for archived JSONL consolidations.
    /// Default: "data/archives". (S_DREAM_JSONL_DIR)
    pub s_dream_jsonl_dir: String,

    // ── Apple Calendar & Reminders ───────────────────────────────────────────
    /// Enable the apple_events tool (Calendar & Reminders via AppleScript).
    /// Default: true. (APPLE_EVENTS_ENABLED)
    pub apple_events_enabled: bool,

    // ── Plugins ───────────────────────────────────────────────────────────────
    /// Paths to plugin directories or files (SENECHAL_PLUGINS, comma-separated).
    pub plugins: Vec<PathBuf>,
    /// Name of the currently active plugin (SENECHAL_ACTIVE_PLUGIN).
    /// Empty string in TOML maps to None.
    #[serde(deserialize_with = "deserialize_empty_string_as_none")]
    pub active_plugin: Option<String>,
}

fn deserialize_empty_string_as_none<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(if s.is_empty() { None } else { Some(s) })
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let mut config = Self::load_defaults()?;
        config.apply_env_overrides()?;
        if config.wake_word.is_empty() {
            warn!("wake_word is empty; defaulting to 'seneschal'");
            config.wake_word = "seneschal".to_string();
        }
        Ok(config)
    }

    fn load_defaults() -> Result<Self> {
        let embedded: toml::Value = toml::from_str(DEFAULT_CONFIG_TOML)
            .context("Failed to parse embedded default config")?;

        if let Some(path) = Self::resolve_config_path()? {
            let contents = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read config file {}", path.display()))?;
            let user: toml::Value = toml::from_str(&contents)
                .with_context(|| format!("Failed to parse config file {}", path.display()))?;
            let merged = Self::merge_toml(embedded, user);
            return merged.try_into::<Self>().with_context(|| {
                format!(
                    "Failed to deserialize merged config from {}",
                    path.display()
                )
            });
        }

        embedded
            .try_into::<Self>()
            .context("Failed to deserialize embedded default config")
    }

    fn resolve_config_path() -> Result<Option<PathBuf>> {
        if let Ok(path) = env::var("SENECHAL_CONFIG_FILE") {
            return Ok(Some(PathBuf::from(path)));
        }

        let env = SeneschalEnv::from_env_var();
        let cwd_config = PathBuf::from(format!("seneschal.{}.toml", env.as_str()));
        if cwd_config.exists() {
            return Ok(Some(cwd_config));
        }

        warn!(
            "Config file {} not found; using embedded default",
            cwd_config.display()
        );
        Ok(None)
    }

    /// Returns the env-specific log file path (e.g. `seneschal.pro.log`).
    /// This is a free function because it is needed before `Config` is loaded.
    pub fn log_file_path() -> String {
        format!("seneschal.{}.log", SeneschalEnv::from_env_var().as_str())
    }

    fn merge_toml(base: toml::Value, overlay: toml::Value) -> toml::Value {
        match (base, overlay) {
            (toml::Value::Table(mut base_table), toml::Value::Table(overlay_table)) => {
                for (key, value) in overlay_table {
                    let entry = base_table
                        .entry(key)
                        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
                    *entry = Self::merge_toml(entry.clone(), value);
                }
                toml::Value::Table(base_table)
            }
            (_, overlay) => overlay,
        }
    }

    fn apply_env_overrides(&mut self) -> Result<()> {
        // Audio
        if let Ok(v) = env::var("AUDIO_SAMPLE_RATE") {
            self.sample_rate = v.parse().context("Invalid AUDIO_SAMPLE_RATE")?;
        }
        if let Ok(v) = env::var("AUDIO_CHANNELS") {
            self.channels = v.parse().context("Invalid AUDIO_CHANNELS")?;
        }
        if let Ok(v) = env::var("AUDIO_CHUNK_MS") {
            self.chunk_ms = v.parse().context("Invalid AUDIO_CHUNK_MS")?;
        }
        if let Ok(v) = env::var("AUDIO_INPUT_DEVICE") {
            self.audio_input_device = Some(v);
        }
        if let Ok(v) = env::var("AUDIO_OUTPUT_DEVICE") {
            self.audio_output_device = Some(v);
        }
        if let Ok(v) = env::var("LIST_AUDIO_DEVICES") {
            self.list_devices = v == "1" || v.to_lowercase() == "true";
        }
        if let Ok(v) = env::var("LIST_VOICES") {
            self.list_voices = v == "1" || v.to_lowercase() == "true";
        }
        if let Ok(v) = env::var("DEVICE_MONITOR_ENABLED") {
            self.device_monitor_enabled = v == "1" || v.to_lowercase() == "true";
        }
        if let Ok(v) = env::var("DEVICE_MONITOR_POLL_SECS") {
            self.device_monitor_poll_secs =
                v.parse().context("Invalid DEVICE_MONITOR_POLL_SECS")?;
        }

        // VAD
        if let Ok(v) = env::var("VAD_START_THRESHOLD") {
            self.vad_start_threshold = v.parse().context("Invalid VAD_START_THRESHOLD")?;
        } else if let Ok(v) = env::var("VAD_THRESHOLD") {
            self.vad_start_threshold = v.parse().context("Invalid VAD_THRESHOLD")?;
        }
        if let Ok(v) = env::var("VAD_END_THRESHOLD") {
            self.vad_end_threshold = v.parse().context("Invalid VAD_END_THRESHOLD")?;
        }
        if let Ok(v) = env::var("VAD_SILENCE_MS") {
            self.vad_silence_ms = v.parse().context("Invalid VAD_SILENCE_MS")?;
        }
        if let Ok(v) = env::var("VAD_MODEL") {
            self.vad_model = v;
        }
        if let Ok(v) = env::var("VAD_CONFIRM_PROBES") {
            self.vad_confirm_probes = v.parse().context("Invalid VAD_CONFIRM_PROBES")?;
        }

        // Language
        if let Ok(v) = env::var("SENECHAL_LANGUAGE") {
            self.language = v;
        }

        // STT
        if let Ok(v) = env::var("STT_PROVIDER") {
            self.stt_provider = v.to_lowercase();
        }
        if let Ok(v) = env::var("WHISPER_MODEL") {
            self.whisper_model = v;
        }
        if let Ok(v) = env::var("PARAKEET_MODEL_DIR") {
            self.parakeet_model_dir = Some(v);
        }
        if let Ok(v) = env::var("WHISPER_THREADS") {
            self.whisper_threads = v.parse().context("Invalid WHISPER_THREADS")?;
        }
        if let Ok(v) = env::var("STT_EARLY_REUSE_ENABLED") {
            self.stt_early_reuse_enabled = v.parse().context("Invalid STT_EARLY_REUSE_ENABLED")?;
        }
        if let Ok(v) = env::var("STT_EARLY_MIN_TOKENS") {
            self.stt_early_min_tokens = v.parse().context("Invalid STT_EARLY_MIN_TOKENS")?;
        }
        if let Ok(v) = env::var("STT_EARLY_REQUIRE_PUNCTUATION") {
            self.stt_early_require_punctuation =
                v.parse().context("Invalid STT_EARLY_REQUIRE_PUNCTUATION")?;
        }
        if let Ok(v) = env::var("STT_NO_SPEECH_THRESHOLD") {
            self.stt_no_speech_threshold = v.parse().context("Invalid STT_NO_SPEECH_THRESHOLD")?;
        }
        if let Ok(v) = env::var("STT_LOGPROB_THRESHOLD") {
            self.stt_logprob_threshold = v.parse().context("Invalid STT_LOGPROB_THRESHOLD")?;
        }
        if let Ok(v) = env::var("STT_COMPRESSION_THRESHOLD") {
            self.stt_compression_threshold =
                v.parse().context("Invalid STT_COMPRESSION_THRESHOLD")?;
        }

        // LLM
        if let Ok(v) = env::var("LLM_PROVIDER") {
            self.llm_provider = v;
        }
        if let Ok(v) = env::var("LLM_URL") {
            self.llm_url = v;
        }
        if let Ok(v) = env::var("LLM_API_KEY") {
            self.llm_api_key = v;
        }
        if let Ok(v) = env::var("LLM_MODEL") {
            self.llm_model = v;
        }
        if let Ok(v) = env::var("LLM_MAX_TOKENS") {
            self.llm_max_tokens = v.parse().context("Invalid LLM_MAX_TOKENS")?;
        }
        if let Ok(v) = env::var("LLM_SYSTEM_PROMPT") {
            self.llm_system_prompt = v;
        }
        if let Ok(v) = env::var("LLM_TEMPERATURE") {
            self.llm_temperature = v.parse().context("Invalid LLM_TEMPERATURE")?;
        }

        // TTS
        if let Ok(v) = env::var("TTS_PROVIDER") {
            self.tts_provider = v;
        }
        if let Ok(v) = env::var("AVSPEECH_VOICE") {
            self.avspeech_voice = v;
        }
        if let Ok(v) = env::var("AVSPEECH_RATE") {
            self.avspeech_rate = v.parse().context("Invalid AVSPEECH_RATE")?;
        }
        if let Ok(v) = env::var("KOKORO_MODEL") {
            self.kokoro_model = v;
        }
        if let Ok(v) = env::var("KOKORO_VOICES") {
            self.kokoro_voices = v;
        }
        if let Ok(v) = env::var("KOKORO_VOICE") {
            self.kokoro_voice = v;
        }
        if let Ok(v) = env::var("KOKORO_LANGUAGE") {
            self.kokoro_language = v;
        }

        // Context consolidation
        if let Ok(v) = env::var("LLM_CONTEXT_TOKENS") {
            self.llm_context_tokens = v.parse().context("Invalid LLM_CONTEXT_TOKENS")?;
        }
        if let Ok(v) = env::var("LLM_SUMMARY_KEEP_TURNS") {
            self.llm_summary_keep_turns = v.parse().context("Invalid LLM_SUMMARY_KEEP_TURNS")?;
        }
        if let Ok(v) = env::var("LLM_CONSOLIDATION_THRESHOLD_PCT") {
            self.llm_consolidation_threshold_pct = v
                .parse()
                .context("Invalid LLM_CONSOLIDATION_THRESHOLD_PCT")?;
        }
        if let Ok(v) = env::var("LLM_IDLE_CONSOLIDATION_SECS") {
            self.llm_idle_consolidation_secs =
                v.parse().context("Invalid LLM_IDLE_CONSOLIDATION_SECS")?;
        }
        if let Ok(v) = env::var("LLM_IDLE_MIN_CONTEXT_PCT") {
            self.llm_idle_min_context_pct =
                v.parse().context("Invalid LLM_IDLE_MIN_CONTEXT_PCT")?;
        }
        if let Ok(v) = env::var("LLM_HISTORY_LOAD_LIMIT") {
            self.llm_history_load_limit = v.parse().context("Invalid LLM_HISTORY_LOAD_LIMIT")?;
        }

        // Agent delegation
        if let Ok(v) = env::var("AGENT_COMMAND") {
            self.agent_command = Some(v);
        }
        if let Ok(v) = env::var("AGENT_TIMEOUT_SECS") {
            self.agent_timeout_secs = v.parse().context("Invalid AGENT_TIMEOUT_SECS")?;
        }
        if let Ok(v) = env::var("AGENT_MODE") {
            self.agent_mode = v;
        }
        if let Ok(v) = env::var("AGENT_ACP_COMMAND") {
            self.agent_acp_command = v;
        }
        if let Ok(v) = env::var("AGENT_ACP_WARMUP") {
            self.agent_acp_warmup = v == "1";
        }
        if let Ok(v) = env::var("AGENT_ACP_KEEPALIVE_ENABLED") {
            self.agent_acp_keepalive_enabled = v == "1" || v.to_lowercase() == "true";
        }
        if let Ok(v) = env::var("AGENT_ACP_KEEPALIVE_INTERVAL_SECS") {
            self.agent_acp_keepalive_interval_secs = v
                .parse()
                .context("Invalid AGENT_ACP_KEEPALIVE_INTERVAL_SECS")?;
        }
        if let Ok(v) = env::var("AGENT_ACP_WARMUP_TIMEOUT_SECS") {
            self.agent_acp_warmup_timeout_secs =
                v.parse().context("Invalid AGENT_ACP_WARMUP_TIMEOUT_SECS")?;
        }
        if let Ok(v) = env::var("AGENT_ACP_RESTART_BACKOFF_SECS") {
            self.agent_acp_restart_backoff_secs = v
                .parse()
                .context("Invalid AGENT_ACP_RESTART_BACKOFF_SECS")?;
        }
        if let Ok(v) = env::var("AGENT_ACP_RESTART_MAX_BACKOFF_SECS") {
            self.agent_acp_restart_max_backoff_secs = v
                .parse()
                .context("Invalid AGENT_ACP_RESTART_MAX_BACKOFF_SECS")?;
        }

        // Visible agent sessions
        if let Ok(v) = env::var("SENECHAL_SESSION_DIR") {
            self.session_dir = v;
        }
        if let Ok(v) = env::var("VISIBLE_AGENT_ENABLED") {
            self.visible_agent_enabled = v == "1" || v.to_lowercase() == "true";
        }

        // Inference daemon
        if let Ok(v) = env::var("DAEMON_ENABLED") {
            self.daemon_enabled = v == "1" || v.to_lowercase() == "true";
        }
        if let Ok(v) = env::var("DAEMON_INTERVAL_SECS") {
            self.daemon_interval_secs = v.parse().context("Invalid DAEMON_INTERVAL_SECS")?;
        }

        // EYES
        if let Ok(v) = env::var("EYES_INTERVAL_SECS") {
            self.eyes_interval_secs = v.parse().context("Invalid EYES_INTERVAL_SECS")?;
        }

        // LLM thinking
        if let Ok(v) = env::var("LLM_THINKING") {
            self.llm_thinking = v == "1" || v.to_lowercase() == "true";
        }
        if let Ok(v) = env::var("LLM_TEMPERATURE_SIMPLE") {
            self.llm_temperature_simple = v.parse().context("Invalid LLM_TEMPERATURE_SIMPLE")?;
        }
        if let Ok(v) = env::var("LLM_TEMPERATURE_COMPLEX") {
            self.llm_temperature_complex = v.parse().context("Invalid LLM_TEMPERATURE_COMPLEX")?;
        }
        if let Ok(v) = env::var("LLM_THINKING_SIMPLE") {
            self.llm_thinking_simple = v == "1" || v.to_lowercase() == "true";
        }
        if let Ok(v) = env::var("LLM_THINKING_COMPLEX") {
            self.llm_thinking_complex = v == "1" || v.to_lowercase() == "true";
        }
        if let Ok(v) = env::var("LLM_COMPLEX_KEYWORDS") {
            self.llm_complex_keywords = v
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect();
        }
        // ── Classifier cascade (C01) ──────────────────────────────────────────
        if let Ok(v) = env::var("CLASSIFIER_CONFIDENCE_THRESHOLD") {
            self.classifier_confidence_threshold = v
                .parse()
                .context("Invalid CLASSIFIER_CONFIDENCE_THRESHOLD")?;
        }
        if let Ok(v) = env::var("CLASSIFIER_ENABLE_EMBEDDING") {
            self.classifier_enable_embedding = v == "1" || v.to_lowercase() == "true";
        }
        if let Ok(v) = env::var("CLASSIFIER_MODEL_PATH") {
            self.classifier_model_path = v;
        }
        if let Ok(v) = env::var("CLASSIFIER_CENTROIDS_PATH") {
            self.classifier_centroids_path = v;
        }
        if let Ok(v) = env::var("CLASSIFIER_WEIGHTS_PATH") {
            self.classifier_weights_path = v;
        }
        if let Ok(v) = env::var("CLASSIFIER_ENABLE_FALLBACK") {
            self.classifier_enable_fallback = v == "1" || v.to_lowercase() == "true";
        }
        if let Ok(v) = env::var("CLASSIFIER_FALLBACK_URL") {
            self.classifier_fallback_url = v;
        }
        if let Ok(v) = env::var("CLASSIFIER_FALLBACK_MODEL") {
            self.classifier_fallback_model = v;
        }
        if let Ok(v) = env::var("CLASSIFIER_FALLBACK_API_KEY") {
            self.classifier_fallback_api_key = v;
        }
        if let Ok(v) = env::var("CLASSIFIER_FALLBACK_TIMEOUT_MS") {
            self.classifier_fallback_timeout_ms = v
                .parse()
                .context("Invalid CLASSIFIER_FALLBACK_TIMEOUT_MS")?;
        }
        if let Ok(v) = env::var("LLM_TOOLS_STRICT") {
            self.llm_tools_strict = v == "1" || v.to_lowercase() == "true";
        }

        // Secondary LLM
        if let Ok(v) = env::var("SECONDARY_LLM_URL") {
            self.secondary_llm_url = Some(v);
        }
        if let Ok(v) = env::var("SECONDARY_LLM_MODEL") {
            self.secondary_llm_model = v;
        }
        if let Ok(v) = env::var("SECONDARY_LLM_MAX_TOKENS") {
            self.secondary_llm_max_tokens =
                v.parse().context("Invalid SECONDARY_LLM_MAX_TOKENS")?;
        }
        if let Ok(v) = env::var("SECONDARY_LLM_API_KEY") {
            self.secondary_llm_api_key = v;
        }
        if let Ok(v) = env::var("SECONDARY_LLM_THINKING") {
            self.secondary_llm_thinking = v == "1" || v.to_lowercase() == "true";
        }

        // Shell tool
        if let Ok(v) = env::var("SHELL_ENABLED") {
            self.shell_enabled = v == "1" || v.to_lowercase() == "true";
        }
        if let Ok(v) = env::var("SHELL_TIMEOUT_SECS") {
            self.shell_timeout_secs = v.parse().context("Invalid SHELL_TIMEOUT_SECS")?;
        }

        // Web Search (Brave public scraper — default, free)
        if let Ok(v) = env::var("BRAVE_PUBLIC_SEARCH") {
            self.brave_public_search_enabled = v == "1" || v.to_lowercase() == "true";
        }

        // Web Search (native API providers)
        if let Ok(v) = env::var("TAVILY_API_KEY") {
            self.tavily_api_key = Some(v);
        }
        if let Ok(v) = env::var("TAVILY_MAX_TOKENS") {
            self.tavily_max_tokens = v.parse().context("Invalid TAVILY_MAX_TOKENS")?;
        }
        if let Ok(v) = env::var("EXA_API_KEY") {
            self.exa_api_key = Some(v);
        }

        // Web Search (SearXNG)
        if let Ok(v) = env::var("SEARXNG_URL") {
            self.searxng_url = Some(v);
        }
        if let Ok(v) = env::var("SEARXNG_SECRET") {
            self.searxng_secret = v;
        }
        if let Ok(v) = env::var("WEB_SEARCH_ENABLED") {
            self.web_search_enabled = v == "1" || v.to_lowercase() == "true";
        }

        // Speaker verification
        if let Ok(v) = env::var("SPEAKER_MODEL") {
            self.speaker_model = Some(v);
        } else if self.speaker_model.is_none() {
            let default = "models/speaker_embedding.onnx";
            if std::path::Path::new(default).exists() {
                self.speaker_model = Some(default.into());
            }
        }
        if let Ok(v) = env::var("SPEAKER_ENROLLMENT_PATH") {
            self.speaker_enrollment_path = v;
        }
        if let Ok(v) = env::var("SPEAKER_SIMILARITY_MIN") {
            self.speaker_similarity_min = v.parse().context("Invalid SPEAKER_SIMILARITY_MIN")?;
        }

        // Conversation mode
        if let Ok(v) = env::var("WAKE_WORD") {
            self.wake_word = v;
        }
        if let Ok(v) = env::var("AMBIENT_CLEAR_SECS") {
            self.ambient_clear_secs = v.parse().context("Invalid AMBIENT_CLEAR_SECS")?;
        }
        if let Ok(v) = env::var("SPEAKER_AMBIENT_TRIGGER") {
            self.speaker_ambient_trigger = v.parse().context("Invalid SPEAKER_AMBIENT_TRIGGER")?;
        }

        // Ambient context buffer
        if let Ok(v) = env::var("SPEAKER_MAX_PROFILES") {
            self.speaker_max_profiles = v.parse().context("Invalid SPEAKER_MAX_PROFILES")?;
        }
        if let Ok(v) = env::var("AMBIENT_BUFFER_MINUTES") {
            self.ambient_buffer_minutes = v.parse().context("Invalid AMBIENT_BUFFER_MINUTES")?;
        }
        if let Ok(v) = env::var("AMBIENT_BUFFER_MAX_ENTRIES") {
            self.ambient_buffer_max_entries =
                v.parse().context("Invalid AMBIENT_BUFFER_MAX_ENTRIES")?;
        }

        // Remote device (WebSocket)
        if let Ok(v) = env::var("WS_PORT") {
            self.ws_port = Some(v.parse().context("Invalid WS_PORT")?);
        }

        if let Ok(v) = env::var("CONTROL_PORT") {
            self.control_port = Some(v.parse().context("Invalid CONTROL_PORT")?);
        }

        // Self-managed LLM process
        if let Ok(v) = env::var("LLM_SELF_MANAGED") {
            self.llm_self_managed = v == "1" || v.to_lowercase() == "true";
        }
        if let Ok(v) = env::var("LLM_COMMAND") {
            self.llm_command = Some(v);
        }

        // DB
        if let Ok(v) = env::var("DB_PATH") {
            self.db_path = v;
        }

        // Hermes ACP session log viewer
        if let Ok(v) = env::var("HERMES_SESSION_VIEWER") {
            self.hermes_session_viewer = v
                .parse::<HermesSessionViewerMode>()
                .unwrap_or(HermesSessionViewerMode::Off);
        }

        // Cold Path Memory (S-DREAM)
        if let Ok(v) = env::var("S_DREAM_INTERVAL_SECS") {
            self.s_dream_interval_secs = v.parse().context("Invalid S_DREAM_INTERVAL_SECS")?;
        }
        if let Ok(v) = env::var("S_DREAM_ON_IDLE") {
            self.s_dream_on_idle = v == "1" || v.to_lowercase() == "true";
        }
        if let Ok(v) = env::var("S_DREAM_IDLE_THRESHOLD_SECS") {
            self.s_dream_idle_threshold_secs =
                v.parse().context("Invalid S_DREAM_IDLE_THRESHOLD_SECS")?;
        }
        if let Ok(v) = env::var("S_DREAM_SCHEDULED_HOUR") {
            self.s_dream_scheduled_hour =
                Some(v.parse().context("Invalid S_DREAM_SCHEDULED_HOUR")?);
        }
        if let Ok(v) = env::var("S_DREAM_L2_MIN_MESSAGES") {
            self.s_dream_l2_min_messages = v.parse().context("Invalid S_DREAM_L2_MIN_MESSAGES")?;
        }
        if let Ok(v) = env::var("S_DREAM_JSONL_DIR") {
            self.s_dream_jsonl_dir = v;
        }

        // Apple Events
        if let Ok(v) = env::var("APPLE_EVENTS_ENABLED") {
            self.apple_events_enabled = v == "1" || v.to_lowercase() == "true";
        }

        // NOOP tool
        if let Ok(v) = env::var("NOOP_TOOL_INSTRUCTIONS") {
            self.noop_tool_instructions = v;
        }

        // Plugins
        if let Ok(v) = env::var("SENECHAL_PLUGINS") {
            self.plugins = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .collect();
        }
        if let Ok(v) = env::var("SENECHAL_ACTIVE_PLUGIN") {
            self.active_plugin = if v.is_empty() { None } else { Some(v) };
        }

        // Validation
        if self.llm_self_managed && self.llm_command.is_none() {
            anyhow::bail!("LLM_COMMAND must be set when LLM_SELF_MANAGED=1");
        }

        if !matches!(
            self.stt_provider.as_str(),
            "whisper" | "parakeet" | "speech"
        ) {
            anyhow::bail!(
                "Invalid STT_PROVIDER '{}'. Supported values: whisper, parakeet, speech",
                self.stt_provider
            );
        }

        if let Some(hour) = self.s_dream_scheduled_hour
            && hour > 23
        {
            anyhow::bail!("Invalid S_DREAM_SCHEDULED_HOUR: {hour}. Must be between 0 and 23.");
        }

        Ok(())
    }

    pub fn samples_per_chunk(&self) -> usize {
        (self.sample_rate as usize * self.chunk_ms as usize) / 1000
    }
}

