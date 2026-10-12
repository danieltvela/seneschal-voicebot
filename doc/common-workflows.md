# Common Workflows

## Running Development

```bash
# 1. Ensure .env exists (cp .env.example .env if not)
# 2. Start external LLM server (mlx-lm or oMLX)
# 3. Run seneschal
cargo run --features tui --release
```

## iOS Companion (LAN dual-channel)

```bash
# Host: remote audio + Control API
export WS_PORT=9090
export CONTROL_PORT=9001
cargo run --features "remote,control,tui" --release

# Client: open clients/voicebot-ios-companion in Xcode
# Connect with host IP, WS 9090, Control 9001
```

Full architecture, Control event list, and QA matrix: [`doc/IOS_COMPANION.md`](IOS_COMPANION.md).

## Adding a New Feature

1. Read AGENTS.md first (architecture guidance).
2. Check existing tools/agents to avoid duplication.
3. If feature affects multiple modules, create integration test in `e2e_tests.rs`.
4. Update AGENTS.md if you discover new conventions.

## Debugging Pipeline Latency

Check these stages:
- VAD sensitivity: `src/audio/vad.rs` (frame thresholds).
- STT provider choice: Whisper vs Parakeet (`STT_PROVIDER` env var).
- Whisper decoding: `src/stt/whisper.rs` (thread count, model size).
- Parakeet decoding: `src/stt/parakeet.rs` (ONNX Runtime, model size).
- LLM response time: External server config, context window size.
- TTS synthesis: `say` vs Kokoro vs AVSpeech backend choice.

Log with `RUST_LOG=trace cargo run` for detailed timing.

## References

- `LICENSE-SENESCHAL.md`: Trademark information
- `readme.md`: User-facing documentation
- `CONTRIBUTING.md`: Contributor guidelines
- `secondary-agent.md`: Secondary LLM orchestration design (Spanish)