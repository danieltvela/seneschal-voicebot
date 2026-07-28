// seneschal-core — Voice AI Pipeline nucleus
//
// STT → LLM → TTS streaming pipeline with VAD, barge-in, FSM,
// memory extraction, and profile management.
// Depends only on seneschal-common for shared types.

pub mod audio;
pub mod stt;
pub mod llm;
pub mod tts;
pub mod pipeline;
pub mod memory;
pub mod profile;
