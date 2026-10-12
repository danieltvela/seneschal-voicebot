//! Preset voice catalogue + discovery for the vLLM-Omni speech service
//! (issue #16).
//!
//! `GET /v1/audio/voices` returns the authoritative list of names the running
//! checkpoint accepts. We keep a small label table for the Qwen3-TTS
//! CustomVoice presets so `--list-voices` is readable, and fall back to
//! discovered-but-unlabelled entries when the service reports more voices.

use serde::Deserialize;

use super::error::{VllmTtsError, VllmTtsResult};

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

/// Fetch the list of voices the service currently offers.
pub async fn fetch_voices(
    client: &reqwest::Client,
    base_url: &str,
) -> VllmTtsResult<VoicesResponse> {
    let url = format!("{}/v1/audio/voices", base_url.trim_end_matches('/'));
    let resp = client.get(&url).send().await.map_err(map_reqwest)?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let message = resp
            .text()
            .await
            .unwrap_or_else(|_| "<no body>".to_string());
        return Err(VllmTtsError::Service { status, message });
    }
    resp.json::<VoicesResponse>()
        .await
        .map_err(|e| VllmTtsError::Decode(format!("invalid /v1/audio/voices response: {e}")))
}

/// Print a human-readable voice table to stdout (`--list-voices`).
pub async fn print_voices(client: &reqwest::Client, base_url: &str) -> VllmTtsResult<()> {
    let resp = fetch_voices(client, base_url).await?;

    println!("Available voices for TTS provider: openai (vLLM-Omni / Qwen3-TTS)");
    println!("Service: {base_url}");
    println!();
    println!(
        "{:<14} {:<18} {:<10}",
        "Voice ID", "Description", "Languages"
    );
    println!("{}", "-".repeat(45));
    for name in &resp.voices {
        let (label, langs) = preset_meta(name).unwrap_or(("(uploaded)", "—"));
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

/// Validate that `voice` is in the service's accepted list. Returns the list of
/// supported voices when it is not, so the caller can print alternatives.
pub async fn validate_voice(
    client: &reqwest::Client,
    base_url: &str,
    voice: &str,
) -> VllmTtsResult<()> {
    let resp = fetch_voices(client, base_url).await?;
    if resp.voices.iter().any(|v| v == voice)
        || resp.uploaded_voices.iter().any(|v| v.name == voice)
    {
        return Ok(());
    }
    Err(VllmTtsError::Invalid(format!(
        "voice '{voice}' is not offered by {base_url}. Supported: {}",
        resp.voices.join(", ")
    )))
}

/// Convert any transport error into a provider error, mapping timeouts.
pub(crate) fn map_reqwest(e: reqwest::Error) -> VllmTtsError {
    if e.is_timeout() {
        VllmTtsError::Timeout(e.to_string())
    } else if e.is_connect() {
        VllmTtsError::Connection(e.to_string())
    } else if e.is_decode() {
        VllmTtsError::Decode(e.to_string())
    } else {
        VllmTtsError::Connection(format!("{e}"))
    }
}
