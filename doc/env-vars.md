# Environment Variables

> **Single source of truth**: the canonical defaults are defined in `seneschal.pro.toml`
> (embedded into the binary at compile time) and in `src/config.rs::SeneschalConfig` (serde `Default` trait).
> Where this document or `readme.md` disagrees, trust `seneschal.pro.toml`.

Read from `.env` (dotenvy loads automatically):

| Variable | Default | Description |
|----------|---------|-------------|
| `SENECHAL_ENV` | `pro` | Environment: pro (default) or dev. Selects seneschal.{env}.toml and data/{env}/ paths. |
| `SENECHAL_SESSION_DIR` | `/tmp/seneschal_sessions` | Directory for visible agent session log files. Each visible PTY session writes its output to `{session_dir}/{session_id}.log`. |
| `VISIBLE_AGENT_ENABLED` | `false` | When `1` or `true`, enables visible agent mode (`mode = "visible"` in agent config). Visible agents launch in a PTY with a Terminal window for real-time monitoring. |
| `AUDIO_SAMPLE_RATE` | `16000` | Audio sample rate |
| `AUDIO_CHANNELS` | `1` | Audio channels |
| `AUDIO_INPUT_DEVICE` | _(default)_ | Microphone device name. Case-insensitive **partial** match against the CPAL device name; append `#N` to pick the Nth match. Run `--list-devices` or `LIST_AUDIO_DEVICES=1` to see names. **Linux/ALSA:** use the CPAL name (e.g. `USB Audio`), which can differ from the OS "friendly" name (the product name lives only in `/proc/asound/cards`). |
| `AUDIO_OUTPUT_DEVICE` | _(default)_ | Speaker device name (same matching rules as `AUDIO_INPUT_DEVICE`). |
| `LIST_AUDIO_DEVICES` | `false` | When `1`, print available input **and** output devices and exit (also `--list-devices`). |
| `SENECHAL_LANGUAGE` | `en` | Language (`en` or `es`) |
| `STT_PROVIDER` | `whisper` | STT backend: `whisper` (default, whisper-cpp-plus + Silero VAD), `speech` (macOS SFSpeechRecognizer, requires `--features speech`), `parakeet` (requires `--features parakeet`), or `openai` (local VAD + remote batch transcription via an OpenAI-compatible `/v1/audio/transcriptions` service, requires `--features openai-stt`). |
| `WHISPER_MODEL` | `models/ggml-large-v3-turbo.bin` | Whisper GGML model path |
| `WHISPER_THREADS` | `0` | CPU threads (0 = auto) |
| `VAD_SILENCE_MS` | `300` | ms of continuous silence before SpeechEnd. Also sets the short-utterance silence window (speech provider): unconfirmed speech followed by this much silence is still fed to STT. |
| `VAD_START_THRESHOLD` | `0.65` | Speech probability to start a segment (silence → speech). |
| `VAD_END_THRESHOLD` | `0.45` | Speech probability floor to keep a segment open. |
| `VAD_CONFIRM_PROBES` | `2` | Consecutive speech probes (100ms each with `STT_PROVIDER=speech`) required before the VAD *commits* to STT. `SpeechStart` (LISTENING + barge-in) fires at commit time (~200ms) or on the short-utterance fallback after `VAD_SILENCE_MS` of silence — not on a single noisy probe. Short words that never reach the confirm count are still transcribed via the short-utterance fallback. Coughs/noise are rejected by `NoSpeechGate`. |
| `VAD_MODEL` | `models/ggml-silero-vad.bin` | Path to Silero VAD model (`.bin`) used by whisper-cpp-plus. |
| `GPU_AUTO` | `1` | Auto-assign the local ONNX models (Silero VAD, Parakeet, Kokoro TTS) round-robin over the GPUs with the most free VRAM (CUDA builds, issue #14). `0` = manual only. |
| `VAD_GPU` | — | Pin Silero VAD to a GPU index (`0`, `1`, ...) or `cpu`. Wins over `GPU_AUTO`. |
| `PARAKEET_GPU` | — | Pin the Parakeet ONNX encoder/decoder to a GPU index or `cpu`. |
| `TTS_GPU` | — | Pin Kokoro ONNX to a GPU index or `cpu`. |
| `PARAKEET_MODEL_DIR` | — | Required when `STT_PROVIDER=parakeet`. Download ONNX from: https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx |
| `STT_URL` | `http://localhost:8024` | Base URL of the OpenAI-compatible STT service (`STT_PROVIDER=openai`); the provider POSTs to `{STT_URL}/v1/audio/transcriptions`. Reference deployment: `seneschal-asr` (vLLM serving `Qwen3-ASR`). |
| `STT_API_KEY` | _(empty)_ | Bearer token sent as `Authorization: Bearer <key>` to the remote STT service. Empty = no auth header. |
| `STT_MODEL` | `qwen3-asr` | Model name sent in the remote transcription `model` field. |
| `STT_TIMEOUT_MS` | `10000` | Per-utterance timeout (ms) for a remote transcription; bounds the HTTP round trip. |
| `LLM_URL` | `http://127.0.0.1:8000` | LLM server URL (mlx-lm default; oMLX is 8001) |
| `LLM_API_KEY` | _(empty)_ | Bearer token sent as `Authorization: Bearer <key>`; leave unset for local servers with no auth. |
| `LLM_MODEL` | `qwen3.8-27b` | Model name sent in the `model` field of API requests. |
| `LLM_MAX_TOKENS` | `400` | Max tokens per response |
| `LLM_TEMPERATURE` | `1.0` | Sampling temperature (NVIDIA generation_config spec for qwen3.8-flash-next) |
| `LLM_TOP_P` | `0.95` | Sampling top-p (nucleus sampling threshold) |
| `LLM_TOP_K` | `20` | Sampling top-k (only the K most likely tokens are considered) |
| `LLM_MIN_P` | `0` | Sampling min-p (reject tokens with probability < min_p × top-token probability) |
| `LLM_PRESENCE_PENALTY` | `0` | Presence penalty (discourages repeating tokens already present in the output). High values (>1.0) trigger premature EOS / truncated generations on NVFP4 checkpoints. |
| `LLM_REPETITION_PENALTY` | `1.0` | Repetition penalty (1.0 = disabled; >1.0 penalizes repeated tokens) |
| `LLM_THINKING` | `false` | Enable Qwen3 thinking mode (`true` sends `chat_template_kwargs: {"enable_thinking": true}` and strips `think` blocks) |
| `LLM_THINKING_STYLE` | `auto` | Wire shape for the thinking flag: `auto` (name heuristic) \| `kwargs` (`chat_template_kwargs`, vLLM/SGLang/llama.cpp) \| `effort` (`reasoning_effort`, o-series/gpt-5) \| `ollama` (`think: true\|false`) \| `none` (server decides; inline thinking stripped from content) |
| `LLM_THINKING_WITH_TOOLS` | `false` | Send the thinking flag alongside `tools` on voice requests. Set `true` on backends verified to accept `chat_template_kwargs`+tools (SGLang/vLLM reasoning-parser) when you need to force thinking OFF on a thinking-by-default server. Keep `false` on mlx-community quantizations (Jinja conflict). |
| `LLM_SYSTEM_PROMPT` | — | System prompt text |
| `LLM_CONTEXT_TOKENS` | `8192` | Context window size |
| `LLM_CONSOLIDATION_THRESHOLD_PCT` | `80` | % threshold for consolidation |
| `LLM_SUMMARY_KEEP_TURNS` | `6` | Recent turns to keep after consolidation |
| `TTS_PROVIDER` | `kokoro` | TTS backend: `kokoro` (default, ONNX, cross-platform), `avspeech` (macOS AVSpeechSynthesizer) or `openai` (Qwen3-TTS via an OpenAI-compatible service, requires `--features openai-tts`) |
| `AVSPEECH_VOICE` | `""` | macOS AVSpeech voice name. Empty string = auto-select first available system voice. At runtime, `avspeech.rs` picks the first voice on the system when this is empty or whitespace-only. |
| `AVSPEECH_RATE` | `0.55` | Speech rate (0.0–1.0) |
| `OPENAI_TTS_URL` | `http://localhost:8025` | Base URL of the OpenAI-compatible speech service (`TTS_PROVIDER=openai`) |
| `OPENAI_TTS_VOICE` | `vivian` | Preset Qwen3-TTS voice; list with `--list-voices` |
| `OPENAI_TTS_LANGUAGE` | `auto` | Language hint; `auto` is sent as `"Auto"` so the model adapts per utterance (ES/EN code-switching) |
| `OPENAI_TTS_INSTRUCTIONS` | _(empty)_ | Optional natural-language style/emotion instruction applied to every request |
| `OPENAI_TTS_SPEED` | `0.0` | Speech speed; `0` omits the field (service default) |
| `OPENAI_TTS_WARMUP` | `false` | Warm up the service at startup (the first request after container start takes ~90 s) |
| `OPENAI_TTS_TIMEOUT_MS` | `120000` | Per-request HTTP timeout; must cover the ~90 s warmup |

> **Qwen3-TTS checkpoint limitation (issue #16):** the service currently runs
> the *CustomVoice* checkpoint, which supports preset voices + `instructions`.
> Voice cloning (`task_type=Base`) and VoiceDesign require separate checkpoints
> and are not supported by this integration yet — see
> [`doc/tts-openai.md`](tts-openai.md).
| `TAVILY_API_KEY` | — | Tavily Search API key. Preferred `quick_search` backend when set (wins over Brave/SearXNG) |
| `TAVILY_MAX_TOKENS` | `512` | Tokens for Tavily's AI-generated answer (0 = raw results only) |
| `EXA_API_KEY` | — | Exa API key. Used by `quick_search` only when `TAVILY_API_KEY` is not set |
| `BRAVE_PUBLIC_SEARCH` | `true` | Free keyless Brave scraper fallback for `quick_search`. Native keys / SearXNG take priority over it |
| `SEARXNG_URL` | — | SearXNG base URL (enables web_search) |
| `SEARXNG_SECRET` | — | SearXNG bearer token |
| `SPEAKER_ENABLED` | `true` | Enable/disable speaker verification at runtime (`0`/`false` = disabled, `1`/`true` = enabled). When disabled, the speaker model is not loaded and the associated 50-200ms latency per utterance is avoided. |
| `WS_PORT` | `9090` | WebSocket server port for remote audio (`--features remote`). Companion mic/TTS path. |
| `CONTROL_PORT` | — (optional) | HTTP/SSE Control API port (`--features control`). Recommended **9001** for iOS companion dual-channel. Binds `0.0.0.0`. See [`IOS_COMPANION.md`](IOS_COMPANION.md). |
| `S_DREAM_INTERVAL_SECS` | `3600` | Seconds between consolidation cycles (0 = disabled) |
| `S_DREAM_ON_IDLE` | `1` | Trigger consolidation when user is idle (1 = true) |
| `S_DREAM_IDLE_THRESHOLD_SECS` | `600` | Idle seconds before consolidation triggers |
| `S_DREAM_SCHEDULED_HOUR` | `3` | Scheduled daily hour (0-23); set empty to disable |
| `S_DREAM_L2_MIN_MESSAGES` | `50` | Min L2 messages before consolidation triggers |
| `S_DREAM_JSONL_DIR` | `data/{env}/archives` | Directory for archived JSONL consolidation files (default: data/{env}/archives) |
| `MCPS` | — | Comma-separated list of MCP server names. Each name is resolved via `MCP_<NAME>_COMMAND` or `MCP_<NAME>_URL`. Example: `MCPS=apple,filesystem` |
| `MCP_<NAME>_URL` | — | **HTTP transport**: base URL for an MCP Streamable HTTP server. Takes priority over `MCP_<NAME>_COMMAND` when both are set (a warning is logged). Example: `MCP_MY_TOOL_URL=http://localhost:8080/mcp` |
| `MCP_<NAME>_COMMAND` | — | **Stdio transport**: shell command to spawn the MCP server subprocess. Example: `MCP_APPLE_COMMAND=bunx apple-mcp@latest` |
| `MCP_<NAME>_TIMEOUT_SECS` | `30` | Hard timeout in seconds for each tool call. Works for both HTTP and stdio transports. Example: `MCP_APPLE_TIMEOUT_SECS=120` |
| `DEEP_WORKER_ENABLED` | `false` | Enable the native deep worker (issue #236). See [`deep-worker.md`](deep-worker.md). |
| `DEEP_WORKER_URL` | — (required when enabled) | Base URL of the worker's own OpenAI-compatible endpoint (e.g. vLLM on `:8006`). |
| `DEEP_WORKER_MODEL` | — (required when enabled) | Model name for the worker (a dense thinking-capable model). |
| `DEEP_WORKER_API_KEY` | _(empty)_ | Bearer token for the worker endpoint; empty = no auth. |
| `DEEP_WORKER_THINKING` | `true` | Send `chat_template_kwargs: {"enable_thinking": true}` on the worker's thinking-only calls. |
| `DEEP_WORKER_THINKING_MODE` | `external` | `external` (default; plan step without tools, then tool loop without thinking) or `inline` (kwargs alongside tools — requires vLLM `--enable-reasoning --reasoning-parser`). |
| `DEEP_WORKER_THINKING_STYLE` | `auto` | Same values as `LLM_THINKING_STYLE`; how the thinking flag is sent to the worker endpoint. |
| `DEEP_WORKER_MAX_TOKENS` | `2048` | Max tokens per worker response. |
| `DEEP_WORKER_SYSTEM_PROMPT` | — | The worker's own system prompt (independent from the voice prompt). |
| `DEEP_WORKER_TEMPERATURE` | `0.6` | Worker sampling temperature. |
| `DEEP_WORKER_TOP_P` | `0.95` | Worker sampling top-p. |
| `DEEP_WORKER_TOP_K` | `20` | Worker sampling top-k. |
| `DEEP_WORKER_MIN_P` | `0` | Worker sampling min-p. |
| `DEEP_WORKER_PRESENCE_PENALTY` | `0` | Worker presence penalty. |
| `DEEP_WORKER_REPETITION_PENALTY` | `1.05` | Worker repetition penalty. |
| `DEEP_WORKER_MAX_ITERATIONS` | `10` | Max tool-calling iterations of the worker loop (the last one forces a final answer without tools). |
| `DEEP_WORKER_TIMEOUT_MS` | `300000` | Hard timeout for a whole worker task. |
| `DEEP_WORKER_VISION_ENABLED` | `false` | Route vision (EYES + take_screenshot) to the worker endpoint instead of the voice LLM. Requires a multimodal worker model. |

**Mixed stdio + HTTP example**: `MCPS=local,remote` with `MCP_LOCAL_COMMAND=bunx my-local-mcp` and `MCP_REMOTE_URL=http://remote:8080/mcp` spawns a local subprocess for `local` and connects via HTTP for `remote`.

**Precedence**: If both `MCP_<NAME>_URL` and `MCP_<NAME>_COMMAND` are set for the same server, the URL variant is used and a warning is emitted. The `MCP_<NAME>_TIMEOUT_SECS` value applies regardless of transport type.

## Visible Agent Mode (PTY-based)

When `mode = "visible"` (in agent configuration), the agent is launched inside a
pseudo-terminal (PTY) instead of a stdin/stdout pipe. The PTY provides a full
terminal experience for the agent, and all I/O is duplicated to a log file at
`{session_dir}/{session_id}.log`. A macOS Terminal window is automatically opened
showing the log in real time via `tail -f`.

Configuration example (TOML):
```toml
[[agents]]
name = "hermes"
mode = "visible"
# CLI command to invoke the agent in the PTY
command = "hermes chat"
when_to_use = "For complex multi-step tasks that require extended reasoning."
prompt = "Eres el agente externo Hermes."
description = "Delegates complex research and multi-step tasks to Hermes."
task_description = "The research task or action to perform"
```

Env var equivalents:
```bash
AGENTS=hermes
AGENT_HERMES_MODE=visible
AGENT_HERMES_COMMAND="hermes chat"
```

## TOML Config (preferred)

MCP servers can also be defined via the `[[mcp_servers]]` TOML array in your config
file (`seneschal.{pro,dev}.toml`). This is the preferred configuration method for
most use cases — no need for env vars beyond `SENECHAL_ENV`.

```toml
[[mcp_servers]]
name = "apple"
command = "bunx apple-mcp@latest"
tool_timeout_secs = 30
```

**Precedence** (first match wins):
1. `MCPS` env var → multi-MCP format (each server resolved via `MCP_<NAME>_*`)
2. `MCP_COMMAND` env var → legacy single-server format
3. `[[mcp_servers]]` TOML array
4. No MCP servers configured (empty registry)