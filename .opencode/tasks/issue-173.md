# Fix Panic in agent_session.rs During Hermes Subagent Delegation

## Context
- Origin: Gitea issue #173 — "Panic en agent_session.rs al delegar tarea a subagente Hermes (tokio runtime)"
- Summary: When the user asks Seneschal to delegate a task to the Hermes subagent (visible mode), the application panics on a tokio-runtime-worker thread pointing to `crates/seneschal-agents/src/agent_session.rs:29`. The tool returns `[Tarea delegada al agente visible...]`, confirming the `run_visible` code path is triggered, which uses `VisibleSession` / `VisibleSessionManager`.
- Proposed branch: `feature/issue-173-panick-en-agent-session-rs-al-delegar-`
- Base branch: master
- Assumptions:
  1. The reported line number (29) in the panic message is inaccurate — line 29 is a struct definition (`pub struct VisibleSession {`), which cannot panic. The real panic is likely one of the `.lock().unwrap()` calls on std::sync::Mutex fields inside `VisibleSession` methods (`send`, `receive`, `close`, `is_alive`), triggered by a poisoned mutex.
  2. Hermes is configured in `visible` mode (`AGENT_HERMES_MODE=visible`), which routes through `RunAgentTool::run_visible()` → `VisibleSession` PTY path.
  3. The panic is caused by `std::sync::Mutex` poisoning: an earlier operation panics while holding a lock, and a subsequent `.lock().unwrap()` on the same poisoned mutex panics.

## Phase 1: Reproduce and Diagnose

- [x] Step 1.1: Run the application with `RUST_BACKTRACE=1` to capture the full panic stack trace.
  - File(s): N/A (runtime environment)
  - Change: Set `RUST_BACKTRACE=1` in the environment before running (`RUST_BACKTRACE=1 cargo run`). Reproduce by sending a message that triggers Hermes delegation (e.g., "lanza Hermes para que busque quién ganó la carrera de F1"), with Hermes configured in visible mode.
  - Acceptance criteria: A full Rust stack trace is captured showing the exact function and line where the panic originates. Document the actual panic location.

- [x] Step 1.2: Confirm Hermes agent mode configuration.
  - File(s): Environment variables (`.env` or shell) and defaults in `crates/seneschal-agents/src/config.rs` (line 202: default mode is `"acp"`)
  - Change: Verify that `AGENT_HERMES_MODE` is set to `"visible"` or determine how the visible mode is being triggered. Check `AGENT_HERMES_COMMAND` is set to a valid command (e.g., `hermes chat`).
  - Acceptance criteria: Document the exact env vars that cause Hermes to run in visible mode. If visible mode is NOT explicitly set, determine why `run_visible` is invoked (check the tool response message against all four mode response strings).

- [x] Step 1.3: Add a unit test that reproduces the panic scenario.
  - File(s): `crates/seneschal-agents/src/agent_session.rs` (test module)
  - Change: Add a `#[test]` that simulates concurrent access to `VisibleSession` from multiple threads: spawn a session, concurrently call `send()` and `receive()` from separate threads (one std::thread, one from within a spawned tokio task), and close while the operations are in-flight. Use `std::panic::catch_unwind` to detect poisoned mutexes.
  - Acceptance criteria: The test reproduces a panic or poisoned-mutex scenario, confirming the root cause. If no panic is reproducible, document the negative result.

## Phase 2: Fix Mutex Poisoning in agent_session.rs

- [x] Step 2.1: Replace all `std::sync::Mutex::lock().unwrap()` with poisoned-mutex recovery in `VisibleSession` methods.
  - File(s): `crates/seneschal-agents/src/agent_session.rs`
  - Change: For every `self.<field>.lock().unwrap()` call in the `impl VisibleSession` block (lines 263, 272, 282, 298, 309, 331, 345, 351, 364, 394, 421), replace `lock().unwrap()` with `lock().unwrap_or_else(|e| e.into_inner())`. This recovers from a poisoned mutex by taking the inner value (which may be in an inconsistent state, but avoids crashing the entire runtime).
    - Specifically change:
      - `send()` lines 263, 272, 282-283
      - `receive()` lines 298, 309
      - `close()` lines 331, 345, 351, 364-365
      - `is_alive()` line 394
      - `last_used()` line 421
  - Acceptance criteria: `cargo test -p seneschal-agents` passes. If a test exists that triggers mutex poisoning, it no longer panics but instead recovers gracefully.

- [x] Step 2.2: Replace `tokio::sync::Mutex` with `std::sync::Mutex` for `output_lines`.
  - File(s): `crates/seneschal-agents/src/agent_session.rs`
  - Change:
    - Line 44: Change `output_lines: Arc<tokio::sync::Mutex<VecDeque<String>>>` to `output_lines: Arc<std::sync::Mutex<VecDeque<String>>>`.
    - Line 119-120: Change `Arc::new(tokio::sync::Mutex::new(VecDeque::new()))` to `Arc::new(std::sync::Mutex::new(VecDeque::new()))`.
    - Line 183: Change `reader_output.blocking_lock()` to `reader_output.lock().unwrap_or_else(|e| e.into_inner())` in the reader thread.
    - Line 208: Same change for the remaining partial line push.
    - Line 298: Change `self.output_lines.blocking_lock()` to `self.output_lines.lock().unwrap_or_else(|e| e.into_inner())` in `receive()`.
  - Rationale: `output_lines` is accessed from a `std::thread` (reader) via `blocking_lock()` and from tokio tasks via `blocking_lock()`. Using `tokio::sync::Mutex` for blocking operations from async context is problematic. A `std::sync::Mutex` is the correct choice since all access is blocking anyway (PTY I/O is synchronous).
  - Acceptance criteria: `cargo test -p seneschal-agents` passes. All existing visible session tests still work.

## Phase 3: Fix Async Safety in run_visible

- [x] Step 3.1: Move the blocking `VisibleSession` operations out of the tokio worker thread pool.
  - File(s): `crates/seneschal-extras/src/run_agent.rs` (the `run_visible` method, lines 498-615)
  - Change: Replace the `tokio::spawn(async move { ... })` block at line 513 with `tokio::task::spawn_blocking(move || { ... })`, and use `tokio::runtime::Handle::current().block_on()` for any async calls needed inside the blocking task (specifically `synthesize_agent_result` which awaits the LLM, and the `proactive_tx.send().await` calls).
    - The `tokio::time::sleep` calls (lines 574, 585) must be replaced with `std::thread::sleep`.
    - The `proactive_tx.send().await` calls (lines 521, 537, 598) must be wrapped in `Handle::current().block_on(...)`.
    - The `synthesize_agent_result(...).await` call (line 596) must be wrapped in `Handle::current().block_on(...)`.
  - Rationale: `VisibleSession::send()` and `receive()` perform synchronous PTY I/O with `std::sync::Mutex` locks. Running these from a `tokio::spawn` task blocks a tokio worker thread, which can cause thread starvation and exacerbate mutex poisoning issues. `spawn_blocking` moves the work to a dedicated blocking thread pool.
  - Acceptance criteria: `cargo test -p seneschal-extras` passes. `cargo build --features full` succeeds.

## Phase 4: Guard Against Future Panics

- [x] Step 4.1: Add a `Drop` implementation for `VisibleSession` that gracefully cleans up.
  - File(s): `crates/seneschal-agents/src/agent_session.rs`
  - Change: After the `impl VisibleSession` block (before line 425), add:
    ```rust
    impl Drop for VisibleSession {
        fn drop(&mut self) {
            // Close if not already closed — best-effort cleanup on drop.
            // Use only fallible operations to avoid panicking during unwinding.
            if !self.closed.load(Ordering::SeqCst) {
                // Kill child process without poisoning mutexes
                if let Ok(mut guard) = self.child.lock() {
                    if let Some(mut child) = guard.take() {
                        let _ = child.kill();
                    }
                }
                // Close log file
                if let Ok(mut guard) = self.log_file.lock() {
                    if let Some(mut f) = guard.take() {
                        let _ = f.flush();
                        drop(f);
                    }
                }
                self.closed.store(true, Ordering::SeqCst);
            }
        }
    }
    ```
  - Rationale: If a `VisibleSession` is dropped while still partially initialized (e.g., if `spawn()` panics after PTY creation but before completion), the orphaned child process and log file handle would leak. A `Drop` impl ensures cleanup even in error paths.
  - Acceptance criteria: No compile errors. `cargo test -p seneschal-agents` passes. The double-close test still passes.

- [x] Step 4.2: Add a `Drop` implementation for `VisibleSessionManager` that closes all sessions.
  - File(s): `crates/seneschal-agents/src/agent_session.rs`
  - Change: In `VisibleSessionManager`, add a `Drop` impl that iterates over all sessions and calls `close()` on each. Use `impl Drop for VisibleSessionManager { fn drop(&mut self) { self.sessions.iter().for_each(|e| e.close()); } }` (approximately).
  - Acceptance criteria: No compile errors. No sessions leak on manager drop.

## Phase 5: Verification

- [x] Step 5.1: Run the full QA suite.
  - File(s): N/A
  - Change: Run `make qa` from the project root.
  - Acceptance criteria: All stages pass: `fmt`, `lint`, `test`, `test-ci`, `test-e2e`, `build`.

- [ ] Step 5.2: Manual smoke test with Hermes visible mode.
  - File(s): N/A
  - Change: Configure `AGENT_HERMES_MODE=visible` and `AGENT_HERMES_COMMAND=hermes chat`, then send a delegation message. Verify the application does not panic and the result is delivered.
  - Acceptance criteria: No panic. Hermes subagent completes the task and the result is spoken or displayed.

## Risk Notes
- The mutex poisoning recovery in Step 2.1 (`unwrap_or_else(|e| e.into_inner())`) may leave the PTY writer/log file in an inconsistent state if the original panic corrupted state. However, the state is already corrupted (the mutex is poisoned), and this change prevents a secondary panic that crashes the entire runtime. The alternative — crashing — is worse.
- Switching `output_lines` from `tokio::sync::Mutex` to `std::sync::Mutex` (Step 2.2) is safe because all access to this mutex already uses blocking patterns (`blocking_lock()` or would use `lock()`). The reader thread is a `std::thread`, not a tokio task.
- The `spawn_blocking` change in Step 3.1 may affect latency slightly (blocking thread pool has limited threads by default — ~512 in tokio). Since visible sessions are rare (one at a time per agent), this is acceptable.
