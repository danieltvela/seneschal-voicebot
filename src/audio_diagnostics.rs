//! Mic/loopback test recording on the existing capture path (issue #34).
//!
//! The main audio loop is the only owner of the configured `SttProvider`, so a
//! diagnostic test is recorded there instead of opening a second CPAL capture
//! stream. While a test is active, chunks still flow through the normal
//! capture channel (keeping the meter live), but they do not feed the STT/LLM
//! pipeline.

use std::time::Instant;

use seneschal_control::control::audio::{AudioTestKind, AudioTestResult};

/// Accumulates mono 16 kHz chunks for one in-flight audio diagnostic.
pub struct AudioTestRecorder {
    kind: AudioTestKind,
    requested_duration_secs: u64,
    sample_rate: u32,
    samples: Vec<f32>,
    sum_squares: f64,
    peak: f32,
    started_at: Instant,
}

impl AudioTestRecorder {
    pub fn new(kind: AudioTestKind, requested_duration_secs: u64, sample_rate: u32) -> Self {
        Self {
            kind,
            requested_duration_secs,
            sample_rate,
            samples: Vec::with_capacity(sample_rate as usize * requested_duration_secs as usize),
            sum_squares: 0.0,
            peak: 0.0,
            started_at: Instant::now(),
        }
    }

    pub fn kind(&self) -> AudioTestKind {
        self.kind
    }

    /// Append one mono chunk and report whether the requested window is full.
    pub fn record(&mut self, mono: &[f32]) -> bool {
        self.samples.extend_from_slice(mono);
        for &sample in mono {
            self.sum_squares += f64::from(sample * sample);
            self.peak = self.peak.max(sample.abs());
        }
        self.samples.len() >= self.sample_rate as usize * self.requested_duration_secs as usize
            || self.started_at.elapsed().as_secs() > self.requested_duration_secs + 1
    }

    pub fn finish(self) -> (AudioTestResult, Vec<f32>) {
        let count = self.samples.len().max(1);
        let rms = (self.sum_squares / count as f64).sqrt() as f32;
        let result = AudioTestResult {
            kind: self.kind,
            transcript: None,
            peak: self.peak,
            rms,
            duration_secs: self.samples.len() as f32 / self.sample_rate as f32,
            provider: None,
        };
        (result, self.samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(amplitude: f32, samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|i| (i as f32 * 2.0 * std::f32::consts::PI / 100.0).sin() * amplitude)
            .collect()
    }

    #[test]
    fn records_the_requested_window_and_peak() {
        let mut recorder = AudioTestRecorder::new(AudioTestKind::Mic, 1, 16_000);
        let mut done = false;
        for _ in 0..10 {
            done = recorder.record(&sine(0.5, 1_600));
        }
        assert!(done);
        let (result, samples) = recorder.finish();
        assert_eq!(samples.len(), 16_000);
        assert_eq!(result.kind, AudioTestKind::Mic);
        assert_eq!(result.duration_secs, 1.0);
        assert!((result.peak - 0.5).abs() < 0.01);
        assert!((result.rms - 0.5 / 2f32.sqrt()).abs() < 0.01);
        assert!(result.transcript.is_none());
    }

    #[test]
    fn loopback_recorder_does_not_require_a_transcript() {
        let mut recorder = AudioTestRecorder::new(AudioTestKind::Loopback, 2, 16_000);
        assert!(!recorder.record(&sine(0.1, 8_000)));
        let (result, samples) = recorder.finish();
        assert_eq!(samples.len(), 8_000);
        assert_eq!(result.kind, AudioTestKind::Loopback);
        assert!(result.transcript.is_none());
        assert!(result.provider.is_none());
    }
}
