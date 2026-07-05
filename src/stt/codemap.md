# src/stt/ — Speech-to-Text Pipeline

## Responsibility

Provide a pluggable speech-to-text abstraction that converts raw 16 kHz mono f32 audio into transcribed text. The module implements a `SttProvider` trait with two concrete backends: **Whisper** (via `whisper-cpp-plus`) and **Parakeet** (via `parakeet-rs` ONNX). Both backends share a common Voice Activity Detection (VAD) state machine built on the Silero VAD model from `whisper-cpp-plus`.

---

## Design

### `SttProvider` Trait (`provider.rs`)

```rust
#[async_trait]
pub trait SttProvider: Send {
    fn provider_name(&self) -> &'static str;
    async fn process_audio(&mut self, audio: &[f32], tx: &mpsc::Sender<SpeechEvent>) -> Result<()>;
    fn transcribe_complete(&self, audio: &[f32]) -> Result<String>;
}
```

- **`provider_name`** — returns `"whisper"` or `"parakeet"` for logging/identification.
- **`process_audio`** — streaming path: feed audio chunks, receive `SpeechEvent` signals via channel. The caller pushes raw f32 samples; the provider manages VAD state and emits `SpeechStart` / `SpeechEnd(text)` events.
- **`transcribe_complete`** — one-shot path: transcribe a complete audio buffer synchronously (used for testing/fallback).

### Factory (`create_provider`)

```rust
pub fn create_provider(config: &Config) -> Result<Box<dyn SttProvider>>
```

Selects backend based on `config.stt_provider`:
- `"whisper"` → `WhisperSttProvider` (always available)
- `"parakeet"` → `ParakeetSttProvider` (requires `--features parakeet`)

### Shared VAD State Machine

Both providers implement identical VAD logic:

1. **Probe buffering**: Incoming audio is accumulated in `probe_carry` until it fills a 200 ms window (3200 samples at 16 kHz).
2. **Silero VAD detection**: Each probe window is classified as speech/silence by `WhisperVadProcessor::detect_speech()`.
3. **Threshold hysteresis**: Separate `vad_start_threshold` (default 0.65) and `vad_end_threshold` (default 0.45) prevent rapid state flapping. End threshold is clamped to start threshold if configured higher.
4. **Pre-roll buffer** (300 ms / 4800 samples): Always retains the last 300 ms of audio before VAD onset so the first phoneme isn't clipped.
5. **Segment finalization**: When silence exceeds `silence_ms` threshold (default 500 ms) or the buffer reaches `MAX_SEGMENT_SAMPLES` (20 seconds), the accumulated audio is transcribed and `SpeechEnd(text)` is emitted.

### `WhisperSttProvider` (`whisper.rs`)

- Wraps `whisper-cpp-plus` `WhisperContext` (shared via `Arc`) + `WhisperVadProcessor`.
- Transcription runs on a blocking task (`tokio::task::spawn_blocking`) to avoid blocking the async runtime.
- Uses greedy decoding with `best_of: 1`, single-segment mode, no timestamps, no special tokens.
- Constants: `SAMPLE_RATE = 16000`, `VAD_PROBE_MS = 200`, `PRE_ROLL_MS = 300`, `MAX_SEGMENT_MS = 20000`.
- Backward-compat alias: `WhisperSTTVAD = WhisperSttProvider`.
- Config struct: `WhisperSTTVADConfig` (whisper_model, vad_model, language, silence_ms, vad_start_threshold, vad_end_threshold).

### `ParakeetSttProvider` (`parakeet.rs`)

- Wraps `parakeet-rs` `ParakeetTDT` model behind `Arc<Mutex<>>` for thread-safe blocking access.
- Shares Silero VAD from `whisper-cpp-plus` (same `WhisperVadProcessor`).
- **Post-roll**: After initial silence detection, enters `in_post_roll` state for 250 ms to capture trailing phonemes before finalizing. If speech resumes during post-roll, aborts post-roll and continues accumulating.
- **Leading silence trim**: `trim_leading_silence()` uses RMS analysis on 20 ms windows to remove quiet prefix before transcription. Threshold is 5% of peak RMS, capped at 90% of total audio.
- Constants: same as Whisper plus `POST_ROLL_MS = 250`, `TRIM_WINDOW_MS = 20`, `MAX_TRIM_PERCENT = 90`.
- Model loading: `ParakeetTDT::from_pretrained(model_dir, None)` — requires ONNX Runtime files (`encoder-model.onnx`, `decoder_joint-model.onnx`, `vocab.txt`).

### `SpeechEvent` Enum (`mod.rs`)

```rust
pub enum SpeechEvent {
    SpeechStart,
    Speech(String),      // intermediate (unused in current pipeline)
    SpeechEnd(String),   // final transcript for the segment
}
```

## Flow

### Streaming Audio → Transcript

```
Audio samples (f32, 16kHz mono)
  │
  ▼
SttProvider::process_audio(audio, tx)
  │
  ├─ probe_carry.extend(audio)
  │
  └─ while probe_carry.len() >= VAD_PROBE_SAMPLES (3200):
        │
        ├─ drain VAD_PROBE_SAMPLES → chunk
        │
        ├─ vad.detect_speech(chunk) → has_speech: bool
        ├─ vad.get_probs() → avg_prob
        ├─ threshold = if in_speech { vad_end_threshold } else { vad_start_threshold }
        └─ silence = avg_prob < threshold
        │
        └─ State machine:
            │
            ├─ !in_speech:
            │   ├─ !silence → SpeechStart
            │   │   ├─ in_speech = true
            │   │   ├─ speech_buf = pre_roll + chunk
            │   │   └─ tx.send(SpeechStart)
            │   └─ silence → keep pre_roll rolling
            │
            ├─ in_speech + !in_post_roll (Parakeet only):
            │   ├─ speech_buf.extend(chunk)
            │   ├─ silence → silence_samples += chunk.len()
            │   └─ !silence → silence_samples = 0
            │   │
            │   ├─ if silence_samples >= threshold → enter post_roll (Parakeet)
            │   │   or finalize directly (Whisper)
            │   └─ if speech_buf >= MAX_SEGMENT_SAMPLES → finalize
            │
            ├─ in_post_roll (Parakeet only):
            │   ├─ speech_buf.extend(chunk)
            │   ├─ silence → decrement post_roll_remaining
            │   ├─ !silence → abort post_roll, reset silence_samples
            │   └─ post_roll_remaining == 0 → finalize
            │
            └─ finalize:
                ├─ audio = mem::take(&mut speech_buf)
                ├─ (Parakeet) trim_leading_silence(audio)
                ├─ spawn_blocking(|| transcribe(ctx/model, language, audio))
                ├─ in_speech = false
                └─ tx.send(SpeechEnd(text))
        │
        └─ pre_roll.push_back(chunk) — maintain 300ms pre-roll window
```

### One-shot Transcription

```
transcribe_complete(audio)
  → WhisperSttProvider: ctx.full(params, audio) → iterate segments → join text
  → ParakeetSttProvider: model.transcribe_samples(audio, 16000, 1, None) → result.text
```

### Leading Silence Trim (Parakeet only)

```
trim_leading_silence(audio)
  → compute RMS per 20ms window (320 samples)
  → max_rms = max of all windows
  → threshold = max(max_rms * 0.05, 0.001)
  → max_trim_windows = 90% of total windows
  → count consecutive windows below threshold from start
  → return audio[trim_windows * 320 ..]
```

---

## Integration

### Dependencies
- **whisper-cpp-plus** — `WhisperContext` (transcription), `WhisperVadProcessor` (Silero VAD), `FullParams`, `SamplingStrategy`
- **parakeet-rs** (feature `parakeet`) — `ParakeetTDT`, `Transcriber` trait
- **async-trait** — async trait support
- **tokio** — `sync::mpsc`, `task::spawn_blocking`
- **anyhow** — error handling

### Consumers
- **`src/daemon.rs` (InferenceDaemon)** — creates provider via `create_provider()`, feeds audio chunks from capture loop, receives `SpeechEvent` on channel
- **`src/pipeline/`** — consumes `SpeechEvent::SpeechEnd(text)` to trigger LLM conversation turn
- **`src/config.rs`** — provides `stt_provider`, `whisper_model`, `vad_model`, `vad_silence_ms`, `vad_start_threshold`, `vad_end_threshold`, `parakeet_model_dir`

### Environment Variables
- `STT_PROVIDER` — `"whisper"` (default) or `"parakeet"`
- `WHISPER_MODEL` — path to GGML Whisper model (default: `models/ggml-large-v3-turbo.bin`)
- `WHISPER_THREADS` — CPU thread count (0 = auto)
- `PARAKEET_MODEL_DIR` — required when `STT_PROVIDER=parakeet`
- `VOICEBOT_LANGUAGE` — language hint for Whisper (default: `en`)