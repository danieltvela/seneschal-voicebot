# Environment Variables

Read from `.env` (dotenvy loads automatically):

| Variable | Default | Description |
|----------|---------|-------------|
| `SENECHAL_ENV` | `pro` | Environment: pro (default) or dev. Selects seneschal.{env}.toml and data/{env}/ paths. |
| `SENECHAL_SESSION_DIR` | `/tmp/seneschal_sessions` | Directory for visible agent session log files. Each visible PTY session writes its output to `{session_dir}/{session_id}.log`. |
| `VISIBLE_AGENT_ENABLED` | `false` | When `1` or `true`, enables visible agent mode (`mode = "visible"` in agent config). Visible agents launch in a PTY with a Terminal window for real-time monitoring. |
| `AUDIO_SAMPLE_RATE` | `16000` | Audio sample rate |
| `AUDIO_CHANNELS` | `1` | Audio channels |
| `SENECHAL_LANGUAGE` | `en` | Language (`en` or `es`) |
| `STT_PROVIDER` | `speech` | `speech` (default on macOS), `whisper`, or `parakeet` |
| `WHISPER_MODEL` | `models/ggml-large-v3-turbo.bin` | Whisper GGML model path |
| `WHISPER_THREADS` | `0` | CPU threads (0 = auto) |
| `VAD_SILENCE_MS` | `500` | ms of continuous silence before SpeechEnd. Also sets the short-utterance silence window (speech provider): unconfirmed speech followed by this much silence is still fed to STT. |
| `VAD_START_THRESHOLD` | `0.65` | Speech probability to start a segment (silence → speech). |
| `VAD_END_THRESHOLD` | `0.45` | Speech probability floor to keep a segment open. |
| `VAD_CONFIRM_PROBES` | `2` | Consecutive speech probes (100ms each with `STT_PROVIDER=speech`) required before the VAD *commits* to STT. `SpeechStart` (LISTENING + barge-in) fires at commit time (~200ms) or on the short-utterance fallback after `VAD_SILENCE_MS` of silence — not on a single noisy probe. Short words that never reach the confirm count are still transcribed via the short-utterance fallback. Coughs/noise are rejected by `NoSpeechGate`. |
| `VAD_MODEL` | `models/ggml-silero-vad.bin` | Path to Silero VAD model (`.bin`) used by whisper-cpp-plus. |
| `PARAKEET_MODEL_DIR` | — | Required when `STT_PROVIDER=parakeet`. Download ONNX from: https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx |
| `LLM_URL` | `http://127.0.0.1:8000` | LLM server URL (mlx-lm default; oMLX is 8001) |
| `LLM_MAX_TOKENS` | `1024` | Max tokens per response |
| `LLM_CONTEXT_TOKENS` | `8192` | Context window size |
| `LLM_CONSOLIDATION_THRESHOLD_PCT` | `80` | % threshold for consolidation |
| `LLM_SUMMARY_KEEP_TURNS` | `6` | Recent turns to keep after consolidation |
| `LLM_TEMPERATURE_SIMPLE` | `0.8` | SIMPLE-mode temperature (thinking off, tools off) |
| `LLM_TEMPERATURE_COMPLEX` | `=LLM_TEMPERATURE` (0.3) | COMPLEX-mode temperature (thinking on, tools on) |
| `LLM_THINKING_SIMPLE` | `false` | `enable_thinking` for SIMPLE requests |
| `LLM_THINKING_COMPLEX` | `=LLM_THINKING` (true in pro) | `enable_thinking` for COMPLEX requests |
| `LLM_COMPLEX_KEYWORDS` | _internal defaults_ | CSV of keywords that classify a request as COMPLEX. Empty → use built-in defaults |
| `LLM_TOOLS_STRICT` | `false` | When true, SIMPLE requests send `tool_choice: "none"` explicitly |
| `CLASSIFIER_CONFIDENCE_THRESHOLD` | `0.6` | Confidence threshold [0.0–1.0]; below this the cascade continues |
| `CLASSIFIER_ENABLE_EMBEDDING` | `false` | Enable Nivel 2/3 (embedding + logistic) — requires feature `classifier-embedding` |
| `CLASSIFIER_MODEL_PATH` | — | Path to ONNX embedding model (dir or .onnx file) |
| `CLASSIFIER_CENTROIDS_PATH` | — | Path to JSON centroids file for cosine-similarity classification |
| `CLASSIFIER_WEIGHTS_PATH` | — | Path to JSON logistic regression weights file (Nivel 3) |
| `CLASSIFIER_ENABLE_FALLBACK` | `false` | Enable Nivel 4 (SLM fallback to second LLM endpoint) |
| `CLASSIFIER_FALLBACK_URL` | — | Base URL of the fallback SLM endpoint (OpenAI-compatible) |
| `CLASSIFIER_FALLBACK_MODEL` | — | Model name for the fallback SLM |
| `CLASSIFIER_FALLBACK_API_KEY` | — | API key for the fallback SLM endpoint |
| `CLASSIFIER_FALLBACK_TIMEOUT_MS` | `800` | Timeout in milliseconds for the fallback SLM request |
| `AVSPEECH_VOICE` | `"Jorge (Enhanced)"` | macOS AVSpeech voice name |
| `AVSPEECH_RATE` | `0.55` | Speech rate (0.0–1.0) |
| `SEARXNG_URL` | — | SearXNG base URL (enables web_search) |
| `SEARXNG_SECRET` | — | SearXNG bearer token |
| `WS_PORT` | `9090` | WebSocket server port |
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
instructions = "Eres el agente externo Hermes."
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