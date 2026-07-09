# Changelog

All notable changes to Seneschal Voicebot will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## v0.1.0-alpha.5 (2026-07-09)

### Features
- **[#120](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/120)**: Conversation improvements — spoken fillers, async tool result injection, subtask tracking, background sound during tool calls
- **[#118](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/118)**: Reduced AGENTS.md size by splitting documentation into `doc/` directory

### Bug Fixes
- **[#121](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/121)**: Install script LLM provider check used wrong URL

---

## v0.1.0-alpha.4 (2026-07-08)

### Features
- **[#104](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/104)**: macOS SFSpeechRecognizer STT provider
- **[#108](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/108)**: CHANGELOG.md and `/changelog` command
- **[#109](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/109)**: macOS speech as default STT provider in installer
- **[#114](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/114)**: Auto-discover LLM models during installation

### Bug Fixes
- **[#113](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/113)**: Configured API key not sent to LLM provider
- EOF safety for wake word read in install.sh

### Other
- **[#103](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/103)**: Research on STT false positives from coughs and non-speech sounds

---

## v0.1.0-alpha.3 (2026-07-08)

### Features
- **[#99](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/99)**: App starts even when no audio device is found
- **[#100](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/100)**: Project renamed from Voicebot to Seneschal
- Configurable voicebot name via `VOICEBOT_NAME` env var
- Brave Search web search provider integration
- Apple Events tools for macOS automation
- First startup welcome message
- Input device change monitoring — detects Bluetooth headset reconnect
- LLM requirements and benchmarks documentation

### Bug Fixes
- **[#96](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/96)**: Removed hardcoded Hermes routing section from system prompt
- **[#97](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/97)**: Device monitor now detects reconnected audio devices
- **[#93](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/93)**: Second request cache miss after launch
- **[#91](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/91)**: LLM model config value not loading from pro environment
- Fixed environment variables in launch script
- Fixed first launch initialization

### Other
- **[#92](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/92)**: Investigated audio device reconnection detection
- **[#94](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/94)**: Research on system/developer message roles in OpenAI-compatible APIs
- **[#95](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/95)**: LLM response handling for system/developer message roles
- Removed ContextLens from main pipeline
- Added `developer` message role support
- README restructured with Quick Start at the top
- Full codemap documentation

---

## v0.1.0-alpha.2 (2026-06-27)

### Features
- **[#77](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/77)**: `voicebot.toml` config file with env override
- **[#78](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/78)**: PRO and DEV environment separation
- **[#80](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/80)**: Apple Watch companion app scaffolding
- **[#81](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/81)**: Plugin system foundation
- iOS companion app Xcode project
- LLM thinking/support mode
- S-DREAM memory consolidation daemon
- Database inspection tool (`db-inspect`)
- Agent pre-warming on startup

### Bug Fixes
- **[#82](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/82)**: Installer script fixes
- **[#83](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/83)**: Screenshot tool SSH permission diagnostics
- **[#84](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/84)**: Installation bugs and CI fixes
- **[#85](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/85)**: install.sh improvements
- Shared `screen_capture.rs` utility consolidating duplicate code
- Kokoro TTS ONNX Runtime API compatibility patch
- CI workflow macOS-only adjustments
- Installer language prompt and config generation

### Other
- Removed llama-cpp-2 in-process LLM provider
- Removed Gitea references from user-facing code
- Removed LLM evaluation framework
- UI improvements for iOS companion app
- iOS messages view and connection state indicators

---

## v0.1.0-alpha.1 (2026-06-09)

### Features
- **[#42](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/42)**: AI-agent-runnable QA harness (`make qa`)
- **[#46](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/46)**: NOOP tool for idle pipeline handling
- **[#48](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/48)**: `recover_historical_context` tool with FTS5 search
- **[#50](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/50)**: Terminal-native scrollback for TUI message history
- **[#53](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/53)**: iOS companion app concept
- **[#54](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/54)**: STT improvements
- **[#55](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/55)**: Parakeet STT provider with Spanish support
- **[#59](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/59)**: Forced tool selection via ToolRegistry
- **[#63](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/63)**: Clean LLM provider architecture
- Multi-MCP server support with namespace prefixing
- Session search tool
- Tiered web search (fast path vs deep research)
- Barge-in transcript append to last user turn
- Uninstall script (`uninstall.sh`)
- S-DREAM cold-path memory consolidation (L1/L2)
- Speaker verification module
- Kokoro ONNX TTS backend
- WebSocket remote audio streaming server

### Bug Fixes
- Parakeet STT Spanish transcription quality
- Stream input audio handling
- STT VAD audio processing
- Current_time tool explicit-request detection
- Flaky current_time test
- Vendor COPY in CI Dockerfile

### Other
- **[#47](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/47)**: STT→LLM latency improvement research
- **[#49](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/49)**: Voicebot R&D exploration
- **[#51](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/51)**: Niche use case analysis
- **[#52](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/52)**: Multi-user speech detection research
- **[#56](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/56)**: Evaluation test cases
- **[#57](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/57)**: Multi-user LLM research
- **[#58](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/58)**: Embedded LLM exploration
- **[#60](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/60)**: Telnyx research
- **[#64](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/64)**: apple/ml-ssd model research
- **[#73](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/73)**: Self-improvement analysis
- **[#74](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/74)**: Database inspection website
- **[#75](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/75)**: Config file design (replaced by #77)
- **[#76](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/76)**: Secondary LLM as agent research