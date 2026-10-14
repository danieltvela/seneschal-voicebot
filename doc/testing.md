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

## STT/VAD contract tests (regression guard)

The STT/VAD subsystem has a written contract (`doc/stt-contract.md`, P1–P6)
and three test layers that lock it:

1. **L1 — deterministic unit tests** (no models, run in `make qa`): state
   machine + streaming orchestrator behavior in
   `crates/seneschal-core/src/stt/segmenter.rs` and `streaming.rs` (onset
   preservation, commit budget, trailing silence, hysteresis, live cadence,
   single-flight, stale discard).
2. **L2 — pipeline contract tests** (no models): `src/audio_events.rs` drives
   the extracted SpeechEvent handler with a mock STT provider (barge-in on
   SpeechStart, TranscriptReady gating, stale drop, NoSpeechGate, ambient
   gating).
3. **L3 — real-model audits** (`make test-stt`, `#[ignore]`, espeak fixtures
   under `target/stt-fixtures/`): `tests/stt_audit_contract.rs` (onset/tail/
   live), `stt_audit_continuous.rs` (silence-only close on >25 s of unbroken
   speech, commit budget), plus
   code-switching and terms audits. Provider selection via
   `STT_AUDIT_PROVIDER=whisper|openai`. The audits assert wall-clock RTF and
   commit budgets measured against real decode work, so run them serialized
   (`-- --ignored stt --test-threads=1`, as `make test-stt` does); parallel
   test threads oversubscribe the CPU on CPU-only hosts and trip the budgets.

When a budget trips in L3, run the `vad_probe_calibration` audit first —
Silero's probability behavior on synthetic audio is the usual suspect.

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