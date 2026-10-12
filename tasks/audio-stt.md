# Audio & STT — Task List

Module: `src/audio/`, `src/stt/`

---

## [M0.2] Audio Pipeline

### Audio Capture (`src/audio/audio_capture.rs`)
- [x] CPAL mic capture with configurable sample rate, channels, chunk size
- [x] `async_channel` (capacity 200) to main loop
- [x] Device substring matching (`AUDIO_INPUT_DEVICE`, `AUDIO_OUTPUT_DEVICE`)
- [ ] **Backpressure**: if VAD/STT is delayed, audio chunk accumulation causes memory growth
  - Add a drop-oldest policy or backpressure signal when channel exceeds 150/200 capacity
  - Verify: memory stays stable under sustained load
- [ ] **Device index disambiguation**: when multiple devices match the same substring, `#N` suffix selects the Nth match — add test

### Audio Resampling (`src/audio/audio_transform.rs`)
- [x] Rubato resampling from arbitrary input rate to 16000 Hz
- [ ] **Resampling quality config**: add env var `AUDIO_RESAMPLE_QUALITY` (low/medium/high) for CPU/quality tradeoff

### Audio Output (`src/audio/output.rs`)
- [x] CPAL speaker playback with blocking write
- [x] `play_cancel: AtomicBool` shared with TTS task
- [ ] **Output device change handling**: if output device disconnects (Bluetooth speaker), attempt reconnect or fall back to default
- [ ] **Playback underrun guard**: if CPAL callback requests more data than available, insert silence instead of glitching

### Audio Buffer (`src/audio/buffer.rs`)
- [x] Circular `VecDeque<f32>` buffer with write/read/drain
- [ ] **Buffer size limit**: enforce max buffer size (e.g., 30 seconds of 16kHz mono = 480000 samples); log warning on overflow

### Ambient Buffer (`src/audio/ambient_buffer.rs`)
- [x] Rolling window buffer for ambient speech transcription
- [ ] **Persistence**: on shutdown, save ambient buffer to DB; restore on startup
- [ ] **Language tagging**: tag each buffered utterance with detected language

---

## [M0.2] STT Providers

### WhisperSTTVAD (`src/stt/mod.rs` + `src/stt/whisper.rs`)
- [x] Unified STT+VAD via whisper-cpp-plus + Silero VAD
- [x] 200ms probe windows, 300ms pre-roll, 20s hard cap
- [x] `SpeechEvent::SpeechStart`, `SpeechEvent::SpeechEnd(transcript)`, `SpeechEvent::Silence`
- [x] Configurable silence threshold (`VAD_SILENCE_MS`), language (`SENECHAL_LANGUAGE`)
- [ ] **Transcription timeout**: if whisper-cpp-plus takes > 5 seconds for a segment, abort and log; fall back to empty transcript
- [ ] **Model reload**: add capability to reload Whisper model at runtime (for language/model switching)
- [ ] **Confidence score**: expose whisper-cpp-plus confidence per segment in `SpeechEnd`; log low-confidence transcripts
- [ ] **Legacy whisper.rs removal**: the deprecated `src/stt/whisper.rs` (whisper-rs) is still present; remove after confirming no remaining callers

### Parakeet (`src/stt/parakeet.rs`)
- [x] ParakeetTDT ONNX as `SttProvider`
- [ ] **Model warmup**: ONNX first inference can take several seconds; add warmup inference on startup
- [ ] **Language detection accuracy**: ParakeetTDT auto-detects language; log detection accuracy vs `SENESCHAL_LANGUAGE` hint

### SFSpeechRecognizer (`src/stt/speech_recognizer.rs`)
- [x] macOS SFSpeechRecognizer via `speech` crate
- [ ] **60s task limit handling**: Apple framework 60-second limit per utterance — restart the recognizer automatically on long speech
- [ ] **On-device vs server fallback**: detect when on-device recognition is unavailable (first launch, no models downloaded) and show clear error message

### NoSpeechGate (`src/stt/mod.rs`)
- [x] Rejects coughs and non-speech sounds
- [ ] **Tunable threshold**: add `VAD_NOISE_REJECTION` env var for sensitivity tuning
- [ ] **Logging**: log rejected audio segments with reason (too short, low confidence, non-speech)

---

## [M0.2] Speaker Verification

### Speaker module (`src/audio/speaker.rs`)
- [x] Sherpa-onnx speaker embedding ONNX model
- [x] Auto-enrollment up to N profiles
- [x] Cosine similarity matching
- [ ] **Model path auto-detection**: if `SPEAKER_MODEL` not set, search standard paths (`models/speaker_embedding.onnx`, `~/.seneschal/models/`)
- [ ] **Enrollment quality check**: reject low-quality enrollments (ambient noise, too short) with user feedback
- [ ] **Profile persistence**: save/load speaker profiles to/from DB (currently file-based `data/speaker.emb`)

### Identity Analyzer (`src/analysis/identity.rs`)
- [x] Ties speaker verification to conversation mode switching
- [ ] **Confidence smoothing**: use rolling average of recent similarity scores (not point-in-time) for mode switching decisions

---

## [M0.4] Emotion Detection

- [ ] **Prosody analyzer**: detect speaking rate, pitch variance, volume from audio signal
- [ ] **Emotion labels**: classify as neutral, urgent, frustrated, happy, tired
- [ ] **ContextLens integration**: write emotion tags to ContextLens bus
- [ ] **TTS adaptation**: adjust TTS rate and voice based on detected emotion (slower for frustrated, upbeat for happy)

---

## [M1.1] Reliability

- [ ] **Audio device reconnect**: when audio device disconnects (Bluetooth headset), detect and reconnect without restart
- [ ] **CPAL stream recovery**: if CPAL stream errors mid-run, attempt restart up to 3 times before fatal
- [ ] **VAD calibration test**: at startup, run a silent VAD calibration pass to set noise floor baseline

---

*Last updated: 2026-07-27.*
