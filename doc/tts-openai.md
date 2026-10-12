# TTS provider: OpenAI-compatible (Qwen3-TTS) — issue #16

The `openai` backend synthesises speech through a speech API that follows the
**OpenAI-compatible** `POST /v1/audio/speech` contract. The current local
deployment is a **vLLM-Omni** container serving a **Qwen3-TTS** checkpoint, but
the provider targets the API shape, not that specific runtime. It is the
concrete "external TTS service" half of issue #15 and complements the
in-process `avspeech` / `kokoro` backends.

- Feature: `--features openai-tts`
- Select: `TTS_PROVIDER=openai`
- Config: `OPENAI_TTS_*`
- Deployment (current): the `seneschal-tts` service in
  `~/services/tesla-docker/docker-compose.yml` (`vllm/vllm-omni:qwen-image21`),
  GPU 0, port **8025**.

## Verified API behaviour

Measured against `vllm/vllm-omni` v0.29.0 serving
`Qwen3-TTS-12Hz-1.7B-CustomVoice`:

| Endpoint | Notes |
|---|---|
| `POST /v1/audio/speech` | `input`, `voice`, `language`, `instructions`, `response_format`, `stream`, `stream_format`, `speed`, `task_type`, `ref_audio`/`ref_text`, `x_vector_only_mode`, `initial_codec_chunk_frames`, `word_timestamps` |
| `GET /v1/audio/voices` | `{ voices: [...], uploaded_voices: [...] }` |
| `POST /v1/audio/voices` | multipart; `name` **and** `consent` are required |
| `DELETE /v1/audio/voices/{name}` | delete an uploaded voice |
| `POST /v1/audio/speech/stream` | WebSocket; requires a `session.config` handshake (not used) |

Observed data path with
`stream=true&stream_format=audio&response_format=pcm`:

- **Raw i16-LE mono PCM, 24 000 Hz** — no WAV header. Only `8000` and `24000`
  are accepted for `sample_rate`; anything else is a 400.
- First bytes arrive in **~60–80 ms**; the first ~80 ms are silence (trimmed by
  the client). First real audio: **~0.4 s**.
- Output arrives in bursts (one decoder block every ~0.35 s).
- Throughput ≈ **5× real time** on the shared GPU.
- The **first request after container start takes ~90 s** (warmup); subsequent
  requests ~0.5 s. Use `OPENAI_TTS_WARMUP=1` to absorb it at startup, and keep the
  default 120 s request timeout.

## Design

- `crates/seneschal-core/src/tts/openai/`
  - `mod.rs` — `OpenAiTts` client: request shaping, streaming pump, warmup.
  - `pcm.rs` — `Pcm16ToF32`: carries partial samples across TCP chunks and trims
    leading silence.
  - `voices.rs` — voice discovery/validation + a label table for the presets.
  - `error.rs` — `OpenAiTtsError` with a readable message extracted from the
    OpenAI-style error body.
- `TtsEngine::OpenAi` and `TtsEngine::synthesize_stream()` in `tts/mod.rs` give the
  pipeline a uniform streaming interface. Non-streaming backends emit a single
  chunk, so `tts_task` has one code path.
- **Incremental playback** (`audio/output.rs`): `AudioOutput::start_stream()`
  returns a `StreamSession` that resamples chunks with state (`BlockResampler`),
  feeds a bounded ring buffer and lets CPAL start playing the first chunk while
  the rest is still being generated. Barge-in still works through the shared
  `AtomicBool`.

### Why not SSE or WebSocket?

- `stream_format=sse` wraps PCM in base64 JSON events — pure overhead versus the
  raw PCM path.
- The WebSocket endpoint delivers *incremental text* input, which only pays off
  if tokens are sent to TTS as they arrive. The pipeline works sentence by
  sentence (`SentenceSplitter`), so it would not help today.

## Checkpoint limitation (deferred)

Each vLLM-Omni instance serves **one** checkpoint, and the three Qwen3-TTS tasks
need different ones:

| Task | Checkpoint | Status here |
|---|---|---|
| Preset voices + `instructions` | **CustomVoice** (running) | ✅ supported |
| Voice cloning (`task_type=Base`, `ref_audio`) | **Base** | ⛔ 400 from the running service |
| VoiceDesign (`task_type=VoiceDesign`) | **VoiceDesign** | ⛔ 400 from the running service |

Uploading a voice via `POST /v1/audio/voices` returns 200 even on CustomVoice,
but synthesising with it fails later with *"CustomVoice checkpoint does not
support task_type='Base'"*. For that reason the CLI does **not** expose voice
registration yet (`--list-voices` only lists). Cloning and VoiceDesign are
tracked as a follow-up: they require additional `seneschal-tts-base` /
`seneschal-tts-designed` containers (each with its own VRAM budget) and routing
per task type.

## GPU budget

`seneschal-tts` shares GPU 0 with the 27B LLM and the ASR service
(`--stage-overrides` bring the two TTS stages down to ~0.09 + 0.06 VRAM). The
host has ~22 GB free on GPU 0 at the time of writing, so the TTS footprint is
not the binding constraint; the LLM and ASR loads dominate time-to-first-token
variance.

## Testing

```sh
# Unit + wiremock contract tests
cargo test -p seneschal-core --features openai-tts

# Real service (skipped when unreachable); wired as `make test-openai`
OPENAI_TTS_TEST_URL=http://localhost:8025 \
  cargo test -p seneschal-core --features openai-tts -- --ignored openai_live

# Manual
TTS_PROVIDER=openai OPENAI_TTS_VOICE=vivian cargo run --features openai-tts -- --list-voices
TTS_PROVIDER=openai OPENAI_TTS_INSTRUCTIONS="habla con entusiasmo" \
  RUST_LOG=performance cargo run --features openai-tts
```

Look for `[+Nms] SpeechStart → FirstAudioPlayback` in the logs; a medium sentence
should be well under 500 ms.
