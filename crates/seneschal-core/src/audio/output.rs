use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, StreamConfig};
use rubato::{FftFixedIn, Resampler};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use tracing::{debug, info, warn};

struct AudioOutputInner {
    /// `None` in the null/headless variant — `play_blocking` returns immediately.
    device: Option<Device>,
    config: StreamConfig,
    /// Native sample format of the device. The stream is built with a matching
    /// callback type; ALSA devices that only expose S16 (e.g. the Poly Sync)
    /// reject an f32 stream with "unable to open".
    sample_format: SampleFormat,
}

pub struct AudioOutput {
    inner: Mutex<AudioOutputInner>,
}

impl AudioOutput {
    pub fn new(device_name: Option<&str>) -> Result<Self> {
        let host = cpal::default_host();

        let device = if let Some(name) = device_name {
            Self::find_output_device(&host, name)?
        } else {
            host.default_output_device()
                .context("No output device available")?
        };

        info!(target: "audio", "Output device: {}", device.description().map(|d| d.name().to_string()).unwrap_or_default());

        let supported = device
            .default_output_config()
            .context("Failed to get default output config")?;

        // Cap to stereo — multi-channel virtual devices (BlackHole 8ch, etc.) cause issues
        let channels = supported.channels().min(2);
        let sample_format = supported.sample_format();

        info!(
            target: "audio",
            "Output config: {}Hz, {}ch, {:?}",
            supported.sample_rate(),
            channels,
            sample_format
        );

        let config = StreamConfig {
            channels,
            sample_rate: supported.sample_rate(),
            // 50 ms period instead of ALSA's minimal default — prevents
            // TTS underruns (audible crackle) under inference load and
            // enables cpal's RT-priority boost on the ALSA thread.
            buffer_size: crate::audio::robust_buffer_size(
                &supported,
                crate::audio::PLAYBACK_PERIOD_MS,
            ),
        };

        Ok(Self {
            inner: Mutex::new(AudioOutputInner {
                device: Some(device),
                config,
                sample_format,
            }),
        })
    }

    /// Returns `true` when a real audio output device is available,
    /// `false` for the null/headless variant.
    pub fn has_device(&self) -> bool {
        self.inner.lock().unwrap().device.is_some()
    }

    /// Like `new()`, but falls back to a null/headless output when no device is
    /// available. Used at startup so the app can launch without an audio device
    /// connected; `swap_device` upgrades to a real device when one appears.
    pub fn new_or_null(device_name: Option<&str>) -> Self {
        match Self::new(device_name) {
            Ok(output) => output,
            Err(e) => {
                warn!(
                    target: "audio",
                    "No output device available at startup ({}), using null output",
                    e
                );
                Self::null()
            }
        }
    }

    /// Hot-swap the output device at runtime. Called when the device monitor
    /// detects that the configured audio device has become available.
    ///
    /// Blocks until any in-flight `play_blocking` call releases the lock
    /// (i.e. between sentences). Safe to call from `spawn_blocking`.
    pub fn swap_device(&self, device_name: Option<&str>) -> Result<()> {
        let host = cpal::default_host();

        let device = if let Some(name) = device_name {
            Self::find_output_device(&host, name)?
        } else {
            host.default_output_device()
                .context("No output device available")?
        };

        let desc = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_default();

        let supported = device
            .default_output_config()
            .context("Failed to get default output config")?;

        let channels = supported.channels().min(2);
        let sample_format = supported.sample_format();
        let config = StreamConfig {
            channels,
            sample_rate: supported.sample_rate(),
            buffer_size: crate::audio::robust_buffer_size(
                &supported,
                crate::audio::PLAYBACK_PERIOD_MS,
            ),
        };

        {
            let mut inner = self.inner.lock().unwrap();
            inner.device = Some(device);
            inner.config = config;
            inner.sample_format = sample_format;
        }

        info!(
            target: "audio",
            "Output device swapped: {} ({}Hz, {}ch, {:?})",
            desc,
            supported.sample_rate(),
            channels,
            sample_format
        );

        Ok(())
    }

    /// Find an output device by name, handling duplicates (e.g. USB vs Bluetooth).
    ///
    /// Supports `"Name#N"` suffix to select the Nth match (0-based). Without an index,
    /// picks the first candidate whose `default_output_config()` succeeds.
    fn find_output_device(host: &cpal::Host, name: &str) -> Result<Device> {
        let (name_filter, index) = if let Some(pos) = name.rfind('#') {
            if let Ok(idx) = name[pos + 1..].parse::<usize>() {
                (&name[..pos], Some(idx))
            } else {
                (name, None)
            }
        } else {
            (name, None)
        };

        let candidates: Vec<Device> = host
            .output_devices()
            .context("Failed to enumerate output devices")?
            .filter(|d| {
                d.description()
                    .map(|desc| desc.name().contains(name_filter))
                    .unwrap_or(false)
            })
            .collect();

        if candidates.is_empty() {
            return Err(anyhow!(
                "Output device '{}' not found. Use LIST_AUDIO_DEVICES=1 to see available devices.",
                name_filter
            ));
        }

        info!(
            target: "audio",
            "Found {} output device(s) matching '{}'",
            candidates.len(),
            name_filter
        );

        if let Some(idx) = index {
            return candidates.into_iter().nth(idx).ok_or_else(|| {
                anyhow!(
                    "Output device index #{} out of range for '{}'",
                    idx,
                    name_filter
                )
            });
        }

        for device in candidates {
            let desc = device
                .description()
                .map(|d| d.name().to_string())
                .unwrap_or_default();
            match device.default_output_config() {
                Ok(cfg) => {
                    info!(
                        target: "audio",
                        "Selected output device: {} ({} Hz, {} ch)",
                        desc,
                        cfg.sample_rate(),
                        cfg.channels()
                    );
                    return Ok(device);
                }
                Err(e) => {
                    info!(target: "audio", "Skipping output device '{}': {}", desc, e);
                }
            }
        }

        Err(anyhow!(
            "No working output device found matching '{}'. Use LIST_AUDIO_DEVICES=1 to see available devices.",
            name_filter
        ))
    }

    /// Null/headless audio output — `play_blocking` discards all audio immediately.
    ///
    /// Used in tests and CI environments where no sound device is available,
    /// and as the startup fallback when no output device is connected.
    #[allow(dead_code)]
    pub fn null() -> Self {
        Self {
            inner: Mutex::new(AudioOutputInner {
                device: None,
                config: StreamConfig {
                    channels: 1,
                    sample_rate: 22050,
                    buffer_size: cpal::BufferSize::Default,
                },
                sample_format: SampleFormat::F32,
            }),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.inner.lock().unwrap().config.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.inner.lock().unwrap().config.channels
    }

    /// Resample mono `samples` from `source_rate` to `target_rate`, then
    /// duplicate to `channels`. Returns interleaved f32 PCM ready for CPAL.
    ///
    /// Exposed as `pub` so tests can exercise it without audio hardware.
    pub fn prepare(
        samples: &[f32],
        source_rate: u32,
        target_rate: u32,
        channels: u16,
    ) -> Result<Vec<f32>> {
        // Step 1 — resample
        let resampled = if source_rate != target_rate {
            resample(samples, source_rate, target_rate)
                .context("Failed to resample audio for playback")?
        } else {
            samples.to_vec()
        };

        // Step 2 — expand mono to device channel count
        if channels <= 1 {
            return Ok(resampled);
        }
        let ch = channels as usize;
        let mut interleaved = Vec::with_capacity(resampled.len() * ch);
        for s in resampled {
            for _ in 0..ch {
                interleaved.push(s);
            }
        }
        Ok(interleaved)
    }

    /// Play mono f32 samples (at `source_rate`) through the default output
    /// device. Resamples and expands channels as needed. Blocks the calling
    /// thread until the speaker has finished playing every sample, or until
    /// `cancel` is set to `true` (barge-in / interruption).
    ///
    /// If this is a null/headless output (no device), returns immediately
    /// without producing any audio.
    ///
    /// Holds the inner lock for the duration of playback. This is safe because
    /// `play_blocking` runs inside `spawn_blocking` (tts_task.rs), and
    /// `swap_device` also uses `spawn_blocking` — no tokio worker thread is
    /// blocked. The lock ensures device swaps happen between sentences, never
    /// mid-playback.
    pub fn play_blocking(
        &self,
        samples: &[f32],
        source_rate: u32,
        cancel: &Arc<AtomicBool>,
    ) -> Result<()> {
        let inner = self.inner.lock().unwrap();

        let Some(device) = &inner.device else {
            return Ok(());
        };

        let sample_rate = inner.config.sample_rate;
        let channels = inner.config.channels;
        let sample_format = inner.sample_format;

        let prepared = Self::prepare(samples, source_rate, sample_rate, channels)?;

        if prepared.is_empty() {
            return Ok(());
        }

        debug!(
            target: "audio",
            "play_blocking: {} samples in, source={}Hz → device={}Hz {}ch, prepared={}",
            samples.len(), source_rate, sample_rate, channels, prepared.len()
        );

        let total = prepared.len();

        // Drain tail: silence frames served after all audio content has been
        // written into CPAL's buffer. This keeps the stream alive long enough
        // for CoreAudio/ALSA to flush its internal buffers to the DAC.
        // 150ms is sufficient for CoreAudio; reduces inter-sentence gap vs 400ms.
        let drain_ms: usize = std::env::var("AUDIO_DRAIN_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(150);
        let drain_samples = (sample_rate as usize * channels as usize) * drain_ms / 1000;
        let stop_pos = total + drain_samples;

        let buf = Arc::new(prepared);
        let pos = Arc::new(AtomicUsize::new(0));
        let done = Arc::new((Mutex::new(false), Condvar::new()));

        let buf_cb = Arc::clone(&buf);
        let pos_cb = Arc::clone(&pos);
        let done_cb = Arc::clone(&done);
        let cancel_cb = Arc::clone(cancel);

        let stream = match sample_format {
            SampleFormat::F32 => device
                .build_output_stream(
                    &inner.config,
                    move |data: &mut [f32], _| {
                        // Barge-in: stop immediately when cancelled
                        if cancel_cb.load(Ordering::Relaxed) {
                            data.fill(0.0);
                            let (lock, cvar) = &*done_cb;
                            *lock.lock().unwrap() = true;
                            cvar.notify_one();
                            return;
                        }

                        let p = pos_cb.load(Ordering::Relaxed);

                        // Write audio samples up to `total`, then silence up to `stop_pos`.
                        let audio_n = data.len().min(total.saturating_sub(p));
                        if audio_n > 0 {
                            data[..audio_n].copy_from_slice(&buf_cb[p..p + audio_n]);
                        }
                        data[audio_n..].fill(0.0);

                        let new_pos = (p + data.len()).min(stop_pos);
                        pos_cb.store(new_pos, Ordering::Relaxed);

                        if new_pos >= stop_pos {
                            let (lock, cvar) = &*done_cb;
                            *lock.lock().unwrap() = true;
                            cvar.notify_one();
                        }
                    },
                    crate::audio::log_stream_error,
                    None,
                )
                .context("Failed to build output stream")?,
            SampleFormat::I16 => device
                .build_output_stream(
                    &inner.config,
                    move |data: &mut [i16], _| {
                        // Barge-in: stop immediately when cancelled
                        if cancel_cb.load(Ordering::Relaxed) {
                            data.fill(0);
                            let (lock, cvar) = &*done_cb;
                            *lock.lock().unwrap() = true;
                            cvar.notify_one();
                            return;
                        }

                        let p = pos_cb.load(Ordering::Relaxed);

                        // Write audio samples up to `total`, then silence up to `stop_pos`.
                        let audio_n = data.len().min(total.saturating_sub(p));
                        if audio_n > 0 {
                            for (dst, src) in
                                data[..audio_n].iter_mut().zip(&buf_cb[p..p + audio_n])
                            {
                                *dst = f32_to_i16(*src);
                            }
                        }
                        data[audio_n..].fill(0);

                        let new_pos = (p + data.len()).min(stop_pos);
                        pos_cb.store(new_pos, Ordering::Relaxed);

                        if new_pos >= stop_pos {
                            let (lock, cvar) = &*done_cb;
                            *lock.lock().unwrap() = true;
                            cvar.notify_one();
                        }
                    },
                    crate::audio::log_stream_error,
                    None,
                )
                .context("Failed to build output stream")?,
            other => {
                return Err(anyhow!(
                    "Unsupported output sample format: {other:?} (supported: F32, I16)"
                ));
            }
        };

        stream.play().context("Failed to start output stream")?;

        // Log actual playback start for first-speech timing
        if std::env::var("RUST_LOG")
            .unwrap_or_default()
            .contains("performance")
        {
            info!(target: "performance", "  └─ Playback started on device");
        }

        let (lock, cvar) = &*done;
        let mut finished = lock.lock().unwrap();
        while !*finished {
            finished = cvar.wait(finished).unwrap();
        }

        Ok(())
    }
}

// ── Resampling ─────────────────────────────────────────────────────────────────

const RESAMPLE_CHUNK: usize = 1024;

/// Convert an f32 sample in [-1.0, 1.0] to its i16 representation (clamped).
#[inline]
fn f32_to_i16(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>> {
    let expected_out = (samples.len() as f64 * to_rate as f64 / from_rate as f64).ceil() as usize;

    let mut resampler =
        FftFixedIn::<f32>::new(from_rate as usize, to_rate as usize, RESAMPLE_CHUNK, 2, 1)
            .context("Failed to create resampler")?;

    // Pad to a multiple of RESAMPLE_CHUNK so every chunk is full.
    let padded_len = samples.len().div_ceil(RESAMPLE_CHUNK) * RESAMPLE_CHUNK;
    let mut padded = samples.to_vec();
    padded.resize(padded_len, 0.0);

    let mut output = Vec::with_capacity(expected_out + RESAMPLE_CHUNK);
    for chunk in padded.chunks(RESAMPLE_CHUNK) {
        let out = resampler
            .process(&[chunk.to_vec()], None)
            .context("Resampling chunk failed")?;
        output.extend_from_slice(&out[0]);
    }

    output.truncate(expected_out);
    Ok(output)
}
