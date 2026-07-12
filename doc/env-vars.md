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