//! First-run onboarding support behind the Control API (issue #36).
//!
//! The wizard is an orchestration surface over Settings, Models and Audio. This
//! module only centralizes the per-OS recommendation so the frontend does not
//! hard-code a hardware/backend matrix, and keeps the answer limited to
//! backends that are actually compiled into the running daemon.

use serde::Serialize;

use seneschal_core::stt;
use seneschal_core::tts::TtsEngine;

#[derive(Debug, Clone, Serialize)]
pub struct OnboardingDefaults {
    /// `macos`, `linux`, `windows`, ... (`std::env::consts::OS`).
    pub platform: &'static str,
    pub gpu_available: bool,
    pub recommended_stt: String,
    pub recommended_tts: String,
}

/// Recommend a working backend pair for this build and host.
///
/// The recommendation is a sensible starting point, never a hard restriction:
/// the wizard only offers backends reported by `GET /control/config.providers`.
fn pick_provider(preferred: &[&'static str], stt: &[&str], tts: &[&str]) -> &'static str {
    preferred
        .iter()
        .find(|candidate| stt.contains(candidate) || tts.contains(candidate))
        .copied()
        // The test/control-only build may have no TTS feature; OpenAI-compatible
        // is still the safest wire value for a desktop sidecar build.
        .unwrap_or("openai")
}

pub fn defaults(gpu_available: bool) -> OnboardingDefaults {
    let platform = std::env::consts::OS;
    let stt = stt::supported_providers();
    let tts = TtsEngine::supported_providers();

    let (recommended_stt, recommended_tts) = if platform == "macos" {
        (
            pick_provider(&["speech", "whisper", "nemotron"], &stt, &tts),
            pick_provider(&["avspeech", "kokoro", "openai"], &stt, &tts),
        )
    } else if gpu_available
        && stt.contains(&"nemotron")
        && (tts.is_empty() || tts.contains(&"openai"))
    {
        ("nemotron", "openai")
    } else {
        (
            pick_provider(&["whisper", "nemotron"], &stt, &tts),
            pick_provider(&["kokoro", "openai", "avspeech"], &stt, &tts),
        )
    };

    OnboardingDefaults {
        platform,
        gpu_available,
        recommended_stt: recommended_stt.to_string(),
        recommended_tts: recommended_tts.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_prefers_the_zero_download_native_pair() {
        let defaults = defaults(false);
        if std::env::consts::OS == "macos" {
            assert!(stt::supported_providers().contains(&defaults.recommended_stt.as_str()));
            let tts = TtsEngine::supported_providers();
            if tts.is_empty() {
                assert_eq!(defaults.recommended_tts, "openai");
            } else {
                assert!(tts.contains(&defaults.recommended_tts.as_str()));
            }
            if stt::supported_providers().contains(&"speech") {
                assert_eq!(defaults.recommended_stt, "speech");
            }
            if tts.contains(&"avspeech") {
                assert_eq!(defaults.recommended_tts, "avspeech");
            }
        }
    }

    #[test]
    fn linux_defaults_are_compilation_aware() {
        let defaults = defaults(false);
        if std::env::consts::OS == "linux" {
            assert!(stt::supported_providers().contains(&defaults.recommended_stt.as_str()));
            let tts = TtsEngine::supported_providers();
            if tts.is_empty() {
                // The isolated control crate can be built without TTS features;
                // the desktop sidecar always compiles at least OpenAI TTS.
                assert_eq!(defaults.recommended_tts, "openai");
            } else {
                assert!(tts.contains(&defaults.recommended_tts.as_str()));
            }
            if !stt::supported_providers().contains(&"nemotron") {
                assert_eq!(defaults.recommended_stt, "whisper");
            }
        }
    }

    #[test]
    fn gpu_can_prefer_the_nemo_pair_on_non_macos_hosts() {
        let defaults = defaults(true);
        if std::env::consts::OS != "macos"
            && stt::supported_providers().contains(&"nemotron")
            && (TtsEngine::supported_providers().is_empty()
                || TtsEngine::supported_providers().contains(&"openai"))
        {
            assert_eq!(defaults.recommended_stt, "nemotron");
            assert_eq!(defaults.recommended_tts, "openai");
        }
        assert!(defaults.gpu_available);
    }
}
