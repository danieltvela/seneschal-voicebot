# Tools — Task List

Module: `src/tools/`

---

## [M0.3] Tool System Infrastructure

### Tool Trait (`src/tools/mod.rs`)
- [x] `Tool` trait with `name()`, `description()`, `parameters()`, `is_background()`, `is_silent()`, `should_force_for()`, `run()`
- [x] `ToolRegistry` with `register()`, `tool_definitions()`, `parse_tool_call()`, `execute()`
- [x] `forced_tool_for_query()` for keyword-based tool forcing
- [x] `system_prompt_section()` for Spanish/English tool usage rules
- [ ] **Tool categories/groups**: add `category()` method to `Tool` trait for documentation and LLM system prompt grouping
- [ ] **Tool documentation generation**: script that reads all `Tool` implementations and generates `doc/TOOLS.md` automatically
- [ ] **Tool validation**: validate `parameters()` JSON Schema against a JSON Schema meta-validator on registration

### Background Tool Dispatch
- [x] Background tools spawn `tokio::spawn` and return immediately
- [x] Results delivered via `ProactiveEvent::AgentResult`
- [x] `SubtaskTracker` records background task status
- [x] `list_tasks` tool queries `SubtaskTracker`
- [ ] **Background tool timeout**: enforce per-tool timeout config; abort and log on timeout
- [ ] **Result streaming**: for long-running tools, stream intermediate results to TTS (e.g., web search partial results)
- [ ] **Concurrent background task limit**: cap at 5 concurrent background tools; queue new ones

---

## [M0.3] Individual Tool Enhancements

### `web_search` (`src/tools/web_search.rs`)
- [x] SearXNG backend
- [x] Optional secondary LLM synthesis for voice-friendly summaries
- [ ] **Result formatting for voice**: rewrite search results as natural spoken sentences (currently raw JSON-like format)
- [ ] **Safe search**: add `safe_search` parameter to SearXNG requests
- [ ] **Search history**: record recent searches in DB; allow LLM to reference previous search results

### `quick_search` (`src/tools/quick_search.rs`)
- [x] Synchronous fast-path search (Tavily, Exa, SearXNG)
- [ ] **Provider fallback**: if primary search provider fails, try secondary provider automatically

### `deep_research` (`src/tools/deep_research.rs`)
- [x] Agent delegation for multi-step research
- [ ] **Progress reporting**: stream agent progress back to pipeline for TTS updates ("Still researching, I've found 5 sources...")
- [ ] **Research session persistence**: save research results to DB for later reference

### `run_shell` (`src/tools/run_shell.rs`)
- [x] Shell command execution with safety denylist
- [x] Configurable timeout (`SHELL_TIMEOUT_SECS`)
- [ ] **Allowlist mode**: add `SHELL_ALLOWLIST` env var — if set, only commands in the allowlist can execute (stronger than denylist)
- [ ] **Command output streaming**: stream stdout line-by-line to TTS for long-running commands
- [ ] **Working directory**: add `cwd` parameter to allow specifying working directory

### `take_screenshot` (`src/tools/take_screenshot.rs`)
- [x] macOS `screencapture` + vision LLM analysis
- [ ] **Region capture**: add `region` parameter (window name, screen index) for selective capture
- [ ] **Screenshot cache**: cache recent screenshots to avoid re-capturing for follow-up questions about the same screen

### `apple_events` (`src/tools/apple_events.rs`)
- [x] Calendar (list, create, delete) and Reminders (list, create, complete, delete)
- [ ] **Calendar performance**: AppleScript Calendar queries are slow for large calendars; add FTS or EventKit direct API
- [ ] **Recurring events**: handle recurring calendar events correctly in list output
- [ ] **Reminder due dates**: format due dates in user's local timezone

### `read_file` (`src/tools/read_file.rs`)
- [x] File reading with 16 KB cap
- [ ] **Streaming read**: for files > 16 KB, allow paginated reading via `offset`/`limit` parameters
- [ ] **Binary detection**: improve binary file detection (currently basic check)
- [ ] **Allowed paths**: add `FILE_READ_ALLOWLIST` env var for path restrictions

### `conversation_mode` (`src/tools/conversation_mode.rs`)
- [x] Active/Ambient/AmbientLocked switching
- [ ] **Timer-based auto-switch**: "stay in ambient for 30 minutes" → set timer, auto-switch back

### `prompt_build` (`src/tools/prompt_build.rs`)
- [x] Iterative prompt building in TUI
- [ ] **Prompt templates**: save/load prompt templates from disk
- [ ] **Prompt validation**: check prompt length, token count, missing sections before finalizing

### `recover_historical_context` (`src/tools/recover_historical_context.rs`)
- [x] FTS5 full-text search of message archive
- [ ] **Semantic search**: add embedding-based search as fallback when FTS5 returns no results
- [ ] **Date range filter**: add `since`/`until` parameters for time-bounded searches

---

## [M0.3] New Tools

- [ ] **`schedule_tool`** — schedule any tool to run at a future time or on a recurring interval
  - Params: `tool_name`, `args`, `scheduled_at` (ISO 8601), `repeat` (cron-like)
  - Implementation: spawn a timer task that calls `ToolRegistry::execute()` at the scheduled time
- [ ] **`weather`** — fetch current weather for a location via Open-Meteo or similar free API
- [ ] **`system_info`** — report CPU usage, memory, disk space, battery level
- [ ] **`play_music`** — play a song/playlist via Music.app (AppleScript) or Spotify API
- [ ] **`send_message`** — send a text message via iMessage (AppleScript) or similar

---

## [M0.3] Tool Permission System

- [ ] **Permission levels**: define `safe` (always allowed), `confirm` (ask user), `dangerous` (requires explicit enable)
- [ ] **Confirmation flow**: when a `confirm` tool is called, TTS says "Should I run [tool description]?" and waits for user "yes"/"no"
- [ ] **Permission persistence**: remember user's permission decisions for the session; reset on restart
- [ ] **Permission audit log**: log all tool executions with parameters, result, and timestamp

---

*Last updated: 2026-07-27.*
