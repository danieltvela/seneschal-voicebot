use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, FromSample, Sample, SampleFormat, SizedSample, StreamConfig};
use rubato::{FftFixedIn, Resampler};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

struct AudioOutputInner {
    /// `None` in the null/headless variant — playback returns immediately.
    device: Option<Device>,
    config: StreamConfig,
    /// Native sample rate of the device (f32, mono-per-channel).
    /// Kept separate from `config` for quick access.
    config_rate: u32,
    /// Number of output channels after capping to stereo.
    channels: u16,
    /// Native sample format of the device. The stream is built with a matching
    /// callback type; ALSA devices that only expose S16 (e.g. the Poly Sync)
    /// reject an f32 stream with "unable to open".
    sample_format: SampleFormat,
}

impl AudioOutputInner {
    fn snapshot(&self) -> OutputSnapshot {
        OutputSnapshot {
            device: self.device.clone(),
            config: self.config.clone(),
            rate: self.config_rate,
            channels: self.channels,
            sample_format: self.sample_format,
        }
    }
}

/// Owned copy of the output device settings used by a single [`StreamSession`].
#[derive(Clone)]
struct OutputSnapshot {
    device: Option<Device>,
    config: StreamConfig,
    rate: u32,
    channels: u16,
    sample_format: SampleFormat,
}

/// Shared, `Arc`-held state of an [`AudioOutput`].
///
/// `StreamSession`s keep the `Arc` alive after the caller drops the output, so
/// they can release their active-session slot on `Drop`.
struct AudioOutputState {
    inner: RwLock<AudioOutputInner>,
    /// Number of live playback sessions. `swap_device` and `start_stream` wait
    /// for it to reach 0, restoring the serialisation the previous
    /// `Mutex`-per-playback gave.
    active_streams: AtomicUsize,
    /// Signalled whenever `active_streams` drops, so waiters can proceed.
    idle: Condvar,
    idle_lock: Mutex<()>,
}

impl AudioOutputState {
    fn release_stream(&self) {
        if self.active_streams.fetch_sub(1, Ordering::SeqCst) == 1 {
            let _guard = self.idle_lock.lock().unwrap();
            self.idle.notify_all();
        }
    }

    /// Block until no session is active, then claim a slot.
    fn claim_slot(&self) {
        let mut guard = self.idle_lock.lock().unwrap();
        while self.active_streams.load(Ordering::SeqCst) > 0 {
            let (g, _) = self
                .idle
                .wait_timeout(guard, Duration::from_millis(50))
                .unwrap();
            guard = g;
        }
        self.active_streams.fetch_add(1, Ordering::SeqCst);
    }
}

pub struct AudioOutput {
    state: Arc<AudioOutputState>,
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
        let sample_rate = supported.sample_rate();

        info!(
            target: "audio",
            "Output config: {}Hz, {}ch, {:?}",
            sample_rate, channels, sample_format
        );

        let config = StreamConfig {
            channels,
            sample_rate,
            // 50 ms period instead of ALSA's minimal default — prevents
            // TTS underruns (audible crackle) under inference load and
            // enables cpal's RT-priority boost on the ALSA thread.
            buffer_size: crate::audio::robust_buffer_size(
                &supported,
                crate::audio::PLAYBACK_PERIOD_MS,
            ),
        };

        Ok(Self {
            state: Arc::new(AudioOutputState {
                inner: RwLock::new(AudioOutputInner {
                    device: Some(device),
                    config,
                    config_rate: sample_rate,
                    channels,
                    sample_format,
                }),
                active_streams: AtomicUsize::new(0),
                idle: Condvar::new(),
                idle_lock: Mutex::new(()),
            }),
        })
    }

    /// Returns `true` when a real audio output device is available,
    /// `false` for the null/headless variant.
    pub fn has_device(&self) -> bool {
        self.state.inner.read().unwrap().device.is_some()
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
    /// Waits for any active playback session to finish, so the swap happens
    /// between utterances, never mid-playback.
    pub fn swap_device(&self, device_name: Option<&str>) -> Result<()> {
        // Wait for in-flight playback to complete.
        {
            let mut guard = self.state.idle_lock.lock().unwrap();
            while self.state.active_streams.load(Ordering::SeqCst) > 0 {
                let (g, _) = self
                    .state
                    .idle
                    .wait_timeout(guard, Duration::from_millis(50))
                    .unwrap();
                guard = g;
            }
        }

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
        let sample_rate = supported.sample_rate();
        let config = StreamConfig {
            channels,
            sample_rate,
            buffer_size: crate::audio::robust_buffer_size(
                &supported,
                crate::audio::PLAYBACK_PERIOD_MS,
            ),
        };

        {
            let mut inner = self.state.inner.write().unwrap();
            inner.device = Some(device);
            inner.config = config;
            inner.config_rate = sample_rate;
            inner.channels = channels;
            inner.sample_format = sample_format;
        }

        info!(
            target: "audio",
            "Output device swapped: {} ({}Hz, {}ch, {:?})",
            desc,
            sample_rate,
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

    /// Null/headless audio output — playback discards all audio immediately.
    ///
    /// Used in tests and CI environments where no sound device is available,
    /// and as the startup fallback when no output device is connected.
    #[allow(dead_code)]
    pub fn null() -> Self {
        Self {
            state: Arc::new(AudioOutputState {
                inner: RwLock::new(AudioOutputInner {
                    device: None,
                    config: StreamConfig {
                        channels: 1,
                        sample_rate: 22050,
                        buffer_size: cpal::BufferSize::Default,
                    },
                    config_rate: 22050,
                    channels: 1,
                    sample_format: SampleFormat::F32,
                }),
                active_streams: AtomicUsize::new(0),
                idle: Condvar::new(),
                idle_lock: Mutex::new(()),
            }),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.state.inner.read().unwrap().config_rate
    }

    pub fn channels(&self) -> u16 {
        self.state.inner.read().unwrap().channels
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
        let resampled = if source_rate != target_rate {
            resample(samples, source_rate, target_rate)
                .context("Failed to resample audio for playback")?
        } else {
            samples.to_vec()
        };

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

    /// Start a streaming playback session. Chunks are pushed with
    /// [`StreamSession::push`], the utterance is closed with
    /// [`StreamSession::finish`], and [`StreamSession::wait`] blocks until every
    /// sample has reached the device (or `cancel` is set).
    ///
    /// Blocks while another session is active, so only one playback runs at a
    /// time (restores the previous `Mutex`-per-playback serialisation).
    pub fn start_stream(&self, source_rate: u32, cancel: Arc<AtomicBool>) -> Result<StreamSession> {
        self.state.claim_slot();
        let snapshot = self.state.inner.read().unwrap().snapshot();
        match StreamSession::new(snapshot, source_rate, cancel, Arc::clone(&self.state)) {
            Ok(s) => Ok(s),
            Err(e) => {
                self.state.release_stream();
                Err(e)
            }
        }
    }

    /// Play mono f32 samples (at `source_rate`) through the output device.
    /// Blocks until playback finishes or `cancel` is set (barge-in).
    ///
    /// Implemented on top of [`StreamSession`]; a null/headless output returns
    /// immediately.
    pub fn play_blocking(
        &self,
        samples: &[f32],
        source_rate: u32,
        cancel: &Arc<AtomicBool>,
    ) -> Result<()> {
        let session = self.start_stream(source_rate, Arc::clone(cancel))?;
        if !session.has_device() {
            return Ok(());
        }

        debug!(
            target: "audio",
            "play_blocking: {} samples in, source={}Hz → device={}Hz {}ch",
            samples.len(), source_rate, session.target_rate(), session.channels()
        );

        const PUSH_CHUNK: usize = 4096;
        for chunk in samples.chunks(PUSH_CHUNK) {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            session.push(chunk);
        }
        session.finish();
        session.wait();
        Ok(())
    }
}

// ── Streaming playback ─────────────────────────────────────────────────────────

/// State shared by the CPAL callback, the fill thread, the producer and `wait`.
struct PlaybackState {
    /// Interleaved device-rate samples ready to play.
    ring: VecDeque<f32>,
    /// Producer called `finish()`.
    input_finished: bool,
    /// Fill thread finished draining the resampler.
    drained: bool,
    /// Callback emitted its first real sample.
    playback_started: bool,
    /// Playback was cancelled (barge-in).
    cancelled: bool,
    /// Total mono samples produced by the resampler (0 = silence only).
    mono_produced: u64,
    /// When the ring first became empty after `drained`; used to keep the
    /// stream alive long enough to flush the device buffer (no tail truncation).
    drain_started: Option<Instant>,
}

/// One streaming playback session (one sentence/utterance).
pub struct StreamSession {
    #[allow(dead_code)]
    source_rate: u32,
    has_device: bool,
    shared: Arc<(Mutex<PlaybackState>, Condvar)>,
    /// Shared with the fill thread: turns arbitrary input chunks into
    /// device-rate blocks and pushes them into the ring.
    fill_gen: Arc<Mutex<BlockResampler>>,
    finished: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    fill_thread: Option<std::thread::JoinHandle<()>>,
    /// Kept alive so CPAL keeps calling the callback until the session drops.
    _stream: Option<cpal::Stream>,
    /// Device settings snapshot (rate/channels/format) for this session.
    snapshot: OutputSnapshot,
    /// The output state, so `Drop` can release the active-session slot.
    output: Arc<AudioOutputState>,
}

impl StreamSession {
    fn new(
        snapshot: OutputSnapshot,
        source_rate: u32,
        cancel: Arc<AtomicBool>,
        output: Arc<AudioOutputState>,
    ) -> Result<Self> {
        let has_device = snapshot.device.is_some();
        let shared = Arc::new((
            Mutex::new(PlaybackState {
                ring: VecDeque::new(),
                input_finished: false,
                drained: false,
                playback_started: false,
                cancelled: false,
                mono_produced: 0,
                drain_started: None,
            }),
            Condvar::new(),
        ));
        let finished = Arc::new(AtomicBool::new(false));
        let fill_gen = Arc::new(Mutex::new(BlockResampler::new(source_rate, snapshot.rate)));

        let mut session = StreamSession {
            source_rate,
            has_device,
            shared: Arc::clone(&shared),
            fill_gen: Arc::clone(&fill_gen),
            finished,
            cancel,
            fill_thread: None,
            _stream: None,
            snapshot,
            output,
        };

        if !has_device {
            return Ok(session);
        }

        let snapshot = session.snapshot.clone();
        let channels = snapshot.channels;

        // Fill thread: drives the resampler and moves blocks into the ring.
        {
            let shared = Arc::clone(&shared);
            let cancel = Arc::clone(&session.cancel);
            let finished = Arc::clone(&session.finished);
            let resampler = Arc::clone(&fill_gen);
            session.fill_thread = Some(
                std::thread::Builder::new()
                    .name("tts-fill".into())
                    .spawn(move || {
                        fill_loop(resampler, shared, finished, cancel, channels as usize)
                    })
                    .expect("failed to spawn tts-fill thread"),
            );
        }

        // The cpal stream: drained by the audio callback. Built generically so
        // any device sample format (e.g. the I32 default of some USB DACs) works.
        let device = snapshot.device.as_ref().unwrap();
        let config = snapshot.config.clone();
        let cb_shared = Arc::clone(&shared);
        let cb_cancel = Arc::clone(&session.cancel);

        macro_rules! build {
            ($t:ty) => {
                build_output_stream::<$t>(
                    device,
                    &config,
                    Arc::clone(&cb_shared),
                    Arc::clone(&cb_cancel),
                )?
            };
        }

        let stream = match snapshot.sample_format {
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
                return Err(anyhow!("Unsupported output sample format: {other:?}"));
            }
        };
        stream.play().context("Failed to start output stream")?;
        session._stream = Some(stream);
        Ok(session)
    }

    pub fn has_device(&self) -> bool {
        self.has_device
    }

    pub fn target_rate(&self) -> u32 {
        self.snapshot.rate
    }

    pub fn channels(&self) -> u16 {
        self.snapshot.channels
    }

    /// Push mono samples at the session's source rate. Blocks when the ring is
    /// full (backpressure), so the producer cannot outrun playback.
    pub fn push(&self, samples: &[f32]) {
        if !self.has_device || samples.is_empty() {
            return;
        }
        if self.cancel.load(Ordering::Relaxed) {
            return;
        }

        let mono = { self.fill_gen.lock().unwrap().push(samples) };
        if mono.is_empty() {
            self.notify();
            return;
        }
        let interleaved = interleave(&mono, self.snapshot.channels);
        self.enqueue(&interleaved);
    }

    /// Enqueue interleaved samples into the bounded ring, blocking while full.
    fn enqueue(&self, interleaved: &[f32]) {
        let (lock, cvar) = &*self.shared;
        let mut guard = lock.lock().unwrap();
        let mut offset = 0;
        loop {
            offset += enqueue_interleaved(&mut guard, &interleaved[offset..]);
            if offset >= interleaved.len() {
                return;
            }
            if guard.cancelled || self.cancel.load(Ordering::Relaxed) {
                return;
            }
            cvar.notify_all();
            let (g, _) = cvar.wait_timeout(guard, Duration::from_millis(20)).unwrap();
            guard = g;
        }
    }

    /// Signal that no more samples will be pushed.
    pub fn finish(&self) {
        if !self.has_device {
            return;
        }
        let (lock, cvar) = &*self.shared;
        lock.lock().unwrap().input_finished = true;
        cvar.notify_all();
    }

    /// Block until playback finishes (audio drained to the device) or `cancel`.
    ///
    /// After the ring empties, the stream is kept alive for
    /// [`drain_after_empty`] so the device buffer flushes instead of truncating
    /// the final samples.
    pub fn wait(&self) {
        if !self.has_device {
            return;
        }
        let (lock, cvar) = &*self.shared;
        let mut st = lock.lock().unwrap();
        loop {
            if st.cancelled || self.cancel.load(Ordering::Relaxed) {
                return;
            }
            if playback_complete(&st) {
                let drain = drain_after_empty();
                match st.drain_started {
                    None => {
                        st.drain_started = Some(Instant::now());
                    }
                    Some(t) if t.elapsed() >= drain => return,
                    Some(_) => {}
                }
            }
            let (st2, _) = cvar.wait_timeout(st, Duration::from_millis(20)).unwrap();
            st = st2;
        }
    }

    /// Returns `true` once the device callback has emitted its first sample.
    pub fn playback_started(&self) -> bool {
        self.shared.0.lock().unwrap().playback_started
    }

    fn notify(&self) {
        let (_, cvar) = &*self.shared;
        cvar.notify_all();
    }
}

/// How long to keep the stream alive after the ring empties, to flush the
/// device buffer (AUDIO_DRAIN_MS, default 150 ms).
fn drain_after_empty() -> Duration {
    let ms = std::env::var("AUDIO_DRAIN_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(150);
    Duration::from_millis(ms)
}

impl Drop for StreamSession {
    fn drop(&mut self) {
        self.finished.store(true, Ordering::SeqCst);
        self.notify();
        if let Some(h) = self.fill_thread.take() {
            let _ = h.join();
        }
        // Release the active-session slot so the next playback / device swap
        // can proceed.
        self.output.release_stream();
    }
}

// The session shares the resampler with the fill thread (both hold an `Arc`).

/// Blocking fill loop: drains the resampler into the ring buffer.
fn fill_loop(
    resampler: Arc<Mutex<BlockResampler>>,
    shared: Arc<(Mutex<PlaybackState>, Condvar)>,
    finished: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    channels: usize,
) {
    let (lock, cvar) = &*shared;
    loop {
        if cancel.load(Ordering::Relaxed) {
            let mut st = lock.lock().unwrap();
            st.cancelled = true;
            cvar.notify_all();
            return;
        }

        // Completed?
        {
            let st = lock.lock().unwrap();
            if st.drained && st.ring.is_empty() {
                return;
            }
        }

        let have_room = {
            let st = lock.lock().unwrap();
            st.ring.len() < RING_CAP_SAMPLES
        };

        if have_room {
            // Pull whatever the resampler can produce right now.
            let mono = { resampler.lock().unwrap().pump() };
            if !mono.is_empty() {
                let interleaved = interleave(&mono, channels as u16);
                let mut st = lock.lock().unwrap();
                enqueue_interleaved(&mut st, &interleaved);
                st.mono_produced += mono.len() as u64;
                cvar.notify_all();
                continue;
            }
            // No full blocks yet. If input finished, flush the tail once.
            let (need_flush, already) = {
                let st = lock.lock().unwrap();
                (st.input_finished && !st.drained, st.drained)
            };
            if need_flush && !already {
                let tail = { resampler.lock().unwrap().flush() };
                if !tail.is_empty() {
                    let interleaved = interleave(&tail, channels as u16);
                    let mut st = lock.lock().unwrap();
                    enqueue_interleaved(&mut st, &interleaved);
                    st.mono_produced += tail.len() as u64;
                }
                let mut st = lock.lock().unwrap();
                st.drained = true;
                cvar.notify_all();
                continue;
            }
        }

        if finished.load(Ordering::SeqCst) {
            return;
        }
        // Wait for a producer push or callback drain (bounded poll).
        let (st, _) = cvar
            .wait_timeout(lock.lock().unwrap(), std::time::Duration::from_millis(10))
            .unwrap();
        drop(st);
    }
}

/// Ring capacity in interleaved samples (~0.5 s at 48 kHz stereo).
const RING_CAP_SAMPLES: usize = 48_000;

/// Enqueue as many samples as fit in the ring, returning how many were added.
fn enqueue_interleaved(state: &mut PlaybackState, interleaved: &[f32]) -> usize {
    let room = RING_CAP_SAMPLES.saturating_sub(state.ring.len());
    let take = room.min(interleaved.len());
    state.ring.extend(interleaved[..take].iter().copied());
    take
}

/// Whether playback is complete (ring drained to the device or cancelled).
/// Deliberately independent of `playback_started`, so an utterance with no
/// device output still terminates.
fn playback_complete(state: &PlaybackState) -> bool {
    state.cancelled
        || (state.input_finished && state.drained && state.ring.is_empty())
        || (state.input_finished && state.drained && state.mono_produced == 0)
}

fn interleave(mono: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return mono.to_vec();
    }
    let ch = channels as usize;
    let mut v = Vec::with_capacity(mono.len() * ch);
    for &s in mono {
        for _ in 0..ch {
            v.push(s);
        }
    }
    v
}

/// Callback body: pop samples into `data`, mark start, wake the producer.
/// Returns the number of samples written.
fn drain_callback<T>(shared: &Arc<(Mutex<PlaybackState>, Condvar)>, data: &mut [T]) -> usize
where
    T: Sample + FromSample<f32>,
{
    let (lock, cvar) = &**shared;
    let mut st = lock.lock().unwrap();
    let n = data.len().min(st.ring.len());
    for (dst, src) in data.iter_mut().take(n).zip(st.ring.drain(..n)) {
        *dst = T::from_sample(src);
    }
    // Remaining frames stay zeroed by CPAL.
    if n > 0 {
        st.playback_started = true;
        cvar.notify_all();
    }
    n
}

/// Build an output stream for any device sample format, converting from the
/// internal f32 PCM via cpal/dasp's `FromSample`.
fn build_output_stream<T>(
    device: &Device,
    config: &StreamConfig,
    shared: Arc<(Mutex<PlaybackState>, Condvar)>,
    cancel: Arc<AtomicBool>,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                if cancel.load(Ordering::Relaxed) {
                    for s in data.iter_mut() {
                        *s = T::EQUILIBRIUM;
                    }
                    let (lock, cvar) = &*shared;
                    let mut st = lock.lock().unwrap();
                    st.cancelled = true;
                    cvar.notify_all();
                    return;
                }
                drain_callback(&shared, data);
            },
            crate::audio::log_stream_error,
            None,
        )
        .context("Failed to build output stream")
}

// ── Stateful resampling ────────────────────────────────────────────────────────

const RESAMPLE_CHUNK: usize = 1024;

/// Streaming resampler: accepts arbitrary-sized input chunks and yields
/// arbitrary-sized output chunks, flushing at end of stream.
struct BlockResampler {
    resampler: Option<FftFixedIn<f32>>,
    buffer: Vec<f32>,
}

impl BlockResampler {
    fn new(source_rate: u32, target_rate: u32) -> Self {
        let resampler = if source_rate != target_rate {
            FftFixedIn::<f32>::new(
                source_rate as usize,
                target_rate as usize,
                RESAMPLE_CHUNK,
                2,
                1,
            )
            .ok()
        } else {
            None
        };
        Self {
            resampler,
            buffer: Vec::new(),
        }
    }

    fn push(&mut self, samples: &[f32]) -> Vec<f32> {
        if self.resampler.is_none() {
            return samples.to_vec();
        }
        self.buffer.extend_from_slice(samples);
        self.pump()
    }

    /// Process any complete `RESAMPLE_CHUNK` blocks currently buffered.
    fn pump(&mut self) -> Vec<f32> {
        let mut out = Vec::new();
        if let Some(rs) = self.resampler.as_mut() {
            while self.buffer.len() >= RESAMPLE_CHUNK {
                let block: Vec<f32> = self.buffer.drain(..RESAMPLE_CHUNK).collect();
                if let Ok(y) = rs.process(&[block], None) {
                    out.extend_from_slice(&y[0]);
                }
            }
        }
        out
    }

    /// Flush the residual buffer at end of stream (zero-padded).
    fn flush(&mut self) -> Vec<f32> {
        let mut out = Vec::new();
        if !self.buffer.is_empty() {
            let mut tail = std::mem::take(&mut self.buffer);
            tail.resize(RESAMPLE_CHUNK, 0.0);
            if let Some(rs) = self.resampler.as_mut() {
                if let Ok(y) = rs.process(&[tail], None) {
                    out.extend_from_slice(&y[0]);
                }
            } else {
                out = tail;
            }
        }
        out
    }
}

// ── Resampling ─────────────────────────────────────────────────────────────────

fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>> {
    let expected_out = (samples.len() as f64 * to_rate as f64 / from_rate as f64).ceil() as usize;

    let mut resampler =
        FftFixedIn::<f32>::new(from_rate as usize, to_rate as usize, RESAMPLE_CHUNK, 2, 1)
            .context("Failed to create resampler")?;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> PlaybackState {
        PlaybackState {
            ring: VecDeque::new(),
            input_finished: false,
            drained: false,
            playback_started: false,
            cancelled: false,
            mono_produced: 0,
            drain_started: None,
        }
    }

    #[test]
    fn interleave_expands_channels() {
        assert_eq!(interleave(&[1.0, 2.0], 1), vec![1.0, 2.0]);
        assert_eq!(interleave(&[1.0, 2.0], 2), vec![1.0, 1.0, 2.0, 2.0]);
    }

    #[test]
    fn enqueue_is_bounded_by_ring_capacity() {
        let mut st = state();
        let big = vec![0.5f32; RING_CAP_SAMPLES + 100];
        let added = enqueue_interleaved(&mut st, &big);
        assert_eq!(added, RING_CAP_SAMPLES);
        assert_eq!(st.ring.len(), RING_CAP_SAMPLES);
        // A second call has no room.
        assert_eq!(enqueue_interleaved(&mut st, &[0.1, 0.2]), 0);
    }

    #[test]
    fn drain_callback_consumes_ring_and_marks_start() {
        let shared = Arc::new((Mutex::new(state()), Condvar::new()));
        {
            let mut st = shared.0.lock().unwrap();
            st.ring.extend([0.25f32, 0.5, 0.75]);
        }
        let mut out = [0.0f32; 8];
        let n = drain_callback(&shared, &mut out);
        assert_eq!(n, 3);
        assert_eq!(&out[..3], &[0.25, 0.5, 0.75]);
        assert!(shared.0.lock().unwrap().playback_started);
        assert!(shared.0.lock().unwrap().ring.is_empty());
    }

    #[test]
    fn playback_complete_requires_drain_and_empty_ring() {
        let mut st = state();
        assert!(!playback_complete(&st));
        st.mono_produced = 100; // real audio was produced
        st.input_finished = true;
        assert!(!playback_complete(&st), "still draining");
        st.drained = true;
        assert!(playback_complete(&st), "drained and empty");
        st.ring.push_back(0.0);
        assert!(!playback_complete(&st), "ring not empty");
    }

    #[test]
    fn playback_complete_when_no_audio_produced() {
        let mut st = state();
        st.input_finished = true;
        st.drained = true;
        st.mono_produced = 0;
        assert!(playback_complete(&st));
    }

    #[test]
    fn cancelled_is_complete_even_with_buffered_audio() {
        let mut st = state();
        st.ring.extend([1.0, 2.0]);
        st.cancelled = true;
        assert!(playback_complete(&st));
    }

    #[test]
    fn block_resampler_passthrough_when_rates_match() {
        let mut r = BlockResampler::new(24_000, 24_000);
        assert_eq!(r.push(&[0.1, 0.2, 0.3]), vec![0.1, 0.2, 0.3]);
        assert!(r.flush().is_empty());
    }

    #[test]
    fn block_resampler_buffers_partial_blocks() {
        let mut r = BlockResampler::new(24_000, 48_000);
        // Less than one RESAMPLE_CHUNK → no output yet, kept buffered.
        assert!(r.push(&[0.0; 10]).is_empty());
        // Filling a full block yields upsampled output.
        let out = r.push(&vec![0.0; RESAMPLE_CHUNK - 10]);
        assert!(!out.is_empty());
    }

    #[test]
    fn null_output_play_blocking_is_instant() {
        let out = AudioOutput::null();
        assert!(!out.has_device());
        let cancel = Arc::new(AtomicBool::new(false));
        let samples = vec![0.1f32; 1000];
        out.play_blocking(&samples, 24_000, &cancel).unwrap();
    }
}
