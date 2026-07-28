# Audio Internals — Architecture Reference

The audio subsystem (`src/audio/`) handles microphone capture, speaker playback, sample-rate resampling, ring buffering, ambient context buffering, and background filler sound.

## AudioOutput (Playback)

```rust
pub struct AudioOutput;

impl AudioOutput {
    /// Create a null audio output for testing (no-op).
    pub fn null() -> Arc<AudioOutput>;

    /// Play audio samples to CPAL speaker output.
    /// cancel: atomic flag checked per-chunk; set to true to abort playback.
    pub fn play_blocking(
        &self,
        samples: &[f32],
        sample_rate: u32,
        cancel: &Arc<AtomicBool>,
    ) -> Result<()>;
}
```

The `null()` constructor returns an `AudioOutput` that silently discards all samples — used in tests and when no output device is available.

The `play_blocking` method:
1. Opens a CPAL output stream with the requested sample rate and mono channels.
2. Splits samples into chunks (configurable via `AUDIO_CHUNK_MS`, default 100 ms).
3. For each chunk, checks `cancel.load(Ordering::Relaxed)` — if true, returns immediately (barge-in).
4. Writes chunk to CPAL and sleeps for chunk duration.

## AudioCapture (Microphone)

```rust
pub struct AudioCapture;

impl AudioCapture {
    pub fn start(
        config: &Config,
        audio_tx: async_channel::Sender<AudioChunk>,
        cancel: CancellationToken,
    ) -> Result<tokio::task::JoinHandle<()>>;
}
```

- Opens CPAL input stream at 16 kHz mono `f32`.
- Pushes raw `AudioChunk` structs into a bounded `async_channel` (200 slots).
- The `cancel` token stops capture cleanly.

## AudioChunk

```rust
pub struct AudioChunk {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
    pub timestamp: Instant,
}
```

## AudioTransformer (Resampling)

```rust
pub struct AudioTransformer {
    target_sample_rate: u32,
    target_channels: u16,
    target_bit_depth: u16,
    resampler: Option<FftFixedIn<f32>>,   // rubato
    source_sample_rate: u32,
    source_channels: u16,
    chunk_size: usize,
}

impl AudioTransformer {
    pub fn new(config: &Config, source_sample_rate: u32, source_channels: u16) -> Result<Self>;
    pub fn transform(&mut self, chunk: AudioChunk) -> Result<TransformedAudio>;
}
```

### Transform Pipeline

```
AudioChunk (raw f32 samples)
  │
  ├─ to_mono / to_stereo  (channel conversion)
  │
  ├─ resample            (rubato::FftFixedIn<f32>, band-limited SRC)
  │
  └─ to_pcm_bytes        (f32 → i16 LE or i24 LE or f32 LE)
  │
  ▼
TransformedAudio { data: Vec<u8>, sample_rate, channels, bit_depth }
```

- **Channel conversion:** averages frames for stereo-to-mono; duplicates for mono-to-stereo.
- **Resampling:** Uses `rubato::FftFixedIn<f32>` with 2 sub-chunks. Handles deinterleaving/reinterleaving.
- **PCM encoding:** 16-bit signed LE (default), 24-bit (3 bytes), or 32-bit float LE depending on `bit_depth`.

### Lightweight helper

```rust
pub fn resample_nearest(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32>;
```

Nearest-neighbour resampling for STT/VAD input. No interpolation — just picks the nearest source index for each output index.

## Ring Buffer (Speech Buffer)

```rust
// src/audio/buffer.rs
pub struct SpeechBuffer { /* circular buffer */ }
```

Stores the rolling window of speech audio before STT processing. Used by `WhisperSTTVAD` to maintain the pre-roll window (300 ms before speech start) and the speech buffer during active utterances.

Key behaviors:
- Fixed capacity in samples.
- `push(chunk)` — appends audio; wraps around when full.
- `drain()` — extracts all buffered samples and clears.
- `len()` — current fill level.

## Ambient Context Buffer

```rust
// src/audio/ambient_buffer.rs
pub struct AmbientContextBuffer {
    entries: VecDeque<AmbientEntry>,
    max_entries: usize,
    window_duration: Duration,
}

struct AmbientEntry {
    text: String,
    timestamp: Instant,
    speaker_id: Option<usize>,
    is_wake_word: bool,
}
```

Maintains a rolling window of ambient speech context for:
- Speaker verification (identifying who is speaking)
- Wake word detection
- Ambient-to-Active mode transitions

**Eviction policy:**
- Fixed `max_entries` (default 30).
- Time-based eviction: entries older than `window_duration` (default 3 minutes) are removed.
- Oldest entries evicted first when both limits are reached.

### Env Vars

| Variable | Default | Description |
|----------|---------|-------------|
| `AMBIENT_BUFFER_MINUTES` | `3` | Rolling window duration |
| `AMBIENT_BUFFER_MAX_ENTRIES` | `30` | Max buffered utterances |

## Filler Sound (Background Processing Cue)

```rust
pub struct FillerController {
    audio_output: Arc<AudioOutput>,
    generation: Arc<AtomicU64>,
    active: Arc<AtomicBool>,
    sample_rate: u32,
}

impl FillerController {
    pub fn new(audio_output: Arc<AudioOutput>, sample_rate: u32) -> Self;

    /// Start playing a subtle "processing" sound.
    pub fn start(&self);

    /// Stop the sound.
    pub fn stop(&self);

    pub fn is_active(&self) -> bool;
}
```

### Sound Characteristics

- **Tone:** 440 Hz sine wave
- **Amplitude:** 0.05 (very quiet, background level)
- **Burst:** 200 ms on, 800 ms off, repeating
- **Envelope:** 10 ms fade-in/fade-out to avoid clicks
- **Thread:** Runs on a **blocking thread** (not tokio task) with cancel check per tick

### When Used

- Started when a tool call is dispatched (auditory feedback that work is happening)
- Stopped when the tool returns or barge-in occurs

### Cancellation

Uses a generation counter: `start()` increments it, `stop()` increments it. The loop checks the generation on every tick (every 50 ms) and exits on mismatch.

## Device Monitor

```rust
// src/device_monitor.rs
pub struct DeviceMonitor;

impl DeviceMonitor {
    pub fn spawn(
        audio_tx: async_channel::Sender<AudioChunk>,
        cancel: CancellationToken,
    ) -> tokio::task::JoinHandle<()>;
}
```

Monitors CPAL for device hotplug events (e.g., Bluetooth headset connection). On device change:
1. Emits `ProactiveEvent::DeviceConnected`.
2. Pipeline can restart audio capture with the new default device.

## Speaker Verification

```rust
// src/audio/speaker.rs
pub struct SpeakerVerifier { /* sherpa-onnx embedding model */ }
```

Uses sherpa-onnx speaker embedding model to identify who is speaking. Maintains up to `SPEAKER_MAX_PROFILES` (default 5) speaker profiles. The first profile (id=0) is the main user.

### Env Vars

| Variable | Default | Description |
|----------|---------|-------------|
| `SPEAKER_MODEL` | auto-detect (`models/speaker_embedding.onnx`) | Path to ONNX model |
| `SPEAKER_ENROLLMENT_PATH` | `data/pro/speaker.emb` | Base path for profiles |
| `SPEAKER_SIMILARITY_MIN` | `0.45` | Cosine similarity threshold |
| `SPEAKER_AMBIENT_TRIGGER` | `3` | Consecutive non-main-user segments to switch to Ambient mode |
| `SPEAKER_MAX_PROFILES` | `5` | Max auto-enrolled profiles |
