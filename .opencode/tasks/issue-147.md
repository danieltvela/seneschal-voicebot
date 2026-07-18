# Fix TUI viewport sizing and status bar position (#147)

## Context
- Origin: Gitea issue #147 — "The screen view size doesn't fit the viewport of the TUI"
- Summary: On launch the TUI must occupy the full terminal. The status bar must always sit on the last visible row and must follow terminal resizes.
- Assumptions:
  - Full-screen TUI (alternate screen) is desired; native terminal scrollback for chat history is abandoned in favor of in-app history rendering (auto-scroll to bottom, same pattern as streaming).
  - PageUp/PageDown history scrolling is **out of scope** for this issue unless already trivial; default is always show the latest messages (clip from the bottom).
  - Feature flag remains `tui`; ratatui 0.29 already supports `Viewport::Fullscreen`.

## Phase 1: Switch terminal to fullscreen + alternate screen

- [x] Step 1.1: Enter/leave alternate screen and use fullscreen viewport
  - File(s): `src/tui/mod.rs`
  - Change:
    1. Update crossterm imports to include `EnterAlternateScreen` and `LeaveAlternateScreen`.
    2. Delete the constant `VIEWPORT_HEIGHT` and its doc comment.
    3. After `enable_raw_mode()` and cursor hide, add `execute!(io::stdout(), EnterAlternateScreen)?`.
    4. Change terminal creation from `Viewport::Inline(VIEWPORT_HEIGHT)` to `Viewport::Fullscreen`.
    5. On shutdown: add `LeaveAlternateScreen`, remove `MoveToNextLine(1)`.
  - Acceptance criteria: `cargo build --features tui` succeeds. No remaining references to `VIEWPORT_HEIGHT` or `Viewport::Inline`.

- [x] Step 1.2: Remove broken resize poll branch
  - File(s): `src/tui/mod.rs`
  - Change: Delete the `tokio::select!` arm that sleeps 100ms and polls for `Event::Resize`.
  - Acceptance criteria: That `select!` arm is gone. Loop still has: event_rx, keys, tick sleep.

- [x] Step 1.3: Checkpoint compile
  - File(s): none
  - Change: Run `cargo build --features tui`. Fix any unused imports.
  - Acceptance criteria: build green.

## Phase 2: Render chat history inside the fullscreen frame

- [x] Step 2.1: Rewrite `ui::render` layout for full terminal
  - File(s): `src/tui/ui.rs`
  - Change: Replace the layout in `pub fn render` so the full `frame.area()` is used.
    1. `status_height = 1`
    2. `input_height` = existing logic
    3. `prompt_height` = existing logic
    4. `chrome = status_height + input_height + prompt_height`
    5. `remaining = total.height.saturating_sub(chrome)`
    6. If streaming empty: `streaming_height = 0`, `history_height = remaining`
    7. Else: `streaming_height = min(remaining, max(3, remaining / 3))`, `history_height = remaining - streaming_height`
    8. Layout with `Layout::vertical` using `Constraint::Length` for each: history, streaming, prompt, input, status.
  - Acceptance criteria: Status area `y + height` equals `frame.area().bottom()`. Heights sum to `total.height`.

- [x] Step 2.2: Add `render_history` and call it from `render`
  - File(s): `src/tui/ui.rs`
  - Change: Add `fn render_history` that builds lines from `app.messages`, auto-scrolls to bottom, renders in area. Call from `render` when `history_height > 0`.
  - Acceptance criteria: Splash and chat messages appear inside the TUI.

- [x] Step 2.3: Remove `flush_new_messages` / `insert_before` path
  - File(s): `src/tui/mod.rs`, `src/tui/app.rs`
  - Change: Delete `flush_new_messages`, remove call sites, remove `last_printed_index` field.
  - Acceptance criteria: No `insert_before` or `last_printed_index` in the crate. Build green.

## Phase 3: Resize correctness and tests

- [x] Step 3.1: Ensure resize is observed every frame
  - File(s): `src/tui/mod.rs`
  - Change: Confirm `terminal.draw` is called every loop iteration. No extra resize handler needed.
  - Acceptance criteria: With app running, resizing moves status bar to new last row within one tick.

- [x] Step 3.2: Add unit tests for layout height math
  - File(s): `src/tui/ui.rs`
  - Change: Extract `compute_layout_heights` helper, add tests.
  - Acceptance criteria: `cargo test --features tui` passes.

- [x] Step 3.3: QA checkpoint
  - File(s): none
  - Change: Run fmt, clippy, test, build.
  - Acceptance criteria: all green.

## Phase 4: Manual verification notes

- [ ] Step 4.1: Document manual check on the PR/issue
  - File(s): none
  - Acceptance criteria: checklist confirmed when closing #147.