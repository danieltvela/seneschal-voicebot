# Memory & Persistence — Task List

Modules: `src/db/`, `src/memory/`, `src/profile/`, `src/dream/`, `src/analysis/`

---

## [M0.2] Database (`src/db/`)

### Schema & Migrations (`src/db/database.rs` + `src/db/migrations/`)
- [x] SQLite via `sqlx` with migration-first schema
- [x] Tables: `sessions`, `messages`, `user_profile`, `memories`
- [x] Session restore on startup (history, profile, memories)
- [x] Message persistence after each turn
- [ ] **Database vacuum**: schedule periodic VACUUM to reclaim space from deleted messages
- [ ] **Database backup**: on shutdown, create a timestamped backup of the SQLite file
- [ ] **Migration safety**: add integration test that verifies every migration can be applied and rolled back
- [ ] **Concurrent access guard**: ensure `sqlx::Pool` size is appropriate (default 5 connections); test concurrent read/write

### FTS5 (`src/db/database.rs`)
- [x] Full-text search index on `messages.content`
- [ ] **FTS5 rebuild**: if FTS5 index becomes corrupted, add automatic rebuild on startup
- [ ] **Search ranking**: add BM25 or custom ranking to FTS5 results (currently chronological)

### Query Performance
- [ ] **Message pagination**: add `LIMIT`/`OFFSET` to message queries; avoid loading 10000+ messages into memory
- [ ] **Index audit**: add missing indexes on frequently queried columns (`session_id`, `timestamp`, `role`)

---

## [M0.4] Memory Extraction (`src/memory/`)

- [x] `extract_memories()` — LLM-based extraction of persistent notes from conversation
- [x] `build_memory_context()` — inject memories into system prompt
- [ ] **Memory decay**: add `importance` and `last_accessed` fields; auto-archive low-importance memories not accessed in 30 days
- [ ] **Memory deduplication**: detect and merge duplicate/similar memories using embedding similarity or simple Jaccard
- [ ] **Memory categories**: tag memories with categories (project, personal, technical, preference) for better context injection
- [ ] **Memory confidence**: assign confidence score (0.0–1.0) to extracted memories; only inject high-confidence memories

---

## [M0.4] User Profile (`src/profile/`)

- [x] Extract structured facts (name, city, preferences) from conversation
- [x] Inject into system prompt as `[USER PROFILE]` block
- [ ] **Profile update frequency**: don't re-extract profile on every turn; only after significant new information
- [ ] **Fact conflict resolution**: if a new fact contradicts an existing one, flag for user confirmation
- [ ] **Profile sections**: organize profile into sections (identity, preferences, health, work) for cleaner system prompt injection

---

## [M0.4] S-DREAM Consolidation (`src/dream/`)

- [x] L1 → L2 archival: scheduled and idle-triggered
- [x] Configurable interval, idle threshold, message minimums
- [x] JSONL archive files in `data/{env}/archives/`
- [ ] **L3 cross-session patterns**: extract long-term patterns across multiple sessions (habits, recurring topics, frequent contacts)
  - Implementation: periodic batch job analyzing L2 archives with secondary LLM
- [ ] **Archive compression**: gzip L2 JSONL files older than 7 days
- [ ] **Archive search**: add tool to search L2 archives (similar to `recover_historical_context` but for archived data)
- [ ] **Consolidation metrics**: track consolidation frequency, token savings, memory extraction quality

---

## [M0.4] ContextLens & Identity (`src/analysis/`)

- [x] `IdentityAnalyzer` — speaker verification integration
- [x] `ContextLens` — multi-observer bus for identity, emotion, video
- [ ] **Emotion analyzer**: add emotion detection observer to ContextLens (see `tasks/audio-stt.md`)
- [ ] **Gaze/video analyzer**: if camera is available, add gaze detection (user looking at screen vs away)
- [ ] **ContextLens persistence**: save ContextLens observations to DB for cross-session context
- [ ] **Observer priority**: define observer priority so critical analyzers (identity) are not blocked by slow ones (video)

---

*Last updated: 2026-07-27.*
