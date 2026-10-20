pub mod local;
pub mod no_speech_gate;
pub mod provider;
pub mod segmenter;
pub mod streaming;
pub mod wav;
pub mod whisper;
pub mod whisper_log;

#[cfg(feature = "parakeet")]
pub mod parakeet;

#[cfg(feature = "openai-stt")]
pub mod openai;

#[cfg(feature = "speech")]
pub mod speech_recognizer;

pub use no_speech_gate::{NoSpeechGate, TranscriptionQuality};
pub use provider::{SttProvider, create_provider};
pub use segmenter::{SegmenterConfig, SegmenterEvent, SpeechDetector, VadSegmenter};
pub use streaming::{LiveDecoder, LiveStreamConfig, LiveStreamer};
#[allow(unused_imports)]
pub use whisper::{WhisperSTTVAD, WhisperSTTVADConfig, WhisperSttProvider};

#[cfg(feature = "openai-stt")]
pub use openai::{OpenAiSttConfig, OpenAiSttProvider};

/// Events emitted while processing the audio stream.
///
/// `segment_seq` is the id the VAD segmenter assigned to the utterance; it
/// lets the pipeline discard stale results after a barge-in (contract P2, see
/// `doc/stt-contract.md`). The macOS `speech` provider starts its own seq
/// counter; 0 marks an end without a committed segment (FSM-reset paths).
#[derive(Debug, Clone)]
pub enum SpeechEvent {
    SpeechStart {
        segment_seq: u64,
    },
    /// Live partial transcript (contract P3): emitted while the user speaks.
    Speech(String),
    SpeechEnd {
        segment_seq: u64,
        quality: TranscriptionQuality,
    },
}
