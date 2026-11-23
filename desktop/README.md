# Desktop app

Tauri v2 shell for the Seneschal daemon. It launches the real `seneschal`
binary as a sidecar, connects to the existing Control SSE API, renders live
`PipelineState`, and manages Settings over the Control API (issue #33).

The Audio view (issue #34) exposes device pickers, a live VU meter, TTS
preview, speaker test, mic→STT test and loopback. Audio tests use the active
daemon configuration; restart-tier device/provider changes wait for the
sidecar reconnect before the test buttons are enabled.

Settings edits the active TOML config, validates before applying, and uses a
tiered apply model: TTS voice/rate, output volume, language and wake word apply
without a restart; backend/model/URL/port changes restart the sidecar
automatically while the window stays open.

## Commands

```bash
npm install

# Build the sidecar (release, feature-gated)
npm run sidecar:build

# Build sidecar + launch the desktop app
npm run dev

# Type check, frontend tests and production frontend build
npm run check
npm test
npm run build

# Rust shell tests/lint
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --no-deps -- -D warnings
```

## Audio diagnostics QA

- Restart-tier device/provider changes resolve only after the shell observes a
  fresh connected Control SSE snapshot; then the test buttons are enabled.
- Mic test and loopback require an active capture and an idle pipeline. They do
  not create conversational turns or open a second capture stream.
- macOS packaged app: the Info.plist includes `NSMicrophoneUsageDescription`.
  To re-test the prompt, reset TCC for `org.veladan.seneschal` with
  `tccutil reset Microphone org.veladan.seneschal`, then use the Audio panel.
- Linux: CPAL uses ALSA names, which can differ from PipeWire's friendly names.
  Compare picker values with `pactl list short sources` / `sinks` when
  debugging.

On macOS, the default sidecar enables `control,avspeech,openai-tts`. On Linux
it enables `control,kokoro,openai-tts`, so Settings can offer both compiled
TTS backends. Override it when your local configuration uses another
backend:

```bash
SENESCHAL_SIDECAR_FEATURES="control,openai-tts" npm run sidecar:build
```

For a Linux smoke test against the repository's Kokoro models, launch the shell
with `TTS_PROVIDER=kokoro` so the existing `TTS_PROVIDER=avspeech` `.env`
override does not apply to the Linux sidecar.

The shell picks a free loopback `CONTROL_PORT` at startup and passes it to the
sidecar, so it does not collide with a manually started daemon or the default
companion port. Override it for manual QA with
`SENESCHAL_DESKTOP_CONTROL_PORT=<port>`.

## Platform prerequisites

- Rust + Node.js
- Tauri Linux dependencies: WebKitGTK 4.1, GTK 3 and related development
  packages
- macOS: standard Xcode command-line tools

Packaging, auto-update and daemon/tray lifecycle are later epic phases; the
current shell remains a development/QA surface.
