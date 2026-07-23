# Recover Ambient/Active Automatic Mode Switching

## Context
- Origin: Gitea issue #156 — Recover ambient/active automatic change
- Summary: The ambient/active conversation mode automatic switching must be recovered or reimplemented. The speaker verification was moved to an async tokio::spawn (fire-and-forget) at some point, which broke the synchronous speaker identity check needed for correct mode gating. The full spec requires:
  1. Active mode on by default
  2. User tool for explicit Active↔Ambient switching
  3. Silence timeout → automatic switch to Ambient
  4. Main user wake word → respond AND switch to Active
  5. Non-main-user voice → automatic switch to Ambient
  6. Secondary voice + wake word → respond BUT stay in Ambient mode
- Proposed branch: feature/issue-156-recover-ambient-active-automatic-change
- Base branch: master

## Phase 1: Make Speaker Verification Synchronous

The speaker verification currently runs in a fire-and-forget tokio::spawn (lines 1725-1762 of src/main.rs), which means is_main_speaker is always true (hardcoded at line 1712) when the mode/response decisions are made at lines 1765-1819. The spawn only updates the mode and streak as side effects, but not the current utterance's gating.

- [ ] **Step 1.1: Replace async spawn with awaited blocking speaker verification**
  - File(s): src/main.rs
  - Change: In the SpeechEvent::SpeechEnd handler (around line 1715), replace the tokio::spawn(async move { ... }) block with a synchronous approach using tokio::task::spawn_blocking + .await to get IdentityResult (contains is_main_speaker and speaker_label). If identity_analyzer is None, default is_main_speaker=true and speaker_label="Usuario". Use the result for ALL subsequent decisions (mode change, ambient buffer, response gating).
  - Acceptance criteria:
    - is_main_speaker correctly reflects the speaker identity before mode/response decisions
    - Speaker verification still works when disabled (identity_analyzer is None)
    - No compilation warnings or clippy errors
    - The non-user streak increment and mode change logic from the old spawn is preserved in the synchronous code

- [ ] **Step 1.2: Move mode-change side effects into synchronous code**
  - File(s): src/main.rs
  - Change: After getting is_main_speaker and speaker_label from Step 1.1, replicate the mode-change logic that was in the old spawn:
    1. If !is_main: increment non_user_streak (via Mutex), and if streak reaches config.speaker_ambient_trigger, set conv_mode to ConversationMode::Ambient if currently Active
    2. If !is_main: push the transcript to ambient_buffer with the speaker label
    3. If is_main: reset non_user_streak to 0
  - Acceptance criteria:
    - Non-user streak counting works identically to before
    - Ambient buffer still receives non-main-speaker transcripts
    - Streak resets to 0 when main user speaks

- [ ] **Step 1.3: Restructure SpeechEnd handling to support .await**
  - File(s): src/main.rs
  - Change: The current while let Ok(event) = stt_rx.try_recv() loop does not support .await on each iteration. Before the existing try_recv loop, collect all pending SpeechEvent items into a Vec. Then iterate the vec with a regular for loop that can .await inside the SpeechEnd arm.
  - Acceptance criteria:
    - All existing event types (SpeechStart, Speech, SpeechEnd) still work
    - The audio capture loop does not deadlock
    - cargo build --features tui,remote,control succeeds

## Phase 2: Implement Wake-Word-Driven Active Mode Transition

With is_main_speaker now correct, update the mode/response gating logic at lines 1765-1819 of src/main.rs to implement the issue requirements.

- [ ] **Step 2.1: Rewrite Ambient-mode response gating**
  - File(s): src/main.rs
  - Change: Replace the current logic (lines 1765-1819 in the SpeechEnd handler) with the following decision tree:
    - Take a mode snapshot with conv_mode.lock().unwrap().clone()
    - Determine is_ambient using matches!(mode, Ambient | AmbientLocked)
    - Check has_wake_word via case-insensitive substring match
    - If is_ambient:
      - has_wake_word AND is_main → switch conv_mode to Active, respond (fall through to LLM)
      - has_wake_word AND !is_main → respond but stay in Ambient (fall through, mode unchanged)
      - !has_wake_word → push to ambient_buffer, continue (skip LLM)
    - Else (Active mode):
      - !is_main → continue (skip LLM, streak/buffer already handled in Phase 1)
      - is_main → respond normally (fall through)
  - Acceptance criteria:
    - Main user + wake word in Ambient → mode switches to Active, utterance sent to LLM
    - Secondary voice + wake word in Ambient → mode STAYS Ambient/AmbientLocked, utterance sent to LLM
    - No wake word in Ambient → utterance discarded/buffered, mode unchanged
    - Main user in Active → utterance sent to LLM normally
    - Non-main user in Active → utterance discarded, mode unchanged

- [ ] **Step 2.2: Handle AmbientLocked distinction**
  - File(s): src/main.rs
  - Change: Ensure AmbientLocked is only different from Ambient in that:
    - Automatic triggers (silence timeout, non-user streak) do NOT override AmbientLocked (already handled: periodic checker at lines 1007-1037 checks *mode == ConversationMode::Active, and the streak handler also checks for Active)
    - Wake word from main user DOES override AmbientLocked → Active (handled in Step 2.1 by matching both Ambient and AmbientLocked)
    - The set_conversation_mode tool already uses AmbientLocked for explicit user requests (no change needed)
  - Acceptance criteria:
    - AmbientLocked is not auto-overridden by silence timeout or non-user streak
    - Main user wake word transitions from AmbientLocked → Active
    - User tool still sets AmbientLocked when saying "ambient" / "sleep"

- [ ] **Step 2.3: Update the ConversationMode doc comments**
  - File(s): src/tools/conversation_mode.rs
  - Change: Update the doc comment on ConversationMode::Ambient (line 13) from "Any speech from the main user immediately returns the bot to Active." to "Requires wake word to respond. Main-user wake word switches to Active; secondary-voice wake word responds without mode change."
  - Acceptance criteria: Doc comments match actual behavior

## Phase 3: Gate Active Mode Responses by Speaker Identity

In Active mode, non-main-user speech must NOT go to the LLM pipeline. It should be buffered in AmbientBuffer and count toward the non-user streak which triggers Ambient mode.

- [ ] **Step 3.1: Discard non-main-user speech in Active mode**
  - File(s): src/main.rs
  - Change: This is already handled in Step 2.1's else branch (!is_main → continue). Verify that:
    1. Non-main-user speech in Active mode does NOT reach transcript_tx
    2. The streak increment (Phase 1, Step 1.2) still happens before the continue
    3. The ambient buffer push (Phase 1, Step 1.2) still happens before the continue
  - Acceptance criteria:
    - Non-main-user speech in Active mode is NOT sent to LLM
    - Non-main-user speech in Active mode is buffered in AmbientBuffer
    - Streak counter increments correctly

## Phase 4: Update E2E Tests

- [ ] **Step 4.1: Update existing ambient E2E tests**
  - File(s): src/e2e_tests.rs
  - Change: The existing tests ambient_mode_discards_utterance_without_wake_word and ambient_mode_responds_when_wake_word_present should still pass. Review run_with_opts (line 317) — it currently short-circuits for ambient mode without wake word. This mocks the audio loop behavior and needs to match the new logic.
  - Acceptance criteria:
    - ambient_mode_discards_utterance_without_wake_word passes
    - ambient_mode_responds_when_wake_word_present passes

- [ ] **Step 4.2: Add E2E test for main-user wake word switching to Active**
  - File(s): src/e2e_tests.rs
  - Change: Add new test ambient_mode_wake_word_by_main_user_switches_to_active:
    1. Set up harness in Ambient mode
    2. Send transcript containing wake word from main user (simulated)
    3. Verify mode is now Active
    4. Verify subsequent non-wake-word speech from main user is responded to
  - Acceptance criteria: Test passes with cargo test e2e -- --ignored

- [ ] **Step 4.3: Add E2E test for secondary voice + wake word staying in Ambient**
  - File(s): src/e2e_tests.rs
  - Change: Add new test ambient_mode_wake_word_by_secondary_voice_responds_but_stays_ambient:
    1. Set up harness in Ambient mode
    2. Simulate secondary-voice utterance with wake word
    3. Verify TTS response is generated
    4. Verify mode remains Ambient
  - Acceptance criteria: Test passes with cargo test e2e -- --ignored

- [ ] **Step 4.4: Add E2E test for non-main user in Active mode**
  - File(s): src/e2e_tests.rs
  - Change: Add new test active_mode_discards_non_main_user_speech:
    1. Set up harness in Active mode
    2. Simulate non-main-user utterance
    3. Verify no TTS response
    4. Verify mode switches to Ambient after N consecutive non-user utterances
  - Acceptance criteria: Test passes with cargo test e2e -- --ignored

## Phase 5: Update Documentation

- [ ] **Step 5.1: Update doc/MAIN_PROCESS.md state machine diagram**
  - File(s): doc/MAIN_PROCESS.md
  - Change: Update the ASCII state machine diagram (lines 200-211) to reflect new behavior:
    - In Ambient: "main-user wake word" → Active (not "any speech from main user")
    - In Ambient: "secondary-voice wake word" → respond, stay Ambient (new)
    - In Active: "non-main-user voice" → buffer only, do not respond (clarify)
  - Acceptance criteria: Diagram matches actual behavior

- [ ] **Step 5.2: Review doc/doc.md for contradictions**
  - File(s): doc/doc.md
  - Change: Review section 8 (lines 1094-1351) and update state machine description (lines 1241-1311) if it contradicts new behavior. The design doc already describes the wake-word-driven Ambient mode (lines 1257-1262), which matches.
  - Acceptance criteria: No contradictions between doc and code

## Phase 6: QA Verification

- [ ] **Step 6.1: Run make qa**
  - Change: Run make qa to verify format, lint, test, test-ci, test-e2e, and build all pass
  - Acceptance criteria: All checks green, zero warnings or errors

- [ ] **Step 6.2: Manual review of transition table**
  - Change: Review ALL state transitions for completeness:
    | Current Mode | Speaker | Wake Word? | Action |
    | Active | Main | N/A | Respond normally |
    | Active | Non-main | N/A | Buffer, increment streak; if streak >= N → Ambient |
    | Ambient (auto) | Main | Yes | Respond, switch to Active |
    | Ambient (auto) | Non-main | Yes | Respond, stay Ambient |
    | Ambient (auto) | Any | No | Buffer, stay Ambient |
    | AmbientLocked | Main | Yes | Respond, switch to Active |
    | AmbientLocked | Non-main | Yes | Respond, stay AmbientLocked |
    | AmbientLocked | Any | No | Buffer, stay AmbientLocked |
    | Any | — | (silence >= clear_secs) | Active → Ambient only (not if AmbientLocked) |
