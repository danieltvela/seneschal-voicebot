# Pipeline — Task List

Module: `src/pipeline/`, `src/main.rs`

---

## [M0.2] Pipeline Refactor — Eliminate SharedSession

### Step 1: Verify PipelineFrame coverage ✅
- [x] `PipelineFrame` enum defined in `src/pipeline/frames.rs`
- [x] `TranscriptReady { utterance_id, text }` — STT → LLM
- [x] `TextInput { text }` — TUI → LLM
- [x] `SystemNotification { text }` — background → LLM
- [x] `LLMToken { utterance_id, token }` — LLM → SEN
- [x] `LLMResponseDone { utterance_id, full_text }` — LLM → SEN
- [x] `SentenceReady { utterance_id, sentence }` — SEN → TTS
- [x] `PlaybackDone { utterance_id }` — TTS → pipeline

### Step 2: Migrate SharedSession fields to typed channels
- [ ] **2a.** Migrate `pending_system_injection` + `pending_tool_response` → `mpsc<PipelineFrame>` (lowest risk first)
  - Verify: `cargo build && cargo test`
- [ ] **2b.** Migrate `sentences` + `sentence_ready` → dedicated `mpsc<SentenceReady>` between `sen_task` and `tts_task`
  - Verify: `cargo build && cargo test`
- [ ] **2c.** Migrate `assistant_text` + `llm_post_received` → `mpsc<LLMToken>` between `llm_task` and `sen_task`
  - Verify: `cargo build && cargo test`
- [ ] **2d.** Migrate `transliterated_text` + `vad_finish` → `mpsc<TranscriptReady>` from audio loop to `llm_task`
  - Verify: `cargo build && cargo test`
- [ ] **2e.** Migrate latency timestamps → fields on `PipelineState` variants (carried with state transitions)
  - Verify: latency metrics still logged correctly
- [ ] **2f.** Remove `SharedSession` struct entirely
  - Verify: `cargo build && cargo test`

### Step 3: Split cancellation signals
- [ ] **3a.** Add `pause_tx: broadcast::Sender<()>` to `PipelineEvents`
  - Only `consolidation_task` sends. Only `llm_task` listens.
- [ ] **3b.** Ensure `barge_in_tx` is only sent by VAD on `SpeechStart` (not by consolidation)
- [ ] **3c.** Verify: barge-in and consolidation pause no longer interfere with each other
  - Test: trigger consolidation while user is speaking → pipeline should handle both gracefully

### Step 4: Remove pipeline signal reuse
- [ ] **4a.** Audit all uses of `transliterated_text` — ensure each semantic use has its own channel/frame variant
  - User text → `TranscriptReady`
  - System notifications → `SystemNotification`
  - Consolidation results → separate path (not reusing vad_finish)
  - ACP agent results → `AgentResult` proactive event
  - Initial greeting → `SystemNotification`
- [ ] **4b.** Verify: no `transliterated_text` writes from consolidation task
- [ ] **4c.** Verify: no `vad_finish.notify()` calls outside the main audio loop

---

## [M0.2] FSM & State Tracking

- [x] `PipelineState` enum defined with `Idle`, `Listening`, `Thinking`, `Speaking`, `Paused`
- [x] `watch::Sender<PipelineState>` shared across actors
- [x] FSM observer logs state transitions
- [ ] **Add latency fields** to `PipelineState` variants: `t_vad_end`, `t_llm_post_send`, `t_first_speech_played`
  - Migrate from SharedSession latency timestamps (Step 2e above)
- [ ] **Add `Paused { reason: PauseReason }` usage** — ensure `PauseReason::Consolidation` is used consistently
- [ ] **FSM transition validation** — add debug assertions that verify valid transitions (e.g., can't go Idle → Speaking directly)
  - Verify: `debug_assert!` fires on invalid transitions in debug builds

---

## [M0.2] Per-Utterance Task Robustness

### llm_task (`src/pipeline/llm_task.rs`)
- [ ] **Error boundary**: if `OpenAIClient::stream()` returns an error (connection refused, timeout), send a TTS fallback message ("Lo siento, no puedo conectar con el servidor...") instead of crashing
- [ ] **Tool loop guard**: enforce `MAX_TOOL_ITERATIONS = 5` with a counter; break with warning on overflow
- [ ] **Barge-in during tool execution**: if `cancel_rx` fires while a synchronous tool is running, drop the result and abort
- [ ] **Empty transcript guard**: do not call LLM with empty or whitespace-only transcript
- [ ] **Streaming timeout**: add a deadline to the streaming loop — if no token arrives for 30 seconds, abort with error

### sen_task (`src/pipeline/sen_task.rs`)
- [ ] **Buffer overflow guard**: enforce max sentence size (4096 chars); flush on limit exceeded with `[truncated]` marker
- [ ] **Punctuation extension**: add `\n` (newline paragraph) as sentence boundary
- [ ] **Flush on barge-in**: ensure all buffered text is cleared when `cancel_rx` fires

### tts_task (`src/pipeline/tts_task.rs`)
- [ ] **Queue depth limit**: cap pending sentences at 3; drop oldest if queue exceeds limit
- [ ] **Synthesis timeout**: if `synthesize()` takes > 5 seconds, skip this sentence and log warning
- [ ] **play_cancel fence**: ensure `play_cancel` stays `true` until CPAL callback has had time to observe it (already addressed; add regression test)
- [ ] **Empty sentence guard**: do not synthesize empty or whitespace-only sentences

### consolidation_task (`src/pipeline/consolidation.rs`)
- [ ] **Rate limiting**: enforce minimum interval between consolidation cycles (60 seconds)
- [ ] **Concurrent guard**: ensure only one consolidation runs at a time via `AtomicBool`
- [ ] **Silent mode**: when idle consolidation triggers, do NOT send pipeline pause/resume events to TTS (no audio interruption)

---

## [M1.1] Reliability & Polish

- [ ] **48-hour soak test** — run Seneschal with real voice interaction for 48 hours; log all errors, crashes, memory growth
- [ ] **Memory leak audit** — profile heap usage with `cargo instruments` (macOS) or `heaptrack` (Linux)
  - Check: audio buffers, channel backlogs, DB connection pool, Whisper context leaks
- [ ] **Error recovery** — for each pipeline stage, define fallback behavior on error:
  - STT timeout → retry transcription once, then skip
  - LLM unreachable → TTS fallback message, enter idle
  - TTS synthesis failure → skip sentence, continue to next
  - DB write failure → log error, continue (non-fatal)
- [ ] **Startup time** — profile `async_main()` startup; target < 5 seconds to first "ready" greeting
  - Consider: lazy Whisper model load, async DB init, parallel init where possible

---

## [M1.3] TUI UX

- [ ] **Status bar redesign** — show pipeline state, TTS mute toggle, conversation mode, active plugin
- [ ] **Conversation search** — `/search <query>` in TUI text input searches message history
- [ ] **Command palette** — `/mute`, `/plugin <name>`, `/mode active|ambient` quick commands
- [ ] **Resizable panels** — allow user to resize conversation/status areas

---

*Last updated: 2026-07-27.*
