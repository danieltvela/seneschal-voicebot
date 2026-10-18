//! Preset voice catalogue + discovery for the OpenAI-compatible speech service
//! (issue #16).
//!
//! `GET /v1/audio/voices` returns the authoritative list of names the running
//! checkpoint accepts. We keep a small label table for the Qwen3-TTS
//! CustomVoice presets so `--list-voices` is readable, and fall back to
//! discovered-but-unlabelled entries when the service reports more voices.
//!
//! Not every OpenAI-compatible server has `/v1/audio/voices`: NVIDIA
//! NeMo-Speech.cpp (MagpieTTS, issue #20) advertises its `speech` model's
//! `voices`/`languages` through `GET /v1/models` instead. When the legacy route
//! is absent we fall back to that inventory.

use serde::Deserialize;

use super::error::{OpenAiTtsError, OpenAiTtsResult};

/// Preset Qwen3-TTS CustomVoice voices with a short human label + language hint.
pub const PRESET_VOICES: &[(&str, &str, &str)] = &[
    ("aiden", "Male, clear", "en/zh"),
    ("dylan", "Male, deep", "en/zh"),
    ("eric", "Male, lively", "en/zh"),
    ("ono_anna", "Female, soft", "ja/zh"),
    ("ryan", "Male, neutral", "en/zh"),
    ("serena", "Female, warm", "en/zh"),
    ("sohee", "Female, bright", "ko/zh"),
    ("uncle_fu", "Male, mature", "zh"),
    ("vivian", "Female, neutral", "en/zh"),
];

/// Look up the label/language hint for a known preset voice.
pub fn preset_meta(name: &str) -> Option<(&'static str, &'static str)> {
    PRESET_VOICES
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, label, langs)| (*label, *langs))
}

/// Response shape of `GET /v1/audio/voices`.
#[derive(Debug, Deserialize)]
pub struct VoicesResponse {
    #[serde(default)]
    pub voices: Vec<String>,
    #[serde(default)]
    pub uploaded_voices: Vec<UploadedVoice>,
}

/// An uploaded (cloned) voice as reported by the service.
#[derive(Debug, Deserialize)]
pub struct UploadedVoice {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub ref_text: Option<String>,
}

/// Response shape of `GET /v1/models` (only the fields we need). OpenAI lists
/// models under `data`; each entry may carry the speech model's `voices` and
/// `languages` (NeMo-Speech.cpp, issue #20).
#[derive(Debug, Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
struct ModelEntry {
    #[serde(default)]
    capability: String,
    #[serde(default)]
    voices: Vec<String>,
}

/// Fetch the list of voices the service currently offers.
///
/// Tries the vLLM-Omni-style `GET /v1/audio/voices` first; when that route is
/// missing (e.g. NeMo-Speech.cpp) it reads the `speech` model's `voices` from
/// `GET /v1/models`.
pub async fn fetch_voices(
    client: &reqwest::Client,
    base_url: &str,
) -> OpenAiTtsResult<VoicesResponse> {
    match fetch_audio_voices(client, base_url).await {
        Ok(resp) => Ok(resp),
        Err(primary_err) => match fetch_models_voices(client, base_url).await {
            Ok(resp) if !resp.voices.is_empty() => Ok(resp),
            _ => Err(primary_err),
        },
    }
}

/// `GET /v1/audio/voices` (vLLM-Omni / Qwen3-TTS).
async fn fetch_audio_voices(
    client: &reqwest::Client,
    base_url: &str,
) -> OpenAiTtsResult<VoicesResponse> {
    let url = format!("{}/v1/audio/voices", base_url.trim_end_matches('/'));
    let resp = client.get(&url).send().await.map_err(map_reqwest)?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let message = resp
            .text()
            .await
            .unwrap_or_else(|_| "<no body>".to_string());
        return Err(OpenAiTtsError::Service { status, message });
    }
    resp.json::<VoicesResponse>()
        .await
        .map_err(|e| OpenAiTtsError::Decode(format!("invalid /v1/audio/voices response: {e}")))
}

/// `GET /v1/models`, taking the voices of the `speech` capability
/// (NeMo-Speech.cpp).
async fn fetch_models_voices(
    client: &reqwest::Client,
    base_url: &str,
) -> OpenAiTtsResult<VoicesResponse> {
    let url = format!("{}/v1/models", base_url.trim_end_matches('/'));
    let resp = client.get(&url).send().await.map_err(map_reqwest)?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let message = resp
            .text()
            .await
            .unwrap_or_else(|_| "<no body>".to_string());
        return Err(OpenAiTtsError::Service { status, message });
    }
    let models = resp
        .json::<ModelsResponse>()
        .await
        .map_err(|e| OpenAiTtsError::Decode(format!("invalid /v1/models response: {e}")))?;
    let voices = models
        .data
        .into_iter()
        .find(|m| m.capability == "speech")
        .map(|m| m.voices)
        .unwrap_or_default();
    Ok(VoicesResponse {
        voices,
        uploaded_voices: Vec::new(),
    })
}

/// Print a human-readable voice table to stdout (`--list-voices`).
pub async fn print_voices(client: &reqwest::Client, base_url: &str) -> OpenAiTtsResult<()> {
    let resp = fetch_voices(client, base_url).await?;

    println!("Available voices for TTS provider: openai (Qwen3-TTS via an OpenAI-compatible API)");
    println!("Service: {base_url}");
    println!();
    println!(
        "{:<14} {:<18} {:<10}",
        "Voice ID", "Description", "Languages"
    );
    println!("{}", "-".repeat(45));
    for name in &resp.voices {
        let (label, langs) = preset_meta(name).unwrap_or(("(service)", "—"));
        println!("{:<14} {:<18} {:<10}", name, label, langs);
    }
    println!("\nTotal: {} voices", resp.voices.len());

    if !resp.uploaded_voices.is_empty() {
        println!("\nUploaded (cloned) voices:");
        for v in &resp.uploaded_voices {
            let has_transcript = if v.ref_text.as_deref().unwrap_or("").is_empty() {
                "no transcript"
            } else {
                "with transcript"
            };
            println!("  {:<14} ({has_transcript})", v.name);
        }
        println!(
            "\nNote: the running CustomVoice checkpoint may reject uploaded voices at \
             synthesis time — cloning requires the Base checkpoint (issue #16)."
        );
    }
    Ok(())
}

/// OpenAI voice aliases some servers accept as a shortcut to their configured
/// default speaker (NeMo-Speech.cpp). They are not real voices but must not be
/// reported as unknown.
const STANDARD_ALIASES: &[&str] = &[
    "alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer", "verse",
];

/// Validate that `voice` is in the service's accepted list. Returns the list of
/// supported voices when it is not, so the caller can print alternatives.
pub async fn validate_voice(
    client: &reqwest::Client,
    base_url: &str,
    voice: &str,
) -> OpenAiTtsResult<()> {
    if voice.eq_ignore_ascii_case("default")
        || STANDARD_ALIASES
            .iter()
            .any(|a| a.eq_ignore_ascii_case(voice))
    {
        return Ok(());
    }
    let resp = fetch_voices(client, base_url).await?;
    if resp.voices.iter().any(|v| v.eq_ignore_ascii_case(voice))
        || resp.uploaded_voices.iter().any(|v| v.name == voice)
    {
        return Ok(());
    }
    Err(OpenAiTtsError::Invalid(format!(
        "voice '{voice}' is not offered by {base_url}. Supported: {}",
        resp.voices.join(", ")
    )))
}

/// Convert any transport error into a provider error, mapping timeouts.
pub(crate) fn map_reqwest(e: reqwest::Error) -> OpenAiTtsError {
    if e.is_timeout() {
        OpenAiTtsError::Timeout(e.to_string())
    } else if e.is_connect() {
        OpenAiTtsError::Connection(e.to_string())
    } else if e.is_decode() {
        OpenAiTtsError::Decode(e.to_string())
    } else {
        OpenAiTtsError::Connection(format!("{e}"))
    }
}
