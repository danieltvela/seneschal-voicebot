# Testing

- **VAD/audio tests**: Use synthetic sine waves / silence (see `src/audio/` tests).
- **STT tests**: Skip if model file missing (`#[ignore]`). Uses `whisper-cpp-plus`.
- **TTS tests**: macOS requires voices installed; kokoro for Linux CI.
- **Parallel tests**: Use `temp-env` crate to safely override env vars.
- **Mock LLM**: Use `wiremock` crate for HTTP client tests.

Run specific test:
```bash
cargo test <test_name> -- --nocapture
```

## Debugging Binary

The `test_stt_plus` binary provides standalone STT testing without full pipeline:
```bash
cargo run --bin test_stt_plus --release
```

## Adding New Test Categories

- Default unit tests: `#[test]` / `#[tokio::test]` anywhere under `src/`. Picked up by `test`.
- Wiremock-based e2e: add to `src/e2e_tests.rs` and mark `#[ignore]`. Picked up by `test-e2e`.
- Real-LLM / real-audio integration: `#[ignore]` + check env at top of fn. Add to `test-llm` / `test-stt` filter patterns.
- Zero-coverage modules: **add unit tests**, do not ignore them.