pub mod no_speech_gate;
pub mod provider;
pub mod whisper;

#[cfg(feature = "parakeet")]
pub mod parakeet;

pub use no_speech_gate::{NoSpeechGate, TranscriptionQuality};
pub use provider::{SttProvider, create_provider};
#[allow(unused_imports)]
pub use whisper::{WhisperSTTVAD, WhisperSTTVADConfig, WhisperSttProvider};

/// Events emitted while processing the audio stream.
#[derive(Debug, Clone)]
pub enum SpeechEvent {
    SpeechStart,
    #[allow(dead_code)]
    Speech(String),
    SpeechEnd(TranscriptionQuality),
}
