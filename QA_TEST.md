# Seneschal QA Test Suite

This document defines the automated quality assurance scenarios for the Seneschal TUI. 
An AI agent should execute these tests using `computer_use` and report any failures as issues in Gitea.

## General Instructions for the QA Agent
1. **Environment**: Work within current directory.
2. **Execution**: Launch the application using `./mac-seneschal.sh`.
3. **Verification**: Use `capture` to verify visual state and `ax` tree to verify element presence.
4. **Reporting**: If a test fails, capture a screenshot and the last 50 lines of terminal output, then create a Gitea issue.

---

## Test Scenarios

### Test 01: Smoke Test (Boot & Main Menu)
- **Goal**: Verify that the application starts and reaches the main menu without crashing.
- **Action**: 
    1. Run `./mac-seneschal.sh`.
- **Expected Result**: 
    - The TUI renders successfully.
    - The "Seneschal" banner or title is visible.
    - The main menu options are displayed.

### Test 02: Classifier Intent Badge & Force Toggle
- **Goal**: Verify that automatic SIMPLE/COMPLEX classification is visible on the TUI status bar, and that the debug force override cycles without crashing.
- **Background**: Intent is classified **per turn** (not a session mode). The status bar shows a badge (`—` until first turn, then `SIMPLE` / `COMPLEX`). `Ctrl+M` cycles force override: `AUTO → SIMPLE → COMPLEX → AUTO` (forced badges show `🔒`).
- **Action**:
    1. Boot and identify the classifier badge on the status bar (between conversation mode and INSERT/NORMAL).
    2. Type `hola` and submit → badge should become **SIMPLE**.
    3. Type a research-style query (e.g. `Investiga la estructura del proyecto`) and submit → badge should become **COMPLEX**.
    4. Press **Ctrl+M** once or more to cycle force override; confirm a system notification (`Classifier force: …`) and badge update (`SIMPLE🔒` / `COMPLEX🔒` / clear force).
- **Expected Result**:
    - Status bar includes the intent badge (`—` / `SIMPLE` / `COMPLEX`, optional `🔒` when forced).
    - Automatic classification updates the badge after user turns.
    - `Ctrl+M` force cycle does not crash.
    - Shortcuts hint includes `Ctrl+M force`.

### Test 03: Memory Retrieval
- **Goal**: Verify that the TUI can query and display information from the memory store.
- **Action**: 
    1. Enter a search query for a known stable fact in the project memory.
- **Expected Result**: 
    - The TUI displays the correct retrieved information in the results area.

### Test 04: Graceful Shutdown
- **Goal**: Ensure the application closes without leaving zombie processes or panicking.
- **Action**: 
    1. Press `Ctrl+C` or the designated exit command.
- **Expected Result**: 
    - The application terminates cleanly.
    - Return to the shell prompt without a Rust panic message.

### Test 05: Research & Subagent Orchestration
- **Goal**: Verify that launching a complex investigation triggers subagents and displays their progress.
- **Action**: 
    1. Enter a command that requires a research/investigation workflow (e.g., "Analyze the project structure").
- **Expected Result**: 
    - The TUI indicates that subagents have been spawned.
    - Real-time progress updates or logs from subagents are visible.
    - The final consolidated report is rendered in the TUI.
