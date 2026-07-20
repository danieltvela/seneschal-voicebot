# Environment Variables

Read from `.env` (dotenvy loads automatically):

| Variable | Default | Description |
|----------|---------|-------------|
| `SENECHAL_ENV` | `pro` | Environment: pro (default) or dev. Selects seneschal.{env}.toml and data/{env}/ paths. |
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