# Pipeline Orchestration (`src/pipeline/`)

## Responsibility

Pipeline orchestration module — the central data-flow engine of Voicebot. It coordinates four concurrent async actors (`llm_task`, `sen_task`, `tts_task`, `consolidation_task`) connected by typed tokio channels, manages the pipeline finite-state machine (FSM), and implements context consolidation when the LLM context window approaches its limit.

## Design

### Core Abstractions

**`PipelineFrame`** (`frames.rs`) — Typed message enum that flows between pipeline actors. Every variant carries an `utterance_id: u64` for end-to-end correlation across STT→LLM→TTS stages. Variants:
- `TranscriptReady` — STT completed; transcript ready for LLM
- `LLMToken` — Single streamed token from LLM
- `LLMResponseDone` — LLM stream finished; full concatenated response
- `SentenceReady` — Complete sentence ready for TTS synthesis
- `PlaybackDone` — TTS playback of last sentence finished
- `SystemNotification` — Background/system notification injected as a system turn
- `AgentResult` — Background tool/agent result continuing a prior LLM turn
- `TextInput` — TUI text input (no voice path)

**`PipelineState`** (`fsm.rs`) — Global FSM held in `watch::Sender<PipelineState>`. Decentralized transitions: each actor writes directly; observers (TUI, logger, control API) subscribe via `watch::Receiver`. States:
- `Idle` — No active utterance
- `Listening { utterance_id }` — VAD detected speech; STT accumulating
- `Thinking { utterance_id }` — Transcript ready; LLM generating
- `Speaking { utterance_id }` — LLM done; TTS playing
- `Paused { reason: PauseReason }` — Pipeline paused (currently only `Consolidation`)

Helper methods: `utterance_id()` extracts the ID from active states; `is_busy()` returns true for any non-Idle state.

**`PipelineEvents` (`state.rs`) — Inter-task signals:
- `barge_in_tx: broadcast::Sender<u64>` — Barge-in cancellation (VAD SpeechStart fires this to all actors)
- `llm_post_finished: Arc<Notify>` — Signals consolidation_task that LLM has finished a turn

**`MAX_TOOL_ITERATIONS: usize = 5`** — Hard limit on sequential tool calls per user turn.

### Actor Architecture

Four permanent tokio tasks, each owning a specific channel pair:

| Actor | Input Channel | Output Channel | FSM Transitions |
|-------|---------------|----------------|-----------------|
| `llm_task` | `transcript_rx` (from main loop) | `llm_tx` → sen_task, `sentences_tx` → tts_task (error path) | `Idle → Thinking` |
| `sen_task` | `llm_rx` (from llm_task) | `sentences_tx` → tts_task | (none — passthrough) |
| `tts_task` | `sentences_rx` (from sen_task) | (none — drives audio output) | `Thinking → Speaking → Idle` |
| `consolidation_task` | `llm_post_finished` Notify | `transcript_tx` (SystemNotification) | `* → Paused → Idle` |

### Context Consolidation (`consolidation.rs`)

**`build_system_prompt()`** — Assembles the full system prompt from components in strict order:
`[plugin prepend] → [base prompt] → [plugin append] → [tool instructions] → [IMMUTABLE RULES] → [USER PROFILE] → [MEMORIES] → [AGENTS]`

Plugin replace mode substitutes the entire base prompt. Tool instructions are placed early so the model cannot ignore them. Agent routing is handled by `AgentRegistry::system_prompt_section()` which only includes routing information when agents are configured via `AGENT_{}_WHEN_TO_USE`.

**`check_system_prompt_saturation()`** — L1 saturation detection. When `[USER PROFILE]` + `[MEMORIES]` exceeds `L1_SATURATION_THRESHOLD_CHARS` (4000), emits `ProactiveEvent::L1Saturated` (at most once per session via `AtomicBool` compare-exchange).

**`run_consolidation_cycle()`** — The core consolidation work:
1. Extract profile facts from conversation text via `extract_facts()` → upsert to DB
2. Extract persistent memories via `extract_memories()` → save new, archive outdated
3. Summarize old turns via `background_client.complete()` → persist summary to DB
4. Rebuild system prompt with fresh profile + memories
5. Apply summary to `LlmSession` (drops old turns, keeps last N)
6. Emit L1 saturation check

**`consolidation_task()`** — Background consolidation daemon. Waits on `llm_post_finished` Notify or idle timer. Two trigger modes:
- **Post-turn**: Fires after LLM completes a turn. If context exceeds `threshold_pct` of `context_tokens`, waits for pipeline to idle, sends "reorganize memory" notification, pauses pipeline, runs consolidation.
- **Idle**: Fires after `idle_consolidation_secs` of inactivity. Uses lower `idle_min_context_pct` threshold. Runs silently (no user notification).

## Flow

### Main Turn Flow

```
Audio loop (main.rs) → STT provider → SpeechEvent::SpeechEnd
  → transcript_tx.send(PipelineFrame::TranscriptReady { utterance_id, text })
  → llm_task receives TranscriptReady
    → Updates LlmSession (add_user_turn or update_last_user_turn for barge-in append)
    → Saves user message to SQLite (async spawn)
    → Sends PipelineState::Thinking to watch channel
    → Calls llm_client.stream() with tool definitions
    → For each StreamToken::Content:
        → Sends PipelineFrame::LLMToken to llm_tx (→ sen_task)
        → Sends TuiEvent::AssistantToken to TUI
    → For StreamToken::ToolCall:
        → If background tool: spawn async execution, send acknowledgment to TTS, break
        → If synchronous tool: execute, inject tool call + result into messages, loop
    → On stream completion (no tool call):
        → Sends PipelineFrame::LLMResponseDone to llm_tx
        → Saves assistant message to SQLite
        → Notifies llm_post_finished
```

### sen_task Flow

```
llm_rx.recv() → PipelineFrame::LLMToken
  → splitter.push(token) → SentenceSplitter buffers until punctuation boundary
  → When sentence complete: send PipelineFrame::SentenceReady to sentences_tx
  → Logs latency metrics (VAD end → first token → first sentence)

llm_rx.recv() → PipelineFrame::LLMResponseDone
  → splitter.flush() → send remaining partial sentence
  → Forwards LLMResponseDone to sentences_tx (signals tts_task: no more coming)
```

### tts_task Flow

```
sentences_rx.recv() → PipelineFrame::SentenceReady
  → If first sentence of utterance: send PipelineState::Speaking
  → Wait for previous playback to finish (or abort on barge-in)
  → tts.synthesize(sentence) on spawn_blocking thread
  → audio_output.play_blocking(samples) on spawn_blocking thread
  → On playback completion: notify playback_done

sentences_rx.recv() → PipelineFrame::LLMResponseDone
  → Set no_more_sentences = true
  → If no active playback: transition to Idle

barge-in (cancel_rx.recv()):
  → play_cancel.store(true) → CPAL callback stops playback
  → Drain sentences_rx, reset state, transition to Idle
```

### Consolidation Flow

```
consolidation_task loop:
  → Wait on: llm_post_finished OR idle timer OR barge-in cancel
  → Lock LlmSession, check needs_consolidation(context_tokens, threshold_pct)
  → If not needed: continue
  → If post-turn trigger:
      → Wait for pipeline to become Idle
      → Send "reorganize memory" SystemNotification
      → Wait for that turn to complete
      → Pause pipeline (Paused { Consolidation })
  → run_consolidation_cycle()
  → If post-turn: resume pipeline (Idle), send "memory reorganized" notification
```

## Integration

### Dependencies (consumed by pipeline)
- `crate::llm::{LlmProvider, LlmSession, StreamToken}` — LLM client interface and session state
- `crate::tts::{TtsEngine, SentenceSplitter}` — TTS synthesis and sentence boundary detection
- `crate::audio::output::AudioOutput` — Audio playback
- `crate::db::Database` — SQLite persistence for messages, summaries, profile, memories
- `crate::tools::ToolRegistry` — Tool definitions and execution
- `crate::memory::{extract_memories, build_memory_context}` — Memory extraction
- `crate::profile::{extract_facts, build_profile_context, ProfileFact}` — Profile fact extraction
- `crate::plugins::PluginPromptSections` — Plugin prompt injection
- `crate::agents::ProactiveEvent` — Background event notifications
- `crate::i18n` — Internationalized notification strings

### Consumers (depend on pipeline)
- `src/main.rs` — Spawns all four pipeline tasks; owns the audio loop that feeds `transcript_tx`; reads `pipeline_state_rx` for state checks
- `src/tui/` — Subscribes to FSM via TUI events; displays state changes
- `src/control/` — Subscribes to FSM via `ControlBroadcast`; exposes state over HTTP/SSE
- `src/daemon.rs` — `InferenceDaemon` uses `proactive_tx` to inject events into pipeline
- `src/eyes.rs` — `EyesDaemon` uses `proactive_tx` to inject visual observations
- `src/device_monitor.rs` — Sends `PipelineFrame::SystemNotification` via `transcript_tx` on device reconnect
- `src/pipeline/consolidation.rs` — Called at startup by `main.rs` if context exceeds idle threshold