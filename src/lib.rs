pub mod agent_session;
pub mod agents;
pub mod audio;
pub mod config;
pub mod db;
pub mod dream;
pub mod i18n;
pub mod llm;
pub mod mcp;
pub mod memory;

pub mod plugins;
pub mod profile;
pub mod screen_capture;
pub mod search;
pub mod stt;
pub mod tools;
pub mod tts;

pub mod control_client {
    pub use crate::control::broadcast::ControlEvent;
    pub use crate::control::client::{
        ClientControlEvent, ControlClient, ControlClientBuilder, ControlClientError,
        HealthResponse, StateResponse,
    };
}

mod control {
    pub mod broadcast;
    pub mod client;
}

pub use audio::buffer::AudioBuffer;
pub use audio::output::AudioOutput;
pub use config::Config;
pub use db::Database;
pub use llm::client::OpenAIClient;
pub use llm::{LlmProvider, LlmSession, OpenAiLlmProvider};
pub use stt::{
    NoSpeechGate, SpeechEvent, SttProvider, TranscriptionQuality, WhisperSTTVAD,
    WhisperSTTVADConfig, WhisperSttProvider, create_provider,
};
pub use tts::SentenceSplitter;
