pub mod agent_session;
pub mod agents;
pub mod config;
pub mod db;
pub mod dream;
pub mod i18n;
pub mod mcp;
pub mod plugins;
pub mod screen_capture;
pub mod search;
pub mod tools;

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

// Re-export core pipeline types from seneschal-core for backward compatibility.
pub use seneschal_core::audio::buffer::AudioBuffer;
pub use seneschal_core::audio::output::AudioOutput;
pub use seneschal_core::llm::client::OpenAIClient;
pub use seneschal_core::llm::{LlmProvider, LlmSession, OpenAiLlmProvider};
pub use seneschal_core::stt::{
    NoSpeechGate, SpeechEvent, SttProvider, TranscriptionQuality, WhisperSTTVAD,
    WhisperSTTVADConfig, WhisperSttProvider, create_provider,
};
pub use seneschal_core::tts::SentenceSplitter;
