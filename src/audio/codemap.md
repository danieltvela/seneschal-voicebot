# src/audio/ — Audio Pipeline

## Responsibility

Acquire raw microphone samples, transform them to a uniform format (sample rate, channels, bit depth), buffer them for VAD/STT consumption, and play back synthesized speech through the default output device. Provides optional speaker verification and ambient-speech context capture.

---

## Design

| File | Role |
|------|------|
| `mod.rs` | Module declarations only (`ambient_buffer`, `audio_capture`, `audio_transform`, `buffer`, `output`, `speaker`). |
| `audio_capture.rs` | `AudioCapture` — CPAL-based microphone input. Handles device selection (partial name match with `#N` index suffix), sample-format normalization (I16/U16/F32 → f32), and chunked delivery via `async_channel::Sender<AudioChunk>`. |
| `audio_transform.rs` | `AudioTransformer` — resampling (rubato `FftFixedIn`), channel mixing (mono↔stereo), and bit-depth conversion (16/24/32-bit PCM). Also exposes `resample_nearest()` for lightweight nearest-neighbor downsample to 16 kHz. |
| `buffer.rs` | `AudioBuffer` — fixed-capacity `VecDeque<f32>` ring buffer. Evicts oldest samples when full. Used by VAD/STT to retain pre-roll samples. |
| `output.rs` | `AudioOutput` — CPAL-based speaker playback. Supports blocking playback (`play_blocking`) with barge-in cancellation via `Arc<AtomicBool>`, automatic resampling + channel expansion, and a null/headless variant for CI. |
| `speaker.rs` | `SpeakerVerifier` — multi-speaker identity verification using sherpa-rs ONNX embeddings (feature-gated behind `speaker`). Persists profiles as `{profiles_dir}/speaker_{id}.emb` (raw f32 LE bytes). |
| `ambient_buffer.rs` | `AmbientBuffer` — rolling window of transcribed ambient utterances (speaker label + text + timestamp). Used to inject `[Contexto reciente]` context into the LLM prompt. |

### Key Types

- **`AudioChunk`** — raw microphone samples: `Vec<f32>` + `sample_rate` + `channels`.
- **`TransformedAudio`** — format-normalized samples: `Vec<u8>` (PCM bytes) + `sample_rate` + `channels` + `bit_depth`.
- **`SpeakerVerdict`** — enum: `Known { id, label, similarity }`, `Unknown { similarity }`, `Enrolled { id, label }`.
- **`AmbientEntry`** — single utterance: `speaker_label`, `transcript`, `timestamp: Instant`.

### Architectural Decisions

- **f32 internal representation**: All modules use `Vec<f32>` for in-process audio. Conversion to PCM bytes only happens at the transform boundary (`to_pcm_bytes`).
- **Non-blocking capture**: `AudioCapture::start_capture` uses `tx.try_send()` — drops chunks if the downstream consumer is behind.
- **Barge-in support**: `AudioOutput::play_blocking` checks `AtomicBool` cancellation token on every callback frame, allowing immediate interruption.
- **Drain tail**: After audio content is written, 150 ms of silence is appended (`AUDIO_DRAIN_MS` env var) to allow CoreAudio/ALSA to flush DAC buffers, reducing inter-sentence gaps.
- **Null output**: `AudioOutput::null()` creates a headless output that returns immediately from `play_blocking` — used in tests/CI.
- **Channel cap**: Output device channels are capped at 2 (`min(2)`) to avoid issues with multi-channel virtual devices (e.g., BlackHole 8ch).

---

## Flow

### Capture → Transform → VAD/STT

```
Microphone (CPAL)
  → AudioCapture::start_capture(tx, samples_per_chunk)
      → CPAL input callback (I16/U16/F32)
      → normalize to f32 (I16: /i16::MAX, U16: (/u16::MAX * 2 - 1))
      → AudioCapture::process_samples() → accumulate in Arc<Mutex<Vec>>
      → when buffer >= samples_per_chunk * channels:
          ├─ drain chunk
          └─ tx.try_send(AudioChunk { samples, sample_rate, channels })
              (non-blocking — drops if channel full)

  → (downstream consumer, e.g. pipeline)
      → AudioTransformer::transform(AudioChunk)
          → to_mono() (average channels) or to_stereo() (duplicate)
          → resample() via rubato FftFixedIn (if rate mismatch)
          → to_pcm_bytes() (16/24/32-bit LE PCM)
      → TransformedAudio { data, sample_rate, channels, bit_depth }
```

### Playback

```
TTS engine (synthesize) → Vec<f32> at engine sample rate
  → AudioOutput::play_blocking(samples, source_rate, cancel)
      → AudioOutput::prepare(samples, source_rate, target_rate, channels)
          → resample() via rubato FftFixedIn (if rate mismatch, chunked at 1024)
          → interleave to device channel count
      → CPAL output stream callback:
          ├─ if cancel.load() → fill silence, signal done, return
          ├─ write prepared[pos..pos+n] into data buffer
          ├─ fill remaining with silence (drain tail: 150ms default)
          ├─ advance pos
          └─ if pos >= stop_pos → signal Condvar
      → calling thread blocks on Condvar until done or cancelled
```

### Speaker Verification

```
AudioBuffer (post-VAD segment) → SpeakerVerifier::verify(sample_rate, samples)
  → sherpa-rs EmbeddingExtractor::compute_speaker_embedding()
  → cosine_similarity() against each SpeakerProfile
  → if best match >= threshold → Known
  → if no match + profiles < max_profiles → Enrolled (persist to disk)
  → if no match + profiles full → Unknown
  → (no feature flag) → stub: always Known { id: 0, "Usuario", 1.0 }
```

### Ambient Buffer

```
SpeechEvent::SpeechEnd(text) → AmbientBuffer::push(speaker_label, transcript)
  → evict entries older than max_duration
  → evict oldest if at max_entries
  → push new AmbientEntry
  → (later) AmbientBuffer::format_context()
      → if empty → None
      → else → "[Contexto reciente]\n{label}: {text}\n..."
```

### Buffer

```
AudioBuffer::new(sample_rate, max_duration_secs) → capacity = sample_rate * max_duration_secs
AudioBuffer::push(samples) → append, evict front if at max_size
AudioBuffer::get_samples_from(offset) → skip offset samples from front
AudioBuffer::sample_count() → current length
```

---

## Integration

### Dependencies
- **cpal** — cross-platform audio I/O (input capture, output playback)
- **rubato** — high-quality FFT-based resampling (`FftFixedIn<f32>`)
- **async-channel** — multi-producer channel for capture → pipeline
- **sherpa-rs** (feature `speaker`) — speaker embedding extraction (`EmbeddingExtractor`)
- **std::sync** — `Arc<Mutex>`, `Arc<Condvar>`, `Arc<AtomicBool>` for thread coordination

### Consumers
- **`src/daemon.rs` (InferenceDaemon)** — creates `AudioCapture`, starts capture loop, feeds chunks to STT provider
- **`src/pipeline/`** — consumes `AudioChunk` from capture channel, passes to STT, plays TTS output via `AudioOutput`
- **`src/stt/`** — receives f32 samples, runs VAD + transcription
- **`src/tts/`** — produces f32 samples consumed by `AudioOutput`
- **`src/config.rs`** — provides `sample_rate`, `channels`, `samples_per_chunk()`

### Environment Variables
- `AUDIO_SAMPLE_RATE` — target sample rate (default: 16000)
- `AUDIO_CHANNELS` — target channel count (default: 1)
- `AUDIO_DRAIN_MS` — silence drain tail in ms (default: 150)