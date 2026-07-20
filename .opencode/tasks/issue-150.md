# Fix short user phrases — faster VAD onset + short-utterance fallback (#150)

## Context
- Origin: Gitea issue #150 — "Fix short user phrases"
- Proposed branch: `feature/issue-150-fix-short-user-phrases`
- Base branch: `master`
- Summary: With `STT_PROVIDER=speech` (macOS SFSpeechRecognizer), two problems exist:
  1. **Short phrases are silently dropped.** `src/stt/speech_recognizer.rs` only commits to STT after `vad_confirm_probes = 2` consecutive 200ms probes each averaging ≥ `vad_start_threshold = 0.65`. A short word ("yes", "Vale", ~300–500ms) usually produces only **1** probe above threshold (the 200ms window dilutes the word with surrounding silence), so confirmation never happens and the accumulated audio is discarded after `MAX_ACCUM_PROBES` (10s) — the utterance never reaches Apple.
  2. **`SpeechStart` (→ `PipelineState::Listening` + barge-in) is slow.** It is only emitted when Apple returns its first non-empty `DidHypothesizeTranscription` partial, i.e. *after* VAD confirm (≥400ms) **plus** task creation + recognition latency. Total onset→LISTENING is ~0.7–1.2s+, so barge-in is sluggish.
- Assumptions (confirmed with user):
  - Fire `SpeechEvent::SpeechStart` on the **first speech-like VAD probe** (fastest possible barge-in). Cough/noise false-triggers are rejected downstream by the existing `NoSpeechGate` (empty/garbage transcription).
  - Reduce the VAD probe window from **200ms → 100ms** (finer granularity, less dilution of short words, negligible CPU).
  - Scope: change **`src/stt/speech_recognizer.rs` only** (+ the small `main.rs` robustness step). Do **not** touch `whisper.rs`/`parakeet.rs` (follow-up issue).

## Key files
- `src/stt/speech_recognizer.rs` — VAD state machine + Apple task lifecycle (PRIMARY).
- `src/main.rs` — `SpeechStart` handler, `MIN_SPEECH_DURATION_MS` filter, TUI state transitions (small robustness fix).
- `src/stt/mod.rs` — `SpeechEvent` enum (no change needed).
- `src/tui/events.rs` — `tui::events::PipelineState::Idle` exists (used by robustness fix).
- `doc/env-vars.md` + `src/config.rs` VAD comments — documentation of new behavior.
- `.env` / `seneschal.dev.toml` — user already set `VAD_SILENCE_MS=300`; short-utterance fallback derives its silence window from this.

## Phase 1: VAD state machine changes (speech_recognizer.rs)

- [x] Step 1.1: Reduce VAD probe window to 100ms
  - File(s): `src/stt/speech_recognizer.rs`
  - Change: Change `const VAD_PROBE_MS: usize = 200;` → `100;`. `VAD_PROBE_SAMPLES` is derived from it, so no other change needed. Keep `MAX_ACCUM_PROBES = 50` (now 50 × 100ms = 5s — fine).
  - Acceptance criteria: `grep "VAD_PROBE_MS" src/stt/speech_recognizer.rs` shows `100`. `cargo build --features speech,tui,control,remote` succeeds.

- [x] Step 1.2: Signal accumulation start from the VAD probe
  - File(s): `src/stt/speech_recognizer.rs`
  - Change: In the `VadAction` enum, add a new variant `AccumStarted(Vec<f32>)` that carries the first speech chunk (the same chunk already appended to `accum_buf`). In `process_probe`, in the `else` (silence) branch where `is_speech` is true and we set `self.accumulating = true`, change the returned action from `VadAction::None` to `VadAction::AccumStarted(chunk.to_vec())`. (The chunk is already appended to `accum_buf` at that point — keep that.)
  - Acceptance criteria: The first speech probe returns `AccumStarted` (verified by adding a unit test or by log later). Build green.

- [x] Step 1.3: Emit `SpeechStart` on accumulation start (fast barge-in)
  - File(s): `src/stt/speech_recognizer.rs`
  - Change: In `process_audio`, add a match arm for `VadAction::AccumStarted(_)`:
    1. Lock `self.state`, set `st.speech_start_sent = true` (so the existing `SpeechStart (from first partial)` emission in `drain_events` does NOT emit a duplicate).
    2. Send `SpeechEvent::SpeechStart` via `tx.send(...).await`.
    3. The `VadAction::Start` (confirmation) and `VadAction::Feed`/normal paths must NOT emit `SpeechStart` again.
  - Acceptance criteria: For a real utterance, `SpeechStart` is logged near the `Start accumulating` debug line (within one probe ~100ms), NOT after Apple's first partial. Confirm no duplicate `SpeechStart` per utterance (the `speech_start_sent` flag dedupes). Build green.

- [x] Step 1.4: Add short-utterance fallback (finalize unconfirmed speech)
  - File(s): `src/stt/speech_recognizer.rs`
  - Change:
    1. Add field `accum_silence_probes: usize` (init 0 in `new`; reset in `reset_accum` and `reset_vad`).
    2. In the `else if self.accumulating` branch, after appending `chunk` to `accum_buf` and incrementing `accum_probes_total`:
       - Track silence: if `avg_prob < self.vad_start_threshold` → `self.accum_silence_probes += 1`; else (speech) → `self.accum_silence_probes = 0`. (Place this BEFORE the existing `if self.accum_probes_total >= MAX_ACCUM_PROBES` discard check.)
       - Compute the short-utterance silence threshold: `let short_silence_probes = (self.silence_samples_threshold + VAD_PROBE_SAMPLES - 1) / VAD_PROBE_SAMPLES;` (i.e. `ceil(silence_ms / probe_ms)`; with 300ms → 3 probes).
       - After the confirmation check (`if self.consecutive_speech_probes >= self.vad_confirm_probes` → `return VadAction::Start(buf)`), add: if `self.accum_silence_probes >= short_silence_probes` (meaning we saw ≥1 speech probe, then a full silence window) → return `VadAction::StartShort(std::mem::take(&mut self.accum_buf))` and clear `self.accumulating`/`consecutive_speech_probes`/`accum_silence_probes` (do NOT clear `speech_start_sent`).
    3. Add the `VadAction::StartShort(Vec<f32>)` variant to the enum.
  - Acceptance criteria: A ~300–500ms word that peaks for 1–2 probes then goes silent produces a `StartShort` action (confirmed via debug log of a new `Short utterance finalized` debug line + that the audio reaches Apple). Confirmation path (`VadAction::Start`) still triggers for normal-length speech. Build green.

- [x] Step 1.5: Handle `VadAction::StartShort` (feed Apple + end immediately)
  - File(s): `src/stt/speech_recognizer.rs`
  - Change: In `process_audio`, add a match arm for `VadAction::StartShort(buf)`:
    1. `self.create_task().await?;`
    2. `self.feed_audio(&buf).await?;`
    3. `self.signal_end_audio().await;` (forces Apple to finalize in ~200ms; `SpeechStart` was already emitted at accumulation start, so `t_speech_start` in main.rs is already set and `segment_duration_ms` ≈ silence window + finalize ≥ `MIN_SPEECH_DURATION_MS`).
    4. Do NOT set `self.in_speech = true` (silence already happened). Leave VAD in accumulating-cleared state; `task_done` cleanup at the bottom of `process_audio` still applies.
  - Acceptance criteria: Short utterances reach Apple, `SpeechEnd` is emitted with the transcription, the `NoSpeechGate` downstream rejects empty/garbage (cough) transcriptions. A short word like "Vale" now produces a transcription instead of being discarded. Build green.

- [x] Step 1.6: Emit empty `SpeechEnd` on accumulation discard so state resets
  - File(s): `src/stt/speech_recognizer.rs`
  - Change: In the `if self.accum_probes_total >= MAX_ACCUM_PROBES` discard branch (currently returns `VadAction::None` after `reset_accum`), instead emit `SpeechEvent::SpeechEnd(TranscriptionQuality { text: String::new(), no_speech_prob: 0.0, avg_logprob: 0.0, compression_ratio: 0.0 })`. Reset `self.consecutive_speech_probes = 0; self.accum_silence_probes = 0;` and set `st.speech_start_sent = false` (lock state).
  - Acceptance criteria: Continuous non-speech noise that never confirms/silences no longer leaves the pipeline stuck in LISTENING forever — the empty `SpeechEnd` flows to main.rs where it is rejected by `NoSpeechGate` (see Phase 2), resetting TUI. Build green.
  - NOTE: Also reset `self.accum_silence_probes` inside `reset_accum()` (called by `reset_vad`) to avoid stale counts across utterances.

- [x] Step 1.7: Checkpoint compile (speech feature)
  - File(s): none
  - Change: Run `cargo build --features speech,tui,control,remote` (and `--features speech,parakeet` is NOT required). Fix any unused-import/clippy warnings introduced.
  - Acceptance criteria: build is green with the speech feature.

## Phase 2: main.rs robustness (unstick LISTENING on rejected/empty final)

- [x] Step 2.1: Send TUI Idle when a `SpeechEnd` is rejected or too short
  - File(s): `src/main.rs`
  - Change: In the `SpeechEvent::SpeechEnd(quality)` arm, there are two early `continue` paths:
    - `if no_speech_gate.should_reject(&quality) { ... continue; }`
    - `if segment_duration_ms < MIN_SPEECH_DURATION_MS { ... continue; }`
    Before each `continue`, send `tui_tx.send(tui::events::TuiEvent::StateChange(tui::events::PipelineState::Idle)).ok();` so the TUI status bar returns to IDLE instead of being stuck on LISTENING. (This benefits all providers, not just speech — today a rejected `SpeechEnd` can leave LISTENING stuck.)
  - Acceptance criteria: When a transcription is rejected by `NoSpeechGate` or is too short (e.g. the empty `SpeechEnd` from Step 1.6, or a cough), the TUI status returns to `● IDLE`. `cargo clippy --features speech,tui,control,remote -- -D warnings` passes.

- [x] Step 2.2: Build + lint full feature set
  - File(s): none
  - Change: Run `cargo build --features control,speech,avspeech,tui,parakeet,remote` and `cargo clippy --features control,speech,avspeech,tui,parakeet,remote --all-targets -- -D warnings`.
  - Acceptance criteria: both green (this is the exact feature set the user runs via `mac-seneschal.sh`).

## Phase 3: Tests, docs, and QA

- [x] Step 3.1: Extract pure accumulator decision logic into a testable helper
  - File(s): `src/stt/speech_recognizer.rs`
  - Change: Refactor the `else if self.accumulating` branch bookkeeping (consecutive speech counter, `accum_silence_probes` tracking, `MAX_ACCUM_PROBES` guard, and the `Start`/`StartShort`/`discard` decisions) into a small `struct AccumTracker { ... }` with a method `fn on_probe(&mut self, is_speech: bool) -> AccumDecision` returning an enum (`Confirmed`, `ShortFinalize`, `Discard`, `Continue`). Move the counters (`consecutive_speech_probes`, `accum_silence_probes`, `accum_probes_total`) into this struct. `process_probe` calls it. Keep `WhisperVadProcessor` usage in `process_probe` (the helper only consumes `is_speech: bool`, so unit tests need no model/audio).
  - Acceptance criteria: Logic identical to Steps 1.4/1.6; `cargo test --features speech` still compiles and the existing `compression_ratio_calculation` / `empty_text_quality` tests pass.

- [x] Step 3.2: Add unit tests for `AccumTracker`
  - File(s): `src/stt/speech_recognizer.rs`
  - Change: Add `#[cfg(test)]` tests covering:
    - 2 consecutive speech probes → `Confirmed`.
    - 1 speech probe then 3 silence probes (silence_ms=300, probe_ms=100) → `ShortFinalize`.
    - 1 speech probe then 2 silence probes → `Continue` (not yet finalize).
    - 50 speech+silence alternations that never confirm → `Discard` at probe 50.
    - A speech probe after a reset zeroes `accum_silence_probes` (no premature `ShortFinalize`).
  - Acceptance criteria: `cargo test --features speech speech_recognizer` is green and assertions reflect the expected decisions.

- [x] Step 3.3: Update docs
  - File(s): `doc/env-vars.md`, `src/config.rs` (VAD field comments)
  - Change:
    - In `doc/env-vars.md`, under the VAD section, document the new behavior: `SpeechStart` now fires on the first speech probe (fast barge-in); a short-utterance fallback feeds unconfirmed speech to STT and relies on `NoSpeechGate` to drop coughs/noise; `VAD_SILENCE_MS` also controls the short-utterance silence window.
    - In `src/config.rs`, update the `vad_confirm_probes` doc comment: clarify that below `vad_confirm_probes` the utterance is still transcribed via the short-utterance fallback (so short answers like "yes"/"Vale" work).
  - Acceptance criteria: Docs accurately describe the new flow; no code behavior change.

- [ ] Step 3.4: QA gate
  - File(s): none
  - Change: Run `cargo fmt --check`, `cargo clippy --features control,speech,avspeech,tui,parakeet,remote --all-targets -- -D warnings`, `cargo test`, `cargo test --features tui,remote,control` (the project's `make qa` fast suite, but scoped so the build includes `speech`).
  - Acceptance criteria: all green.

## Phase 4: Manual verification (user-run, not CI)

- [ ] Step 4.1: Real-phrase check with the debug-log skill
  - File(s): `seneschal.pro.log` (log captured during a manual run)
  - Change: Run the app with `STT_PROVIDER=speech` (e.g. `mac-seneschal.sh`). Say short phrases ("Vale", "Sí", "No") and a long sentence. Clear the log first (`> seneschal.pro.log`). Then inspect with the `stt-debug-log` skill patterns: `grep -E "(stt|Speech|Partial|Finish|end_audio|accumul|confirm|SpeechEnd)" seneschal.pro.log`.
  - Acceptance criteria:
    - `Start accumulating` now appears and is immediately followed by a `SpeechStart` log line (`[+0ms] SpeechStart` at `target: performance`) within ~1 probe.
    - A short word produces a `SpeechEnd` with the correct text (previously: no `SpeechEnd` at all / `Accum timeout` discard).
    - A cough produces a `SpeechStart` then an empty/garbage `SpeechEnd` that is rejected by `NoSpeechGate` (TUI returns to IDLE).
    - `Segment` timing in the log shows onset→LISTENING dropped to ~100–350ms (was ~0.7–1.2s).

- [ ] Step 4.2: Barge-in timing check
  - File(s): none
  - Change: While the bot is speaking, say a short word. Confirm via the log that `SpeechStart — firing BARGE_IN` appears promptly (within ~100–350ms of voice onset) and TTS stops.
  - Acceptance criteria: barge-in latency is visibly faster than before the change.
