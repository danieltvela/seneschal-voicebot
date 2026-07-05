# src/tts/ — Text-to-Speech Engines

## Responsibility

Convert LLM-generated text into playable audio (mono f32 PCM samples). Provides a unified `TtsEngine` enum that abstracts over multiple synthesis backends (macOS AVSpeech, Kokoro ONNX) so the pipeline is backend-agnostic. Includes sentence-boundary splitting to enable streaming synthesis — audio begins playing before the full LLM response is complete.

---

## Design

| File | Role |
|------|------|
| `mod.rs` | Module declarations + `TtsEngine` enum + test-only `MockTts`. |
| `avspeech.rs` | `AvSpeechTts` — macOS `AVSpeechSynthesizer` via `objc2` bindings. Synthesizes in-process, captures PCM into buffer (no subprocess). Requires `--features avspeech`. |
| `kokoro.rs` | `KokoroTts` — Kokoro ONNX model via `kokorox` crate. CPU-based, 24 kHz output. Requires `--features kokoro` + espeak-ng. |
| `sentence.rs` | `SentenceSplitter` — buffers streaming tokens, emits complete sentences at punctuation boundaries. Implements first-sentence acceleration (early split at commas/dashes). |
| `piper.rs` | `PiperTts` — subprocess-based Piper TTS wrapper (kept for reference, not active in pipeline). |

### `TtsEngine` Enum (`mod.rs`)

```rust
pub enum TtsEngine {
    #[cfg(feature = "avspeech")]
    AvSpeech(AvSpeechTts),
    #[cfg(feature = "kokoro")]
    Kokoro(KokoroTts),
    #[cfg(test)]
    Mock(mock_tts::MockTts),
}
```

Unified interface:
- **`synthesize(&self, text: &str) -> Result<Vec<f32>>`** — synthesize text to mono f32 PCM.
- **`sample_rate(&self) -> u32`** — output sample rate (22050 for AVSpeech, 24000 for Kokoro).

### `SentenceSplitter` (`sentence.rs`)

Stateful tokenizer that buffers incoming LLM tokens and emits complete sentences:

- **Sentence boundary regex**: `[.!?;:]+(?:\s|\n)` — requires whitespace after punctuation to avoid false positives on decimals (`3.14`) and times (`10:30`).
- **First-sentence acceleration**: The first sentence of each response splits more aggressively:
  - At commas/dashes (`[,\-—]+\s`) after 20+ characters (`EARLY_SPLIT_MIN_CHARS`).
  - Fallback: at last whitespace before 80 characters (`EARLY_SPLIT_MAX_CHARS`) if no punctuation found.
- **`flush()`** — emits remaining buffer as final fragment, resets `first_emitted` flag.

### `AvSpeechTts` (`avspeech.rs`)

- Uses `objc2_avf_audio` bindings to call `AVSpeechSynthesizer.writeUtterance_toBufferCallback:` directly.
- **Threading**: AVSpeech callbacks fire on the main thread's CFRunLoop. Synthesis is dispatched via GCD (`dispatch_async_f` to `DISPATCH_MAIN_Q`). The calling thread blocks on `Condvar` until callbacks complete.
- **Sample rate probing**: On construction, synthesizes a single period (`.`) to discover the actual sample rate (typically 22050 Hz).
- **Silence prepend**: `synthesize()` prepends 30 ms of silence to absorb CoreAudio stream-init latency.
- Voice lookup: exact display name match (e.g., `"Jorge (Enhanced)"` → `"com.apple.voice.enhanced.es-MX.Jorge"`).

### `KokoroTts` (`kokoro.rs`)

- Wraps `kokorox::tts::koko::TTSKoko` (ONNX-based, 24 kHz output).
- Async constructor (`new() -> Result<Self>`) loads model + voice embeddings.
- CPU-intensive synthesis — intended to be called from `tokio::task::spawn_blocking`.
- Voice naming: `{lang}{gender}_{name}` (e.g., `af_bella`, `em_jamil`, `ef_silvia`).

### `PiperTts` (`piper.rs`) — Reference Only

- Subprocess-based: spawns `piper --model <path> --output_raw`, writes text to stdin, reads raw 16-bit PCM from stdout.
- Not integrated into `TtsEngine` enum. Kept as reference implementation.

### `MockTts` (`mod.rs` — test-only)

- Captures synthesized text into `Arc<Mutex<Vec<String>>>` instead of producing audio.
- Returns single silent sample (`vec![0.0]`) so `AudioOutput::play_blocking` returns immediately.

---

## Flow

### LLM Streaming → Sentence Splitting → TTS → Audio Output

```
LLM token stream (SSE)
  │
  ▼
SentenceSplitter::push(token)
  │
  ├─ buffer.push_str(token)
  ├─ check sentence_end_re: [.!?;:]+(?:\s|\n)
  │   └─ match → emit sentence, clear buffer up to match
  │
  ├─ first-sentence acceleration (if !first_emitted && buffer >= 20 chars):
  │   ├─ check early_split_re: [,\-—]+\s
  │   │   └─ match at position >= 20 → emit, clear buffer
  │   └─ if buffer >= 80 chars → split at last whitespace before 80
  │
  └─ return Some(sentence) | None

  → on Some(sentence):
      TtsEngine::synthesize(sentence)
        → AvSpeech: synth_text(text, voice_id, rate)
            → dispatch_synth_on_main(text, id, rate, block_ptr)
            → GCD dispatch to main queue
            → AVSpeechSynthesizer.writeUtterance_toBufferCallback
            → callbacks accumulate f32 samples
            → Condvar signals completion
            → prepend 30ms silence
        → Kokoro: inner.tts_raw_audio(text, lang, voice, speed=1.0)
            → ONNX inference → f32 PCM @ 24kHz

      → Vec<f32> samples at engine sample rate
      → AudioOutput::play_blocking(samples, sample_rate, cancel)
          → resample to device rate + expand channels
          → CPAL playback (blocking until done or cancelled)
```

### End of LLM Stream

```
SentenceSplitter::flush()
  → trim buffer → if non-empty → Some(remaining)
  → clear buffer, reset first_emitted
  → (if Some) → TtsEngine::synthesize(remaining) → AudioOutput::play_blocking
```

---

## Integration

### Dependencies
- **objc2 / objc2-avf-audio / objc2-foundation / block2** (feature `avspeech`) — macOS AVSpeech bindings
- **kokorox** (feature `kokoro`) — Kokoro ONNX TTS engine
- **regex** — sentence boundary detection (`OnceLock<Regex>` for lazy init)
- **anyhow** — error handling
- **std::sync** — `Arc<Mutex>`, `Arc<Condvar>` for AVSpeech callback synchronization
- **GCD FFI** — `dispatch_async_f`, `DISPATCH_MAIN_Q` for main-thread dispatch

### Consumers
- **`src/pipeline/`** — creates `TtsEngine`, feeds sentences from `SentenceSplitter`, plays audio via `AudioOutput`
- **`src/tts/sentence.rs`** — used by pipeline to split streaming LLM tokens into sentences
- **`src/config.rs`** — provides `tts_provider`, `avspeech_voice`, `avspeech_rate`, `kokoro_model_path`, `kokoro_voices_path`, `kokoro_voice`

### Environment Variables
- `TTS_PROVIDER` — `"avspeech"` (default) or `"kokoro"`
- `AVSPEECH_VOICE` — voice display name (default: `"Jorge (Enhanced)"`)
- `AVSPEECH_RATE` — normalized rate [0.0, 1.0] (default: 0.55 ≈ 215 wpm)
- `VOICEBOT_LANGUAGE` — affects Kokoro voice selection (`en` → `af_*`/`am_*`, `es` → `ef_*`/`em_*`)

### Voice Listing
- `cargo run -- --list-voices` or `LIST_VOICES=1 cargo run`
- `AvSpeechTts::list_voices()` — iterates `AVSpeechSynthesisVoice::speechVoices()`
- `KokoroTts::list_voices()` — iterates `inner.get_available_voices()`, parses prefix for language/gender