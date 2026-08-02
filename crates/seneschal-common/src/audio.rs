//! Shared audio types used across pipeline crates (core ↔ remote).

/// TTS audio packet sent from the pipeline (`tts_task`) to a remote WebSocket sink.
///
/// Lives in `seneschal-common` so `seneschal-core` can produce packets without depending
/// on `seneschal-remote` (which itself depends on core).
#[derive(Debug, Clone)]
pub struct TtsAudioPacket {
    /// Mono f32 samples at `sample_rate`.
    pub samples: Vec<f32>,
    /// Sample rate of the audio (Hz).
    pub sample_rate: u32,
}
