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
| `openai-tts` | OpenAI-compatible TTS (Qwen3-TTS via vLLM-Omni) | — (reqwest already present) | reachable speech service (`OPENAI_TTS_URL`) |
| `parakeet` | NVIDIA Parakeet STT (ONNX) | parakeet-rs | ParakeetTDT model files |
| `speech` | macOS SFSpeechRecognizer STT | speech | macOS only, microphone permission |
| `speaker` | Speaker verification | sherpa-rs | `models/speaker_embedding.onnx` |
| `cuda` | CUDA for whisper.cpp + ONNX Runtime (Parakeet/Kokoro) | — | Linux/NVIDIA host, CUDA toolkit (`nvcc`); ONNX Runtime GPU binaries are downloaded at build time |

**On macOS**: whisper-cpp-plus uses Metal by default (faster STT via metal feature).
Model files: `models/ggml-large-v3-turbo.bin` + `models/*-encoder.mlmodelc` for CoreML encoder fallback.

**On Linux**: whisper-cpp-plus-sys compiles ggml-cpu with OpenMP but does not link
`libgomp`, so the final link fails with undefined `GOMP_*` symbols. The workspace
`.cargo/config.toml` adds `-C link-arg=-fopenmp` for Linux targets to fix this.

## Multi-GPU (issue #14)

On CUDA builds the local models can be spread across GPUs. `GPU_AUTO` (default
on) assigns Silero VAD, Parakeet and Kokoro round-robin over the GPUs with the
most free VRAM; `VAD_GPU`, `PARAKEET_GPU` and `TTS_GPU` pin a model to a device
index or to `cpu`. Whisper follows whisper.cpp's default device (GPU 0 of the
visible devices) — use `CUDA_VISIBLE_DEVICES` to reorder devices for it.

```bash
# Build with CUDA on a Linux/NVIDIA host
cargo build --features "kokoro,parakeet,cuda"

# Inspect detected GPUs and the resolved assignment
./linux-seneschal.sh --list-devices
SENECHAL_CUDA=1 ./linux-seneschal.sh   # opt-in CUDA build via the launcher
```

Device indices refer to the devices visible to the process (after
`CUDA_VISIBLE_DEVICES` filtering). The external LLM server is pinned separately
(e.g. `CUDA_VISIBLE_DEVICES=1 vllm serve ...`).

## Build commands

```bash
cargo build                         # default features (core + tools + agents)
cargo build --features full         # everything
cargo build --no-default-features   # core pipeline only (no tools, no agents)
cargo build --features tui          # default + TUI
cargo build --features control      # default + Control API
```
