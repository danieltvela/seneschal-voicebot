// seneschal-common — Shared types for the Seneschal voice AI pipeline.
//
// This crate provides the foundational types that ALL other crates need:
// - Config (environment-based configuration)
// - Database (SQLite persistence)
// - Tool trait + ToolRegistry (LLM-callable tool infrastructure)
// - i18n (multilingual notification templates)
//
// It is a leaf dependency — it does NOT depend on any other workspace crate.

pub mod acp_writer;
pub mod classifier;
pub mod config;
pub mod db;
pub mod events;
pub mod i18n;
pub mod tools;

// Re-export the most commonly used types for convenience.
pub use classifier::{
    ClassifierLevel, ClassifierPipeline, ClassifyResult, Intent, build_classifier,
};
pub use config::Config;
pub use config::SeneschalEnv;
pub use db::Database;
pub use events::{PluginPromptSections, PluginSwitchEvent, ProactiveEvent};
pub use tools::ConversationMode;
pub use tools::PromptBuildState;
pub use tools::Tool;
pub use tools::ToolRegistry;
