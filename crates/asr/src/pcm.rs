//! f32 -> PCM16 little-endian conversion for the realtime WebSocket.
//!
//! The server accepts little-endian PCM16 binary frames. The pipeline is
//! already 16 kHz mono, so no framing or resampling is added here.

/// Convert mono f32 samples (`[-1.0, 1.0]`) to little-endian PCM16 bytes.
pub fn encode_pcm16_le(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for &sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_little_endian_pcm16() {
        let pcm = encode_pcm16_le(&[0.0, 0.5, -0.5]);
        assert_eq!(pcm.len(), 6);
        assert_eq!(i16::from_le_bytes(pcm[0..2].try_into().unwrap()), 0);
        assert_eq!(
            i16::from_le_bytes(pcm[2..4].try_into().unwrap()),
            (0.5 * i16::MAX as f32).round() as i16
        );
        assert_eq!(
            i16::from_le_bytes(pcm[4..6].try_into().unwrap()),
            -(0.5 * i16::MAX as f32).round() as i16
        );
    }

    #[test]
    fn clamps_out_of_range_samples() {
        let pcm = encode_pcm16_le(&[2.0, -2.0]);
        assert_eq!(i16::from_le_bytes(pcm[0..2].try_into().unwrap()), i16::MAX);
        assert_eq!(i16::from_le_bytes(pcm[2..4].try_into().unwrap()), -i16::MAX);
    }

    #[test]
    fn empty_audio_is_empty_bytes() {
        assert!(encode_pcm16_le(&[]).is_empty());
    }
}
