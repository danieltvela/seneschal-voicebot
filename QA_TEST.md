# Seneschal QA Test Suite

This document defines automated quality assurance scenarios for the Seneschal **TUI** (product/UI layer).
An AI agent should execute these tests and report any failures as issues in Gitea.

> **Not in scope here:** pipeline unit/e2e already covered by `cargo test e2e -- --ignored`
> (barge-in, ambient/wake-word, STT wav fixtures, multi-sentence TTS, DB multi-turn).
> Do **not** re-implement those as manual TUI scenarios.

---

## Layers

| Layer | How | Typical duration |
|-------|-----|------------------|
| **Smoke TUI** | PTY + string markers (preferred) or Terminal + capture | &lt; 2 min |
| **Interactive TUI** | Real LLM, tools, agents | 5–15 min |
| **Voice / STT** | Hardware or wav fixtures | separate |
| **e2e mocked** | `make qa` / `cargo test e2e -- --ignored` | CI |

---

## General Instructions for the QA Agent

1. **Environment**: Work within the repo root (`./mac-seneschal.sh`).
2. **Execution**: Launch with `./mac-seneschal.sh`. Prefer a **PTY harness** over GUI automation for Terminal TUIs (ratatui is not AX-friendly; Screen Recording of Terminal often fails).
3. **Boot ready**: Wait until **all** of these appear (do **not** match bare `seneschal` — cargo compile paths also contain it):
   - status brand `seneschal`
   - `● IDLE` (or later pipeline states)
   - `INSERT` or `NORMAL`
   - shortcuts `Ctrl+M force` and `Ctrl+C quit`
4. **Between turns**: Wait until status shows **IDLE** (and no `[streaming]`) before typing the next prompt. Do not send `i` if already in INSERT (prefixes the message).
5. **Verification**: Capture TUI text buffer (PTY strip-ANSI) and/or screenshots; assert status-bar labels and chat roles.
6. **Reporting**: On failure, save a TUI snapshot + last 50 lines of process log, then create a Gitea issue.
7. **Ctrl+M note**: In raw PTY, byte `0x0D` is Enter, not Control+M. Prefer a real Terminal, or kitty/CSI-u sequences, or accept unit coverage for the keybinding and only assert force via a real keyboard path when available.

---

## Test Scenarios (core)

### Test 01: Smoke Test (Boot & Main Surface)
- **Goal**: Application starts and reaches a usable TUI without crashing.
- **Action**:
    1. Run `./mac-seneschal.sh`.
- **Expected Result**:
    - TUI renders (ASCII banner and/or `seneschal` status brand).
    - Status bar shows pipeline state (`● IDLE` initially).
    - Input destination `→ Seneschal` and shortcuts hint are visible.
    - No Rust panic.

### Test 02: Classifier Intent Badge & Force Toggle
- **Goal**: Per-turn SIMPLE/COMPLEX classification is visible; force override cycles without crashing.
- **Background**: Badge is per turn (`—` until first turn). `Ctrl+M`: `AUTO → SIMPLE → COMPLEX → AUTO` (forced badges show `🔒`).
- **Action**:
    1. Boot; locate badge between conversation mode and INSERT/NORMAL.
    2. Type `hola` and submit → badge **SIMPLE**.
    3. Type a research-style query (e.g. `Investiga la estructura del proyecto`) → **COMPLEX**.
    4. Cycle `Ctrl+M`; expect system line `Classifier force: …` and `SIMPLE🔒` / `COMPLEX🔒` / clear.
- **Expected Result**:
    - Badge updates after user turns.
    - Force cycle does not crash.
    - Shortcuts include `Ctrl+M force`.

### Test 03: Memory Retrieval
- **Goal**: Assistant can recall a known fact from memory / session context.
- **Action**:
    1. Ask about a stable project fact (e.g. OpenCode / Hermes roles, or a fact seeded earlier).
- **Expected Result**:
    - Response references the fact (names, roles, or stored content) in the chat area.

### Test 04: Graceful Shutdown (idle)
- **Goal**: Clean exit with no zombies/panics when idle.
- **Action**:
    1. From IDLE, press `Ctrl+C` (quit).
- **Expected Result**:
    - Process exits (prefer code 0; accept 130 on some shells).
    - No `panicked at` in output.
    - No leftover `target/release/seneschal` process.

### Test 05: Research & Subagent Orchestration
- **Goal**: Complex investigation triggers agents and shows progress.
- **Action**:
    1. e.g. `Analyze the project structure. Use subagents or tools if needed.`
- **Expected Result**:
    - Agent/subagent activity visible (Hermes/OpenCode/AgentTask lifecycle or tool progress).
    - Consolidated answer appears in the TUI.

---

## Test Scenarios (P0 — keyboard & product UX)

### Test 06: Keyboard modes INSERT / NORMAL
- **Goal**: Vim-like modes work as documented on the status bar.
- **Action**:
    1. Boot (default **INSERT**).
    2. Press `Esc` → status shows **NORMAL**.
    3. Type letters (e.g. `xyz`) — must **not** enter the input buffer / must not submit.
    4. Press `i` → **INSERT**.
    5. Type `ping` + Enter.
- **Expected Result**:
    - Mode labels track Esc/`i`.
    - Only the INSERT-mode message is submitted.
    - No crash.

### Test 07: TTS toggle (`Ctrl+T`)
- **Goal**: Mute/unmute TTS from the TUI.
- **Action**:
    1. Confirm status **TTS ON**.
    2. Press `Ctrl+T` → **TTS OFF**.
    3. Send a short greeting; optionally confirm no (or reduced) speak path / no SPEAKING if muted.
    4. Press `Ctrl+T` again → **TTS ON**.
- **Expected Result**:
    - Label toggles each time.
    - App stays stable; shortcut hint still lists `Ctrl+T TTS`.

### Test 08: Streaming assistant UX
- **Goal**: Token streaming is visible and completes cleanly.
- **Action**:
    1. From IDLE, send a short question (e.g. `ping` or `hola`).
    2. Observe status and chat **during** the reply.
- **Expected Result**:
    - Status transitions include **THINKING** (and optionally **SPEAKING** if TTS ON).
    - Streaming area / `[streaming]` (or growing assistant text) appears before finalization.
    - After done: finalized assistant message; returns toward **IDLE**.

### Test 09: Force SIMPLE blocks research path
- **Goal**: Force override changes **routing**, not only the badge.
- **Action**:
    1. Cycle force to **SIMPLE🔒** (notification `Classifier force: SIMPLE`).
    2. Submit the same research-style prompt used in Test 05.
- **Expected Result**:
    - Badge stays forced SIMPLE (🔒).
    - No Hermes/OpenCode/AgentTask lifecycle for that turn (or clearly no multi-agent orchestration).
    - Answer may be short / refuse deep tools — still no crash.
    - Optional: cycle force back to AUTO and confirm research works again.

### Test 10: Tool call visible in timeline
- **Goal**: Tool use surfaces in the TUI (`TuiEvent::ToolCall` / tool role).
- **Precondition**: `current_time` (or another always-on tool) is registered.
- **Action**:
    1. Ask `¿Qué hora es?` / `What time is it?` (explicit time request).
- **Expected Result**:
    - Tool activity appears (tool line or equivalent) and/or answer contains a clock time.
    - No panic.

### Test 11: Shutdown mid-agent
- **Goal**: Quit is safe while research/agent work is in flight.
- **Action**:
    1. Start a research/agent prompt (as in Test 05).
    2. While status is THINKING or an agent task is Running, press `Ctrl+C`.
- **Expected Result**:
    - Process terminates without panic.
    - No long-lived orphan `seneschal` (and no runaway agent children after a short grace period).

---

## Test Scenarios (P1 — recommended next)

These are specified for a later pass; implement when P0 is green.

### Test 12: Session restore
1. Tell a unique fact (`recuerda el código AZUL-42`).
2. Quit cleanly; relaunch.
3. Ask for the code.
- **Expected**: fact recovered via memory/session context (document if chat history is not rehydrated in UI but model still recalls).

### Test 13: LLM unavailable
1. Point LLM URL at a dead endpoint (or stop the server).
2. Send a message.
- **Expected**: Error role/message in TUI; no panic; quit still works.

### Test 14: Agent permission UI
1. Trigger an ACP/agent action that requests permission.
- **Expected**: `[Necesita confirmación]` (or equivalent), options listed; answering yes/no unblocks without hanging.

### Test 15: Agent task lifecycle completeness
1. Full research turn.
- **Expected**: same `task_id` progresses Started → Running → (Delegated?) → Finalizing → Completed/Failed without duplicate spam rows.

### Test 16: Layout / input stress
1. Long paste (wrap past 4 rows), Spanish punctuation, emoji.
- **Expected**: input height caps at max rows; cursor OK; status bar intact; no panic.

### Test 17: ACTIVE / AMBIENT badge
1. Switch conversation mode (tool or configured silence path).
- **Expected**: status shows `ACTIVE` / `AMBIENT` / `AMBIENT🔒` correctly.

---

## P2 — Feature-gated / specialized

| ID | Scenario | When |
|----|----------|------|
| 18 | Prompt-build read-only pane | `set_prompt_build` enabled |
| 19 | CLI `--list-devices` / `--list-voices` | smoke without full TUI |
| 20 | Control API health | `features control` |
| 21 | Remote companion | `features remote` |
| 22 | Real barge-in (voice) | hardware; e2e covers synthetic |
| 23 | Real STT provider | Speech / Whisper / Parakeet + fixtures |
| 24 | web_search / screenshot / open_app | only if tools registered in build |
| 25 | Perceived TTFT budget | optional product metric |

---

## Failure artifacts

For every failed test attach:

1. TUI snapshot (strip-ANSI text and/or screenshot).
2. Last 50 lines of process log (`RUST_LOG` stderr).
3. Gitea issue with test id, expected vs actual, environment (macOS, features from `mac-seneschal.sh`).
