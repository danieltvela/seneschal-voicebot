use anyhow::Result;

// Import path for kokorox (fork of kokoros/Kokoros).
// If the crate re-exports TTSKoko at the root, use: `kokorox::TTSKoko`.
// Check with: cargo doc --features kokoro --open
use kokorox::tts::koko::TTSKoko;

/// Kokoro TTS wrapper.
///
/// Wraps `TTSKoko` (ONNX-based, 24 kHz output) with the same `synthesize` /
/// `sample_rate` interface as `SayTts` so it can be swapped in via `TtsEngine`.
///
/// Requires:
/// - `brew install espeak-ng`  (macOS)
/// - `models/kokoro-v1.0.onnx` + `models/voices-v1.0.bin`
///   (download from <https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX>)
pub struct KokoroTts {
    inner: TTSKoko,
    /// Voice style name, e.g. `"af_bella"`, `"es_*"`.
    voice: String,
    /// BCP-47 language code passed to espeak-ng, e.g. `"en-us"`, `"es"`.
    language: String,
    /// Default speech speed (e.g. 1.3).
    speed: f32,
}

impl KokoroTts {
    /// Load the Kokoro ONNX model and voice embeddings asynchronously.
    ///
    /// `model_path`  — path to `kokoro-v1.0.onnx`
    /// `voices_path` — path to `voices-v1.0.bin`
    /// `voice`       — voice style name (see `tts.get_available_voices()`)
    /// `language`    — BCP-47 code for espeak-ng phonemisation
    /// `speed`       — default speech speed (e.g. 1.3)
    /// `device_id`   — CUDA device for the ONNX Runtime session (issue #14);
    ///                 `None` = CPU. Ignored when built without `cuda`.
    pub async fn new(
        model_path: &str,
        voices_path: &str,
        voice: &str,
        language: &str,
        speed: f32,
        device_id: Option<i32>,
    ) -> Result<Self> {
        if device_id.is_some() && !cfg!(feature = "cuda") {
            tracing::warn!(
                target: "tts",
                "Kokoro assigned to a GPU but this build lacks the 'cuda' feature; running on CPU (rebuild with --features cuda, or set GPU_AUTO=0)"
            );
        }
        let inner =
            TTSKoko::new_with_device_id(Some(model_path), Some(voices_path), device_id).await;
        tracing::info!(
            target: "tts",
            "Kokoro TTS loaded: model={} voice={} lang={} speed={} device={}",
            model_path,
            voice,
            language,
            speed,
            device_id
                .map(|i| i.to_string())
                .unwrap_or_else(|| "cpu".into())
        );
        Ok(Self {
            inner,
            voice: voice.to_string(),
            language: language.to_string(),
            speed,
        })
    }

    /// Synthesise `text` into mono f32 PCM at 24 000 Hz.
    ///
    /// CPU-intensive — call from `tokio::task::spawn_blocking`.
    pub fn synthesize(&self, text: &str) -> Result<Vec<f32>> {
        self.inner
            .tts_raw_audio(
                text,
                &self.language,
                &self.voice,
                self.speed,
                None,  // initial_silence
                false, // auto_detect_language (we set it explicitly)
                true,  // force_style
                false, // phonemes mode
            )
            .map_err(|e| anyhow::anyhow!("Kokoro TTS: {}", e))
    }

    pub fn sample_rate(&self) -> u32 {
        24_000
    }

    /// Print all available Kokoro voice styles to stdout.
    ///
    /// Voice naming convention: `{lang}{gender}_{name}`
    /// - `af_*` = American English, Female
    /// - `am_*` = American English, Male
    /// - `bf_*` = British English, Female
    /// - `bm_*` = British English, Male
    /// - `ef_*` = Spanish, Female
    /// - `em_*` = Spanish, Male
    /// - `ff_*` = French, Female
    /// - `hf_*` = Hindi, Female
    /// - `jf_*` = Japanese, Female
    /// - `zf_*` = Chinese, Female
    pub fn list_voices(&self) {
        let voices = self.inner.get_available_voices();

        println!("Available voices for TTS provider: kokoro");
        println!("{:<25} {:<20} {:<10}", "Voice ID", "Language", "Gender");
        println!("{}", "-".repeat(55));
        for voice in &voices {
            let (lang, gender) = parse_kokoro_voice_id(voice);
            println!("{:<25} {:<20} {:<10}", voice, lang, gender);
        }
        println!("\nTotal: {} voices", voices.len());
    }
}

/// Parse a Kokoro voice ID prefix into (language, gender) labels.
fn parse_kokoro_voice_id(id: &str) -> (&str, &str) {
    if id.len() < 2 {
        return ("Unknown", "Unknown");
    }
    let prefix: Vec<char> = id.chars().take(2).collect();
    let lang = match prefix[0] {
        'a' => "English (American)",
        'b' => "English (British)",
        'e' => "Spanish",
        'f' => "French",
        'h' => "Hindi",
        'i' => "Italian",
        'j' => "Japanese",
        'p' => "Portuguese (Brazilian)",
        'z' => "Chinese (Mandarin)",
        _ => "Unknown",
    };
    let gender = match prefix[1] {
        'f' => "Female",
        'm' => "Male",
        _ => "Unknown",
    };
    (lang, gender)
}
