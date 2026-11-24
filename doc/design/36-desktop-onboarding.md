# ADR — Desktop onboarding wizard (issue #36)

**Status:** implemented
**Date:** 2026-11-23

## Context

The desktop app needs a first-run path from install to a speaking assistant with
zero terminal use. Settings (#33), audio diagnostics (#34) and the model
manager (#35) already exist behind the Control API. The wizard must orchestrate
them without rebuilding their state machines, and must be re-runnable without
duplicating models or settings.

## Decisions

1. **The shell owns the setup marker; the daemon owns the pipeline config.**
   `onboarding.json` lives in Tauri's app-config directory and records
   `completed`, `in_progress`, the current step and a unique set of skipped
   steps. It is versioned, atomically replaced and never stores secrets.
   Existing installs without the marker are treated as completed so an upgrade
   does not force setup.

2. **First-run detection uses a real filesystem check.**
   `SettingsSnapshot.config_file` contains the destination path even when the
   file does not exist yet. The shell checks the resolved path directly, then
   writes a bootstrap TOML before the first sidecar spawn.

3. **One explicit config path for detection and every daemon spawn.**
   In debug the repository workdir is preserved. In a packaged app the daemon
   workdir is Tauri's per-user data directory. The shell resolves
   `seneschal.{env}.toml` there (or honors `SENECHAL_CONFIG_FILE`) and injects
   the resolved value as `SENECHAL_CONFIG_FILE`.

4. **The bootstrap config must let Control start without local models.**
   macOS uses the zero-download native pair (`speech` + `avspeech`). Linux
   starts with `nemotron` + `openai`, which do not require downloaded local
   artifacts, while the wizard's recommended final selection is Whisper + Kokoro
   (or NeMo when a GPU is present). The bootstrap file is normal TOML and is
   updated by the existing Settings apply flow.

5. **Secrets are write-only in Settings.**
   `llm_api_key` can be set through `POST /control/config/apply`, persists in
   the active TOML, and is never serialized in snapshots, SSE or onboarding
   state. A blank patch value means "keep"; an explicit empty string clears it.
   If `LLM_API_KEY` is present in the environment, an in-app write is rejected
   instead of silently shadowed.

6. **The wizard suppresses the startup greeting until completion.**
   While setup is active the shell injects
   `SENESCHAL_DEFER_STARTUP_GREETING=1`; the daemon skips both the normal
   startup greeting and the later device-connected greeting. Before the final
   apply/restart the shell clears the runtime flag, so the final daemon starts
   with the normal greeting.

7. **Model selection reuses the model manager.**
   The catalog is filtered to the selected backends, dependencies (for example
   Silero VAD) are included, downloads use the existing resumable progress flow,
   and activation reuses `POST /control/models/{id}/activate`. Re-running does
   not re-download installed models or reapply unchanged settings.

8. **Completion requires audible confirmation.**
   After the final restart the wizard asks whether the greeting was heard. Only
   then does it mark the marker completed. A "No" leaves the marker incomplete
   and returns the user to the relevant setup step.

## API

- `GET /control/onboarding/defaults`
  Returns `{ platform, gpu_available, recommended_stt, recommended_tts }`.
  Recommendations are compilation-aware and hardware-aware, but are only
  suggestions; the UI still limits choices to `GET /control/config.providers`.
- `GET /control/config` remains secret-free.
- `POST /control/config/apply` accepts the write-only `llm_api_key`.
- Existing model/audio/settings endpoints perform all downloads, activation and
  diagnostics.

## UI flow

```text
detect → language → LLM → backends → models → audio → finish → done
```

Each step records skip state when a sensible default exists. Back/Next resyncs
from Settings, Models and Audio snapshots, so a restart or app close does not
create a parallel wizard state. The final patch is sparse and idempotent.

## Consequences

- A fresh packaged install has a writable per-user location for TOML, SQLite
  and relative data paths before the daemon starts.
- The marker can distinguish a resumable first run from a pre-onboarding
  install without adding a migration to the daemon config schema.
- Packaging still owns signing, notarization, updater and CI artifacts (#37).
- Manual platform QA remains necessary for real microphone/speech permissions,
  audio hardware and NeMo containers.
