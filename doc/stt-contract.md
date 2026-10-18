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

## Input level

The VAD input is normalized per probe: a probe whose peak is below 0.1 is
scaled up to a 0.5 peak (capped at ×50). Silero's probabilities collapse on very
quiet captures — measured on this model, the same speech scores 0.6–0.9 at peak
≥ 0.1, 0.2–0.8 at peak 0.03 and 0.05–0.36 at peak 0.01 — so a quiet microphone
(USB speakerphones deliver 20–40 dB below full scale, and their ALSA capture
control is usually a switch rather than a gain) made clear speech read as
silence: the turn closed mid-dictation while the remote ASR, which is robust to
level, kept transcribing correctly. Quiet noise stays below 0.11 after the
boost, so genuine silence still closes the segment. The applied gain is logged
(`input_gain=×N`) together with a warning while the capture is quiet. *Locked by
`vad_input_gain_normalizes_quiet_probes` (unit) and
`stt_audit_quiet_capture_es` (real VAD: a fixture attenuated to peak
0.03/0.01/0.003 stays a single utterance and closes only after the speech
offset).*

## Properties

### P1 — Start detection at the acoustic onset
`SpeechStart` fires on the **first** above-threshold VAD probe of an utterance
(the acoustic onset), before confirmation: barge-in and LISTENING start as early
as the detector allows (worst case one probe window plus Silero's own reaction
lag). Confirmation (`vad_confirm_probes` decisions inside a sliding window of
`vad_confirm_probes + 1` probes, one natural inter-word gap tolerated) still
governs the **segment** — live decoding and the close — not the barge-in. An
onset that never confirms (a cough/click blip) is aborted: `SpeechAborted` →
an empty `SpeechEnd`, dropped by `NoSpeechGate`, no transcript. *Locked by
`speech_start_within_commit_budget` (unit: budget = one probe),
`single_probe_fires_onset_then_aborts` and `isolated_burst_fires_onset_then_aborts`
(unit).*

### P2 — Barge-in on speech onset; STT/VAD keep running
When `SpeechStart` fires (the onset), BARGE_IN is broadcast on `barge_in_tx`.
Subscribers cancel the **LLM request in flight** (including one just sent and one
already streaming), the **SentenceSplitter**, and **TTS — both the queued
sentences and the audio currently playing**; the background filler tone is
stopped too. The **STT/VAD pipeline is never cancelled**: the utterance that
just started keeps accumulating and its live transcription keeps running. A
`SpeechEnd` whose `segment_seq` differs from the last committed segment is
**stale** and is dropped: a transcript from a superseded utterance never reaches
the LLM. Consequence: if the user chains phrases faster than the final decode,
the previous phrase is cancelled — by design, uniform across providers.

**Deep-worker carve-out:** once a `run_deep_task` delegation has been dispatched
it is detached by design and keeps running across the interruption (the worker
engine never subscribes to `barge_in_tx`); only a launch still pending at
barge-in is skipped. *Locked by `speechstart_fires_barge_in_and_opens_epoch`,
`stale_speech_end_is_dropped_after_new_speechstart` (unit, mock provider),
`barge_in_cancels_during_synthesis_and_drops_queue` (tts unit), the e2e
`barge_in_*` scenarios and `e2e_deep_worker_survives_barge_in`.*

### P3 — Live transcription during speech
While the segment is open, transcription runs **during** the user's speech:
partial re-decodes of the buffered speech every `STT_LIVE_INTERVAL_MS`
(default 1 s, window capped at `STT_LIVE_WINDOW_MS`), single-flight, results
emitted as `SpeechEvent::Speech`. The full-buffer-transcribed-only-after-close
pattern is forbidden for all providers (the remote provider dispatches prefix
POSTs). `STT_LIVE_ENABLED=0` restores the historical batch-at-close behavior.
Barge-in **never disturbs STT**, so the segment just opened keeps its live
state and its prefix requests (the remote provider would otherwise never send
prefix requests for the new utterance). A superseded segment's live requests are
aborted when the next segment opens, and its `SpeechEnd` is dropped by the seq
check. *Locked by
`partial_decodes_follow_interval_and_stay_single_flight` (unit),
`live_prefix_requests_emit_partials_before_final` and
`speechstart_keeps_live_transcription` (wiremock),
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

### P5 — Prompt close, silence-only
`SegmentReady` fires ≈ `VAD_SILENCE_MS` after the last speech, plus the
post-roll grace (`VAD_POST_ROLL_MS`, one-shot "last-chance" live decode on
entry); the segment audio extends at least `VAD_SILENCE_MS` past the last
speech probe and never closes before the speech offset. A segment has **no
length limit**: while the VAD keeps reporting speech the utterance stays open,
however long it runs, so the transcript is never handed to the LLM before the
user stops talking.

The silence is measured at the detector's window resolution (7 windows per
200 ms probe on Silero), not per whole probe: a probe counts only for the
fraction of its windows below `VAD_END_THRESHOLD`, and a probe whose average is
at or above that threshold ends the run — a fading tail or a soft syllable
cannot add up to a timeout. A pause shorter than `VAD_SILENCE_MS` therefore
cannot close the segment; Silero's own reaction lag adds ~100–200 ms on top of
the configured value. *Locked by `segment_includes_trailing_silence`,
`noise_between_thresholds_keeps_segment_open` (hysteresis),
`continuous_speech_is_not_cut_before_silence`,
`silence_is_measured_at_window_resolution`,
`mostly_speech_probe_breaks_the_silence_run` (unit),
`stt_audit_pause_tolerance_es` (real model: 600/800 ms pauses stay inside a
1000 ms window), `stt_audit_tail_es` (fading tail stays one utterance) and
`stt_audit_continuous_es`/`_en` (≥25 s of unbroken speech closes as one segment,
only after the measured speech offset).*

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
| `VAD_CONFIRM_PROBES` | 2 | Speech decisions required within the sliding N+1 window before the segment **confirms** (live decoding + close). `SpeechStart`/barge-in fire on the first speech probe, not here. |
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
