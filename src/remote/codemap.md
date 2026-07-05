# src/remote/ — WebSocket Server for Remote Audio Streaming

## Responsibility

WebSocket-based remote client support for the Voicebot pipeline. Accepts a single remote client connection that replaces local audio capture (microphone) and playback (speakers). Binary frames carry audio samples; text frames carry control messages and pipeline events.

## Design

### Module Structure

| File | Role |
|------|------|
| `mod.rs` | Re-exports `protocol` and `server` |
| `server.rs` | WebSocket router, connection handler, audio routing |
| `protocol.rs` | Message types: `ClientMessage`, `ServerMessage`, `TtsAudioPacket` |

### Shared State (`RemoteState`)

```rust
pub struct RemoteState {
    pub audio_tx: async_channel::Sender<AudioChunk>,
    pub samples_per_chunk: usize,
    pub barge_in_tx: broadcast::Sender<u64>,
    pub play_cancel: Arc<AtomicBool>,
    pub tts_audio_tx: Arc<Mutex<Option<mpsc::Sender<TtsAudioPacket>>>>,
    pub connected: AtomicBool,
    #[cfg(feature = "control")]
    pub control_broadcast_tx: broadcast::Sender<ControlEvent>,
}
```

### Protocol (Text Frames)

**Client → Server**:
- `{"type": "session.start", "sample_rate": 16000}` — Begin session, set sample rate.
- `{"type": "barge_in"}` — Interrupt current pipeline.

**Server → Client**:
- `{"type": "session.ready"}` — Session acknowledged.
- `{"type": "transcript", "text": "..."}` — STT result.
- `{"type": "response.text", "text": "..."}` — LLM token.
- `{"type": "response.end"}` — LLM response complete.
- `{"type": "audio.start"}` — TTS audio beginning.
- `{"type": "audio.end"}` — TTS audio complete.
- `{"type": "error", "message": "..."}` — Error condition.

**Binary frames**: Raw audio samples (i16 LE, mono).

### Connection Model

- **Single connection only**: `connected` flag prevents concurrent clients. Race condition handled with `compare_exchange`.
- **Audio routing**: When connected, `tts_audio_tx` is set to send TTS audio to the WebSocket instead of CPAL speakers. On disconnect, restored to `None` (local speakers).

## Flow

### Server Startup

```
start_server(port, state)
  → router(state) → Axum Router
  → TcpListener::bind(0.0.0.0:port)
  → axum::serve(listener, app)

router(state)
  → GET /ws → ws_upgrade
```

### Connection Lifecycle

```
ws_upgrade(ws, state):
  → If connected == true: return 409 CONFLICT
  → ws.on_upgrade(handle_connection)

handle_connection(socket, state):
  → CAS connected: false → true (fail if race)
  
  → Split socket: ws_read, ws_write
  
  // Install TTS routing
  tts_tx, tts_rx = mpsc::channel(32)
  *tts_audio_tx.lock() = Some(tts_tx)
  
  // Spawn 4 concurrent tasks:
  
  1. reader_handle: Read WS messages → fan out binary vs text
  2. audio_handle: Read binary → AudioChunk → pipeline
  3. sink_handle: Read tts_rx → WS binary frames
  4. events_handle: Read control events → WS text frames (feature-gated)
  
  → Wait for reader_handle (client disconnect)
  → Abort all other tasks
  → *tts_audio_tx.lock() = None (restore local audio)
  → connected = false
```

### Task 1: Message Reader (ws_read → fan-out)

```
for msg in ws_read:
  Binary(data):
    → binary_tx.try_send(data) → to audio_handle
  
  Text(json):
    ClientMessage::SessionStart { sample_rate }:
      → remote_sample_rate.store(sample_rate)
      → Send ServerMessage::SessionReady
  
    ClientMessage::BargeIn:
      → barge_in_tx.send(0)
  
  Close:
    → break (signal disconnect)
```

### Task 2: Audio Source (binary → pipeline)

```
for data in binary_rx:
  → Parse i16 LE pairs → f32 normalized samples
  → Buffer until samples_per_chunk reached
  → audio_tx.try_send(AudioChunk { samples, sample_rate, channels: 1 })
```

### Task 3: TTS Sink (tts_rx → binary)

```
for packet in tts_rx:
  → If play_cancel: skip
  
  → If packet.sample_rate != 16000:
    resample_mono_simple(samples, from_rate, 16000)
  
  → Send ServerMessage::AudioStart (text)
  
  → For each 320-sample chunk (20ms @ 16kHz):
    → f32_to_i16le(chunk) → binary frame
  
  → Send ServerMessage::AudioEnd (text)
```

### Task 4: Control Events (feature = "control")

```
for event in control_broadcast_tx:
  → Map ControlEvent to ServerMessage:
    Transcript → ServerMessage::Transcript
    LlmToken → ServerMessage::ResponseText
    LlmDone → ServerMessage::ResponseEnd
    Error → ServerMessage::Error
    (others: ignored)
  → Send as text frame
```

### Audio Conversion Utilities

```
f32_to_i16le(samples):
  → For each f32: clamp(-1.0, 1.0) → i16 → to_le_bytes()

resample_mono_simple(samples, from_rate, to_rate):
  → rubato::FftFixedIn<f32> resampler
  → Process in 1024-sample chunks
  → Handle tail padding
```

## Integration

### Dependencies
- `axum` — WebSocket server framework.
- `futures_util` — Stream/sink operations.
- `async_channel` — Binary audio channel.
- `rubato` — FFT-based audio resampling.
- `crate::audio::audio_capture::AudioChunk` — Pipeline audio format.
- `crate::control::broadcast::ControlEvent` — Event forwarding (feature-gated).

### Consumers
- `src/main.rs` — Creates `RemoteState`, calls `start_server`.
- `src/daemon.rs` — Writes to `tts_audio_tx` for TTS routing.
- `src/audio/` — Reads from `audio_tx` for pipeline input.

### Feature Flags
- `remote` feature enables the entire module.
- `control` feature enables event forwarding (Task 4).