// seneschal-memory — Cold-path memory consolidation (S-DREAM).
//
// Scheduled Dream Daemon for incremental JSONL export, profile fact extraction,
// memory consolidation, correction detection, and LLM summarization.

pub mod dream;

pub use dream::{SDreamConfig, SDreamDaemon};
