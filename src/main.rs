#![allow(dead_code)]
#![allow(unreachable_code)]
#![allow(unused_mut)]
#![allow(unused_variables)]

mod agents;
mod analysis;
mod audio;
mod config;
#[cfg(feature = "control")]
mod control;
mod daemon;
mod db;
mod dream;
mod eyes;
mod i18n;
mod llm;
mod mcp;
mod memory;
mod pipeline;
mod profile;
#[cfg(feature = "remote")]
mod remote;
mod search;
mod stt;
mod tools;
mod tts;
#[cfg(feature = "tui")]
mod tui;

use anyhow::{Context, Result};
use async_channel::bounded;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

use crate::agents::{AcpSessionManager, AgentRegistry, ProactiveEvent};
use crate::analysis::ContextLens;
use crate::analysis::identity::IdentityAnalyzer;
use crate::audio::ambient_buffer::AmbientBuffer;
use crate::audio::audio_capture::{AudioCapture, AudioChunk};
use crate::audio::audio_transform::resample_nearest;
use crate::audio::buffer::AudioBuffer;
use crate::audio::output::AudioOutput;
use crate::audio::speaker::SpeakerVerifier;
use crate::config::Config;
use crate::db::{Database, Memory};
use crate::dream::{SDreamConfig, SDreamDaemon};
use crate::llm::{LlmProvider, LlmSession, OpenAiLlmProvider};
use crate::pipeline::{
    PipelineEvents, PipelineFrame, PipelineState, build_system_prompt,
    check_system_prompt_saturation, consolidation_task, llm_task, run_consolidation_cycle,
    sen_task, tts_task,
};
use crate::profile::ProfileFact;
use crate::stt::{SpeechEvent, SttProvider, create_provider};
use crate::tools::{
    ActiveTask, ConversationMode, CurrentTimeTool, DeepResearchTool, McpToolProxy, NoopTool,
    OpenAppTool, PendingInteractionEntry, QuickSearchTool, ReadClipboardTool, ReadFileTool,
    RecoverHistoricalContextTool, RunAgentTool, RunShellTool, SetClipboardTool,
    SetConversationModeTool, TakeScreenshotTool, ToolRegistry, WebSearchTool,
};
#[cfg(feature = "avspeech")]
use crate::tts::AvSpeechTts;
#[cfg(feature = "kokoro")]
use crate::tts::KokoroTts;
use crate::tts::TtsEngine;

#[cfg(test)]
mod e2e_tests;

const AUDIO_CHANNEL_CAPACITY: usize = 200;
const MAX_SPEECH_BUFFER_SECS: u32 = 30;
const MIN_SPEECH_DURATION_MS: u32 = 300;

// When the `avspeech` feature is enabled, the main thread must run CFRunLoop
// so that AVSpeechSynthesizer buffer callbacks are delivered.
#[cfg(feature = "avspeech")]
fn main() {
    unsafe extern "C" {
        fn CFRunLoopRunInMode(mode: *const std::ffi::c_void, seconds: f64, ret: u8) -> i32;
        static kCFRunLoopDefaultMode: *const std::ffi::c_void;
    }

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let quit = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let quit_c = quit.clone();

    let handle = std::thread::spawn(move || {
        let result = rt.block_on(async_main());
        quit_c.store(true, std::sync::atomic::Ordering::SeqCst);
        result
    });

    while !quit.load(std::sync::atomic::Ordering::SeqCst) {
        unsafe {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, 0);
        }
    }

    if let Err(e) = handle.join().expect("tokio thread panicked") {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}

#[cfg(not(feature = "avspeech"))]
#[tokio::main]
async fn main() -> Result<()> {
    async_main().await
}

async fn async_main() -> Result<()> {
    #[cfg(feature = "tui")]
    {
        let log_file =
            std::fs::File::create("voicebot.log").expect("failed to create voicebot.log");
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
            )
            .with_writer(std::sync::Mutex::new(log_file))
            .with_ansi(false)
            .init();
    }
    #[cfg(not(feature = "tui"))]
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    dotenvy::dotenv().ok();

    info!(target: "voicebot", "Starting voicebot...");
    let mut config = Config::from_env()?;

    // ── Device listing shortcut ───────────────────────────────────────────────
    let list_devices = config.list_devices
        || std::env::args().any(|a| a == "--list-devices" || a == "list-devices");
    if list_devices {
        AudioCapture::print_devices()?;
        return Ok(());
    }

    // ── Voice listing shortcut ────────────────────────────────────────────────
    let list_voices =
        config.list_voices || std::env::args().any(|a| a == "--list-voices" || a == "list-voices");
    if list_voices {
        match config.tts_provider.as_str() {
            #[cfg(feature = "avspeech")]
            "avspeech" => {
                AvSpeechTts::list_voices();
            }
            #[cfg(not(feature = "avspeech"))]
            "avspeech" => {
                eprintln!(
                    "TTS_PROVIDER=avspeech requires the 'avspeech' feature: cargo run --features avspeech"
                );
                std::process::exit(1);
            }
            #[cfg(feature = "kokoro")]
            "kokoro" => {
                let k = KokoroTts::new(
                    &config.kokoro_model,
                    &config.kokoro_voices,
                    &config.kokoro_voice,
                    &config.kokoro_language,
                )
                .await?;
                k.list_voices();
            }
            #[cfg(not(feature = "kokoro"))]
            "kokoro" => {
                eprintln!(
                    "TTS_PROVIDER=kokoro requires the 'kokoro' feature: cargo run --features kokoro"
                );
                std::process::exit(1);
            }
            _ => {
                eprintln!(
                    "Unknown TTS_PROVIDER '{}'. Available: avspeech, kokoro",
                    config.tts_provider
                );
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    info!(target: "voicebot", "Language: {}", config.language);

    // ── Standalone S-DREAM launch mode ────────────────────────────────────────
    if std::env::args().any(|a| a == "--dream") {
        info!(
            target: "llm",
            "Secondary LLM endpoint: {} (model={})",
            config.secondary_llm_url.as_deref().unwrap_or(""),
            config.secondary_llm_model,
        );
        return run_dream_mode(&config).await;
    }

    // ── Proactive event channel ───────────────────────────────────────────────
    let (proactive_tx, proactive_rx) = mpsc::channel::<ProactiveEvent>(32);
    // Per-session flag: ensures L1Saturated is emitted at most once.
    let l1_saturation_notified: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

    // ── Secondary LLM client ─────────────────────────────────────────────────
    let secondary_llm_client: Option<Arc<dyn LlmProvider>> =
        config.secondary_llm_url.as_ref().map(|url| {
            let client: Arc<dyn LlmProvider> = Arc::new(
                OpenAiLlmProvider::new(
                    url,
                    &config.secondary_llm_model,
                    config.secondary_llm_max_tokens,
                    0.3,
                )
                .with_api_key(&config.secondary_llm_api_key)
                .with_thinking(config.secondary_llm_thinking),
            );
            client
        });
    if secondary_llm_client.is_some() {
        info!(
            target: "llm",
            "Secondary LLM endpoint: {} (model={})",
            config.secondary_llm_url.as_deref().unwrap_or(""),
            config.secondary_llm_model,
        );
    }

    // ── Tools ─────────────────────────────────────────────────────────────────
    let shared_history: Arc<RwLock<String>> = Arc::new(RwLock::new(String::new()));
    let mut tool_registry = ToolRegistry::new();

    let conv_mode: Arc<Mutex<ConversationMode>> = Arc::new(Mutex::new(ConversationMode::Active));

    tool_registry.register(CurrentTimeTool);
    tool_registry.register(ReadFileTool);
    tool_registry.register(ReadClipboardTool);
    tool_registry.register(SetClipboardTool);
    tool_registry.register(OpenAppTool);
    tool_registry.register(SetConversationModeTool::new(Arc::clone(&conv_mode)));

    if config.shell_enabled {
        tool_registry.register(RunShellTool::new(config.shell_timeout_secs));
        info!(target: "voicebot", "run_shell tool enabled (timeout={}s)", config.shell_timeout_secs);
    }

    if let Some(ref sec_client) = secondary_llm_client {
        info!(
            target: "voicebot",
            "Vision tool enabled via secondary LLM (model={})",
            config.secondary_llm_model,
        );
        tool_registry.register(TakeScreenshotTool::new(sec_client.clone()));
    }

    if config.web_search_enabled
        && let Some(ref searxng_url) = config.searxng_url
    {
        let mut wst = WebSearchTool::new(searxng_url.clone(), config.searxng_secret.clone());
        if let Some(ref sec) = secondary_llm_client {
            wst = wst.with_synthesis(sec.clone());
            info!(target: "voicebot", "web_search synthesis via secondary LLM enabled");
        }
        tool_registry.register(wst);
        info!(target: "voicebot", "web_search tool enabled (url={})", searxng_url);
    }

    // ── Agent delegation (multi-agent registry) ───────────────────────────────
    let agent_registry = AgentRegistry::from_env();
    let agent_section = agent_registry.system_prompt_section();
    let session_manager = Arc::new(AcpSessionManager::new());

    // ── Search provider (fast path) ───────────────────────────────────────────
    if let Some(provider) = crate::search::from_config(&config) {
        let provider = Arc::from(provider);
        tool_registry.register(QuickSearchTool::new(Arc::clone(&provider)));
        info!(target: "voicebot", "quick_search tool enabled (provider={})", provider.name());

        if let Some(agent) = agent_registry.agents.first() {
            tool_registry.register(DeepResearchTool::new(agent.clone(), shared_history.clone()));
            info!(target: "voicebot", "deep_research tool enabled (agent={})", agent.name);
        }
    }

    for agent in &agent_registry.agents {
        info!(target: "voicebot", "Agent '{}' enabled (mode={})", agent.name, agent.mode);
        let task_map: Arc<dashmap::DashMap<String, ActiveTask>> = Arc::new(dashmap::DashMap::new());
        let mut run_agent_tool = RunAgentTool::new(
            agent.clone(),
            task_map,
            shared_history.clone(),
            proactive_tx.clone(),
        );
        if let Some(ref sec) = secondary_llm_client {
            run_agent_tool = run_agent_tool.with_synthesis(sec.clone());
            info!(target: "voicebot", "run_{} result synthesis via secondary LLM enabled", agent.name);
        }
        if agent.mode == "acp" {
            run_agent_tool = run_agent_tool.with_session_manager(Arc::clone(&session_manager));
            run_agent_tool = run_agent_tool.with_hermes_viewer(config.hermes_session_viewer);
        }
        tool_registry.register(run_agent_tool);
    }

    // ── ACP pre-warm (per-agent) ─────────────────────────────────────────────
    for agent in &agent_registry.agents {
        if agent.mode != "acp" || !agent.acp_warmup {
            continue;
        }
        let mgr = Arc::clone(&session_manager);
        let config = agent.clone();
        let agent_name = agent.name.clone();

        tokio::spawn(async move {
            match mgr.prewarm_agent(&config).await {
                Ok(sid) => {
                    info!(target: "agent", "ACP pre-warm [{}]: session ready (sid={sid})", agent_name);
                }
                Err(e) => {
                    warn!(target: "agent", "ACP pre-warm [{}]: init failed: {e}", agent_name);
                }
            }
        });
    }

    // ── MCP tools (multi-server) ──────────────────────────────────────────────
    let mcp_registry = mcp::McpRegistry::from_env();
    for server in &mcp_registry.servers {
        info!(target: "mcp", "Spawning MCP server '{}': {}", server.name, server.command);
        match mcp::McpClient::spawn_and_init(&server.command, server.tool_timeout_secs).await {
            Ok((client, tool_defs)) => {
                let client = std::sync::Arc::new(client);
                let mut count = 0;
                for def in tool_defs {
                    let prefixed_name = format!("{}_mcp__{}", server.name, def.name);
                    let prefixed_desc =
                        format!("[MCP server: {}] {}", server.name, def.description);
                    info!(
                        target: "mcp",
                        "Tool `{}`: schema={}",
                        prefixed_name,
                        serde_json::to_string(&def.input_schema).unwrap_or_default(),
                    );
                    tool_registry.register(McpToolProxy::new(
                        prefixed_name,
                        def.name,
                        prefixed_desc,
                        def.input_schema,
                        std::sync::Arc::clone(&client),
                    ));
                    count += 1;
                }
                info!(
                    target: "mcp",
                    "Registered {} MCP tool(s) from server '{}'",
                    count, server.name
                );
            }
            Err(e) => {
                warn!(
                    target: "mcp",
                    "MCP server '{}' failed to start: {}", server.name, e
                );
            }
        }
    }

    // ── Database ──────────────────────────────────────────────────────────────
    let db = Database::new(&config.db_path).await?;
    let session_id = db.get_or_create_session().await?;
    config.llm_system_prompt = db
        .ensure_active_system_prompt(session_id, &config.llm_system_prompt)
        .await?;
    let (summary, history) = db
        .get_session_context(session_id, config.llm_history_load_limit)
        .await?;
    info!(
        target: "db",
        "Loaded {} messages from history (summary: {})",
        history.len(),
        if summary.is_some() { "yes" } else { "no" }
    );

    let profile_facts: Vec<ProfileFact> = db
        .load_user_profile()
        .await?
        .into_iter()
        .map(|(key, value, confidence)| ProfileFact {
            key,
            value,
            confidence,
        })
        .collect();
    if !profile_facts.is_empty() {
        info!(target: "profile", "Loaded {} user profile facts", profile_facts.len());
    }

    let memories: Vec<Memory> = db.load_active_memories().await?;
    if !memories.is_empty() {
        info!(target: "memory", "Loaded {} persistent memories", memories.len());
    }

    tool_registry.register(RecoverHistoricalContextTool::new(Some(db.clone())));
    info!(target: "voicebot", "recover_historical_context tool enabled");

    tool_registry.register(NoopTool::new(config.noop_tool_instructions.clone()));
    info!(
        target: "voicebot",
        "noop tool enabled (instructions: {})",
        config.noop_tool_instructions,
    );

    let tools = Arc::new(tool_registry);

    // ── LLM session ───────────────────────────────────────────────────────────
    let immutable_rules: Vec<crate::profile::Correction> = db
        .get_immutable_rules()
        .await?
        .into_iter()
        .map(|(key, value, confidence)| crate::profile::Correction {
            topic: key,
            correction_text: value,
            confidence,
        })
        .collect();

    let tool_section = tools.system_prompt_section();
    let system_prompt = build_system_prompt(
        &config.llm_system_prompt,
        &profile_facts,
        &memories,
        &agent_section,
        &tool_section,
        &immutable_rules,
    );

    // Check if the initial context is already saturated and emit event if so.
    check_system_prompt_saturation(
        &profile_facts,
        &memories,
        &proactive_tx,
        &l1_saturation_notified,
    );

    let llm_session = Arc::new(Mutex::new(LlmSession::from_history(
        &system_prompt,
        summary.as_deref(),
        &history,
    )));

    // ── Self-managed LLM process ──────────────────────────────────────────────
    if config.llm_self_managed {
        let command = config.llm_command.as_deref().unwrap();
        let child = llm::manager::start_and_wait_ready(command, &config.llm_url)
            .await
            .context("Failed to start self-managed LLM server")?;
        let (notify_tx, mut notify_rx) = tokio::sync::mpsc::channel::<String>(1);
        let cmd = command.to_string();
        let url = config.llm_url.clone();
        tokio::spawn(llm::manager::supervise(child, cmd, url, notify_tx));
        tokio::spawn(async move {
            if let Some(msg) = notify_rx.recv().await {
                error!(target: "llm_manager", "{}", msg);
            }
        });
    }

    // ── LLM client ────────────────────────────────────────────────────────────
    let llm_client = crate::llm::create_provider(&config)?;
    info!(target: "llm", "LLM endpoint: {}", config.llm_url);

    let background_client = secondary_llm_client
        .clone()
        .unwrap_or_else(|| llm_client.clone());

    // ── Inference daemon ──────────────────────────────────────────────────────
    if config.daemon_enabled {
        info!(
            target: "daemon",
            "Inference daemon enabled (interval={}s)",
            config.daemon_interval_secs
        );
        daemon::InferenceDaemon {
            interval_secs: config.daemon_interval_secs,
            llm_client: llm_client.clone(),
            llm_session: Arc::clone(&llm_session),
            proactive_tx: proactive_tx.clone(),
        }
        .spawn();
    }

    // ── EYES (visual awareness) ───────────────────────────────────────────────
    if config.eyes_interval_secs > 0 {
        if let Some(ref sec_client) = secondary_llm_client {
            info!(
                target: "eyes",
                "EYES enabled (interval={}s, model={})",
                config.eyes_interval_secs,
                config.secondary_llm_model,
            );
            eyes::EyesDaemon {
                interval_secs: config.eyes_interval_secs,
                vision_client: sec_client.clone(),
                proactive_tx: proactive_tx.clone(),
            }
            .spawn();
        } else {
            warn!(
                target: "eyes",
                "EYES_INTERVAL_SECS={} but SECONDARY_LLM_URL is not set — EYES disabled",
                config.eyes_interval_secs
            );
        }
    }

    // ── STT + VAD unified processor ────────────────────────────────────────────
    let mut stt_provider: Box<dyn SttProvider> = create_provider(&config)?;
    info!(
        target: "stt",
        "Initialized STT provider '{}' (configured: {})",
        stt_provider.provider_name(),
        config.stt_provider
    );

    // ── Analysis Ring: ContextLens + IdentityAnalyzer ─────────────────────────
    // ContextLens is the shared blackboard for all analyzers. It is injected
    // into the LLM task so fresh context (speaker identity, emotion, etc.)
    // enriches every LLM call without being persisted to the session.
    let context_lens = Arc::new(Mutex::new(ContextLens::new()));

    let identity_analyzer: Option<Arc<Mutex<IdentityAnalyzer>>> = if let Some(ref model_path) =
        config.speaker_model
    {
        match SpeakerVerifier::new(
            model_path,
            std::path::Path::new(&config.speaker_enrollment_path),
            config.speaker_similarity_min,
            config.speaker_max_profiles,
        ) {
            Ok(sv) => {
                info!(
                    target: "speaker",
                    "Speaker verification enabled (threshold={})",
                    config.speaker_similarity_min
                );
                Some(Arc::new(Mutex::new(IdentityAnalyzer::new(
                    sv,
                    Arc::clone(&context_lens),
                ))))
            }
            Err(e) => {
                warn!(target: "speaker", "Speaker verification disabled — model load failed: {e}");
                None
            }
        }
    } else {
        info!(
            target: "speaker",
            "Speaker verification disabled \
             (place model at models/speaker_embedding.onnx to enable)"
        );
        None
    };

    // ── Ambient context buffer ────────────────────────────────────────────────
    let ambient_buffer = Arc::new(std::sync::Mutex::new(AmbientBuffer::new(
        config.ambient_buffer_max_entries,
        config.ambient_buffer_minutes,
    )));
    info!(
        target: "pipeline",
        "Ambient buffer: {}min / {} entries max",
        config.ambient_buffer_minutes,
        config.ambient_buffer_max_entries
    );

    // ── TTS ───────────────────────────────────────────────────────────────────
    let tts: TtsEngine = match config.tts_provider.as_str() {
        #[cfg(feature = "avspeech")]
        "avspeech" => {
            info!(target: "tts", "TTS provider: AVSpeechSynthesizer (voice={}, rate={:.2})", config.avspeech_voice, config.avspeech_rate);
            let voice = config.avspeech_voice.clone();
            let rate = config.avspeech_rate;
            let t = tokio::task::spawn_blocking(move || AvSpeechTts::new(&voice, rate)).await??;
            TtsEngine::AvSpeech(t)
        }
        #[cfg(not(feature = "avspeech"))]
        "avspeech" => {
            anyhow::bail!(
                "TTS_PROVIDER=avspeech requires the 'avspeech' feature: cargo run --features avspeech"
            );
        }
        #[cfg(feature = "kokoro")]
        "kokoro" => {
            info!(target: "tts", "TTS provider: Kokoro (voice={}, lang={})", config.kokoro_voice, config.kokoro_language);
            let k = KokoroTts::new(
                &config.kokoro_model,
                &config.kokoro_voices,
                &config.kokoro_voice,
                &config.kokoro_language,
            )
            .await?;
            TtsEngine::Kokoro(k)
        }
        #[cfg(not(feature = "kokoro"))]
        "kokoro" => {
            anyhow::bail!(
                "TTS_PROVIDER=kokoro requires the 'kokoro' feature: cargo run --features kokoro"
            );
        }
        _ => {
            anyhow::bail!(
                "Unknown TTS_PROVIDER '{}'. Available: avspeech, kokoro",
                config.tts_provider
            );
        }
    };
    let tts_sample_rate = tts.sample_rate();
    let tts = Arc::new(tts);

    // ── Audio output ──────────────────────────────────────────────────────────
    let audio_output = Arc::new(AudioOutput::new(config.audio_output_device.as_deref())?);
    info!(
        target: "audio",
        "Audio output: {}Hz, {}ch",
        audio_output.sample_rate(),
        audio_output.channels()
    );

    // ── Audio capture ─────────────────────────────────────────────────────────
    let audio_capture = AudioCapture::new(config.audio_input_device.as_deref())?;
    let source_sample_rate = audio_capture.sample_rate();
    info!(target: "audio", "Audio input: {}Hz", source_sample_rate);

    let samples_per_chunk = config.samples_per_chunk();
    let (tx, rx) = bounded(AUDIO_CHANNEL_CAPACITY);
    let _stream = audio_capture.start_capture(tx.clone(), samples_per_chunk)?;

    let (stt_tx, mut stt_rx) = mpsc::channel::<SpeechEvent>(32);

    let mut speech_buffer = AudioBuffer::new(source_sample_rate, MAX_SPEECH_BUFFER_SECS);
    let mut t_speech_start: Option<Instant> = None;

    let turn_commit_counter = Arc::new(AtomicU64::new(0));
    let mut last_cleared_commit: u64 = 0;
    let mut speech_buffer_start_offset: usize = 0;

    let non_user_streak = Arc::new(Mutex::new(0u8));
    let last_speech_at = Arc::new(Mutex::new(Instant::now()));

    // ── Pipeline timing context & events ─────────────────────────────────────
    let t_vad_end: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let t_llm_post_send: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let events = Arc::new(PipelineEvents::new());
    let play_cancel = Arc::new(AtomicBool::new(false));
    let tts_muted = Arc::new(AtomicBool::new(false));
    let shutdown = Arc::new(AtomicBool::new(false));

    // Pipeline FSM state — replaces AtomicBool flags (llm_busy, consolidation_active, etc.)
    // Each actor that owns a transition writes it directly; observers subscribe read-only.
    let (pipeline_state_tx, pipeline_state_rx) = tokio::sync::watch::channel(PipelineState::Idle);
    let pipeline_state_tx = Arc::new(pipeline_state_tx);

    #[cfg(feature = "control")]
    let control_broadcast = control::broadcast::ControlBroadcast::new(256);

    // Supervisor observer: logs every state transition (off the hot path).
    {
        let mut rx = pipeline_state_rx.clone();
        #[cfg(feature = "control")]
        let ctrl = control_broadcast.clone();
        tokio::spawn(async move {
            loop {
                if rx.changed().await.is_err() {
                    break;
                }
                let state = rx.borrow().clone();
                tracing::debug!(target: "fsm", "Pipeline state → {:?}", state);
                #[cfg(feature = "control")]
                ctrl.send(control::broadcast::ControlEvent::StateChanged {
                    state: format!("{state:?}"),
                    utterance_id: state.utterance_id(),
                });
            }
        });
    }
    // pipeline_state_rx is kept alive and cloned for each consumer below.

    // Periodic Ambient timeout checker — replaces dead SpeechEvent::Silence handler
    {
        let conv_mode_clone = conv_mode.clone();
        let last_speech_at_clone = last_speech_at.clone();
        let non_user_streak_clone = non_user_streak.clone();
        let shutdown_clone = shutdown.clone();
        let clear_secs = config.ambient_clear_secs;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                if shutdown_clone.load(Ordering::SeqCst) {
                    break;
                }
                let elapsed = last_speech_at_clone.lock().unwrap().elapsed().as_secs();
                if elapsed >= clear_secs {
                    let mut mode = conv_mode_clone.lock().unwrap();
                    if *mode == ConversationMode::Active {
                        *mode = ConversationMode::Ambient;
                        *non_user_streak_clone.lock().unwrap() = 0;
                        drop(mode);
                        info!(
                            target: "pipeline",
                            "Ambient mode: {}s of silence — returning to Ambient",
                            clear_secs
                        );
                    }
                }
            }
        });
    }

    #[cfg(feature = "remote")]
    let remote_tts_tx: Arc<
        tokio::sync::Mutex<Option<tokio::sync::mpsc::Sender<remote::protocol::TtsAudioPacket>>>,
    > = Arc::new(tokio::sync::Mutex::new(None));

    #[cfg(feature = "tui")]
    let (tui_tx, tui_rx) = tokio::sync::mpsc::unbounded_channel::<tui::events::TuiEvent>();

    let mut pending_agent_results: std::collections::VecDeque<(String, String)> =
        std::collections::VecDeque::new();
    let mut current_agent_announcement: Option<(String, String)> = None;
    let mut pending_agent_questions: std::collections::VecDeque<PendingInteractionEntry> =
        std::collections::VecDeque::new();

    let utterance_epoch = Arc::new(AtomicU64::new(0));

    // ── Sentences channel: sen_task + llm_task(errors) → tts_task ───────────
    let (sentences_tx, sentences_rx) = tokio::sync::mpsc::channel::<PipelineFrame>(64);
    // ── LLM token channel: llm_task → sen_task ────────────────────────────────
    let (llm_tx, llm_rx) = tokio::sync::mpsc::channel::<PipelineFrame>(256);
    // ── Transcript channel: audio loop + proactive → llm_task ────────────────
    let (transcript_tx, transcript_rx) = tokio::sync::mpsc::channel::<PipelineFrame>(16);

    // ── Spawn permanent pipeline tasks ────────────────────────────────────────
    {
        let events_c = Arc::clone(&events);
        let pipeline_state_tx_c = Arc::clone(&pipeline_state_tx);
        let pipeline_state_rx_c = pipeline_state_rx.clone();
        let sentences_tx_c = sentences_tx.clone();
        let llm_tx_c = llm_tx.clone();
        let t_llm_post_send_c = Arc::clone(&t_llm_post_send);
        let llm_session_c = Arc::clone(&llm_session);
        let llm_client_c = llm_client.clone();
        let db_c = db.clone();
        let tools_c = Arc::clone(&tools);
        let shared_history_c = Arc::clone(&shared_history);
        let turn_commit_c = Arc::clone(&turn_commit_counter);
        let proactive_tx_c = proactive_tx.clone();
        let context_lens_c = Arc::clone(&context_lens);
        #[cfg(feature = "tui")]
        let tui_tx_c = tui_tx.clone();
        #[cfg(feature = "control")]
        let control_broadcast_c = control_broadcast.clone();
        tokio::spawn(async move {
            llm_task(
                events_c,
                pipeline_state_tx_c,
                pipeline_state_rx_c,
                sentences_tx_c,
                llm_tx_c,
                transcript_rx,
                t_llm_post_send_c,
                llm_session_c,
                llm_client_c,
                db_c,
                session_id,
                tools_c,
                shared_history_c,
                turn_commit_c,
                proactive_tx_c,
                context_lens_c,
                #[cfg(feature = "tui")]
                tui_tx_c,
                #[cfg(feature = "control")]
                control_broadcast_c,
            )
            .await;
        });
    }
    {
        let events_c = Arc::clone(&events);
        let sentences_c = sentences_tx.clone();
        let t_vad_end_c = Arc::clone(&t_vad_end);
        let t_llm_post_send_c = Arc::clone(&t_llm_post_send);
        tokio::spawn(async move {
            sen_task(
                events_c,
                llm_rx,
                sentences_c,
                t_vad_end_c,
                t_llm_post_send_c,
            )
            .await;
        });
    }
    {
        let events_c = Arc::clone(&events);
        let pipeline_state_tx_c = Arc::clone(&pipeline_state_tx);
        let t_vad_end_c = Arc::clone(&t_vad_end);
        let tts_c = Arc::clone(&tts);
        let audio_out_c = Arc::clone(&audio_output);
        let play_cancel_c = Arc::clone(&play_cancel);
        let tts_muted_c = Arc::clone(&tts_muted);
        #[cfg(feature = "tui")]
        let tui_tx_c = tui_tx.clone();
        #[cfg(feature = "remote")]
        let remote_tts_tx_c = Arc::clone(&remote_tts_tx);
        #[cfg(feature = "control")]
        let control_broadcast_c = control_broadcast.clone();
        tokio::spawn(async move {
            tts_task(
                events_c,
                pipeline_state_tx_c,
                t_vad_end_c,
                sentences_rx,
                tts_c,
                audio_out_c,
                tts_sample_rate,
                play_cancel_c,
                tts_muted_c,
                #[cfg(feature = "tui")]
                tui_tx_c,
                #[cfg(feature = "remote")]
                remote_tts_tx_c,
                #[cfg(feature = "control")]
                control_broadcast_c,
            )
            .await;
        });
    }
    {
        let events_c = Arc::clone(&events);
        let pipeline_state_tx_c = Arc::clone(&pipeline_state_tx);
        let pipeline_state_rx_c = pipeline_state_rx.clone();
        let transcript_tx_c = transcript_tx.clone();
        let llm_session_c = Arc::clone(&llm_session);
        let background_c = background_client.clone();
        let db_c = db.clone();
        let context_tokens = config.llm_context_tokens;
        let keep_turns = config.llm_summary_keep_turns;
        let threshold_pct = config.llm_consolidation_threshold_pct;
        let idle_secs = config.llm_idle_consolidation_secs;
        let idle_min_pct = config.llm_idle_min_context_pct;
        let base_prompt = config.llm_system_prompt.clone();
        let agent_section_c = agent_section.clone();
        let tool_section_c = tool_section.clone();
        let language_c = config.language.clone();
        let proactive_tx_c = proactive_tx.clone();
        let already_notified_c = Arc::clone(&l1_saturation_notified);
        tokio::spawn(async move {
            consolidation_task(
                events_c,
                pipeline_state_tx_c,
                pipeline_state_rx_c,
                transcript_tx_c,
                llm_session_c,
                background_c,
                db_c,
                session_id,
                context_tokens,
                keep_turns,
                threshold_pct,
                idle_secs,
                idle_min_pct,
                base_prompt,
                agent_section_c,
                tool_section_c,
                language_c,
                proactive_tx_c,
                already_notified_c,
            )
            .await;
        });
    }

    info!(target: "voicebot", "Ready. Speak to interact...");

    // ── TUI ───────────────────────────────────────────────────────────────────
    #[cfg(feature = "tui")]
    {
        let transcript_tx_c = transcript_tx.clone();
        let tts_muted_c = Arc::clone(&tts_muted);
        let conv_mode_c = Arc::clone(&conv_mode);
        tokio::spawn(async move {
            if let Err(e) = tui::run(tui_rx, transcript_tx_c, tts_muted_c, conv_mode_c).await {
                tracing::error!("TUI error: {e}");
            }
            std::process::exit(0);
        });
    }

    // ── Remote device WebSocket server ────────────────────────────────────────
    #[cfg(feature = "remote")]
    if let Some(ws_port) = config.ws_port {
        let remote_state = Arc::new(remote::server::RemoteState {
            audio_tx: tx.clone(),
            samples_per_chunk,
            barge_in_tx: events.barge_in_tx.clone(),
            play_cancel: Arc::clone(&play_cancel),
            tts_audio_tx: Arc::clone(&remote_tts_tx),
            connected: AtomicBool::new(false),
        });
        tokio::spawn(async move {
            if let Err(e) = remote::server::start_server(ws_port, remote_state).await {
                error!(target: "remote", "WebSocket server error: {e}");
            }
        });
    }

    // ── Control API (HTTP + SSE) ──────────────────────────────────────────────
    #[cfg(feature = "control")]
    if let Some(ctrl_port) = config.control_port {
        let ctrl_state = Arc::new(control::state::ControlState {
            broadcast: control_broadcast.clone(),
            pipeline_state_rx: pipeline_state_rx.clone(),
            tts_muted: Arc::clone(&tts_muted),
            play_cancel: Arc::clone(&play_cancel),
            barge_in_tx: events.barge_in_tx.clone(),
            transcript_tx: transcript_tx.clone(),
            llm_session: Arc::clone(&llm_session),
        });
        tokio::spawn(async move {
            if let Err(e) = control::api::start_control_server(ctrl_port, ctrl_state).await {
                error!(target: "control", "Control API error: {e}");
            }
        });
    }

    // ── Startup consolidation ─────────────────────────────────────────────────
    {
        let needs = {
            let s = llm_session.lock().unwrap();
            s.needs_consolidation(config.llm_context_tokens, config.llm_idle_min_context_pct)
        };
        if needs {
            info!(target: "memory", "Startup: context exceeds idle threshold — running silent consolidation before greeting");
            run_consolidation_cycle(
                background_client.as_ref(),
                &db,
                session_id,
                &llm_session,
                config.llm_summary_keep_turns,
                &config.llm_system_prompt,
                &agent_section,
                &tool_section,
                &proactive_tx,
                &l1_saturation_notified,
            )
            .await;
        }
    }

    // ── Startup greeting ──────────────────────────────────────────────────────
    {
        let now = chrono::Local::now();
        let time_str = now.format("%H:%M").to_string();
        let date_str = now.format("%d/%m/%Y").to_string();
        let notification = i18n::get_notification("startup", &config.language)
            .replace("{time_str}", &time_str)
            .replace("{date_str}", &date_str);
        transcript_tx
            .send(PipelineFrame::SystemNotification { text: notification })
            .await
            .ok();
    }

    let mut proactive_rx = proactive_rx;
    tokio::select! {
        _ = async {
            loop {
                // Inject pending agent results when LLM is idle.
                let llm_idle = !pipeline_state_rx.borrow().is_busy();
                if llm_idle && current_agent_announcement.is_none()
                    && let Some((task, result)) = pending_agent_results.pop_front()
                {
                    let notification = i18n::get_notification("background_task_done", &config.language)
                        .replace("{task}", &task)
                        .replace("{result}", &result);
                    current_agent_announcement = Some((task, result));
                    transcript_tx.send(PipelineFrame::SystemNotification { text: notification }).await.ok();
                }
                if current_agent_announcement.is_some() && llm_idle {
                    current_agent_announcement = None;
                }

                let chunk: AudioChunk = tokio::select! {
                    result = rx.recv() => match result {
                        Ok(c) => c,
                        Err(e) => {
                            error!(target: "audio", "Audio channel closed: {}", e);
                            #[cfg(feature = "tui")]
                            tui_tx.send(tui::events::TuiEvent::Error(format!("Audio channel closed: {e}"))).ok();
                            break;
                        }
                    },
                    Some(event) = proactive_rx.recv() => {
                        match event {
                            ProactiveEvent::AgentResult { task, result, tool_call_id, .. } => {
                                if let Some(id) = tool_call_id {
                                    let payload = format!(
                                        "[Resultado de la tarea en segundo plano '{task}']\n{result}"
                                    );
                                    let sys_msg = serde_json::json!({
                                        "role": "user",
                                        "content": payload,
                                    });
                                    {
                                        let mut s = llm_session.lock().unwrap();
                                        s.add_tool_exchange(vec![sys_msg.clone()]);
                                    }
                                    {
                                        let db_c = db.clone();
                                        let exchange = vec![sys_msg];
                                        tokio::spawn(async move {
                                            if let Err(e) = db_c.save_tool_exchanges(session_id, &exchange).await {
                                                warn!(target: "db", "Failed to save system tool_result exchange: {}", e);
                                            }
                                        });
                                    }
                                    // Channel buffers this if llm_task is busy; it will pick it up when idle.
                                    transcript_tx.send(PipelineFrame::AgentResult {
                                        task,
                                        result,
                                        tool_call_id: Some(id),
                                    }).await.ok();
                                } else if !pipeline_state_rx.borrow().is_busy() {
                                    pending_agent_results.push_front((task, result));
                                } else {
                                    pending_agent_results.push_back((task, result));
                                }
                            }
                            ProactiveEvent::InferenceDaemon { .. } => {}
                            ProactiveEvent::L1Saturated { total_chars, threshold } => {
                                let prompt = i18n::get_notification("l1_saturated", &config.language)
                                    .replace("{total_chars}", &total_chars.to_string())
                                    .replace("{threshold}", &threshold.to_string());
                                transcript_tx.send(PipelineFrame::SystemNotification { text: prompt }).await.ok();
                            }
                            ProactiveEvent::AgentQuestion { task_id, agent_name, question, options, response_tx } => {
                                if pipeline_state_rx.borrow().is_busy() {
                                    events.barge_in_tx.send(0).ok();
                                    if let Some(announcement) = current_agent_announcement.take() {
                                        pending_agent_results.push_front(announcement);
                                    }
                                }
                                let entry = PendingInteractionEntry {
                                    task_id,
                                    agent_name,
                                    server_request_id: 0,
                                    question,
                                    options,
                                    response_tx,
                                };
                                pending_agent_questions.push_back(entry);
                                let opts_str = match pending_agent_questions.front() {
                                    Some(entry) => entry.options.join(" / "),
                                    None => String::new(),
                                };
                                let prompt = i18n::get_notification("acp_permission", &config.language)
                                    .replace("{question}", &pending_agent_questions.front().map(|e| e.question.clone()).unwrap_or_default())
                                    .replace("{opts_str}", &opts_str);
                                transcript_tx.send(PipelineFrame::SystemNotification { text: prompt }).await.ok();
                            }
                        }
                        continue;
                    },
                };

                let mono: Vec<f32> = if chunk.channels > 1 {
                    chunk.samples
                        .chunks(chunk.channels as usize)
                        .map(|f| f.iter().sum::<f32>() / chunk.channels as f32)
                        .collect()
                } else {
                    chunk.samples
                };
                let mono = resample_nearest(&mono, chunk.sample_rate, config.sample_rate);

                stt_provider.process_audio(&mono, &stt_tx).await.ok();

                while let Ok(event) = stt_rx.try_recv() {
                    match event {
                        SpeechEvent::SpeechStart => {
                            t_speech_start = Some(Instant::now());
                            info!(target: "performance", "[+0ms] SpeechStart");
                            #[cfg(feature = "tui")]
                            tui_tx.send(tui::events::TuiEvent::StateChange(
                                tui::events::PipelineState::Listening,
                            )).ok();
                            *last_speech_at.lock().unwrap() = Instant::now();

                            info!(target: "pipeline", "SpeechStart — firing BARGE_IN");
                            events.barge_in_tx.send(utterance_epoch.load(Ordering::SeqCst)).ok();
                            if let Some(announcement) = current_agent_announcement.take() {
                                info!(target: "pipeline", "SpeechStart interrupted agent announcement — re-queueing");
                                pending_agent_results.push_front(announcement);
                            }

                            let current_commits = turn_commit_counter.load(Ordering::SeqCst);
                            if current_commits > last_cleared_commit {
                                speech_buffer.clear();
                                last_cleared_commit = current_commits;
                            }
                            speech_buffer_start_offset = speech_buffer.sample_count();
                            speech_buffer.push(&mono);

                            utterance_epoch.fetch_add(1, Ordering::SeqCst);
                        }
                        SpeechEvent::Speech(partial_text) => {
                            speech_buffer.push(&mono);
                            debug!(target: "stt", "Partial: {}", partial_text);
                        }
                        SpeechEvent::SpeechEnd(final_stream_text) => {
                            speech_buffer.push(&mono);

                            let segment_duration_ms = t_speech_start.as_ref()
                                .map(|t| t.elapsed().as_millis() as u32)
                                .unwrap_or(0);

                            if segment_duration_ms < MIN_SPEECH_DURATION_MS {
                                debug!(target: "pipeline", "Too short ({}ms), skipping", segment_duration_ms);
                                continue;
                            }

                            let current_commits = turn_commit_counter.load(Ordering::SeqCst);
                            if current_commits > last_cleared_commit {
                                last_cleared_commit = current_commits;
                            }
                            let audio = speech_buffer.get_samples_from(speech_buffer_start_offset);
                            let duration_ms = audio.len() as u32 * 1000 / source_sample_rate;

                            info!(target: "pipeline", "Speech: {}ms (segment {}ms)", duration_ms, segment_duration_ms);

                            let mut segment_text = final_stream_text;

                            #[cfg(feature = "tui")]
                            tui_tx.send(tui::events::TuiEvent::StateChange(
                                tui::events::PipelineState::Transcribing,
                            )).ok();

                            let vad_elapsed = t_speech_start.take()
                                .map(|t| t.elapsed().as_millis()).unwrap_or(0);
                            info!(target: "performance", "[+{}ms] VAD end ({}ms speech)", vad_elapsed, duration_ms);
                            *t_vad_end.lock().unwrap() = Some(Instant::now());

                            *last_speech_at.lock().unwrap() = Instant::now();

                            let mut is_main_speaker = true;
                            let mut speaker_label = "Usuario".to_string();

                            if let Some(analyzer) = identity_analyzer.clone() {
                                let audio_c = audio.clone();
                                let streak_c = Arc::clone(&non_user_streak);
                                let mode_c = Arc::clone(&conv_mode);
                                let amb_trigger = config.speaker_ambient_trigger;
                                let amb_buf_c = Arc::clone(&ambient_buffer);
                                let stt_cfg = config.clone();
                                let sample_rate = config.sample_rate;
                                let label = speaker_label.clone();

                                tokio::spawn(async move {
                                    let mut analyzer = analyzer.lock().unwrap();
                                    let result = analyzer.verify(sample_rate, &audio_c);
                                    let is_main = result.is_main_speaker;
                                    let speaker_label = result.speaker_label;

                                    if !is_main {
                                        let mut streak = streak_c.lock().unwrap();
                                        *streak = streak.saturating_add(1);
                                        if *streak >= amb_trigger {
                                            let mut mode = mode_c.lock().unwrap();
                                            if *mode == ConversationMode::Active {
                                                *mode = ConversationMode::Ambient;
                                                drop(mode);
                                                info!(
                                                    target: "pipeline",
                                                    "Ambient mode: {} consecutive non-user voices",
                                                    *streak
                                                );
                                            }
                                        }
                                        let t0 = Instant::now();
                                        if let Ok(provider) = create_provider(&stt_cfg)
                                            && let Ok(text) = provider.transcribe_complete(&audio_c)
                                            && !text.is_empty()
                                        {
                                            amb_buf_c.lock().unwrap().push(speaker_label.clone(), text.clone());
                                            debug!(
                                                target: "pipeline",
                                                "Ambient buffer ← {speaker_label}: {text} ({}ms)",
                                                t0.elapsed().as_millis()
                                            );
                                        }
                                    } else {
                                        *streak_c.lock().unwrap() = 0;
                                    }
                                });
                            }

                            let mode_snapshot = conv_mode.lock().unwrap().clone();
                            let ambient_locked = mode_snapshot == ConversationMode::AmbientLocked;
                            let ambient_auto   = mode_snapshot == ConversationMode::Ambient;
                            let wake_word_check = config.wake_word.clone();

                            // ── ACP permission gate (FIFO queue) ──────────────────
                            if let Some(entry) = pending_agent_questions.pop_front() {
                                let audio_for_task = audio.clone();
                                let stt_cfg = config.clone();

                                info!(target: "acp", "Answering pending question for task={} agent={}", entry.task_id, entry.agent_name);

                                tokio::spawn(async move {
                                    let t0 = Instant::now();
                                    let answer = if let Ok(provider) = create_provider(&stt_cfg) {
                                        provider.transcribe_complete(&audio_for_task).unwrap_or_default()
                                    } else {
                                        String::new()
                                    };
                                    info!(target: "acp", "STT for permission question took {}ms", t0.elapsed().as_millis());
                                    let outcome = map_answer_to_outcome(&answer);
                                    info!(target: "acp", "Permission answer: {:?} → {}", answer, outcome);
                                    let _ = entry.response_tx.send(outcome);
                                });
                                continue;
                            }

                            let stt_elapsed_ms = segment_duration_ms as u128;
                            info!(
                                target: "performance",
                                "[+{}ms] STT transcription complete (audio={}samples, {}chars)",
                                stt_elapsed_ms, audio.len(), segment_text.len()
                            );
                            debug!(target: "stt", "Segment final: {}", segment_text);

                            if segment_text.trim().is_empty() {
                                debug!(target: "pipeline", "Empty transcription — skipping");
                                continue;
                            }

                            let mut final_text = segment_text;

                            if ambient_locked {
                                let lower = final_text.to_lowercase();
                                if !lower.contains(&wake_word_check) {
                                    ambient_buffer.lock().unwrap()
                                        .push("Usuario".to_string(), final_text.clone());
                                    debug!(target: "pipeline", "Ambient (locked): no wake word — buffered");
                                    continue;
                                }
                                info!(target: "pipeline", "Ambient (locked): wake word detected");
                            } else if ambient_auto {
                                *conv_mode.lock().unwrap() = ConversationMode::Active;
                                info!(target: "pipeline", "Auto-ambient: main user spoke — returning Active");
                            }

                            // Inject ambient context if query contains a referential.
                            final_text = {
                                let buf = ambient_buffer.lock().unwrap();
                                if crate::audio::ambient_buffer::has_referential(&final_text) {
                                    if let Some(ctx) = buf.format_context() {
                                        format!("{ctx}\n---\n{final_text}")
                                    } else {
                                        final_text
                                    }
                                } else {
                                    final_text
                                }
                            };

                            if final_text.trim().is_empty() {
                                debug!(target: "pipeline", "Empty after context injection — skipping");
                                continue;
                            }

                            if let Some(t0) = t_vad_end.lock().unwrap().as_ref() {
                                info!(
                                    target: "performance",
                                    "[+{}ms] STT done → transcript channel",
                                    t0.elapsed().as_millis()
                                );
                            }
                            let uid = utterance_epoch.load(Ordering::SeqCst);
                            transcript_tx.send(PipelineFrame::TranscriptReady {
                                utterance_id: uid,
                                text: final_text,
                            }).await.ok();
                        }
                    }
                }
            }
        } => {}
        _ = tokio::signal::ctrl_c() => {
            info!(target: "voicebot", "Shutting down...");
            events.barge_in_tx.send(0).ok();
            play_cancel.store(true, Ordering::SeqCst);
            shutdown.store(true, Ordering::SeqCst);
        }
    }

    Ok(())
}

/// Runs a single S-DREAM consolidation cycle and exits.
///
/// This is the implementation of `cargo run -- --dream`. It constructs the
/// minimum dependencies needed by the daemon (database, optional secondary
/// LLM, proactive channel, idle tracker), runs one cycle, and returns so the
/// process can terminate.
async fn run_dream_mode(config: &Config) -> Result<()> {
    info!(target: "voicebot", "S-DREAM standalone mode");

    let db = Database::new(&config.db_path).await?;

    let secondary_llm_client: Option<Arc<dyn LlmProvider>> =
        config.secondary_llm_url.as_ref().map(|url| {
            let client: Arc<dyn LlmProvider> = Arc::new(
                OpenAiLlmProvider::new(
                    url,
                    &config.secondary_llm_model,
                    config.secondary_llm_max_tokens,
                    0.3,
                )
                .with_api_key(&config.secondary_llm_api_key)
                .with_thinking(config.secondary_llm_thinking),
            );
            client
        });

    let (proactive_tx, _proactive_rx) = mpsc::channel::<ProactiveEvent>(32);
    let last_activity = Arc::new(AtomicU64::new(0));

    let dream_config = SDreamConfig {
        interval_secs: config.s_dream_interval_secs,
        on_idle: config.s_dream_on_idle,
        idle_threshold_secs: config.s_dream_idle_threshold_secs,
        scheduled_hour: config.s_dream_scheduled_hour,
        l2_min_messages: config.s_dream_l2_min_messages,
        jsonl_dir: config.s_dream_jsonl_dir.clone(),
    };

    SDreamDaemon {
        config: dream_config,
        db,
        secondary_client: secondary_llm_client,
        proactive_tx,
        last_activity,
    }
    .run_once()
    .await?;

    info!(target: "voicebot", "S-DREAM standalone cycle complete; exiting");
    Ok(())
}

/// Map a spoken yes/no transcript to an ACP permission outcome string.
fn map_answer_to_outcome(transcript: &str) -> String {
    let t = transcript.to_lowercase();
    if t.contains("sí")
        || t.contains("si")
        || t.contains("yes")
        || t.contains("claro")
        || t.contains("dale")
        || t.contains("ok")
        || t.contains("adelante")
        || t.contains("permite")
        || t.contains("permiso")
        || t.contains("autorizo")
    {
        "allow_once".to_string()
    } else {
        "reject_once".to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::llm::LlmProvider;

    /// Integration test for summarization using a real LLM server.
    ///
    /// Requires a running LLM server (default http://localhost:8000, e.g. mlx-lm or oMLX).
    /// Run manually:
    /// ```sh
    /// cargo test test_summarize_real_llm -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore]
    async fn test_summarize_real_llm() {
        let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();

        let db_dir = tempfile::TempDir::new().unwrap();
        let db_path = db_dir.path().join("test_summarize.db");
        let db = crate::db::Database::new(db_path.to_str().unwrap())
            .await
            .unwrap();
        let session_id = db.get_or_create_session().await.unwrap();

        let _ = dotenvy::dotenv();
        let llm_url =
            std::env::var("LLM_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
        let llm_model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "local-model".to_string());
        let llm_api_key = std::env::var("LLM_API_KEY").unwrap_or_default();
        let llm_client: Arc<dyn LlmProvider> = Arc::new(
            crate::llm::OpenAiLlmProvider::new(&llm_url, &llm_model, 400, 0.3)
                .with_api_key(&llm_api_key),
        );

        let system_prompt = "You are a helpful assistant.";
        let mut session = crate::llm::LlmSession::new(system_prompt);

        let turns = vec![
            (
                "What is the capital of France?",
                "The capital of France is Paris, a city known for the Eiffel Tower and its rich cultural heritage.",
            ),
            (
                "Tell me about the Rust programming language.",
                "Rust is a systems programming language focused on safety, speed, and concurrency. It was created by Mozilla.",
            ),
            (
                "What is photosynthesis?",
                "Photosynthesis is the process by which plants convert sunlight, water, and carbon dioxide into glucose and oxygen.",
            ),
            (
                "Who wrote Don Quixote?",
                "Don Quixote was written by Miguel de Cervantes and published in two parts in 1605 and 1615.",
            ),
            (
                "Explain quantum computing briefly.",
                "Quantum computing uses quantum bits or qubits that can exist in superposition, enabling parallel computation for certain problems.",
            ),
            (
                "What is the tallest mountain on Earth?",
                "Mount Everest is the tallest mountain on Earth at 8,849 meters above sea level, located in the Himalayas.",
            ),
            (
                "How does a combustion engine work?",
                "A combustion engine burns fuel in cylinders, creating expanding gases that push pistons connected to a crankshaft to produce rotary motion.",
            ),
            (
                "What is the speed of light?",
                "The speed of light in a vacuum is approximately 299,792,458 meters per second, often rounded to 300,000 km/s.",
            ),
        ];

        for (user_msg, assistant_msg) in &turns {
            session.add_user_turn(user_msg);
            session.add_assistant_turn(assistant_msg);
            db.save_message(session_id, "User", user_msg).await.unwrap();
            db.save_message(session_id, "Assistant", assistant_msg)
                .await
                .unwrap();
        }

        let msg_count_before = session.messages.len();
        let context_tokens: usize = 300;
        let keep_turns: usize = 2;

        assert!(
            session.needs_consolidation(context_tokens, 75),
            "Session should need consolidation with context_tokens={} but doesn't.",
            context_tokens
        );

        let prompt = session.build_summary_prompt(keep_turns).unwrap();
        let summary = llm_client.complete(&prompt).await.unwrap();
        assert!(!summary.is_empty(), "Summary should not be empty");

        let turns_to_summarize = session.summarizable_turn_count(keep_turns);
        let through_id = db
            .get_message_id_at_offset(session_id, 0, turns_to_summarize - 1)
            .await
            .unwrap()
            .unwrap();
        db.save_summary(session_id, &summary, through_id)
            .await
            .unwrap();
        session.apply_summary(&summary, keep_turns);

        let msg_count_after = session.messages.len();
        assert!(msg_count_after < msg_count_before);
        assert_eq!(msg_count_after, keep_turns);

        let all_msgs = session.all_messages_api();
        let system_content = all_msgs[0]["content"].as_str().unwrap();
        assert!(system_content.contains("[CONVERSATION SUMMARY]"));

        let (db_summary, db_recent) = db.get_session_context(session_id, 0).await.unwrap();
        assert!(db_summary.is_some());
        assert!(!db_recent.is_empty());

        println!("\n✓ summarize integration test passed");
    }

    #[tokio::test]
    #[ignore]
    async fn test_kv_cache_after_summarize() {
        let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();

        let _ = dotenvy::dotenv();
        let llm_url =
            std::env::var("LLM_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
        let llm_model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "local-model".to_string());
        let llm_api_key = std::env::var("LLM_API_KEY").unwrap_or_default();
        let llm_client: Arc<dyn LlmProvider> = Arc::new(
            crate::llm::OpenAiLlmProvider::new(&llm_url, &llm_model, 400, 0.3)
                .with_api_key(&llm_api_key),
        );

        let db_dir = tempfile::TempDir::new().unwrap();
        let db_path = db_dir.path().join("bench_kv.db");
        let db = crate::db::Database::new(db_path.to_str().unwrap())
            .await
            .unwrap();
        let session_id = db.get_or_create_session().await.unwrap();

        let system_prompt = "You are a helpful assistant. Answer briefly.";
        let mut session = crate::llm::LlmSession::new(system_prompt);

        println!("\n✓ kv_cache test setup complete (extend as needed)");
    }
}
