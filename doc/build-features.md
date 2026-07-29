# Build Features & Dependencies

## Workspace crate features

These control which workspace crates are linked into the binary. The default
set includes the voice pipeline + essential tools + agents + plugins.

| Feature | Enables crate | Implies |
|---------|--------------|---------|
| (default) | `common`, `core`, `memory`, `tools-core`, `search`, `mcp`, `agents`, `plugins`, `extras` | — |
| `memory` | `seneschal-memory` | — |
| `tools-core` | `seneschal-tools-core` | — |
| `search` | `seneschal-search` | — |
| `mcp` | `seneschal-mcp` | — |
| `agents` | `seneschal-agents` | — |
| `classifier` | `seneschal-classifier` | — |
| `plugins` | `seneschal-plugins` | agents, mcp |
| `control` | `seneschal-control` | axum, tower |
| `remote` | `seneschal-remote` | control |
| `tui` | `seneschal-tui` | agents, ratatui, crossterm |
| `extras` | `seneschal-extras` | agents, plugins, search, mcp |
| `full` | all of the above | meta-feature |

## TTS/STT backend features

| Feature | Enables | Extra deps | Requirements |
|---------|---------|------------|--------------|
| `kokoro` | Kokoro ONNX TTS | kokorox | `brew install espeak-ng` |
| `avspeech` | macOS AVSpeechSynthesizer | objc2*, block2 | macOS only |
| `parakeet` | NVIDIA Parakeet STT (ONNX) | parakeet-rs | ParakeetTDT model files |
| `speech` | macOS SFSpeechRecognizer STT | speech | macOS only, microphone permission |
| `speaker` | Speaker verification | sherpa-rs | `models/speaker_embedding.onnx` |

**On macOS**: whisper-cpp-plus uses Metal by default (faster STT via metal feature).
Model files: `models/ggml-large-v3-turbo.bin` + `models/*-encoder.mlmodelc` for CoreML encoder fallback.

## Build commands

```bash
cargo build                         # default features (core + tools + agents)
cargo build --features full         # everything
cargo build --no-default-features   # core pipeline only (no tools, no agents)
cargo build --features tui          # default + TUI
cargo build --features control      # default + Control API
```
