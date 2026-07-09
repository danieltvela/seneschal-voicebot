# Build Features & Dependencies

| Feature | Enables | Extra deps | Requirements |
|---------|---------|------------|--------------|
| (none) | Core pipeline | whisper-cpp-plus, reqwest, sqlx | — |
| `parakeet` | NVIDIA Parakeet STT (ONNX) | parakeet-rs | ParakeetTDT model files |
| `speech` | macOS SFSpeechRecognizer STT | speech | macOS only, microphone permission |
| `kokoro` | Kokoro ONNX TTS | kokorox | `brew install espeak-ng` |
| `tui` | Terminal UI | ratatui, crossterm | — |
| `remote` | WebSocket server | axum, tower | — |
| `speaker` | Speaker verification | sherpa-rs | `models/speaker_embedding.onnx` |
| `avspeech` | macOS AVSpeechSynthesizer | objc2*, block2 | macOS only |

**On macOS**: whisper-cpp-plus uses Metal by default (faster STT via metal feature). Model files: `models/ggml-large-v3-turbo.bin` + `models/*-encoder.mlmodelc` for CoreML encoder fallback.