# src/memory/ — Persistent Memory Extraction

## Responsibility

Extract **persistent memories** from conversation excerpts via LLM analysis. Memories are long-term facts about the user's projects, decisions, preferences, relationships, and plans — distinct from basic profile facts (name, age, city) which are handled by `src/profile/`. Also builds the `[MEMORIES]` block injected into the system prompt.

## Design

### Core Structures

- **`MemoryExtractionResult`** — Output: `new_memories: Vec<NewMemory>` + `archive_ids: Vec<i64>`.
- **`RawMemoryAction`** — Deserialized LLM response: `content`, `category`, `action` ("add" or "archive"), `archive_id`.

### Constants

- `MAX_MEMORIES_IN_PROMPT = 50` — Cap on memories injected into system prompt to prevent unbounded growth.

### Category Validation

Valid categories: `general`, `project`, `preference`, `decision`, `relationship`. Unknown categories default to `general`.

### System Prompt Injection

`build_memory_context(memories)` formats active memories as:
```
[MEMORIES]
- memory content 1
- memory content 2
```
Returns empty string if no memories. Caps at `MAX_MEMORIES_IN_PROMPT` entries.

## Flow

### Memory Context Building

```
build_memory_context(memories: &[Memory])
  → if empty: return ""
  → format: "\n\n[MEMORIES]\n" + "- {content}\n" for each memory (up to 50)
```

### Memory Extraction

```
extract_memories(client, conversation_text, existing_memories)
  → if conversation_text is empty: return empty result
  
  → Build existing memories block:
    "\n\nExisting memories (do NOT duplicate these...)\n"
    + "[id={id}] {content}\n" for each existing memory
  
  → LLM prompt:
    system: "Extract persistent memories... Focus on projects, decisions, preferences, relationships, plans, technical context... Return JSON array"
    user: "Conversation:\n\n{conversation_text}"
  
  → client.complete(messages) → raw JSON string
  → parse_memory_response(raw) → MemoryExtractionResult

parse_memory_response(raw):
  → strip_code_fence(raw) → strip ```json and ``` markers
  → serde_json::from_str<Vec<RawMemoryAction>>()
  
  → For each action:
    if action == "archive" and archive_id is Some: add to archive_ids
    if action == "add" (or unrecognized) and content is not empty:
      validate_category(action.category)
      add NewMemory { content: trimmed, category } to new_memories
  
  → Return MemoryExtractionResult { new_memories, archive_ids }
```

### Error Handling

- LLM call failure → log warning, return empty result.
- JSON parse failure → log debug, return empty result.
- Empty content with "add" action → skip (no empty memories).
- Missing `archive_id` with "archive" action → skip.

## Integration

### Dependencies
- `crate::llm::LlmProvider` — LLM client for extraction.
- `crate::llm::Message` — Message construction.
- `crate::db::Memory` — Existing memory type.
- `crate::db::NewMemory` — New memory type.
- `serde::Deserialize` — JSON deserialization.
- `tracing` — Logging.

### Consumers
- `src/dream/` — Primary consumer: calls `extract_memories` during consolidation cycles.
- `src/daemon.rs` / `src/llm/` — Calls `build_memory_context` to inject memories into system prompt.

### Data Flow
```
S-DREAM cycle
  → load_active_memories() from Database
  → extract_memories(client, conversation_text, existing_memories)
  → save_memories_batch(new_memories, session_id)
  → deactivate_memory(id) for each archive_id
```