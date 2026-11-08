//! Nemotron ASR streaming provider.
//!
//! Replaces the in-process Silero VAD + Whisper/Parakeet pipeline with a
//! persistent WebSocket session against `nemo-speech serve`. The server owns
//! VAD-driven endpointing and emits partials and finals over
//! `/v1/audio/transcriptions/realtime`; this crate maps them onto the existing
//! `SttProvider` / `SpeechEvent` contract so the rest of the voice pipeline
//! keeps its behavior.
//!
//! The runtime ASR chunk is server-side (`rnnt_right_context`): 1 means the
//! default 160 ms chunk. Do not confuse it with `asr.streaming.chunk_size`,
//! which applies to buffered CTC models only.

pub mod config;
pub mod pcm;
pub mod protocol;
pub mod provider;

pub use config::AsrConfig;
pub use provider::{NemotronAsrProvider, create_provider};
