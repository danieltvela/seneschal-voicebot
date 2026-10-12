# STT/VAD Contract

Canonical specification of the speech-to-text pipeline's observable behavior.
Every property below is locked by a named test; when changing STT providers,
models or thresholds, these are the invariants that must keep holding.

**Sources of truth:** `crates/seneschal-core/src/stt/segmenter.rs` (state
machine), `crates/seneschal-core/src/stt/streaming.rs` (live transcription),
`src/audio_events.rs` (pipeline-side event handling), and the audits in
`crates/seneschal-core/tests/stt_audit_contract.rs`.

## Provider landscape

| Provider | Machine | Probes | Pre-roll | Post-roll | Live partials |
|---|---|---|---|---|---|
| `whisper` (default) | shared `VadSegmenter` | 200 ms (config `VAD_PROBE_MS`) | 300 ms real audio (`VAD_PREROLL_MS`) | 200 ms (`VAD_POST_ROLL_MS`) | yes (re-decode) |
| `parakeet` | shared `VadSegmenter` | same | same | same | yes (re-decode) |
| `openai` (remote) | shared `VadSegmenter` | same | same | same | yes (prefix POSTs) |
| `speech` (macOS) | own state machine (`speech_recognizer.rs`) | 100 ms | 300 ms | 500 ms | yes (Apple partials) |

The `speech` provider predates the shared machine and keeps its own; it must
satisfy the same contract properties (locked by the audits). Unifying it onto
`VadSegmenter` is tracked as follow-up work.

## Properties

### P1 — Immediate start detection
`SpeechStart` fires within the commit budget of the acoustic onset:
the confirmation takes `vad_confirm_probes` speech decisions inside a sliding
window of `vad_confirm_probes + 1` probes (one natural inter-word gap is
tolerated), so the worst case is `(N+1) × VAD_PROBE_MS` plus Silero's own
reaction lag. Measured on synthetic audio: 0.3–0.8 s (the old strict-consecutive
rule measured 0.9–2.8 s). *Locked by `speech_start_within_commit_budget` (unit),
`stt_audit_live_partial_es` (budget 1.0 s) and the tightened continuous audit
budget (0.6 s).*

### P2 — Barge-in cancels everything, stale results are dropped
`SpeechStart` fires BARGE_IN on `barge_in_tx` (LLM, SentenceSplitter, TTS and
consolidation subscribe) and calls `SttProvider::cancel_inflight` (aborts
remote requests; live decoders discard their superseded results by segment
seq). A `SpeechEnd` whose `segment_seq` differs from the last committed
segment is **stale** and is dropped: a transcript from a superseded utterance
never reaches the LLM under a new utterance id. Consequence: if the user
chains phrases faster than the final decode, the first phrase is cancelled —
by design, uniform across providers. *Locked by
`speechstart_fires_barge_in_and_cancels_stt`,
`stale_speech_end_is_dropped_after_new_speechstart` (unit, mock provider),
`stale_flight_result_is_discarded` + `finish_drops_stale_flight_and_decodes_fresh`
(streaming).*

### P3 — Live transcription during speech
While the segment is open, transcription runs **during** the user's speech:
partial re-decodes of the buffered speech every `STT_LIVE_INTERVAL_MS`
(default 1 s, window capped at `STT_LIVE_WINDOW_MS`), single-flight, results
emitted as `SpeechEvent::Speech`. The full-buffer-transcribed-only-after-close
pattern is forbidden for all providers (the remote provider dispatches prefix
POSTs). `STT_LIVE_ENABLED=0` restores the historical batch-at-close behavior.
*Locked by `partial_decodes_follow_interval_and_stay_single_flight` (unit),
`live_prefix_requests_emit_partials_before_final` (wiremock),
`stt_audit_live_partial_es` (real model: ≥1 partial before SpeechEnd).*

### P4 — Complete final text, gated launch
The text handed to the LLM is the transcription of the **complete utterance**:
the real-audio pre-roll keeps the onset that the VAD threshold crossed late
(the old system discarded it — first words were mistranscribed or lost), and
the segment includes the trailing silence tail. If the final decode is not
ready when end-of-speech is detected, the pipeline waits for it (the
`TranscriptReady` hand-off only happens in the `SpeechEnd` handler); when a
live decode already covered every spoken sample, it is **reused** as the
final text and the LLM launches immediately. `NoSpeechGate` applies only to
the final text (partials are ungated). *Locked by `onset_preserved_via_preroll`,
`finish_reuses_flight_that_covered_all_speech`,
`finish_decodes_full_when_flight_misses_the_tail` (unit),
`speechend_launches_transcript_for_active_segment`,
`no_speech_transcription_is_rejected` (unit),
`stt_audit_onset_es` (first words present through a soft ramp),
`stt_audit_tail_es` (last words present, fading tail).*

### P5 — Prompt close
`SegmentReady` fires ≈ `VAD_SILENCE_MS` after the last speech, plus the
post-roll grace (`VAD_POST_ROLL_MS`, one-shot "last-chance" live decode on
entry); the segment audio extends at least `VAD_SILENCE_MS` past the last
speech probe and never closes before the speech offset. *Locked by
`segment_includes_trailing_silence`, `noise_between_thresholds_keeps_segment_open`
(hysteresis), `max_segment_cap_cuts`, `stt_audit_tail_es`.*

### P6 — Provider uniformity
whisper / parakeet / openai share one state machine and the same defaults
(probe, pre-roll, post-roll); rotating providers must not change the
contract. `speech` (macOS) keeps its own machine but satisfies the same
properties. *Locked by the shared unit tests plus the audits running through
`new_provider` with `STT_AUDIT_PROVIDER=whisper|openai`.*

## Tuning knobs (env / config file)

| Key | Default | Effect |
|---|---|---|
| `VAD_PROBE_MS` | 200 | Probe window. 100 ms windows make Silero probabilities oscillate (see `vad_probe_calibration`); keep 200 unless measuring. |
| `VAD_CONFIRM_PROBES` | 2 | Speech decisions required within the sliding N+1 window. |
| `VAD_PREROLL_MS` | 300 | Real-audio pre-roll prepended to confirmed segments. |
| `VAD_POST_ROLL_MS` | 200 | Grace before closing after the silence timeout. |
| `VAD_SILENCE_MS` | 300 | Silence that closes the segment. |
| `STT_LIVE_ENABLED` | 1 | Live partial transcription during speech. |
| `STT_LIVE_INTERVAL_MS` | 1000 | Min new audio between partial decodes. |
| `STT_LIVE_WINDOW_MS` | 15000 | Max audio length per partial decode (tail beyond). |

## Running the test layers

```bash
make qa           # L1 unit + L2 pipeline contract tests (no models, deterministic)
make test-stt     # L3 audits with the real Whisper + Silero models
make test-stt-openai  # L3 against the remote OpenAI-compatible service (:8024)
```

L3 audits generate espeak-ng/ffmpeg fixtures under `target/stt-fixtures/`
(cached; `STT_FORCE_REGEN=1` regenerates) and skip when models/services are
absent. Budgets in the audits are **measured on the reference machine**; when
a budget trips, run `vad_probe_calibration` first — Silero's probability
behavior on synthetic audio is the usual suspect, not the pipeline.
