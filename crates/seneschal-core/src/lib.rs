// seneschal-core — Voice AI Pipeline nucleus
//
// STT → LLM → TTS streaming pipeline with VAD, barge-in, and FSM.
// No dependency on other workspace crates.

pub mod audio;
pub mod stt;
pub mod llm;
pub mod tts;
pub mod pipeline;
