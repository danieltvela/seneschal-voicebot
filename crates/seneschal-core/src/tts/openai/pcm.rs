//! Raw PCM decoding for the vLLM-Omni speech endpoint (issue #16).
//!
//! With `response_format=pcm` + `stream_format=audio` the service returns raw
//! little-endian **16-bit signed mono** samples (24 kHz for Qwen3-TTS), not a
//! WAV container. The TCP chunk boundaries do not respect sample boundaries, so
//! a converter has to carry an odd trailing byte to the next chunk.

/// Number of leading samples whose absolute value must stay below
/// [`SILENCE_THRESHOLD`] before real speech is considered to have started.
const SILENCE_PROBE_SAMPLES: usize = 256;

/// i16 amplitude below which a leading sample counts as silence (~-72 dBFS).
const SILENCE_THRESHOLD: i16 = 64;

/// Incremental i16-LE → f32 converter.
///
/// Feed each HTTP chunk to [`Pcm16ToF32::push`] and append the returned samples
/// to the playback queue. Call [`Pcm16ToF32::finish`] once the stream ends so a
/// dangling odd byte is reported instead of silently dropped.
#[derive(Debug, Default)]
pub struct Pcm16ToF32 {
    /// Single byte held back when a chunk ended mid-sample.
    pending: Option<u8>,
    /// Whether the leading silence of the utterance has been skipped already.
    started: bool,
}

impl Pcm16ToF32 {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode a chunk of i16-LE bytes into normalised f32 samples.
    ///
    /// Any leading run of near-zero samples is dropped for the *first* chunk
    /// only (the service emits a short silence preroll). Samples are scaled by
    /// `1/32768` so the full i16 range maps into `[-1.0, 0.99997]`.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<f32> {
        let (mut buf, all) = match self.pending.take() {
            Some(hi) => {
                let mut v = Vec::with_capacity(1 + bytes.len());
                v.push(hi);
                v.extend_from_slice(bytes);
                (v, true)
            }
            None => {
                let mut v = Vec::with_capacity(bytes.len());
                v.extend_from_slice(bytes);
                (v, false)
            }
        };

        if buf.len() % 2 != 0 {
            self.pending = buf.pop();
        }

        let mut out: Vec<f32> = buf
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect();

        // Trim the service's leading silence once, from the very first chunk.
        if !self.started && (all || !out.is_empty()) && out.len() >= SILENCE_PROBE_SAMPLES {
            let offset = out
                .iter()
                .position(|&s| (s * 32768.0).abs() as i16 > SILENCE_THRESHOLD)
                .unwrap_or(0);
            if offset > 0 {
                out.drain(..offset);
            }
            self.started = true;
        }

        out
    }

    /// Flush a dangling odd byte at end of stream. Returns the number of
    /// dropped bytes (0 or 1).
    pub fn finish(&mut self) -> usize {
        self.pending.take().map(|_| 1).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn i16le(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn decodes_even_chunk() {
        let mut c = Pcm16ToF32::new();
        let out = c.push(&i16le(&[0, 16384, -16384, 32767, -32768]));
        assert_eq!(out.len(), 5);
        assert!((out[1] - 0.5).abs() < 1e-4);
        assert!((out[2] - (-0.5)).abs() < 1e-4);
        assert!(out[3] > 0.999);
        assert!(out[4] <= -0.999);
        assert_eq!(c.finish(), 0);
    }

    #[test]
    fn carries_partial_sample_across_chunks() {
        let mut c = Pcm16ToF32::new();
        // 3 bytes: one complete sample (0x4000 = 16384) + the low byte of the next.
        let out1 = c.push(&[0x00, 0x40, 0x00]);
        assert_eq!(out1, vec![0.5]);
        // The held low byte + the high byte complete a sample (-16384 = 0xC000),
        // followed by a fresh full sample (32767).
        let out2 = c.push(&[0xC0, 0xFF, 0x7F]);
        assert_eq!(out2.len(), 2);
        assert!((out2[0] - (-0.5)).abs() < 1e-4);
        assert!(out2[1] > 0.999);
    }

    #[test]
    fn reports_dangling_byte_at_eof() {
        let mut c = Pcm16ToF32::new();
        let _ = c.push(&[0x01]);
        assert_eq!(c.finish(), 1);
    }

    #[test]
    fn empty_chunk_is_noop() {
        let mut c = Pcm16ToF32::new();
        assert!(c.push(&[]).is_empty());
        assert_eq!(c.finish(), 0);
    }

    #[test]
    fn trims_leading_silence_on_first_chunk() {
        let mut c = Pcm16ToF32::new();
        let mut samples = vec![0i16; SILENCE_PROBE_SAMPLES + 10];
        samples.push(2000);
        samples.push(0);
        let out = c.push(&i16le(&samples));
        // Silence trimmed; the 2000 sample is now first.
        assert!(out[0] > 0.05);
    }
}
