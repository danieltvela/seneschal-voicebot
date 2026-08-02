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
pub mod audio;
pub mod classifier;
pub mod config;
pub mod control_broadcast;
pub mod db;
pub mod events;
pub mod i18n;
pub mod permission;
pub mod tools;
pub mod tui_events;

pub use audio::TtsAudioPacket;
pub use control_broadcast::{ControlBroadcast, ControlEvent};

// Re-export the most commonly used types for convenience.
pub use classifier::{
    ClassifierForceMode, ClassifierLevel, ClassifierPipeline, ClassifyResult, Intent,
    build_classifier,
};
pub use config::Config;
pub use config::SeneschalEnv;
pub use db::Database;
pub use events::{PluginPromptSections, PluginSwitchEvent, ProactiveEvent};
pub use permission::{
    HttpPermissionResult, PermissionGate, PermissionOptionWire, PermissionPhase,
    PermissionSlotView, ResolveOutcome, VoiceClaim, find_allow_option_id, find_deny_option_id,
    map_transcript_to_option_id, permission_options_from_acp_json,
};
pub use tools::ConversationMode;
pub use tools::PromptBuildState;
pub use tools::Tool;
pub use tools::ToolRegistry;
