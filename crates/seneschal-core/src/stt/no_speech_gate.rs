use tracing::debug;

/// Quality metrics extracted from a Whisper transcription.
#[derive(Debug, Clone)]
pub struct TranscriptionQuality {
    pub text: String,
    pub no_speech_prob: f32,
    pub avg_logprob: f32,
    pub compression_ratio: f32,
}

/// Post-STT quality gate that rejects non-speech transcriptions.
#[derive(Debug, Clone)]
pub struct NoSpeechGate {
    /// If no_speech_prob > this threshold AND avg_logprob < logprob_threshold, reject.
    pub no_speech_threshold: f32,
    /// If avg_logprob < this threshold, the transcription is low confidence.
    pub logprob_threshold: f32,
    /// If compression_ratio > this threshold, the text is likely gibberish.
    pub compression_threshold: f32,
}

impl Default for NoSpeechGate {
    fn default() -> Self {
        Self {
            no_speech_threshold: 0.6,
            logprob_threshold: -1.0,
            compression_threshold: 2.4,
        }
    }
}

impl NoSpeechGate {
    pub fn should_reject(&self, quality: &TranscriptionQuality) -> bool {
        let text = quality.text.trim();
        if text.is_empty() {
            return true;
        }

        // Rule 1: no_speech_prob high + low confidence → reject
        if quality.no_speech_prob > self.no_speech_threshold
            && quality.avg_logprob < self.logprob_threshold
        {
            debug!(
                target: "nospeechgate",
                "Rejected (no_speech): ns_prob={:.3}, logprob={:.3}, text={:?}",
                quality.no_speech_prob, quality.avg_logprob, text
            );
            return true;
        }

        // Rule 2: compression ratio too high → gibberish → reject
        if quality.compression_ratio > self.compression_threshold {
            debug!(
                target: "nospeechgate",
                "Rejected (compression): ratio={:.2}, text={:?}",
                quality.compression_ratio, text
            );
            return true;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quality(text: &str, ns: f32, lp: f32, cr: f32) -> TranscriptionQuality {
        TranscriptionQuality {
            text: text.to_string(),
            no_speech_prob: ns,
            avg_logprob: lp,
            compression_ratio: cr,
        }
    }

    #[test]
    fn rejects_empty_text() {
        let gate = NoSpeechGate::default();
        assert!(gate.should_reject(&quality("", 0.0, 0.0, 0.0)));
        assert!(gate.should_reject(&quality("   ", 0.0, 0.0, 0.0)));
    }

    #[test]
    fn rejects_high_no_speech_with_low_logprob() {
        let gate = NoSpeechGate::default();
        assert!(gate.should_reject(&quality("uh huh", 0.8, -1.5, 1.2)));
    }

    #[test]
    fn accepts_high_no_speech_with_good_logprob() {
        let gate = NoSpeechGate::default();
        // no_speech is high but logprob is also good → not rejected by rule 1
        assert!(!gate.should_reject(&quality("hello", 0.8, -0.2, 1.2)));
    }

    #[test]
    fn rejects_high_compression_ratio() {
        let gate = NoSpeechGate::default();
        assert!(gate.should_reject(&quality("aaaa", 0.1, -0.2, 3.0)));
    }

    #[test]
    fn accepts_good_transcription() {
        let gate = NoSpeechGate::default();
        assert!(!gate.should_reject(&quality("Hello world how are you", 0.1, -0.3, 1.5)));
    }

    #[test]
    fn custom_thresholds() {
        let gate = NoSpeechGate {
            no_speech_threshold: 0.8,
            logprob_threshold: -0.5,
            compression_threshold: 3.0,
        };
        // ns=0.7 < 0.8 threshold → rule 1 doesn't fire
        assert!(!gate.should_reject(&quality("test", 0.7, -0.6, 1.0)));
        // ns=0.9 > 0.8 AND lp=-0.6 < -0.5 → rule 1 fires
        assert!(gate.should_reject(&quality("test", 0.9, -0.6, 1.0)));
    }
}
