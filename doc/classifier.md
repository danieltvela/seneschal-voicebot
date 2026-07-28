# Intent Classifier — Architecture & API Reference

The classifier module (`src/classifier/`) provides a **cascading intent classifier** that decides whether a user request is `Simple` (greeting, trivial) or `Complex` (tools, reasoning, search). The LLM pipeline uses this to tune temperature, thinking mode, and tool availability per request.

> **Note:** `embedding.rs` and `logistic.rs` (Niveles 2/3) are **dead code stubs** — feature-gated behind the empty `classifier-embedding` feature flag. They always return an error. Scheduled for removal in the carve-out.

## Core Types

```rust
/// Classification result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Simple,   // casual, greeting, confirmation
    Complex,  // tools, reasoning, code, search, multi-step
}

/// Which cascade level made the decision.
pub enum ClassifierLevel {
    Heuristic,  // Nivel 1 — keyword + trivial-pattern match
    Embedding,  // Nivel 2 — stub (feature-gated, always fails)
    Logistic,   // Nivel 3 — stub (feature-gated, always fails)
    Fallback,   // Nivel 4 — SLM endpoint call
}

#[derive(Debug, Clone)]
pub struct ClassifyResult {
    pub intent: Intent,
    pub level: ClassifierLevel,
    pub confidence: f32,            // [0.0, 1.0]; 1.0 = deterministic
    pub matched_keyword: Option<String>,
}
```

## Cascade Pipeline

```
 User text
    │
    ▼
 Nivel 1: Heuristic (always)
   ├─ Empty?                   → Simple (conf=1.0)  ◀ RESOLVED
   ├─ Trivial greeting?        → Simple (conf=1.0)  ◀ RESOLVED
   ├─ Short (≤2 words, no ?)  → check keywords → Simple or pass
   ├─ Keyword match?           → Complex (conf=1.0)  ◀ RESOLVED
   └─ No match                → Simple (conf=0.0) → Nivel 2
    │
    ▼
 Nivel 2: Embedding (feature-gated, STUB)
   └─ Always skipped (cfg gate + empty deps) → Nivel 4
    │
    ▼
 Nivel 3: Logistic (feature-gated, STUB)
   └─ Always skipped (cfg gate + empty deps) → Nivel 4
    │
    ▼
 Nivel 4: SLM Fallback (optional, config-driven)
   ├─ Disabled                → ⚠ Safety bias: Complex (conf=0.0)
   ├─ HTTP timeout / error    → ⚠ Safety bias: Complex (conf=0.0)
   └─ Model responds          → Simple or Complex (conf=1.0)
```

**Safety bias:** if no stage resolves, the pipeline returns `Complex` with `confidence = 0.0`. This is deliberate — better to use full capabilities (tools, thinking) than miss a real task.

## Heuristic Level (Nivel 1)

```rust
pub fn classify(text: &str, complex_keywords: &[String]) -> ClassifyResult
```

**Algorithm (in order):**

1. **Empty text** → `Simple (conf=1.0)`.
2. **Trivial greetings** — cleaned text (lowered, alphanumeric/space) checked against hardcoded list: `hola`, `buenos días`, `buenas tardes`, `buenas noches`, `hey`, `ola`, `ok`, `vale`, `sí`, `si`, `no`, `gracias`, `adiós`, `adios`, `claro`, `chao`, `bye`. Matches if `cleaned == pat` or `cleaned.starts_with(pat)` followed by whitespace/punctuation. → `Simple (conf=1.0)`.
3. **Short phrases (≤ 2 words, no question mark)** → delegates to keyword matching. Returns `Simple (conf=0.0)` if keywords don't match.
4. **Keyword match** — case-insensitive substring search. First match wins → `Complex (conf=1.0)`.
5. **Default** → `Simple (conf=0.0)`, passes to next level.

### Default Complex Keywords (Spanish)

When `LLM_COMPLEX_KEYWORDS` is empty, these 23 keywords are used:
`investiga`, `lanza`, `ejecuta`, `analiza`, `crea`, `busca`, `abre`, `calcula`, `resume`, `resumen`, `traduce`, `compara`, `lee`, `escribe`, `instala`, `configura`, `diagnostica`, `muestra`, `lista`, `diseña`, `planifica`, `busca en`, `buscame`

Matching is **case-insensitive substring match** (e.g., `"Buscar información"` matches `busca`).

## Fallback Level (Nivel 4)

```rust
pub struct FallbackClassifier { /* reqwest client */ }
pub fn new(url: &str, model: &str, api_key: &str, timeout_ms: u64) -> Self;
pub async fn classify(&self, text: &str) -> Result<Intent>;
```

Calls the SLM endpoint at `{url}/v1/chat/completions` with:
- System prompt: `"You are an intent classifier. Respond with EXACTLY one token: \"SIMPLE\" for casual/greeting/trivial requests, or \"COMPLEX\" for requests requiring tools, reasoning, code, search, or multi-step tasks."`
- `max_tokens: 1`, `temperature: 0.0`, `stream: false`
- Auth: `Authorization: Bearer {api_key}` (omitted if key is empty)
- Timeout: `classifier_fallback_timeout_ms` (default 800 ms)

Response parsing: extracts `choices[0].message.content`, trims, lowercases. Starts with `"simple"` → `Simple`; starts with `"complex"` → `Complex`; anything else → error.

## Pipeline API

```rust
pub trait ClassifierStage: Send + Sync {
    fn name(&self) -> &'static str;
    fn level(&self) -> ClassifierLevel;
    async fn try_classify(&self, text: &str, shared_emb: &Arc<Mutex<Option<Vec<f32>>>>,
                          threshold: f32) -> Option<ClassifyResult>;
}

pub struct ClassifierPipeline {
    stages: Vec<Box<dyn ClassifierStage>>,
    threshold: f32,
}

impl ClassifierPipeline {
    pub fn new(threshold: f32) -> Self;
    pub fn with_stage(mut self, s: Box<dyn ClassifierStage>) -> Self;
    pub async fn classify(&self, text: &str) -> ClassifyResult;
}

/// Factory: builds the full pipeline from Config.
pub fn build_classifier(config: &Config) -> ClassifierPipeline;
```

**Builder pattern:**
```rust
let pipeline = ClassifierPipeline::new(config.classifier_confidence_threshold)
    .with_stage(Box::new(HeuristicStage::new(keywords)))
    .with_stage(Box::new(FallbackStage::new(fallback, true)));
let result = pipeline.classify(text).await;
```

## Environment Variables

| Variable | Config field | Default | Description |
|----------|-------------|---------|-------------|
| `LLM_COMPLEX_KEYWORDS` | `llm_complex_keywords` | `[]` (uses built-in defaults) | CSV of keywords marking a request as Complex |
| `CLASSIFIER_CONFIDENCE_THRESHOLD` | `classifier_confidence_threshold` | `0.6` | Confidence threshold [0.0–1.0] |
| `CLASSIFIER_ENABLE_EMBEDDING` | `classifier_enable_embedding` | `false` | Enable Niveles 2/3 (requires empty `classifier-embedding` feature — effectively dead) |
| `CLASSIFIER_ENABLE_FALLBACK` | `classifier_enable_fallback` | `false` | Enable Nivel 4 (SLM fallback endpoint) |
| `CLASSIFIER_FALLBACK_URL` | `classifier_fallback_url` | `""` | Base URL of SLM endpoint (`/v1/chat/completions` appended) |
| `CLASSIFIER_FALLBACK_MODEL` | `classifier_fallback_model` | `""` | Model name for SLM payload |
| `CLASSIFIER_FALLBACK_API_KEY` | `classifier_fallback_api_key` | `""` | Bearer token (empty = no auth) |
| `CLASSIFIER_FALLBACK_TIMEOUT_MS` | `classifier_fallback_timeout_ms` | `800` | HTTP timeout in milliseconds |

## Usage in Pipeline

The classifier is called before every LLM turn in `llm_task.rs`. Depending on result:
- `Simple` → thinking off, tools disabled (`tool_choice: "none"`), higher temperature
- `Complex` → thinking on, tools available, lower temperature
