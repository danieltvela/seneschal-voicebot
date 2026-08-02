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

### Test 02: Classifier Mode Toggle
- **Goal**: Verify the transition between "Simple" (no thinking) and "Complex" (thinking/tools) modes.
- **Action**: 
    1. Identify the current mode in the UI.
    2. Execute the toggle command (e.g., specific key-bind for Classifier).
- **Expected Result**: 
    - The UI reflects the mode change (e.g., a label changing from "SIMPLE" to "COMPLEX").
    - No crash occurs during the transition.

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
