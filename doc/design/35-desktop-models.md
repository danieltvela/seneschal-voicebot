# ADR — Desktop model manager (issue #35)

**Status:** implemented
**Date:** 2026-11-23

## Context

The desktop app needs a first-class model manager: every model used by the
active STT/TTS backend must be downloadable, selectable and removable from the
app, with visible/resumable progress and checksum verification. The model URL
registry was split across `install.sh`, the docker weight scripts and the
kokorox patch, and models lived in ad-hoc locations (`models/` in the repo,
`~/.seneschal/models` from the installer).

## Decisions

1. **New `seneschal-models` crate owns the catalog and downloads.**
   The catalog (id, purpose, backend, files, sizes, SHA-256, dependencies) is
   a static registry sourced from the `install.sh` URLs plus HF/GitHub
   artifacts; sizes and digests were captured from the upstream files (HF LFS
   pointers for Hugging Face, direct hashes for the GitHub release assets).
   `seneschal-control` consumes it; the desktop shell never links core crates.

2. **`install.sh` stays as-is for now.**
   Fresh installs keep working unchanged. The crate is the canonical registry
   for the desktop; converging the installer onto it (e.g. a `seneschal models`
   CLI) is deliberately deferred to avoid regressions in the shell installer.

3. **Downloads are resumable and checksum-gated.**
   Files download to `<final>.part` with HTTP `Range` resume; a server that
   ignores the range restarts cleanly. SHA-256 is computed while streaming and
   the file is only renamed after the digest matches the catalog. A mismatch
   removes the partial file. Bundles mark installed only when every file is
   verified. Cancellation is deterministic (biased select) and preserves the
   `.part` for resume. Downloads run as background tasks inside the daemon;
   restarting the sidecar marks them `interrupted`/resumable, never installed.

4. **Standard per-OS data dir; no destructive migration.**
   New downloads go to `$XDG_DATA_HOME/seneschal/models` (Linux) or
   `~/Library/Application Support/Seneschal/models` (macOS); the daemon's
   `SENESCHAL_MODELS_DIR` overrides it. Legacy locations (daemon workdir
   `models/`, `~/.seneschal/models`) are detected and reported
   (`legacy: true`, `verified: null`) but never moved or rewritten. Activating
   a catalog model writes absolute paths through the existing Settings apply
   flow (atomic TOML write, restart tier, sidecar reconnect).

5. **Activation reuses Settings, not a parallel config writer.**
   `POST /control/models/{id}/activate` validates that the model and its
   dependencies (e.g. Silero VAD for whisper/parakeet) are installed, then
   builds a `SettingsPatch` (provider + model paths) and applies it through
   `SettingsService`. Field errors, origins and tiers behave exactly like the
   Settings UI. `vad_model`, `kokoro_model` and `kokoro_voices` joined the
   Settings allow-list for this.

6. **NeMo services are cards, not files.**
   The NeMo GGUF weight sets are fetched by `nemo-speech pull` (which verifies
   size + SHA-256 itself), so they are surfaced as service cards backed by one
   `docker compose ps` call plus HTTP probes (`/ready`, `/v1/models`). The ASR
   card compares the served model id against `STT_MODEL` and reports
   `model_mismatch` explicitly. Start/stop shells out to
   `docker compose --profile nvidia up -d|stop <service>` with an allow-listed
   service name; starting is refused while weights are missing. Weight
   downloads run the existing docker scripts with indeterminate progress over
   SSE.

7. **SSE progress is throttled and recoverable.**
   `model_download_progress` events carry `model_id`, `download_id`, `phase`
   (`downloading|verifying|completed|failed|cancelled`), byte counts and the
   current file; NeMo weight progress is indeterminate (`total_bytes` omitted).
   Events are emitted at most every ~200 ms plus terminal transitions, and
   `GET /control/models` is the snapshot of record after reconnect or lag.

## API

- `GET /control/models`
- `POST /control/models/{id}/download` (202; idempotent per-model lock)
- `POST /control/models/{id}/cancel`
- `POST /control/models/{id}/activate` (restart tier)
- `DELETE /control/models/{id}` (409 while downloading, in use, or unknown)
- `GET /control/nemo/services`
- `POST /control/nemo/services/{id}/start|stop|download-weights`
- SSE: `model_download_progress`

Errors use the structured `{ error: { code, message, hint? } }` shape
(`unknown_model`, `download_in_progress`, `model_not_installed`,
`dependency_missing`, `model_in_use`, `weights_missing`, `docker_error`, …);
activation field errors reuse the Settings `{ ok: false, errors: [...] }`
shape with 422.

## Consequences

- A fresh desktop install can reach a working pipeline (download → use) with
  zero manual terminal steps; the onboarding wizard (#36) will chain this.
- The installer and the crate duplicate the URL list until the installer
  converges; catalog tests pin ids/checksums so drift is visible.
- Deleting a model requires switching the active selection first, so the
  pipeline never loses its configured model under the manager.
- Docker operations require the daemon to run from a workdir containing
  `docker/docker-compose.yml` (dev layout); packaged installs surface a clear
  `docker_unavailable`/hint state instead.
