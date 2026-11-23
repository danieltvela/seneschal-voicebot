# ADR — Desktop audio panel (issue #34)

**Status:** implemented
**Date:** 2026-11-23

## Context

The desktop app needs a first-class audio panel: input/output pickers, a live
VU meter, speaker/TTS playback tests, an end-to-end mic→STT test, and a
loopback test. A broken setup (wrong device, muted mic, denied permission) must
be diagnosable from the panel alone, and a provider switch must be testable as
soon as the sidecar reconnects.

## Decisions

1. **Tests use the active daemon configuration.**
   Device and provider changes go through the existing Settings apply flow.
   Restart-tier changes complete before a test is enabled. The endpoints do not
   open temporary CPAL devices or instantiate a second TTS/STT model.

2. **One capture path; no second microphone stream.**
   RMS/peak are computed on the mono, resampled signal just before the normal
   STT call and published as `ControlEvent::AudioLevel`. Mic and loopback tests
   are recorded by the main audio loop, which owns the `SttProvider`.

3. **Diagnostic mode is exclusive and silent to the conversation.**
   Only one audio test runs at a time. During a mic/loopback recording the main
   loop accumulates chunks but does not feed `process_audio` or the pipeline.
   Afterward it discards buffered audio and stale STT events, so the test never
   creates an utterance, LLM turn, or database entry.

4. **Structured diagnostics over string matching.**
   Audio endpoints return `{ error: { code, message, hint? } }`. The UI maps
   stable codes plus `/control/audio/status` into actionable hints. HTTP status
   still distinguishes user input (400), busy/cancelled (409), backend failures
   (502/503), and timeouts (504).

5. **macOS TCC is verified from the capturing process.**
   `seneschal-core::audio::permissions` reads and requests microphone access
   through AVFoundation in the daemon sidecar. The packaged Tauri app merges
   `NSMicrophoneUsageDescription` into its Info.plist. Entitlements are not
   added unless the signed-package smoke test proves they are required.

6. **The shell restarts, then waits for Control.**
   `apply_settings` recognizes the actual wire token `restart` (not
   `restart_required`), restarts the sidecar, and waits for the next connected
   SSE snapshot before returning. Provider/device tests can therefore run
   immediately after the apply resolves.

## API

- `GET /control/audio/devices`
- `GET /control/audio/status`
- `POST /control/audio/permission`
- `POST /control/audio/preview-tts`
- `POST /control/audio/speaker-test`
- `POST /control/audio/mic-test` (`duration_secs`, default 5, range 1–10)
- `POST /control/audio/loopback` (`duration_secs`, default 5, range 1–10)
- SSE: `audio_level { rms, peak }`

Device names use the existing partial-match rules, including `#N` for
duplicate names. `GET /control/audio/devices` is presence-only and never opens
a stream.

## Verification

Deterministic tests cover levels, wire shape, WAV decoding, endpoint status
mapping, shell SSE parsing, VU bus behavior, and recorder aggregation. Manual
platform QA still needs real hardware:

- macOS packaged/signed `.app`: allow, deny and reset microphone TCC.
- Linux PipeWire/ALSA: default and USB devices; compare with
  `pactl list short sources|sinks`; keep `alsa_setup::set_default_pcm_card`.
- Wrong device, muted/zero-gain mic, missing permission, and provider switch.
