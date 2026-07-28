// seneschal-common — Shared types for the Seneschal voice AI pipeline.
//
// This crate provides the foundational types that ALL other crates need:
// - Config (environment-based configuration)
// - Database (SQLite persistence)
// - Tool trait + ToolRegistry (LLM-callable tool infrastructure)
// - i18n (multilingual notification templates)
//
// It is a leaf dependency — it does NOT depend on any other workspace crate.

pub mod classifier;
pub mod config;
pub mod db;
pub mod events;
pub mod i18n;
pub mod tools;

// Re-export the most commonly used types for convenience.
pub use classifier::{ClassifyResult, ClassifierLevel, Intent};
pub use config::Config;
pub use config::SeneschalEnv;
pub use db::Database;
pub use events::{PluginSwitchEvent, ProactiveEvent};
pub use tools::Tool;
pub use tools::ToolRegistry;
