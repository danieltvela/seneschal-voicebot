// agents → seneschal_agents
// agent_session → seneschal_agents::agent_session
pub mod config;
pub mod db;
// dream → seneschal_memory::dream
// i18n → seneschal_common::i18n
// plugins → seneschal_plugins + seneschal_extras::agent_bridge
pub mod plugins;
// screen_capture → seneschal_extras::screen_capture
pub mod tools;

#[cfg(feature = "control")]
pub mod control_client {
    pub use seneschal_control::control::broadcast::ControlEvent;
    pub use seneschal_control::control::client::{
        ClientControlEvent, ControlClient, ControlClientBuilder, ControlClientError,
        HealthResponse, StateResponse,
    };
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
