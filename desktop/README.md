# Desktop app

Tauri v2 shell for the Seneschal daemon. It launches the real `seneschal`
binary as a sidecar, connects to the existing Control SSE API, renders live
`PipelineState`, and manages Settings over the Control API (issue #33).

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

# Type check and frontend tests
npm run check
npm test
```

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

Packaging, auto-update and daemon/tray lifecycle are intentionally out of scope
for the spike.
