# src/dream/ — S-DREAM Cold-Path Memory Consolidation Daemon

## Responsibility

Background daemon that performs **cold-path memory consolidation** at scheduled intervals or when the user is idle. It exports conversation history to JSONL archives (L2), distills profile facts and persistent memories via LLM, generates conversation summaries, detects user corrections, and compacts low-confidence profile facts. Runs as a detached tokio task with no blocking impact on the inference pipeline.

## Design

### Core Structures

- **`SDreamConfig`** — Configuration: `interval_secs`, `on_idle`, `idle_threshold_secs`, `scheduled_hour: Option<u8>`, `l2_min_messages`, `jsonl_dir`.
- **`SDreamDaemon`** — The daemon: owns `Database`, optional `Arc<dyn LlmProvider>` (secondary LLM client), `proactive_tx: mpsc::Sender<ProactiveEvent>`, `last_activity: Arc<AtomicU64>`.

### Scheduling Strategy

Two trigger modes in `run_loop`:
1. **Scheduled hour** — Sleep until the next occurrence of `scheduled_hour` (e.g., 3 AM). If already past today, schedule for tomorrow.
2. **Interval fallback** — Fixed `interval_secs` sleep between cycles (default 3600s).

### Cycle Gate (`should_run_cycle`)

A cycle only runs when ALL conditions are met:
1. If `on_idle`: `now - last_activity >= idle_threshold_secs` (default 600s).
2. `count_messages_after_id(session_id, summary_through_id) >= l2_min_messages` (default 50).

### JSONL Export (L2 Archive)

- **Incremental**: Uses `dream_state` table to track `last_processed_at` timestamp per session.
- **Rotation**: Files rotate when > 10 MB or > 10,000 lines. Rotation scheme: `YYYY-MM-DD.jsonl` → `YYYY-MM-DD.001.jsonl` → `YYYY-MM-DD.002.jsonl` etc.
- **Schema**: Each line is `{"session_id", "timestamp", "role", "content"}`.
- **Path resolution**: `resolve_jsonl_path` checks base file → if full, tries `.001`, `.002`, etc.

### Distillation Pipeline

When `secondary_client` is present, the cycle performs:
1. **Profile fact extraction** — `extract_facts(client, "", conversation_text)` → `upsert_profile_fact` for each fact.
2. **Memory extraction** — `extract_memories(client, conversation_text, existing_memories)` → `save_memories_batch` for new memories, `deactivate_memory` for archived IDs.
3. **Summary generation** — `generate_summary(client, conversation_text)` → `save_summary(session_id, summary, max_message_id)`.
4. **Correction detection** — `detect_corrections(session_id)` scans recent messages for correction patterns → `upsert_profile_fact` with `correction:` prefix and confidence 1.0.
5. **Profile compaction** — `compact_user_profile()` marks facts with confidence < 0.3 for human review.

## Flow

```
spawn() → tokio::spawn(run_loop)

run_loop:
  loop:
    sleep until next scheduled hour OR interval_secs
    if !should_run_cycle(): continue
    run_cycle()

should_run_cycle:
  if on_idle:
    if (now - last_activity) < idle_threshold_secs: return false
  if count_messages_after_id(session, through_id) < l2_min_messages: return false
  return true

run_cycle:
  session = get_or_create_session()
  through_id = get_summary_through_id(session)
  
  // Incremental JSONL export
  last_processed = get_dream_last_processed(session)
  if last_processed is empty:
    messages = get_messages_with_timestamp_after_id(session, through_id)
  else:
    messages = get_messages_since(session, last_processed)
  
  if messages not empty:
    export_to_jsonl(session, messages)
    set_dream_last_processed(session, messages.last().timestamp)
  
  if messages empty: return Ok(())
  
  conversation_text = join(messages as "role: content\n")
  
  if secondary_client present:
    facts = extract_facts(client, "", conversation_text)
    for fact in facts: upsert_profile_fact(fact)
    
    existing = load_active_memories()
    extraction = extract_memories(client, conversation_text, existing)
    save_memories_batch(extraction.new_memories, session)
    for id in extraction.archive_ids: deactivate_memory(id)
    
    summary = generate_summary(client, conversation_text)
    save_summary(session, summary, max_message_id)
  
  detect_corrections(session)  // always runs, even without LLM
  compact_user_profile()

run_once:  // CLI mode — single cycle, no loop
  run_cycle()

export_to_jsonl:
  dir = jsonl_dir
  path = resolve_jsonl_path(YYYY-MM-DD.jsonl)
  // resolve: if file exists and needs_rotation (>10MB or >10000 lines), try .001, .002...
  append messages as JSON lines

resolve_jsonl_path:
  if base doesn't exist: return base
  if base doesn't need rotation: return base
  for suffix = 1..:
    candidate = base.{suffix:03}.jsonl
    if doesn't exist or doesn't need rotation: return candidate

needs_rotation:
  metadata.len() > 10*1024*1024 OR content.lines().count() > 10000

generate_summary:
  messages = [
    system: "Summarize... 2-4 sentences, same language",
    user: "Conversation:\n\n{conversation_text}"
  ]
  return client.complete(messages)

detect_corrections:
  messages = get_session_context(session, 50)
  for (role, content) in messages:
    if role == "user":
      corrections = crate::profile::detect_corrections(content, "")
      for c in corrections:
        key = "correction:{c.topic}"
        upsert_profile_fact(key, c.correction_text, 1.0)
```

## Integration

### Dependencies
- `crate::db::Database` — All persistence operations.
- `crate::llm::LlmProvider` — Secondary LLM for distillation (optional).
- `crate::memory::extract_memories` — Memory extraction from conversation text.
- `crate::profile::extract_facts` — Profile fact extraction from conversation text.
- `crate::profile::detect_corrections` — Rule-based correction detection.
- `crate::agents::ProactiveEvent` — Event channel (received but not used in current implementation).
- `tokio::sync::mpsc` — Proactive event channel.
- `std::sync::atomic::AtomicU64` — Last-activity timestamp (shared with main daemon).

### Consumers
- `src/main.rs` — Creates `SDreamDaemon`, calls `spawn()` or `run_once()`.
- `src/daemon.rs` — Updates `last_activity` timestamp on user interaction.

### Events
- `ProactiveEvent` channel (`proactive_tx`) — Reserved for future use; currently not emitted.
- `dream_state` table — Tracks `last_processed_at` per session for incremental export.