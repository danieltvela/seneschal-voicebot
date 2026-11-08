//! Optional integration test against a live `nemotron-asr` service.
//!
//! Skipped unless a WAV fixture is supplied:
//!
//! ```bash
//! ASR_INTEGRATION_WAV=/path/to/es-sample.wav \
//!   cargo test -p asr --test service -- --ignored --nocapture
//! ```
//!
//! The fixture should be 16 kHz mono (or will be resampled) and contain one
//! short spoken utterance followed by enough silence for endpointing.

use std::time::Duration;

use asr::config::AsrConfig;
use asr::provider::NemotronAsrProvider;
use seneschal_core::audio::audio_transform::resample_nearest;
use seneschal_core::stt::{SpeechEvent, SttProvider};
use tokio::sync::mpsc;

const SAMPLES_PER_CHUNK: usize = 1_600; // 100 ms at 16 kHz
const ENDING_SILENCE_MS: u32 = 1_200;

fn read_mono_16k(path: &str) -> anyhow::Result<Vec<f32>> {
    let reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels.max(1));
    let source_rate = spec.sample_rate;
    let mut frames = Vec::new();
    for sample in reader.into_samples::<i16>() {
        frames.push(f32::from(sample?) / 32_768.0);
    }
    let mono: Vec<f32> = frames
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect();
    Ok(resample_nearest(&mono, source_rate, 16_000))
}

#[tokio::test]
#[ignore = "live Nemotron service: set ASR_INTEGRATION_WAV (and optionally ASR_INTEGRATION_URL)"]
async fn real_service_completes_a_fixture_turn() {
    let Some(path) = std::env::var("ASR_INTEGRATION_WAV")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("ASR_INTEGRATION_WAV not set; skipping");
        return;
    };
    let base_url = std::env::var("ASR_INTEGRATION_URL")
        .unwrap_or_else(|_| "http://localhost:8026".to_string());

    // Best-effort readiness: the ignored test should not fail merely because
    // the Docker profile is not running.
    let check = reqwest::Client::new()
        .get(format!("{}/ready", base_url.trim_end_matches('/')))
        .timeout(Duration::from_secs(2))
        .send()
        .await;
    if let Ok(response) = check {
        if !response.status().is_success() {
            eprintln!("ASR service returned {}", response.status());
            return;
        }
    } else {
        eprintln!("ASR service is not reachable at {base_url}; skipping");
        return;
    }

    let mut audio = read_mono_16k(&path).expect("valid WAV fixture");
    assert!(!audio.is_empty(), "fixture has no samples");
    audio.extend(std::iter::repeat_n(0.0, (ENDING_SILENCE_MS as usize) * 16));

    let config = AsrConfig {
        base_url,
        api_key: std::env::var("STT_API_KEY").unwrap_or_default(),
        model: std::env::var("STT_MODEL").unwrap_or_else(|_| "nemotron-3.5".to_string()),
        language: std::env::var("STT_LANGUAGE").unwrap_or_else(|_| "auto".to_string()),
        endpointing_ms: 300,
        connect_timeout: Duration::from_secs(5),
        http_timeout: Duration::from_secs(15),
    };

    let (event_tx, mut event_rx) = mpsc::channel(256);
    let mut provider = NemotronAsrProvider::new(config).unwrap();
    for chunk in audio.chunks(SAMPLES_PER_CHUNK) {
        provider.process_audio(chunk, &event_tx).await.unwrap();
    }

    let mut saw_partial = false;
    let mut final_text = String::new();
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(20), event_rx.recv()).await
    {
        match event {
            SpeechEvent::Speech(_) => saw_partial = true,
            SpeechEvent::SpeechEnd { quality, .. } => {
                final_text = quality.text;
                break;
            }
            SpeechEvent::SpeechStart { .. } => {}
        }
    }

    assert!(
        saw_partial,
        "the live service should emit at least one partial"
    );
    assert!(
        !final_text.trim().is_empty(),
        "endpointing should produce a non-empty final transcript"
    );
    eprintln!("Live Nemotron transcript: {final_text}");
}
