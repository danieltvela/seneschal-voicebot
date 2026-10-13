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
  GPU 0, port **8025**. Since 2026-10-13 (issue #18) it serves
  `Qwen3-TTS-12Hz-1.7B-Base` for voice cloning; the CustomVoice checkpoint
  stays on disk and the rollback is one commented compose line plus
  `--deploy-config vllm_omni/deploy/qwen3_tts.yaml`.

## Verified API behaviour

Measured against `vllm/vllm-omni` v0.29.0 on this host, serving
`Qwen3-TTS-12Hz-1.7B-CustomVoice` (until 2026-10-13) and then
`Qwen3-TTS-12Hz-1.7B-Base`:

| Endpoint | Notes |
|---|---|
| `POST /v1/audio/speech` | `input`, `voice`, `language`, `instructions`, `response_format`, `stream`, `stream_format`, `speed`, `task_type`, `ref_audio`/`ref_text`, `x_vector_only_mode`, `initial_codec_chunk_frames`, `word_timestamps` |
| `GET /v1/audio/voices` | `{ voices: [...], uploaded_voices: [...] }`. On Base: `voices: ["default", "<registered>"…]` — `default` is a placeholder and is **not** synthesisable |
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

## Checkpoint coupling (`task_type`)

Each vLLM-Omni instance serves **one** checkpoint, and the server maps the
request to a task type before it reaches the model:

- a **preset** name (`aiden`, `vivian`, …) selects `CustomVoice`;
- a **registered** (uploaded or precomputed) voice name, or `ref_audio`/`ref_text`,
  selects `Base`;
- `VoiceDesign` must be requested explicitly.

A mismatch is a hard 400. Both directions have been observed on this host:

| Served checkpoint | Request | Error |
|---|---|---|
| CustomVoice | registered voice or `ref_audio` | `CustomVoice checkpoint does not support task_type='Base'` |
| Base | preset name, or `voice: "default"` | `Base checkpoint does not support task_type='CustomVoice'` |

Consequence of the 2026-10-13 switch to **Base**: synthesis only works with a
registered voice, so `OPENAI_TTS_VOICE` must name one (a preset name such as
`aiden` now fails at synthesis with the second error above), and `instructions`
is not supported — Base has no style control. The nine presets live only on the
CustomVoice checkpoint.

### Registering a voice (upload)

```sh
curl -s -X POST http://localhost:8025/v1/audio/voices \
  -F audio_sample=@clip.wav -F consent=self -F name=jorge \
  -F ref_text="<exact transcript of the clip>" \
  -F speaker_description="Spanish male, calm butler tone"
```

~3 s of clean single-speaker speech is enough; `ref_text` enables in-context
cloning (higher quality). The server enforces the reference length — **1–30 s**
(`Reference audio too short/long` 400s, verified live) — and ≤ 10 MB, in
wav/mp3/flac/ogg/aac/webm/mp4 at any sample rate (resampled internally to
24 kHz). Qwen's design point is ~3 s ("3-second rapid voice clone"); 3–10 s is
the comfortable range. Voices persist as `.safetensors` in
`/root/.cache/vllm-omni/speakers` — mounted to
`/mnt/disk3/ai/llama-server/vllm_omni_speakers` — and are restored on restart.
Re-uploading the same name overwrites it.

Observed timings on this host (first request per new voice): upload instant;
**first synthesis ~62 s** (reference extraction + compile), then **~0.85 s** for
a 3.6 s utterance through the `pcm` + `stream_format=audio` path, because the
reference features are cached in-process.

### Precomputed voices

`examples/online_serving/text_to_speech/qwen3_tts/precompute_custom_voice.py`
(inside the image, `--mode icl`) writes a profile directory that the server
loads at startup through the deploy-config field `custom_voice_dir` (here
`/voices_precomputed`, see `~/services/tesla-docker/vllm-omni/qwen3_tts.base.yaml`).
Usage is identical (`voice=<name>`), without a runtime upload.

## GPU budget

`seneschal-tts` shares GPU 0 with the 27B LLM and the ASR service
(`--stage-overrides` bring the two TTS stages down to ~0.09 + 0.06 VRAM, tuned
on CustomVoice; same weights, unverified on Base). At the time of writing the
full seneschal profile leaves ~15 GB free on GPU 0, so a second TTS container
for another checkpoint would not fit alongside it.

## Testing

```sh
# Unit + wiremock contract tests
cargo test -p seneschal-core --features openai-tts

# Real service (skipped when unreachable); wired as `make test-openai`
OPENAI_TTS_TEST_URL=http://localhost:8025 \
  cargo test -p seneschal-core --features openai-tts -- --ignored openai_live

# Manual
TTS_PROVIDER=openai OPENAI_TTS_VOICE=jorge cargo run --features openai-tts -- --list-voices
TTS_PROVIDER=openai OPENAI_TTS_INSTRUCTIONS="habla con entusiasmo" \
  RUST_LOG=performance cargo run --features openai-tts
```

Look for `[+Nms] SpeechStart → FirstAudioPlayback` in the logs; a medium sentence
should be well under 500 ms.
