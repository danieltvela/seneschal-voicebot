//! Minimal in-memory WAV (RIFF/PCM16) encoder for the remote STT provider.
//!
//! The OpenAI transcription API takes a file upload; the local segmenter
//! produces 16 kHz mono f32 samples, so we wrap them in a canonical
//! 16-bit PCM WAV container before POSTing. No external crates needed.

use super::segmenter::SAMPLE_RATE;

/// Sample rate of the encoded WAV (matches the pipeline).
pub const WAV_SAMPLE_RATE: u32 = SAMPLE_RATE as u32;

/// Encode mono f32 samples (`[-1.0, 1.0]`) as a 16-bit PCM WAV (16 kHz mono).
pub fn encode_pcm16_mono_16k(samples: &[f32]) -> Vec<u8> {
    let bits_per_sample: u16 = 16;
    let channels: u16 = 1;
    let byte_rate = WAV_SAMPLE_RATE * u32::from(channels) * u32::from(bits_per_sample) / 8;
    let block_align = channels * bits_per_sample / 8;
    let data_len = (samples.len() * 2) as u32;

    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&WAV_SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u32_at(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }

    fn u16_at(b: &[u8], at: usize) -> u16 {
        u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
    }

    #[test]
    fn writes_canonical_header() {
        let wav = encode_pcm16_mono_16k(&[0.0, 0.5, -0.5, 1.0]);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(u32_at(&wav, 16), 16); // fmt size
        assert_eq!(u16_at(&wav, 20), 1); // PCM
        assert_eq!(u16_at(&wav, 22), 1); // mono
        assert_eq!(u32_at(&wav, 24), 16_000);
        assert_eq!(u16_at(&wav, 34), 16); // bits
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32_at(&wav, 40), 8);
        assert_eq!(wav.len(), 44 + 8);
        assert_eq!(u32_at(&wav, 4), 36 + 8);
    }

    #[test]
    fn clamps_and_round_trips() {
        let wav = encode_pcm16_mono_16k(&[2.0, -2.0, 1.0]);
        let pcm = &wav[44..];
        assert_eq!(i16::from_le_bytes(pcm[0..2].try_into().unwrap()), i16::MAX);
        assert_eq!(i16::from_le_bytes(pcm[2..4].try_into().unwrap()), -i16::MAX);
        assert_eq!(i16::from_le_bytes(pcm[4..6].try_into().unwrap()), i16::MAX);
    }

    #[test]
    fn empty_input_is_a_valid_empty_wav() {
        let wav = encode_pcm16_mono_16k(&[]);
        assert_eq!(wav.len(), 44);
        assert_eq!(u32_at(&wav, 40), 0);
    }
}
