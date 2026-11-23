use anyhow::{Context, Result, anyhow};
use async_channel::Sender;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, FromSample, Sample, SampleFormat, SizedSample, StreamConfig};
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{info, trace, warn};

/// Raw audio data from the microphone
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AudioChunk {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// One CPAL device as exposed to the Settings/Audio panel (issue #34).
///
/// `selector` is the exact value accepted by `AudioCapture::new` /
/// `AudioOutput::new`: the device name, with `#N` appended when several
/// devices share the same name.
#[derive(Debug, Clone, Serialize)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub selector: String,
    pub is_default: bool,
    /// `None` means CPAL enumerated the device but its default config failed.
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub sample_format: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct AudioDeviceInventory {
    pub inputs: Vec<AudioDeviceInfo>,
    pub outputs: Vec<AudioDeviceInfo>,
}

/// RMS and peak level for a mono chunk. Values are linear `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioLevels {
    pub rms: f32,
    pub peak: f32,
}

impl AudioLevels {
    pub fn compute(samples: &[f32]) -> Self {
        if samples.is_empty() {
            return Self {
                rms: 0.0,
                peak: 0.0,
            };
        }
        let sum_squares: f32 = samples.iter().map(|s| s * s).sum();
        let peak = samples
            .iter()
            .fold(0.0_f32, |max, sample| max.max(sample.abs()));
        Self {
            rms: (sum_squares / samples.len() as f32).sqrt(),
            peak,
        }
    }

    pub fn is_silent(&self, threshold: f32) -> bool {
        self.peak <= threshold
    }
}

pub struct AudioCapture {
    device: Device,
    config: StreamConfig,
    sample_format: SampleFormat,
}

impl AudioCapture {
    /// Create a new AudioCapture instance.
    ///
    /// # Arguments
    /// * `device_name` - Optional device name to use. If None, uses the default input device.
    ///   The name can be a partial match (case-insensitive).
    pub fn new(device_name: Option<&str>) -> Result<Self> {
        let host = cpal::default_host();

        let device = match device_name {
            Some(name) => Self::find_device_by_name(&host, name)?,
            None => host
                .default_input_device()
                .ok_or_else(|| anyhow!("No input device available"))?,
        };

        let actual_device_name = device.description()?;
        info!(target: "audio", "Using input device: {}", actual_device_name);

        let supported_config = device
            .default_input_config()
            .context("Failed to get default input config")?;

        info!(
            target: "audio",
            "Default input config: {:?} Hz, {:?} channels, format: {:?}",
            supported_config.sample_rate(),
            supported_config.channels(),
            supported_config.sample_format()
        );

        let sample_format = supported_config.sample_format();
        // ALSA's default period here is 5 ms — see robust_buffer_size().
        let buffer_size =
            crate::audio::robust_buffer_size(&supported_config, crate::audio::CAPTURE_PERIOD_MS);
        let config: StreamConfig = supported_config.into();
        let config = StreamConfig {
            buffer_size,
            ..config
        };

        Ok(Self {
            device,
            config,
            sample_format,
        })
    }

    /// Like `new()` but returns `None` instead of an error when no input device
    /// is available. Used at startup to defer capture until a device connects.
    pub fn try_new(device_name: Option<&str>) -> Option<Self> {
        Self::new(device_name).ok()
    }

    /// Parse a device name that may include an index suffix: "Name#N" → ("name", Some(N))
    fn parse_device_name(name: &str) -> (&str, Option<usize>) {
        if let Some(pos) = name.rfind('#')
            && let Ok(idx) = name[pos + 1..].parse::<usize>()
        {
            return (&name[..pos], Some(idx));
        }
        (name, None)
    }

    /// Find an input device by name (case-insensitive partial match).
    ///
    /// When multiple devices share the same name (e.g. USB and Bluetooth variants of the
    /// same headset), appending `#N` selects the Nth match (0-based). Without an index
    /// suffix, the first candidate whose `default_input_config()` succeeds is used, which
    /// skips Bluetooth HFP entries that advertise a config but fail on stream open.
    fn find_device_by_name(host: &cpal::Host, name: &str) -> Result<Device> {
        let (name_filter, index) = Self::parse_device_name(name);
        let name_lower = name_filter.to_lowercase();

        let candidates: Vec<Device> = host
            .input_devices()
            .context("Failed to enumerate input devices")?
            .filter(|d| {
                d.description()
                    .map(|desc| desc.name().to_lowercase().contains(&name_lower))
                    .unwrap_or(false)
            })
            .collect();

        if candidates.is_empty() {
            return Err(anyhow!(
                "No input device found matching '{}'. Use LIST_AUDIO_DEVICES=1 to see available devices.",
                name_filter
            ));
        }

        info!(
            target: "audio",
            "Found {} input device(s) matching '{}'",
            candidates.len(),
            name_filter
        );

        if let Some(idx) = index {
            // Explicit index — honour it directly
            return candidates.into_iter().nth(idx).ok_or_else(|| {
                anyhow!("Device index #{} out of range for '{}'", idx, name_filter)
            });
        }

        // No index — pick the first candidate whose default config succeeds
        for device in candidates {
            let desc = device
                .description()
                .map(|d| d.name().to_string())
                .unwrap_or_default();
            match device.default_input_config() {
                Ok(cfg) => {
                    info!(
                        target: "audio",
                        "Selected input device: {} ({} Hz, {} ch)",
                        desc,
                        cfg.sample_rate(),
                        cfg.channels()
                    );
                    return Ok(device);
                }
                Err(e) => {
                    info!(target: "audio", "Skipping input device '{}': {}", desc, e);
                }
            }
        }

        Err(anyhow!(
            "No working input device found matching '{}'. Use LIST_AUDIO_DEVICES=1 to see available devices.",
            name_filter
        ))
    }

    /// Enumerate input and output devices without opening a stream.
    ///
    /// A failing default config is still useful in the UI (the device exists but
    /// cannot currently provide its default format), so it is reported instead
    /// of dropping the entry.
    pub fn enumerate_devices() -> Result<AudioDeviceInventory> {
        let host = cpal::default_host();

        let inputs = Self::device_infos(
            host.input_devices()
                .context("Failed to enumerate input devices")?,
            host.default_input_device(),
            "input",
        )?;
        let outputs = Self::device_infos(
            host.output_devices()
                .context("Failed to enumerate output devices")?,
            host.default_output_device(),
            "output",
        )?;

        Ok(AudioDeviceInventory { inputs, outputs })
    }

    fn device_infos<I>(
        devices: I,
        default_device: Option<Device>,
        direction: &str,
    ) -> Result<Vec<AudioDeviceInfo>>
    where
        I: IntoIterator<Item = Device>,
    {
        let default_name = default_device
            .and_then(|device| device.description().ok().map(|d| d.name().to_string()));
        let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut infos = Vec::new();

        for device in devices {
            let Ok(description) = device.description() else {
                continue;
            };
            let name = description.name().to_string();
            let count = counts.entry(name.clone()).or_default();
            let selector = if *count == 0 {
                name.clone()
            } else {
                format!("{name}#{count}")
            };
            *count += 1;

            let config = if direction == "input" {
                device.default_input_config().ok()
            } else {
                device.default_output_config().ok()
            };
            let (sample_rate, channels, sample_format) = match config {
                Some(config) => (
                    Some(config.sample_rate()),
                    Some(config.channels()),
                    Some(format!("{:?}", config.sample_format())),
                ),
                None => (None, None, None),
            };

            infos.push(AudioDeviceInfo {
                name: name.clone(),
                selector,
                is_default: default_name.as_deref() == Some(name.as_str()),
                sample_rate,
                channels,
                sample_format,
            });
        }

        Ok(infos)
    }

    /// Print all available input devices to stdout
    pub fn print_devices() -> Result<()> {
        let inventory = Self::enumerate_devices()?;

        if let Some(device) = inventory.inputs.iter().find(|device| device.is_default) {
            println!("Default input device: {}", device.name);
        }
        if let Some(device) = inventory.outputs.iter().find(|device| device.is_default) {
            println!("Default output device: {}", device.name);
        }

        println!("\nAvailable input devices:");
        for (idx, device) in inventory.inputs.iter().enumerate() {
            println!("  [{}] {} ({})", idx, device.name, describe_device(device));
        }

        println!("\nAvailable output devices:");
        for (idx, device) in inventory.outputs.iter().enumerate() {
            println!("  [{}] {} ({})", idx, device.name, describe_device(device));
        }

        println!(
            "\nSet AUDIO_INPUT_DEVICE / AUDIO_OUTPUT_DEVICE to a device name above \
             (partial, case-insensitive match; add #N to pick the Nth match)."
        );

        Ok(())
    }

    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    #[allow(dead_code)]
    pub fn channels(&self) -> u16 {
        self.config.channels
    }

    /// Start capturing audio and send chunks through the provided channel.
    ///
    /// If `capture_active` is provided, it is set to `true` on success and
    /// cleared on any stream error, so the device monitor knows when capture is
    /// actually running and can retry/relaunch it without flapping.
    pub fn start_capture(
        &self,
        tx: Sender<AudioChunk>,
        samples_per_chunk: usize,
        capture_active: Option<Arc<AtomicBool>>,
    ) -> Result<cpal::Stream> {
        let channels = self.config.channels;
        let sample_rate = self.config.sample_rate;

        // Buffer to accumulate samples
        let buffer = Arc::new(std::sync::Mutex::new(Vec::with_capacity(
            samples_per_chunk * channels as usize,
        )));

        // Build the input stream generically so any device sample format works
        // (converted to f32 via cpal/dasp's FromSample).
        macro_rules! build {
            ($t:ty) => {
                Self::build_input_stream::<$t>(
                    &self.device,
                    &self.config,
                    Arc::clone(&buffer),
                    tx.clone(),
                    samples_per_chunk,
                    sample_rate,
                    channels,
                    capture_active.clone(),
                )?
            };
        }

        let stream = match self.sample_format {
            SampleFormat::I8 => build!(i8),
            SampleFormat::I16 => build!(i16),
            SampleFormat::I24 => build!(cpal::I24),
            SampleFormat::I32 => build!(i32),
            SampleFormat::I64 => build!(i64),
            SampleFormat::U8 => build!(u8),
            SampleFormat::U16 => build!(u16),
            SampleFormat::U24 => build!(cpal::U24),
            SampleFormat::U32 => build!(u32),
            SampleFormat::U64 => build!(u64),
            SampleFormat::F32 => build!(f32),
            SampleFormat::F64 => build!(f64),
            other => {
                return Err(anyhow!("Unsupported input sample format: {other:?}"));
            }
        };

        stream.play().context("Failed to start audio stream")?;
        info!(target: "audio", "Audio capture started");

        Ok(stream)
    }

    /// Build an input stream for any device sample format, converting to f32.
    #[allow(clippy::too_many_arguments)]
    fn build_input_stream<T>(
        device: &Device,
        config: &StreamConfig,
        buffer: Arc<std::sync::Mutex<Vec<f32>>>,
        tx: Sender<AudioChunk>,
        samples_per_chunk: usize,
        sample_rate: u32,
        channels: u16,
        capture_active: Option<Arc<AtomicBool>>,
    ) -> Result<cpal::Stream>
    where
        T: SizedSample + Sample,
        f32: FromSample<T>,
    {
        let err_fn = move |err: cpal::StreamError| {
            let msg = err.to_string();
            crate::audio::log_stream_error(err);
            if let Some(active) = &capture_active {
                // swap(false) returns the previous value: warn only on the
                // active→idle transition (this is what arms the device
                // monitor's rebuild path — previously invisible at TRACE).
                if active.swap(false, Ordering::SeqCst) {
                    tracing::warn!(
                        target: "audio",
                        "Capture stream degraded ({msg}) — flagged idle, awaiting device-monitor rebuild"
                    );
                }
            }
        };

        device
            .build_input_stream(
                config,
                move |data: &[T], _: &cpal::InputCallbackInfo| {
                    let samples: Vec<f32> = data.iter().map(|&s| f32::from_sample(s)).collect();
                    Self::process_samples(
                        &buffer,
                        samples,
                        &tx,
                        samples_per_chunk,
                        sample_rate,
                        channels,
                    );
                },
                err_fn,
                None,
            )
            .context("Failed to build input stream")
    }

    fn process_samples(
        buffer: &Arc<std::sync::Mutex<Vec<f32>>>,
        samples: Vec<f32>,
        tx: &Sender<AudioChunk>,
        samples_per_chunk: usize,
        sample_rate: u32,
        channels: u16,
    ) {
        let mut buf = buffer.lock().unwrap();
        buf.extend(samples);

        // Total samples needed including all channels
        let total_samples_needed = samples_per_chunk * channels as usize;

        while buf.len() >= total_samples_needed {
            let chunk_samples: Vec<f32> = buf.drain(..total_samples_needed).collect();

            let chunk = AudioChunk {
                samples: chunk_samples,
                sample_rate,
                channels,
            };

            // Non-blocking send - drop chunk if channel is full
            if let Err(e) = tx.try_send(chunk) {
                warn!(target: "audio", "Failed to send audio chunk: {}", e);
            } else {
                trace!(target: "audio", "Sent audio chunk with {} samples", total_samples_needed);
            }
        }
    }
}

fn describe_device(device: &AudioDeviceInfo) -> String {
    match (device.sample_rate, device.channels, &device.sample_format) {
        (Some(rate), Some(channels), Some(format)) => format!("{rate} Hz, {channels} ch, {format}"),
        _ => "config unavailable".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_sine_matches_expected_rms_and_peak() {
        let samples: Vec<f32> = (0..16_000)
            .map(|i| (i as f32 * 2.0 * std::f32::consts::PI / 100.0).sin() * 0.5)
            .collect();
        let levels = AudioLevels::compute(&samples);
        assert!((levels.rms - 0.5 / 2f32.sqrt()).abs() < 0.01);
        assert!((levels.peak - 0.5).abs() < 0.01);
    }

    #[test]
    fn levels_silence_and_empty_are_zero() {
        assert_eq!(
            AudioLevels::compute(&[0.0; 160]),
            AudioLevels {
                rms: 0.0,
                peak: 0.0
            }
        );
        assert_eq!(
            AudioLevels::compute(&[]),
            AudioLevels {
                rms: 0.0,
                peak: 0.0
            }
        );
        assert!(AudioLevels::compute(&[0.0; 160]).is_silent(0.01));
    }
}
