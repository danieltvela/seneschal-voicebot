use anyhow::Result;

/// Voice Activity Detection
/// Detects when speech is present in audio input
pub struct VoiceActivityDetector {
    threshold: f32,
    min_speech_duration_ms: u32,
    min_silence_duration_ms: u32,
    sample_rate: u32,
    is_speech_active: bool,
    speech_frames: u32,
    silence_frames: u32,
}

impl VoiceActivityDetector {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            threshold: 0.02, // Energy threshold for speech detection
            min_speech_duration_ms: 100,
            min_silence_duration_ms: 500,
            sample_rate,
            is_speech_active: false,
            speech_frames: 0,
            silence_frames: 0,
        }
    }

    /// Process audio chunk and detect voice activity
    pub fn process(&mut self, audio_data: &[f32]) -> VadResult {
        let energy = self.calculate_energy(audio_data);
        let is_speech = energy > self.threshold;

        if is_speech {
            self.speech_frames += 1;
            self.silence_frames = 0;

            if !self.is_speech_active
                && self.speech_frames >= self.frames_for_duration(self.min_speech_duration_ms)
            {
                self.is_speech_active = true;
                return VadResult::SpeechStart;
            }
        } else {
            self.silence_frames += 1;

            if self.is_speech_active
                && self.silence_frames >= self.frames_for_duration(self.min_silence_duration_ms)
            {
                self.is_speech_active = false;
                self.speech_frames = 0;
                return VadResult::SpeechEnd;
            }
        }

        if self.is_speech_active {
            VadResult::Speech
        } else {
            VadResult::Silence
        }
    }

    fn calculate_energy(&self, audio_data: &[f32]) -> f32 {
        let sum: f32 = audio_data.iter().map(|&x| x * x).sum();
        sum / audio_data.len() as f32
    }

    fn frames_for_duration(&self, duration_ms: u32) -> u32 {
        (self.sample_rate * duration_ms) / 1000
    }

    pub fn reset(&mut self) {
        self.is_speech_active = false;
        self.speech_frames = 0;
        self.silence_frames = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VadResult {
    Speech,
    Silence,
    SpeechStart,
    SpeechEnd,
}
