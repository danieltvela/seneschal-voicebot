# ADR — Desktop spike (issue #32)

**Status:** accepted for the spike
**Date:** 2026-11-23

## Context

Issue #31 recommends a Tauri v2 shell with the existing Seneschal daemon as a
sidecar. Issue #32 is the time-boxed vertical slice: launch the daemon, show
live `PipelineState` over the existing Control API, and prove sidecar restart
with automatic SSE reconnect.

## Decisions

1. **Frontend: Svelte + TypeScript + Vite (no SvelteKit).**
   The spike needs one plain window. Svelte has less ceremony than React for
   this surface and remains suitable for the later settings, model and audio
   panels.

2. **The Tauri Rust process owns the sidecar and the Control client.**
   Svelte only renders status and invokes `restart_daemon`. Rust handles the
   sidecar lifecycle, health polling, SSE reconnect, snapshot resync and the
   measured reconnect budget. This avoids WebView CORS issues and keeps the
   restart/reconnect coordination in one place.

3. **No core crate is linked into the GUI.**
   The Control API is the boundary. The desktop shell has small local wire
   models for `/control/health`, `/control/state` and `/control/events`; it
   does not compile `seneschal-core` or the full workspace into the Tauri
   binary.

4. **Dynamic loopback control port.**
   The shell reserves a free port and passes `CONTROL_PORT` to the sidecar.
   This avoids collision with an existing daemon and keeps local QA isolated.
   The daemon still binds `0.0.0.0` as today; binding restriction is a later
   settings/security concern, not part of this spike.

5. **Dev working directory is the repository root.**
   In debug builds the sidecar runs with the repository as its current
   directory, preserving the existing `.env`, TOML and relative data path
   behavior. Packaged working/data directories are deferred to the packaging
   phase.

6. **SSE order: connect, snapshot, then drain events.**
   The shell opens `/control/events`, then reads `/control/state`, then
   processes queued SSE frames. A `Missed N events` error triggers another
   `/control/state` resync, matching the existing Control contract.

7. **Manual restart only in the spike.**
   The user can restart an unhealthy or crashed daemon. The app detects the
   crash, keeps running, and reconnects after a manual restart. It does not
   auto-respawn the sidecar.

## Window and identity

- Identifier: `org.veladan.seneschal`
- Initial window: 800 × 600
- Minimum window: 480 × 360

## Reconnect budget

The measured value starts when restart is requested and ends after SSE is
reconnected **and** a fresh `/control/state` snapshot has been applied. During
startup/restart the reconnect backoff is 100 ms → 1 s. The target remains
< 2 s; if normal daemon startup exceeds it, the measurement is recorded rather
than hidden.

## Spike validation

- Linux smoke: the shell launched the real sidecar, `/control/health` and
  `/control/state` answered, and a UI restart produced a new sidecar PID with
  automatic SSE reconnect. Observed reconnect in the smoke environment was
  ~1.1 s from sidecar spawn to the Control connection being healthy again.
- Deterministic tests cover SSE chunking/CRLF/keepalives, unknown events,
  snapshot-first ordering, and generation-based restart cancellation.
- The expected Control API gaps are already scoped by the follow-on phases:
  config endpoints in #33, model catalog in #35, and RMS/preview-TTS in #34.
  No duplicate gap issues were filed.
