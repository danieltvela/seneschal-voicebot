# src/analysis/ — Identity Analysis

## Responsibility

Background audio analysis subsystem for real-time contextual awareness. Provides a **blackboard architecture** (`ContextLens`) where analyzers write time-expired contextual entries that are read by the LLM task before each request. Currently implements speaker identity verification (`IdentityAnalyzer`) to detect whether the current speaker is the enrolled primary user.

## Design

### Module Structure

| File | Role |
|------|------|
| `mod.rs` | Core abstractions: `ContextLens`, `ContextEntry`, `AudioAnalyzer` trait, `AnalysisDispatcher` |
| `identity.rs` | Speaker identity analyzer: wraps `SpeakerVerifier`, writes to `ContextLens` |

### Core Abstractions

**`ContextEntry`** — A single contextual fact:
```rust
pub struct ContextEntry {
    pub key: &'static str,
    pub value: String,
    pub confidence: f32,
    pub valid_until: Instant,   // TTL-based expiration
    pub source: &'static str,
}
```

**`ContextLens`** — Shared blackboard (HashMap keyed by `&'static str`):
- `upsert(entry)` — Insert or replace by key.
- `get(key)` — Return entry if not expired.
- `purge_expired()` — Remove all expired entries.
- `format_for_llm()` — Format fresh entries as `[Analysis Context]` block.

**`AudioAnalyzer` trait** — Interface for background analyzers:
```rust
pub trait AudioAnalyzer: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn analyze(&self, audio: Arc<Vec<f32>>, sample_rate: u32)
        -> impl Future<Output = Option<ContextEntry>> + Send;
}
```

**`AnalysisDispatcher`** — Routes audio clips to registered analyzers:
- `register_audio_channel(capacity)` — Register analyzer, return receiver.
- `dispatch(clip)` — Fan-out to all registered channels (non-blocking, best-effort).

### Identity Analyzer

**`IdentityAnalyzer`** — Wraps `SpeakerVerifier`, writes `speaker_identity` entries:
- `IDENTITY_TTL = 120 seconds` — How long identity entries stay fresh.
- `verify(sample_rate, audio)` — Synchronous call (mirrors underlying model).
- Returns `IdentityResult { is_main_speaker, speaker_label }`.

### Speaker Verdict Types

```rust
enum SpeakerVerdict {
    Enrolled { id, label },      // Exact match to enrolled profile
    Known { id, label, similarity },  // Similar but not exact
    Unknown { similarity },       // No match
}
```

## Flow

### ContextLens Operations

```
ContextLens::new() → empty HashMap

upsert(entry):
  → entries.insert(entry.key, entry)

get(key):
  → entries.get(key).filter(|e| e.valid_until > Instant::now())

purge_expired():
  → entries.retain(|_, e| e.valid_until > Instant::now())

format_for_llm():
  → Filter entries where valid_until > now
  → If empty: return None
  → Format: "\n[Analysis Context]\n{key}: {value}\n" per entry
```

### AnalysisDispatcher

```
AnalysisDispatcher::new() → empty senders list

register_audio_channel(capacity):
  → (tx, rx) = mpsc::channel(capacity)
  → senders.push(tx)
  → return rx

dispatch(clip):
  → For each tx in senders: tx.try_send(Arc::clone(&clip))
```

### Identity Verification

```
IdentityAnalyzer::verify(sample_rate, audio):
  verdict = verifier.verify(sample_rate, audio)
  
  match verdict:
    Enrolled { id, label }:
      is_main_speaker = (id == 0)
      value = "{label} (enrolled main user)" or "(enrolled secondary speaker)"
      confidence = 1.0
    
    Known { id, label, similarity }:
      is_main_speaker = (id == 0)
      value = "{label} (similarity={similarity:.2})"
      confidence = similarity
    
    Unknown { similarity }:
      is_main_speaker = false
      value = "unknown speaker (similarity={similarity:.2})"
      speaker_label = "Ambiente"
      confidence = similarity
  
  entry = ContextEntry {
    key: "speaker_identity",
    value,
    confidence,
    valid_until: Instant::now() + IDENTITY_TTL,
    source: "identity_analyzer",
  }
  
  lens.lock().upsert(entry)
  
  return IdentityResult { is_main_speaker, speaker_label }
```

## Integration

### Dependencies
- `crate::audio::speaker::{SpeakerVerifier, SpeakerVerdict}` — Speaker verification model.
- `std::time::Instant` — TTL-based expiration.
- `std::sync::{Arc, Mutex}` — Shared ContextLens.
- `tokio::sync::mpsc` — Audio clip channels.
- `tracing` — Logging.

### Consumers
- `src/daemon.rs` — Creates `ContextLens`, `AnalysisDispatcher`, `IdentityAnalyzer`.
- `src/pipeline/` — Reads `ContextLens.format_for_llm()` before LLM requests.
- Audio loop — Calls `IdentityAnalyzer.verify()` on `SpeechEnd` events.

### Data Flow
```
Audio loop (SpeechEnd event)
  → IdentityAnalyzer.verify(sample_rate, audio)
  → ContextLens.upsert(speaker_identity entry)

LLM task (before each request)
  → ContextLens.purge_expired()
  → ContextLens.format_for_llm() → inject into system prompt
```