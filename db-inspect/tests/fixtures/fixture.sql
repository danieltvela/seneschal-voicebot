-- Fixture database for db-inspect integration tests.
-- Creates a minimal Voicebot schema plus representative rows.

PRAGMA foreign_keys = ON;

-- ── Sessions ───────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    closed_at TEXT,
    is_active INTEGER NOT NULL DEFAULT 1,
    summary TEXT,
    summary_through_id INTEGER NOT NULL DEFAULT 0
);

INSERT INTO sessions (id, created_at, closed_at, is_active, summary, summary_through_id)
VALUES
    ('550e8400-e29b-41d4-a716-446655440000', '2024-01-15 09:00:00', '2024-01-15 09:15:00', 0, 'First test session summary', 3),
    ('550e8400-e29b-41d4-a716-446655440001', '2024-01-16 10:30:00', NULL, 1, 'Active test session', 0);

-- ── Messages ───────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    FOREIGN KEY (session_id) REFERENCES sessions(id)
);

CREATE INDEX IF NOT EXISTS idx_messages_session_id ON messages(session_id);

INSERT INTO messages (session_id, role, content, timestamp)
VALUES
    ('550e8400-e29b-41d4-a716-446655440000', 'System', 'You are Voicebot, a helpful assistant.', '2024-01-15 09:00:01'),
    ('550e8400-e29b-41d4-a716-446655440000', 'User', 'Hello, can you help me with Rust?', '2024-01-15 09:00:05'),
    ('550e8400-e29b-41d4-a716-446655440000', 'Assistant', 'Of course! What would you like to know about Rust?', '2024-01-15 09:00:06'),
    ('550e8400-e29b-41d4-a716-446655440000', 'ToolExchanges', '{"tool":"current_time","result":"2024-01-15T09:00:10Z"}', '2024-01-15 09:00:10'),
    ('550e8400-e29b-41d4-a716-446655440000', 'Assistant', 'The current time is 09:00 UTC.', '2024-01-15 09:00:11'),
    ('550e8400-e29b-41d4-a716-446655440001', 'User', 'Tell me about async programming in Rust.', '2024-01-16 10:30:00'),
    ('550e8400-e29b-41d4-a716-446655440001', 'Assistant', 'Async Rust uses futures and an executor like tokio.', '2024-01-16 10:30:02');

-- ── User Profile ───────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS user_profile (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    confidence REAL NOT NULL DEFAULT 1.0,
    updated_at TEXT NOT NULL,
    is_under_review INTEGER NOT NULL DEFAULT 0
);

INSERT INTO user_profile (key, value, confidence, updated_at, is_under_review)
VALUES
    ('name', 'Alice', 0.95, '2024-01-15 09:05:00', 0),
    ('favorite_language', 'Rust', 0.88, '2024-01-16 10:35:00', 0);

-- ── Memories ───────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS memories (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    content           TEXT NOT NULL,
    category          TEXT NOT NULL DEFAULT 'general',
    source_session_id TEXT,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL,
    is_active         INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX IF NOT EXISTS idx_memories_active ON memories(is_active);

INSERT INTO memories (content, category, source_session_id, created_at, updated_at, is_active)
VALUES
    ('User is learning Rust programming.', 'learning', '550e8400-e29b-41d4-a716-446655440000', '2024-01-15 09:10:00', '2024-01-15 09:10:00', 1),
    ('User prefers concise answers.', 'preference', '550e8400-e29b-41d4-a716-446655440001', '2024-01-16 10:40:00', '2024-01-16 10:40:00', 1);

-- ── Profile History ────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS profile_history (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    key         TEXT NOT NULL,
    value       TEXT NOT NULL,
    confidence  REAL NOT NULL,
    timestamp   TEXT NOT NULL,
    change_type TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_profile_history_key ON profile_history(key);

INSERT INTO profile_history (key, value, confidence, timestamp, change_type)
VALUES
    ('name', 'Alice', 0.95, '2024-01-15 09:05:00', 'added'),
    ('favorite_language', 'Rust', 0.88, '2024-01-16 10:35:00', 'added');

-- ── Dream State ────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS dream_state (
    session_id         TEXT PRIMARY KEY,
    last_processed_at  TEXT NOT NULL DEFAULT '',
    FOREIGN KEY (session_id) REFERENCES sessions(id)
);

INSERT INTO dream_state (session_id, last_processed_at)
VALUES
    ('550e8400-e29b-41d4-a716-446655440000', '2024-01-15 09:20:00');

-- ── System Prompts ─────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS system_prompts (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    content    TEXT NOT NULL,
    is_active  INTEGER NOT NULL DEFAULT 0
        CHECK (is_active IN (0, 1)),
    created_at TEXT NOT NULL,
    FOREIGN KEY (session_id) REFERENCES sessions(id)
);

CREATE INDEX IF NOT EXISTS idx_system_prompts_session_id ON system_prompts(session_id);

INSERT INTO system_prompts (session_id, content, is_active, created_at)
VALUES
    ('550e8400-e29b-41d4-a716-446655440001', 'You are Voicebot, a helpful assistant.', 1, '2024-01-16 10:25:00');

-- ── FTS5 search index ──────────────────────────────────────────────────
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    role, content,
    content=messages,
    content_rowid=id,
    tokenize='porter unicode61'
);

INSERT INTO messages_fts(messages_fts) VALUES('rebuild');
