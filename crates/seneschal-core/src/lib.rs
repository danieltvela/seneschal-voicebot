// seneschal-core — Voice AI Pipeline nucleus
//
// STT → LLM → TTS streaming pipeline with VAD, barge-in, FSM,
// memory extraction, and profile management.
// Depends only on seneschal-common for shared types.

pub mod audio;
pub mod llm;
pub mod memory;
pub mod pipeline;
pub mod profile;
pub mod stt;
pub mod tts;
