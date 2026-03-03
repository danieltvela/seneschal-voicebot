pub mod llama_omni;
pub mod moshi;
pub mod ultravox;
pub mod lfm;

use serde::{Deserialize, Serialize};

/// Types of S2S models available
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelType {
    LlamaOmni,
    Moshi,
    Ultravox,
    LFM,
}

impl ModelType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "llama-omni" | "llamaomni" => Some(Self::LlamaOmni),
            "moshi" => Some(Self::Moshi),
            "ultravox" => Some(Self::Ultravox),
            "lfm" | "lfm2.5" => Some(Self::LFM),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::LlamaOmni => "llama-omni",
            Self::Moshi => "moshi",
            Self::Ultravox => "ultravox",
            Self::LFM => "lfm",
        }
    }
}

/// Configuration for S2S models
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub model_type: ModelType,
    pub model_path: String,
    pub sample_rate: u32,
    pub max_context_length: usize,
    pub temperature: f32,
    pub top_p: f32,
    pub enable_tools: bool,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            model_type: ModelType::LlamaOmni,
            model_path: "./models".to_string(),
            sample_rate: 16000,
            max_context_length: 4096,
            temperature: 0.7,
            top_p: 0.9,
            enable_tools: true,
        }
    }
}
