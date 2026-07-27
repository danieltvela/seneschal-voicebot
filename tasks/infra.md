# Infrastructure & Build — Task List

Modules: `src/config.rs`, `src/daemon.rs`, `src/eyes.rs`, `src/device_monitor.rs`, `src/i18n.rs`, build system, CI/CD

---

## [M0.2] Configuration (`src/config.rs`)

- [x] `Config::from_env()` — all env var parsing
- [x] TOML config file support (`seneschal.{env}.toml`)
- [x] Config precedence: env > file path > env file > embedded defaults
- [ ] **Config validation**: on startup, validate all config values (valid URLs, file paths exist, numeric ranges); log warnings
- [ ] **Config hot-reload**: watch config file for changes; apply non-disruptive changes (TTS voice, LLM temperature) without restart
- [ ] **Config dump**: `--dump-config` flag prints the resolved config as TOML (with sensitive values redacted)
- [ ] **Config migration**: detect old config formats and auto-migrate to current schema

---

## [M0.4] Background Daemons

### InferenceDaemon (`src/daemon.rs`)
- [x] Periodic proactive reasoning loop (`DAEMON_INTERVAL_SECS`)
- [x] `complete_short()` lightweight LLM queries
- [ ] **Context-aware daemon**: pass current context (time of day, recent conversation topics, calendar state) to daemon prompts
- [ ] **Daemon suppression**: suppress daemon during important conversations (detected by urgency/intent classifier)
- [ ] **Daemon log**: record all daemon suggestions and whether the user engaged with them

### EyesDaemon (`src/eyes.rs`)
- [x] Periodic screenshot → vision LLM analysis (`EYES_INTERVAL_SECS`)
- [ ] **Region of interest**: only capture the active window (not the entire screen) for privacy
- [ ] **Change detection**: only send to vision LLM when the screen has changed significantly (pixel diff > threshold)
- [ ] **Privacy filter**: blur/detect sensitive content (passwords, emails, financial data) before sending to vision LLM
- [ ] **Eyes log**: record all visual observations with screenshots (capped at last 100)

---

## [M1.2] Device Monitor (`src/device_monitor.rs`)

- [x] Audio device connect/disconnect detection (Bluetooth headset)
- [ ] **Device preference**: remember user's preferred device; auto-switch when it becomes available
- [ ] **Multi-device routing**: route TTS output to multiple devices simultaneously (e.g., speakers + headset)

---

## [M0.2] Internationalization (`src/i18n.rs`)

- [x] Language-specific strings (Spanish/English)
- [ ] **String catalog**: move all user-facing strings to a TOML/JSON catalog; replace hardcoded Spanish strings
- [ ] **Language auto-detection**: detect user's preferred language from system locale on first run
- [ ] **Translation pipeline**: add script to extract strings, generate translation template, and validate coverage

---

## [M0.2] Build System

### Cargo Features
- [x] Feature flags: `avspeech`, `kokoro`, `parakeet`, `speech`, `speaker`, `tui`, `remote`, `control`
- [ ] **Feature combinations test**: CI should test all supported feature combinations (not just `tui,remote,control`)
  - Matrix: none, `avspeech`, `kokoro`, `avspeech+kokoro`, `parakeet`, `speech`, `speaker`, `tui`, `tui+avspeech`, `tui+control`, `remote+control`
- [ ] **Feature documentation**: generate a table of feature combinations and their binary sizes

### Makefile
- [x] `make qa` — fmt, lint, test, test-ci, test-e2e, build
- [x] `make qa-full` — adds audit + coverage
- [ ] `make bench` — latency benchmarking harness
- [ ] `make profile` — CPU/memory profiling with `cargo instruments`
- [ ] `make release` — full release pipeline: qa, tag, build, package

### Install Script (`install.sh`)
- [x] Automated model download and configuration
- [x] Multi-language support (es/en)
- [x] LLM provider discovery
- [ ] **Offline install**: detect existing models; skip download if already present
- [ ] **Update mode**: `install.sh --update` updates the binary and config but preserves data

---

## [M0.2] CI/CD

### Gitea Actions
- [x] CI workflow: build, test, lint
- [ ] **Multi-platform CI**: add Linux and Windows runners
- [ ] **Benchmark CI**: run latency benchmarks on each PR; flag regressions > 10%
- [ ] **Coverage reporting**: upload coverage data; enforce minimum coverage (e.g., 70%)
- [ ] **Release automation**: on tag push, build release binaries for all platforms, generate changelog, create Gitea release
- [ ] **Docker builds**: publish Docker images for headless deployment

### Dependency Management
- [ ] **Dependency audit**: run `cargo audit` in CI; block PRs with known vulnerabilities
- [ ] **Dependency update automation**: Dependabot or Renovate for automatic patch updates
- [ ] **License check**: run `cargo deny` in CI to verify all dependencies have compatible licenses

---

## [M1.1] Reliability & Observability

### Monitoring
- [ ] **Health endpoint**: add HTTP health check endpoint at `/health` (always on, not behind `control` feature)
- [ ] **Metrics collection**: expose pipeline metrics (utterances/hour, avg latency, tool call success rate) via `/metrics` endpoint
- [ ] **Crash reporting**: on panic, write crash report to `data/crashes/` with stack trace, config dump, and last N log lines

### Logging
- [x] `tracing` throughout with structured targets
- [ ] **Log rotation**: rotate `seneschal.log` daily; keep 7 days of logs
- [ ] **Log levels at runtime**: allow changing log filter at runtime via control API or TUI command
- [ ] **Structured log format**: add JSON log format option (`LOG_FORMAT=json`) for machine parsing

### Startup & Shutdown
- [ ] **Graceful shutdown**: on SIGTERM/SIGINT, drain pipeline, save state, close DB, stop CPAL streams, wait for background tools to complete (with timeout)
- [ ] **Startup precondition checks**: before spawning tasks, verify: audio device available (or TUI-only fallback), LLM URL reachable, DB writable, model files exist (for Whisper/Kokoro)

---

## [M0.2] Testing Infrastructure

- [x] Wiremock-based LLM client e2e tests
- [x] Synthetic audio VAD tests
- [ ] **Pipeline integration test harness**: create a reusable `TestPipeline` struct that wires up all actors with mock STT/LLM/TTS
- [ ] **Fuzzing**: add fuzz tests for VAD (random audio input), SentenceSplitter (random token streams), tool argument parsing (random JSON)
- [ ] **Property-based testing**: use `proptest` for config parsing, frame serialization, buffer operations

---

*Last updated: 2026-07-27.*
