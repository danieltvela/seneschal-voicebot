pub mod ambient_buffer;
pub mod audio_capture;
pub mod audio_transform;
pub mod buffer;
pub mod filler;
pub mod output;
pub mod speaker;

use tracing::{error, trace};

/// Log a CPAL stream error. The ALSA backend reports `POLLERR` / spurious poll
/// wakeups whenever a stream is torn down (per-sentence output streams, app
/// shutdown, monitor reconnects); those are benign and logged at trace instead
/// of error to avoid flooding the log.
pub(crate) fn log_stream_error(err: cpal::StreamError) {
    let msg = err.to_string();
    if msg.contains("POLLERR") || msg.contains("spuriously returned") {
        trace!(target: "audio", "Audio stream notice: {msg}");
    } else {
        error!(target: "audio", "Audio stream error: {msg}");
    }
}

/// Requested ALSA period (callback) size for the microphone capture stream.
pub(crate) const CAPTURE_PERIOD_MS: u32 = 50;
/// Requested ALSA period (callback) size for the TTS playback stream.
pub(crate) const PLAYBACK_PERIOD_MS: u32 = 50;

/// Choose a robust buffer size for a CPAL stream.
///
/// With `BufferSize::Default`, snd-usb-audio negotiates the SMALLEST period
/// the device allows — observed on the Poly Sync 20 at 16 kHz: period 81 /
/// buffer 162 frames (5 ms / 10 ms). One scheduling stall from the onnx /
/// whisper inference threads overruns that ring and the capture stream dies
/// (POLLERR → stream error). `BufferSize::Fixed` negotiates a real cushion
/// AND is the condition under which cpal boosts the ALSA callback thread to
/// RT priority (alsa backend `boost_current_thread_priority`).
///
/// cpal validates `Fixed` against the device's supported range and REFUSES
/// to build the stream if the request is out of range — so fall back to
/// `Default` (current behaviour) whenever the requested period does not fit.
pub(crate) fn robust_buffer_size(
    supported: &cpal::SupportedStreamConfig,
    period_ms: u32,
) -> cpal::BufferSize {
    let desired = supported.sample_rate() as u64 * period_ms as u64 / 1000;
    match supported.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => {
            pick_period(desired, u64::from(*min), u64::from(*max))
        }
        cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Default,
    }
}

/// Fixed(period) when the range accommodates it, Default otherwise.
fn pick_period(desired: u64, min: u64, max: u64) -> cpal::BufferSize {
    if (min..=max).contains(&desired) {
        cpal::BufferSize::Fixed(desired as cpal::FrameCount)
    } else {
        cpal::BufferSize::Default
    }
}

#[cfg(test)]
mod tests {
    use super::pick_period;
    use cpal::BufferSize;

    #[test]
    fn picks_fixed_when_in_range() {
        // 16 kHz @ 50 ms = 800 frames; Poly range comfortably contains it.
        assert_eq!(pick_period(800, 32, 1_048_576), BufferSize::Fixed(800));
    }

    #[test]
    fn falls_back_to_default_when_out_of_range() {
        // Tiny BT headset whose max cannot hold the request.
        assert_eq!(pick_period(800, 32, 256), BufferSize::Default);
        // Request below the device minimum.
        assert_eq!(pick_period(800, 4096, 1_048_576), BufferSize::Default);
    }
}

#[cfg(target_os = "linux")]
pub mod alsa_setup;

#[cfg(target_os = "linux")]
pub mod alsa_silencer;
