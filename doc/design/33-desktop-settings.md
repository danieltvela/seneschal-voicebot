# ADR — Desktop Settings UI (issue #33)

**Status:** accepted
**Date:** 2026-11-23

## Context

The desktop spike (#32) proved sidecar launch, Control SSE reconnect, and manual
restart. Issue #33 adds a Settings UI over the existing TOML configuration.
The app must never reload its window to apply settings, provider switching must
be first-class, and conversation context must survive an automated sidecar
restart.

## Decisions

1. **The daemon classifies and persists; the shell restarts.**
   `POST /control/config/apply` validates the whole patch, writes the active
   TOML atomically, and returns `hot`, `restart`, or a field-error response.
   The Tauri shell owns the sidecar and invokes its existing restart path when
   the response is `restart`; it then waits for the next connected Control SSE
   snapshot before resolving the apply. The daemon does not try to kill its own
   process.

2. **Settings writes the resolved TOML, not `.env`.**
   `SENESCHAL_CONFIG_FILE` wins. Otherwise Settings writes
   `seneschal.{env}.toml` in the daemon workdir. `toml_edit` preserves
   unrelated keys and comments; writes use a same-directory temp file plus
   atomic rename.

3. **Origin is part of the wire contract.**
   `GET /control/config` exposes only the Settings allow-list, with
   `default`, `file`, or `environment` origin, apply tier, and editability.
   Env-overridden fields are read-only because an env override would continue
   to win after a TOML write. Secrets are not serialized.

4. **Hot tier is narrow and real.**
   TTS voice/rate, output volume, language, and wake word apply without a
   restart. TTS backends expose runtime setters and the pipeline takes the new
   settings on the next utterance. Language/wake word updates rebuild the live
   system prompt without dropping history. Everything else is restart tier.

5. **Control port stays shell-managed in desktop.**
   The shell reserves and injects `CONTROL_PORT` to avoid port collisions. The
   field is shown with its effective origin but is read-only in the desktop
   Settings form. `WS_PORT` remains user-editable and restart-tier.

6. **Graceful stop before force kill.**
   The shell sends SIGTERM to the sidecar process group, waits up to three
   seconds for the daemon's normal shutdown path, then falls back to the
   existing SIGKILL path. SQLite restore at daemon startup remains the source
   of conversation continuity.

7. **No preview endpoint in this phase.**
   The UI includes a disabled TTS preview placeholder. The real preview and
   audio tests belong to issue #34.

## Consequences

- Config endpoints and Settings state live behind the existing Control API
  boundary; no core crate is linked into the GUI.
- An env-origin value cannot be silently shadowed by an edit that would not
  take effect.
- A provider/model switch may take longer than the spike's ~2 s reconnect
  target when a backend has a slow cold start. The measured reconnect budget is
  surfaced rather than hidden.
