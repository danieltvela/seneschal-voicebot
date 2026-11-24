//! Audio diagnostics behind the Control API (issue #34).
//!
//! The endpoints deliberately operate on the **active** daemon configuration:
//! device/provider changes are applied through the Settings API first. That
//! keeps one capture stream, one playback slot and one model instance, while
//! the panel can still test a freshly applied backend immediately after the
//! sidecar reconnects.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;
use tokio::sync::{Mutex as AsyncMutex, broadcast, mpsc, oneshot, watch};
use tokio::time::timeout;

use seneschal_core::audio::audio_capture::{AudioDeviceInventory, AudioLevels};
use seneschal_core::audio::output::AudioOutput;
use seneschal_core::audio::permissions::{
    MicrophonePermission, microphone_permission, request_microphone_permission,
};
use seneschal_core::pipeline::fsm::PipelineState;

use super::settings::SharedTtsEngine;

pub const DEFAULT_TEST_DURATION_SECS: u64 = 5;
const MIN_TEST_DURATION_SECS: u64 = 1;
const MAX_TEST_DURATION_SECS: u64 = 10;
/// Extra budget after recording for a remote/local STT one-shot request.
const STT_COMPLETION_BUDGET: Duration = Duration::from_secs(15);

const SPEAKER_TEST_WAV: &[u8] = include_bytes!("../../assets/speaker-test.wav");
const SPEAKER_TEST_TEXT: &str = "Seneschal audio test.";

/// Direction of the capture test that the main audio loop should run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioTestKind {
    Mic,
    Loopback,
}

impl AudioTestKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mic => "mic",
            Self::Loopback => "loopback",
        }
    }
}

/// Request sent from Control to the exclusive owner of the STT provider.
pub struct AudioTestRequest {
    pub kind: AudioTestKind,
    pub duration_secs: u64,
    pub reply: oneshot::Sender<Result<AudioTestResult, AudioTestError>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AudioTestResult {
    pub kind: AudioTestKind,
    /// STT transcript; `None` for loopback.
    pub transcript: Option<String>,
    pub peak: f32,
    pub rms: f32,
    pub duration_secs: f32,
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioTestError {
    pub code: AudioTestErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioTestErrorCode {
    InvalidDuration,
    AudioBusy,
    CaptureUnavailable,
    OutputUnavailable,
    ChannelClosed,
    TestTimedOut,
    Cancelled,
    ProviderError,
    TestAssetError,
}

impl AudioTestError {
    pub fn new(code: AudioTestErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn cancelled() -> Self {
        Self::new(AudioTestErrorCode::Cancelled, "Audio test cancelled")
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AudioStatusWire {
    pub capture_active: bool,
    pub capture_error: Option<String>,
    pub output_available: bool,
    pub microphone_permission: MicrophonePermission,
    pub configured_input_device: Option<String>,
    pub configured_output_device: Option<String>,
    pub stt_provider: String,
    pub tts_provider: String,
    pub test_in_progress: Option<&'static str>,
}

#[derive(Debug, Deserialize)]
pub struct TestDurationBody {
    #[serde(default)]
    pub duration_secs: Option<u64>,
}

pub struct AudioControl {
    tts: SharedTtsEngine,
    output: Arc<AudioOutput>,
    mic_tx: mpsc::Sender<AudioTestRequest>,
    barge_in: broadcast::Sender<u64>,
    pipeline_rx: watch::Receiver<PipelineState>,
    capture_active: Arc<AtomicBool>,
    capture_error: Arc<Mutex<Option<String>>>,
    test_in_progress: Arc<Mutex<Option<&'static str>>>,
    test_lock: Arc<AsyncMutex<()>>,
}

impl AudioControl {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tts: SharedTtsEngine,
        output: Arc<AudioOutput>,
        mic_tx: mpsc::Sender<AudioTestRequest>,
        barge_in: broadcast::Sender<u64>,
        pipeline_rx: watch::Receiver<PipelineState>,
        capture_active: Arc<AtomicBool>,
        capture_error: Arc<Mutex<Option<String>>>,
    ) -> Self {
        Self {
            tts,
            output,
            mic_tx,
            barge_in,
            pipeline_rx,
            capture_active,
            capture_error,
            test_in_progress: Arc::new(Mutex::new(None)),
            test_lock: Arc::new(AsyncMutex::new(())),
        }
    }

    pub fn devices(&self) -> anyhow::Result<AudioDeviceInventory> {
        seneschal_core::audio::audio_capture::AudioCapture::enumerate_devices()
    }

    pub async fn status(
        &self,
        configured_input_device: Option<String>,
        configured_output_device: Option<String>,
        stt_provider: String,
        tts_provider: String,
    ) -> AudioStatusWire {
        AudioStatusWire {
            capture_active: self.capture_active.load(Ordering::SeqCst),
            capture_error: self.capture_error.lock().unwrap().clone(),
            output_available: self.output.has_device(),
            microphone_permission: microphone_permission(),
            configured_input_device,
            configured_output_device,
            stt_provider,
            tts_provider,
            test_in_progress: *self.test_in_progress.lock().unwrap(),
        }
    }

    pub async fn request_microphone_permission(&self) -> MicrophonePermission {
        request_microphone_permission().await
    }

    pub async fn preview_tts(&self) -> Result<TtsPreviewResult, AudioTestError> {
        let _guard = self.test_lock.lock().await;
        self.begin_test("preview_tts");
        let result = self.play_tts_preview().await;
        self.end_test();
        result
    }

    pub async fn speaker_test(&self) -> Result<SpeakerTestResult, AudioTestError> {
        let _guard = self.test_lock.lock().await;
        self.ensure_idle()?;
        self.begin_test("speaker_test");
        let result = self.play_speaker_test().await;
        self.end_test();
        result
    }

    pub async fn capture_test(
        &self,
        kind: AudioTestKind,
        duration_secs: Option<u64>,
    ) -> Result<AudioTestResult, AudioTestError> {
        let duration_secs = duration_secs.unwrap_or(DEFAULT_TEST_DURATION_SECS);
        if !(MIN_TEST_DURATION_SECS..=MAX_TEST_DURATION_SECS).contains(&duration_secs) {
            return Err(AudioTestError::new(
                AudioTestErrorCode::InvalidDuration,
                format!(
                    "duration_secs must be between {MIN_TEST_DURATION_SECS} and {MAX_TEST_DURATION_SECS}"
                ),
            ));
        }

        let _guard = self.test_lock.lock().await;
        if !self.capture_active.load(Ordering::SeqCst) {
            return Err(AudioTestError::new(
                AudioTestErrorCode::CaptureUnavailable,
                "Audio capture is not active"
            )
            .with_hint("Select an available input device, grant microphone access, then apply the device settings."));
        }
        self.ensure_idle()?;
        self.begin_test(kind.as_str());

        let (reply_tx, reply_rx) = oneshot::channel();
        let request = AudioTestRequest {
            kind,
            duration_secs,
            reply: reply_tx,
        };
        if self.mic_tx.send(request).await.is_err() {
            self.end_test();
            return Err(AudioTestError::new(
                AudioTestErrorCode::ChannelClosed,
                "The main audio loop is not accepting tests",
            ));
        }

        let budget = Duration::from_secs(duration_secs) + STT_COMPLETION_BUDGET;
        let result = match timeout(budget, reply_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(AudioTestError::new(
                AudioTestErrorCode::ChannelClosed,
                "The main audio loop dropped the audio test",
            )),
            Err(_) => Err(AudioTestError::new(
                AudioTestErrorCode::TestTimedOut,
                "The audio test did not complete in time",
            )),
        };
        self.end_test();
        result
    }

    fn ensure_idle(&self) -> Result<(), AudioTestError> {
        if self.pipeline_rx.borrow().is_busy() {
            return Err(AudioTestError::new(
                AudioTestErrorCode::AudioBusy,
                "The voice pipeline is busy",
            )
            .with_hint("Wait until Seneschal is idle, or use barge-in."));
        }
        Ok(())
    }

    fn begin_test(&self, kind: &'static str) {
        *self.test_in_progress.lock().unwrap() = Some(kind);
    }

    fn end_test(&self) {
        *self.test_in_progress.lock().unwrap() = None;
    }

    async fn play_tts_preview(&self) -> Result<TtsPreviewResult, AudioTestError> {
        self.ensure_idle()?;
        let mut cancel_rx = self.barge_in.subscribe();
        let cancel = Arc::new(AtomicBool::new(false));

        let stream = {
            let engine = self.tts.read().await;
            let synth = engine.synthesize_stream(SPEAKER_TEST_TEXT);
            tokio::pin!(synth);
            tokio::select! {
                biased;
                _ = cancel_rx.recv() => return Err(AudioTestError::cancelled()),
                result = &mut synth => result.map_err(|error| {
                    AudioTestError::new(AudioTestErrorCode::ProviderError, format!("TTS synthesis failed: {error:#}"))
                })?,
            }
        };

        let provider = {
            let engine = self.tts.read().await;
            engine.provider_name().to_string()
        };
        self.play_stream(stream.rx, stream.sample_rate, cancel, &mut cancel_rx)
            .await?;
        Ok(TtsPreviewResult {
            provider,
            text: SPEAKER_TEST_TEXT.to_string(),
        })
    }

    async fn play_stream(
        &self,
        mut rx: mpsc::Receiver<Vec<f32>>,
        source_rate: u32,
        cancel: Arc<AtomicBool>,
        cancel_rx: &mut tokio::sync::broadcast::Receiver<u64>,
    ) -> Result<(), AudioTestError> {
        if !self.output.has_device() {
            return Err(AudioTestError::new(
                AudioTestErrorCode::OutputUnavailable,
                "No audio output device is available",
            )
            .with_hint("Select an available output device and apply the settings."));
        }

        let session = self
            .output
            .start_stream(source_rate, Arc::clone(&cancel))
            .map_err(|error| {
                AudioTestError::new(
                    AudioTestErrorCode::OutputUnavailable,
                    format!("Failed to start playback: {error:#}"),
                )
            })?;

        loop {
            tokio::select! {
                biased;
                _ = cancel_rx.recv() => {
                    cancel.store(true, Ordering::SeqCst);
                    session.finish();
                    return Err(AudioTestError::cancelled());
                }
                chunk = rx.recv() => {
                    match chunk {
                        Some(samples) => session.push(&samples),
                        None => break,
                    }
                }
            }
        }
        session.finish();
        session.wait();
        Ok(())
    }

    async fn play_speaker_test(&self) -> Result<SpeakerTestResult, AudioTestError> {
        let (samples, source_rate) = decode_speaker_test_wav()?;
        let duration_secs = samples.len() as f32 / source_rate as f32;
        let mut cancel_rx = self.barge_in.subscribe();
        let cancel = Arc::new(AtomicBool::new(false));
        let playback_cancel = Arc::clone(&cancel);
        let output = Arc::clone(&self.output);
        let playback = tokio::task::spawn_blocking(move || {
            output.play_blocking(&samples, source_rate, &playback_cancel)
        });

        tokio::select! {
            biased;
            _ = cancel_rx.recv() => {
                cancel.store(true, Ordering::SeqCst);
                // Dropping the join handle does not abort the blocking task; the
                // cancel flag makes it return promptly and release the slot.
                Err(AudioTestError::cancelled())
            }
            result = playback => {
                result
                    .map_err(|error| AudioTestError::new(AudioTestErrorCode::OutputUnavailable, format!("Playback task failed: {error}")))?
                    .map_err(|error| AudioTestError::new(AudioTestErrorCode::OutputUnavailable, format!("Playback failed: {error:#}")))?;
                Ok(SpeakerTestResult {
                    duration_secs,
                })
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct TtsPreviewResult {
    pub provider: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SpeakerTestResult {
    pub duration_secs: f32,
}

fn decode_speaker_test_wav() -> Result<(Vec<f32>, u32), AudioTestError> {
    let reader =
        hound::WavReader::new(std::io::Cursor::new(SPEAKER_TEST_WAV)).map_err(|error| {
            AudioTestError::new(
                AudioTestErrorCode::TestAssetError,
                format!("Invalid speaker-test WAV: {error}"),
            )
        })?;
    let spec = reader.spec();
    let sample_rate = spec.sample_rate;
    let samples = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .into_samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                AudioTestError::new(
                    AudioTestErrorCode::TestAssetError,
                    format!("Invalid speaker-test samples: {error}"),
                )
            })?
            .into_iter()
            .map(|sample| sample as f32 / i16::MAX as f32)
            .collect(),
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                AudioTestError::new(
                    AudioTestErrorCode::TestAssetError,
                    format!("Invalid speaker-test samples: {error}"),
                )
            })?,
    };
    let levels = AudioLevels::compute(&samples);
    if levels.is_silent(0.001) {
        return Err(AudioTestError::new(
            AudioTestErrorCode::TestAssetError,
            "The built-in speaker test is silent",
        ));
    }
    Ok((samples, sample_rate))
}

/// Shared helper for the status endpoint: derive the configured value from a
/// Settings snapshot without exposing the whole config.
pub fn setting_string(
    snapshot: &seneschal_common::settings::SettingsSnapshot,
    key: &str,
) -> Option<String> {
    snapshot
        .fields
        .iter()
        .find(|field| field.key == key)
        .and_then(|field| field.value.as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speaker_test_asset_is_decodable_and_audible() {
        let (samples, rate) = decode_speaker_test_wav().expect("built-in WAV decodes");
        assert_eq!(rate, 16_000);
        assert!(samples.len() >= 16_000, "test tone must last at least 1 s");
        let levels = AudioLevels::compute(&samples);
        assert!(levels.peak > 0.05);
    }

    #[test]
    fn duration_validation_bounds_are_stable() {
        assert_eq!(DEFAULT_TEST_DURATION_SECS, 5);
        assert_eq!(MIN_TEST_DURATION_SECS, 1);
        assert_eq!(MAX_TEST_DURATION_SECS, 10);
    }

    #[tokio::test]
    async fn tts_engine_read_guard_is_not_held_during_playback() {
        // The Mock engine synthesizes synchronously; after `synthesize_stream`
        // returns, another task must be able to take the write guard (the
        // Settings hot-apply path).
        let mock = seneschal_core::tts::mock_tts::MockTts::new().0;
        let tts = Arc::new(tokio::sync::RwLock::new(
            seneschal_core::tts::TtsEngine::Mock(mock),
        ));
        let stream = {
            let engine = tts.read().await;
            engine
                .synthesize_stream("one")
                .await
                .expect("mock synthesis")
        };

        {
            let mut engine = tts.write().await;
            engine
                .apply_voice_and_rate(Some("test"), Some(1.0))
                .expect("mock hot apply");
        }

        let mut rx = stream.rx;
        assert!(rx.recv().await.is_some());
    }
}
