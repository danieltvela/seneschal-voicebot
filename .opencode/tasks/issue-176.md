# Temporary: Disable all tools except CurrentTime, SetConversationMode, and RunAgent (testing phase)

## Context
- Origin: Gitea issue #176 — Temporary: Disable all tools except CurrentTime, SetConversationMode, and RunAgent (testing phase)
- Summary of what is requested: Comment out the registration of all tools in `src/main.rs` except `CurrentTimeTool`, `SetConversationModeTool`, `RunAgentTool`, MCP tools (registered dynamically), and `NoopTool`. No code is deleted — every disabled block is prefixed with `// DISABLED (temp)`. Imports, `Cargo.toml`, and tool implementation files are left untouched.
- Proposed branch: feature/issue-176-temporary-disable-all-tools-except-cu
- Base branch: master
- Assumptions made:
  - `SwitchPluginTool` and `register_list_tasks()` are also disabled to honor the acceptance-criteria goal of only 3 named tools registered, since neither is listed in the "keep" section of the issue.
  - Unused-import warnings from commented-out registrations are acceptable (the issue mandates keeping all imports).
  - MCP tools are registered dynamically inside a `connect_mcp_servers` block and are not touched.
  - `NoopTool` registration is kept as specified in the issue's "No tocar" section.

## Phase 1: Disable tools in the first registration block (lines 264–303)

- [x] Step 1.1: Comment out single-line tool registrations at lines 264–267
  - File(s): `src/main.rs`
  - Change: Replace lines 264–267 (the 4 `tool_registry.register(...)` calls for `ReadFileTool`, `ReadClipboardTool`, `SetClipboardTool`, `OpenAppTool`) with commented-out versions, each prefixed with `// DISABLED (temp)`:
    ```rust
    // DISABLED (temp)
    // tool_registry.register(ReadFileTool);
    // DISABLED (temp)
    // tool_registry.register(ReadClipboardTool);
    // DISABLED (temp)
    // tool_registry.register(SetClipboardTool);
    // DISABLED (temp)
    // tool_registry.register(OpenAppTool);
    ```
  - Acceptance criteria: Lines 264–267 no longer contain active `tool_registry.register` calls. The `CurrentTimeTool` at line 263 and `SetConversationModeTool` at line 268 remain unmodified.

- [x] Step 1.2: Comment out `SetPromptBuildTool` registration (lines 270–272)
  - File(s): `src/main.rs`
  - Change: Comment out the three lines that declare `prompt_build_state` and register `SetPromptBuildTool`. Keep the `prompt_build_state` variable declaration as dead code (it will not produce a build error). Use `// DISABLED (temp)` on the `register` line:
    ```rust
    // DISABLED (temp)
    // let prompt_build_state: Arc<Mutex<PromptBuildState>> =
    //     Arc::new(Mutex::new(PromptBuildState::Inactive));
    // tool_registry.register(SetPromptBuildTool::new(Arc::clone(&prompt_build_state)));
    ```
    Actually — only comment out the `tool_registry.register` line. The `let prompt_build_state` binding must remain (or at least its absence must not break anything). However, checking the code: `prompt_build_state` is not used anywhere else in `main.rs` after this point, so commenting out its declaration will cause a dead-code warning but no error. To stay faithful to the issue's "only comment out registrations" approach, **only** comment out the `tool_registry.register(SetPromptBuildTool::new(...))` line, leaving the `let prompt_build_state` line intact. Prefix with `// DISABLED (temp)`.
  - Acceptance criteria: The `SetPromptBuildTool` is not registered. The `prompt_build_state` variable may trigger an unused-variable warning — that is acceptable.

- [x] Step 1.3: Comment out conditional `AppleEventsTool` registration (lines 274–277)
  - File(s): `src/main.rs`
  - Change: Comment out the entire `if config.apple_events_enabled` block (lines 274–277). Every line inside gets `//` prefix. The `info!` log line should also be commented out. Prefix with `// DISABLED (temp)`:
    ```rust
    // DISABLED (temp)
    // if config.apple_events_enabled {
    //     tool_registry.register(AppleEventsTool);
    //     info!(target: "seneschal", "apple_events tool enabled (Calendar & Reminders)");
    // }
    ```
  - Acceptance criteria: `AppleEventsTool` is never registered regardless of config.

- [x] Step 1.4: Comment out conditional `RunShellTool` registration (lines 279–282)
  - File(s): `src/main.rs`
  - Change: Same pattern — comment out the `if config.shell_enabled` block including its `info!` line. Prefix with `// DISABLED (temp)`.
  - Acceptance criteria: `RunShellTool` is never registered.

- [x] Step 1.5: Comment out conditional `TakeScreenshotTool` registration (lines 284–291)
  - File(s): `src/main.rs`
  - Change: Comment out the `if let Some(ref sec_client) = secondary_llm_client` block (lines 284–291) including its `info!` macro. Prefix with `// DISABLED (temp)`.
  - Acceptance criteria: `TakeScreenshotTool` is never registered.

- [x] Step 1.6: Comment out conditional `WebSearchTool` registration (lines 293–303)
  - File(s): `src/main.rs`
  - Change: Comment out the entire `if config.web_search_enabled && let Some(...)` block (lines 293–303), including the `let mut wst` variable and `info!` calls. Prefix with `// DISABLED (temp)`.
  - Acceptance criteria: `WebSearchTool` is never registered.

## Phase 2: Disable search provider tools (lines 313–322)

- [x] Step 2.1: Comment out `QuickSearchTool` and `DeepResearchTool` registrations
  - File(s): `src/main.rs`
  - Change: Comment out the entire `if let Some(provider) = seneschal_search::from_config(&config)` block (lines 313–322), including nested `if` and `info!` calls. Prefix with `// DISABLED (temp)`.
  - Acceptance criteria: Neither `QuickSearchTool` nor `DeepResearchTool` are registered. The `agent_registry` and `provider` variables above this block are still used elsewhere (`RunAgentTool` loop at line 324, etc.) and must not be affected.

## Phase 3: Disable OpenTerminalTool (lines 367–376)

- [x] Step 3.1: Comment out `OpenTerminalTool` registration
  - File(s): `src/main.rs`
  - Change: Comment out the entire `#[cfg(target_os = "macos")]` / `if agent_registry.agents.iter().any(...)` block (lines 367–376) including the `info!` line. Prefix with `// DISABLED (temp)`.
  - Acceptance criteria: `OpenTerminalTool` is never registered.

## Phase 4: Disable SwitchPluginTool (lines 624–630)

- [x] Step 4.1: Comment out `SwitchPluginTool` registration
  - File(s): `src/main.rs`
  - Change: Comment out the entire `if !plugin_manager.list_available().is_empty()` block (lines 624–630) including the `info!` line. Prefix with `// DISABLED (temp)`. The `plugin_switch_tx` channel and `pending_plugin_switch` variable above it (lines 619–621) are left intact — they may cause dead-code warnings, which is acceptable.
  - Acceptance criteria: `SwitchPluginTool` is never registered.

## Phase 5: Disable RecoverHistoricalContextTool and register_list_tasks (lines 674–686)

- [x] Step 5.1: Comment out `RecoverHistoricalContextTool` registration
  - File(s): `src/main.rs`
  - Change: Comment out line 674 (`tool_registry.register(RecoverHistoricalContextTool::new(Some(db.clone())));`) and line 675 (`info!(...)`). Prefix the registration line with `// DISABLED (temp)`.
  - Acceptance criteria: `RecoverHistoricalContextTool` is not registered. The `db` variable is still used elsewhere and must not be affected.

- [x] Step 5.2: Comment out `register_list_tasks()` call
  - File(s): `src/main.rs`
  - Change: Comment out line 685 (`tool_registry.register_list_tasks();`) and line 686 (`info!(...)`). Prefix with `// DISABLED (temp)`.
  - Acceptance criteria: `register_list_tasks` is not called.

## Phase 6: Verification

- [x] Step 6.1: Build release binary
  - File(s): N/A
  - Change: Run `cargo build --release`
  - Acceptance criteria: Build completes without errors. Compiler warnings about unused variables/imports are acceptable but must be noted.

- [x] Step 6.2: Run full test suite
  - File(s): N/A
  - Change: Run `cargo test`
  - Acceptance criteria: All tests pass.

- [x] Step 6.3: Manual sanity check — review remaining active registrations
  - File(s): `src/main.rs`
  - Change: Run `grep -n 'tool_registry.register' src/main.rs | grep -v '// DISABLED' | grep -v '^\s*//'` to verify that only the intended tools remain active:
    - `CurrentTimeTool` (line ~263)
    - `SetConversationModeTool` (line ~268)
    - `RunAgentTool` (line ~364, inside the `for agent` loop)
    - `McpToolProxy` (line ~485, dynamic MCP registration)
    - `NoopTool` (line ~677)
  - Acceptance criteria: Only the five listed registrations appear in the grep output.
