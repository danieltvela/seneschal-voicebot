# TTS — Task List

Module: `src/tts/`

---

## [M0.2] TtsEngine & Backends

### TtsEngine enum (`src/tts/mod.rs`)
- [x] `avspeech` — macOS AVSpeechSynthesizer (default)
- [x] `kokoro` — Kokoro ONNX model
- [x] `mock` — test-only
- [ ] **Provider switching at runtime**: allow changing TTS provider without restart (e.g., "switch to Kokoro")
- [ ] **Voice enumeration**: `--list-voices` flag already works; add `list_voices()` tool callable by LLM

### AvSpeech (`src/tts/avspeech.rs`)
- [x] macOS AVSpeechSynthesizer bindings via objc2
- [x] Configurable voice (`AVSPEECH_VOICE`), rate (`AVSPEECH_RATE`)
- [ ] **Voice quality settings**: expose pitch, volume, pre/post utterance delay via env vars
- [ ] **Voice caching**: cache `AVSpeechSynthesisVoice` lookup to avoid repeated searches
- [ ] **Fallback voice**: if configured voice is not found, fall back to system default voice with a log warning

### Kokoro (`src/tts/kokoro.rs`)
- [x] Kokoro ONNX via `kokorox` crate (patched)
- [x] Configurable voice style (`KOKORO_VOICE`), language (`KOKORO_LANGUAGE`)
- [ ] **Model loading time**: first synthesis takes ~3-5 seconds to load the ONNX model; warm up on startup
- [ ] **Memory usage**: Kokoro ONNX model can use 2+ GB; profile and log memory usage at startup
- [ ] **Voice style discovery**: auto-detect available voice styles from `voices-v1.0.bin`; expose via `--list-voices`

### Piper (`src/tts/piper.rs`)
- [ ] **Evaluate integration**: determine whether Piper should be integrated into `TtsEngine` enum or remain reference-only
- [ ] **If integrated**: add `piper` variant to `TtsEngine`, feature flag, config vars, `--list-voices` support
- [ ] **If not**: add doc comment explaining why (maintenance burden, quality, licensing)

---

## [M0.2] SentenceSplitter (`src/tts/sentence.rs`)

- [x] Buffers tokens until sentence boundary (`.`, `!`, `?`, `;`, `:`)
- [x] Flush on `LLMResponseDone`
- [ ] **Additional boundaries**: add `\n\n` (paragraph break) as sentence boundary
- [ ] **Abbreviation handling**: don't split on `.` in known abbreviations (Mr., Dr., etc., Sr., Sra.)
- [ ] **Minimum sentence length**: don't emit sentences shorter than 10 characters (avoids TTS of single punctuation marks)
- [ ] **Maximum sentence length**: split sentences longer than 500 chars at comma boundaries (avoids TTS buffer overflow)
- [ ] **Language-aware splitting**: Spanish uses `¿...?` and `¡...!` — ensure splitter handles these correctly

---

## [M0.3] TTS Tool Support

- [ ] **Spoken fillers**: when a tool call runs asynchronously, insert a brief spoken filler ("Déjame ver...", "Un momento...") before tool execution starts
- [ ] **Background sound**: during long-running tool calls, play a subtle background sound (tick, chime) to indicate the pipeline is alive
- [ ] **Tool result vocalization**: after background tool completes, vocalize a summary ("He encontrado 3 resultados...")

---

## [M1.1] Reliability

- [ ] **Synthesis timeout**: if `synthesize()` takes > 5 seconds, skip the sentence and log an error
- [ ] **Audio output device**: if output device is unavailable, log clearly and fall back to TUI-only mode
- [ ] **Concurrent synthesis guard**: ensure only one synthesis runs at a time (TtsEngine is not `Sync` for AvSpeech)

---

## [M3.2] Advanced TTS

- [ ] **Voice cloning**: capture user voice sample → fine-tune Kokoro or train custom voice model
- [ ] **Emotion-aware TTS**: adjust rate, pitch, and voice based on detected user emotion
- [ ] **Multi-language TTS**: auto-detect language in LLM response and switch TTS voice accordingly

---

*Last updated: 2026-07-27.*
