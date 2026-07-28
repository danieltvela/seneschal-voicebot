# Sistema de Clasificación de Intenciones Multi-Capa (C01)

## Context
- Origin: Gitea issue #159 — Implementación del Sistema de Clasificación de Intenciones Multi-Capa (C01)
- Summary of what is requested: Implementar un clasificador de intenciones en **cascada** (4 niveles) que determine si una petición del usuario es `SIMPLE` o `COMPLEX` con latencia mínima, antes de invocar al LLM principal. El resultado dispara la configuración del modelo principal (Thinking ON/OFF, Temperatura, Tools Strict) según la Arquitectura C01 (issue #158, ya implementada con clasificador por keywords).
  - **Nivel 1 — Filtro heurístico** (≈0ms): reglas por longitud del enunciado + palabras clave (saludos/confirmaciones resueltos sin invocar modelos).
  - **Nivel 2 — Embeddings** (capa principal): modelo de embeddings ligero local (bge-small/gte-small ONNX) + similitud coseno contra centroides de frases de referencia por categoría. Objetivo: resolver ~90 % con latencia de pocos ms.
  - **Nivel 3 — Regresión logística** (refinamiento): capa lineal sobre el embedding del Nivel 2. Stub con pesos por defecto (neutros) cargados desde fichero; reentrenable después con datos reales.
  - **Nivel 4 — SLM fallback** (razonamiento semántico): invocación de un SLM (0.5B–3B) en un **segundo endpoint LLM dedicado** (vLLM) con `max_tokens=1`, `temperature=0` y prompt de un solo token (`SIMPLE`/`COMPLEX`). Se invoca solo si la confianza de los niveles previos es baja.
  - **Sesgo de seguridad**: en ambigüedad extrema o fallo del clasificador → `COMPLEX`.
  - **Trazabilidad**: registro de cada decisión (input → nivel de resolución → categoría → confianza) en SQLite, para ajuste de umbrales y reentrenamiento.
- Proposed branch: `feature/issue-159-implementaci-n-del-sistema-de-clasificaci`
- Base branch: master
- Assumptions made (confirmadas con el usuario):
  1. **Alcance**: Cascada completa de 4 niveles en esta implementación.
  2. **SLM fallback**: segundo endpoint LLM dedicado configurado por nuevas env vars (`CLASSIFIER_FALLBACK_URL`, `CLASSIFIER_FALLBACK_MODEL`, `CLASSIFIER_FALLBACK_API_KEY`, `CLASSIFIER_FALLBACK_TIMEOUT_MS`). Reutiliza `reqwest` (sin nuevas dependencias) con un payload OpenAI-compatible; la "salida forzada a un token" se logra con `max_tokens=1` + `temperature=0` + prompt de sistema estricto. El `logit_bias`/gramática queda como TODO avanzado documentado.
  3. **Modelo de embeddings**: gestión mediante script de descarga + path en config. Script `scripts/download-embedding-model.sh` que descarga `bge-small-en-v1.5` (ONNX, HuggingFace `optimum`/`Qdrant` mirror) a `models/bge-small-onnx/`. Si el modelo no está presente en runtime, el Nivel 2 se **skipa** y la cascada cae al siguiente nivel (graceful), logueando un `warn!`. Tests que lo requieren se marcan `#[ignore]` y se skipan si no hay modelo.
  4. **Nivel 3 (regresión logística)**: stub con pesos por defecto cargados desde `models/classifier_weights.json` (shippeado, pesos neutros → sigmoid 0.5). Necesita el embedding del Nivel 2 (si Nivel 2 no corre, Nivel 3 se skipa).
  5. **Niveles 2 y 3 bajo feature flag `classifier-embedding`** (opt-in, `Cargo.toml`), porque añaden dependencias pesadas (`ort`, `tokenizers`, `ndarray`). Sin el flag, la cascada usa Niveles 1 y 4 únicamente. El build por defecto y el CI `make qa` base NO activan el flag; los tests del Nivel 2/3 son `#[cfg(feature = "classifier-embedding")]` + `#[ignore]`.
  6. **"Tools Strict"**: se interpreta conservadoramente como un flag `LLM_TOOLS_STRICT`. Cuando está activo y `intent == Simple`, el payload envía `tool_choice: "none"` explícito (en lugar de omitir tools). En `Complex` se mantiene `auto`/`required` (comportamiento actual). Default `false` (no rompe el comportamiento existente).
  7. **Confianza**: Nivel 1 keyword-match → `1.0`; sin match → `0.0` (continúa). Nivel 2 → `max(cos_sim)` contra centroides. Nivel 3 → `σ(w·x + b)` (probabilidad). Umbral configurable `CLASSIFIER_CONFIDENCE_THRESHOLD` (default `0.6`); confianza ≥ umbral resuelve; menor continúa al siguiente nivel.
  8. **Naming**: el proyecto se llama **Seneschal** (ver `AGENTS.md`); no usar marca de terceros en logs/UI. El codename "C01" solo informativo.
  9. **No tocar** `complete`, `complete_short`, `complete_multimodal`, el sistema de Tools Asíncronas, el Dream/consolidación ni la lógica de `forced_tool` (comportamiento actual preservado). Solo el path de `stream()` del pipeline conversacional y el nuevo clasificador.

### Puntos de inserción confirmados por exploración del código
- Clasificador actual (única fuente): `src/classifier/mod.rs` — `pub fn classify(text, complex_keywords) -> ClassifyResult` (keyword only). Único caller en `src/pipeline/llm_task.rs:106`.
- `Intent` enum en `src/classifier/mod.rs:3-7`. `ClassifyResult { intent, matched_keyword }` en `:42-45`.
- Integración en `llm_task.rs`: bloque `:100-118` (clasificación + `RequestOptions` + `tools_enabled`). Las palabras clave se resuelven fuera del loop en `:51-58`.
- `RequestOptions { temperature, thinking, enable_tools }` en `src/llm/provider.rs:16-40` ( Copy, builders `with_temperature`/`with_thinking`). No hay `tool_choice`.
- Payload de `stream()` en `src/llm/client.rs:236-263`: rama `!tools.is_empty()` pone `tool_choice = "required"|"auto"`; rama `tools.is_empty()` pone `chat_template_kwargs`. **Sin** rama para `tool_choice="none"`.
- `LlmProvider::stream()` trait en `src/llm/provider.rs:59-65`. Único impl `OpenAiLlmProvider`.
- Spawn de `llm_task` en `src/main.rs:1077-1131` (args LLM en `:1092-1096`, llamada en `:1101-1128`). Llamada espejo en tests e2e `src/e2e_tests.rs:222-251` (args en `:240-244`).
- DB: `src/db/database.rs`, `run_migrations()` en `:69-262` (migraciones inline `CREATE TABLE IF NOT EXISTS` + `ALTER TABLE` aditivas). No hay carpeta `migrations/`. Patrón: tabla + índice + métodos `save_*`/`get_*` sobre el pool.
- Config: `src/config.rs` bloque `// ── LLM ──` en `:146-175`. Env overrides en `apply_env_overrides()` (referencias: `LLM_TEMPERATURE_SIMPLE` en `:611`, `LLM_COMPLEX_KEYWORDS` en `:735`). Defaults en `seneschal.pro.toml:77-83` y `seneschal.dev.toml:76-80`.
- Dependencias: `Cargo.toml:9-54` (sección `[dependencies]`), features en `:133-141`, dev-deps en `:157-163`. **No** hay `ort`, `tokenizers`, `ndarray`, `fastembed`. Sí `reqwest`, `async-trait`, `serde_json`.
- TUI: `TuiEvent::IntentClassified { intent: String }` en `src/tui/events.rs:40`. Emitido en `llm_task.rs:135-141`. Renderizado (ignora `intent`) en `src/tui/app.rs:291`.
- Tests e2e: `src/e2e_tests.rs` con `wiremock` (MockServer en `:29`), patrón SSE helper `make_sse` en `:44-59`.

## Phase 1: Refactor del módulo `src/classifier/` a arquitectura de cascada (sin nuevas dependencias)
- [ ] Step 1.1: Estructurar submódulos del clasificador
  - File(s): `src/classifier/mod.rs` (refactor), `src/classifier/keyword.rs` (nuevo), `src/classifier/heuristic.rs` (nuevo)
  - Change:
    1. Mantener en `src/classifier/mod.rs` los tipos públicos y **añadir** campos:
       ```rust
       #[derive(Debug, Clone, Copy, PartialEq, Eq)]
       pub enum Intent { Simple, Complex }

       #[derive(Debug, Clone, Copy, PartialEq, Eq)]
       pub enum ClassifierLevel { Heuristic, Embedding, Logistic, Fallback }

       #[derive(Debug, Clone)]
       pub struct ClassifyResult {
           pub intent: Intent,
           /// Nivel que resolvió la decisión (Heuristic/Embedding/Logistic/Fallback).
           pub level: ClassifierLevel,
           /// Confianza [0.0, 1.0]. 1.0 = match keyword determinista.
           pub confidence: f32,
           /// Primera keyword matched (para logs), si la hubo.
           pub matched_keyword: Option<String>,
       }
       impl ClassifyResult { pub fn into_parts(self) -> (Intent, Option<String>) { (self.intent, self.matched_keyword) } }
       pub const DEFAULT_COMPLEX_KEYWORDS: &[&str] = [...] /* MOVER la lista actual aquí, sin cambios */;
       ```
    2. Mover la función `classify` (keyword substring) a `src/classifier/keyword.rs`:
       ```rust
       pub fn classify(text: &str, complex_keywords: &[String]) -> super::ClassifyResult
       ```
       devolviendo `ClassifyResult { intent, level: ClassifierLevel::Heuristic, confidence: 1.0 si match else 0.0, matched_keyword }`.
    3. Crear `src/classifier/heuristic.rs` con el **Nivel 1** completo (heurística pura, sin I/O, sin Tokio):
       - `pub fn classify(text: &str, complex_keywords: &[String]) -> super::ClassifyResult`
       - Reglas (en este orden, primera que match termina):
         (a) **Longitud/longitud mínima**: trimmed text. Si `text.trim().is_empty()`, devolver `Simple, confidence 1.0, matched None`. (b) **Saludos/confirmaciones triviales**: lista constante `TRIVIAL_PATTERNS: &[&str] = ["hola","buenos días","buenas tardes","buenas noches","hey","olá","ok","vale","sí","no","gracias","adiós","claro"];`. Si el texto trim lowercased, tras quitar signos, es `==` alguno o termina en uno de estos seguido de signo → `Simple, confidence 1.0, matched Some("<patrón>")`. Usar `to_lowercase()` + `trim_end_matches(|c: char| !c.is_alphanumeric())`. (c) **Keywords**: delegar en `super::keyword::classify(text, complex_keywords)`; si match → devolver tal cual (`Complex, confidence 1.0`). Si no match → devolver `ClassifyResult { intent: Simple, level: Heuristic, confidence: 0.0, matched_keyword: None }` (0.0 = "no resuelto" → la cascada continúa al siguiente nivel).
    4. Añadir en `src/classifier/mod.rs` un wrapper de compatibilidad temporal `pub fn classify(text: &str, complex_keywords: &[String]) -> ClassifyResult` que delegue en `heuristic::classify`. **Mantiene el API público** para no romper `llm_task.rs` en este paso. Usar `#[allow(dead_code)]` donde el compilador marque los nuevos items hasta su uso en fases posteriores.
  - Acceptance criteria: `cargo build` compila. `cargo test classifier` pasa (tests existentes de `keyword` siguen en verde; adaptarlos a `ClassifyResult`). Añadir en `heuristic.rs` tests: saludo trivial → Simple conf 1.0; "ok" → Simple; "Investiga X" → Complex; "" → Simple; frase larga sin keyword → Simple conf 0.0.

- [ ] Step 1.2: Declarar el submódulo
  - File(s): `src/lib.rs` (sin cambios en `pub mod classifier;` que ya existe), `src/classifier/mod.rs`
  - Change: En `src/classifier/mod.rs` añadir `pub mod heuristic; pub mod keyword;` (alfabético). Los archivos `heuristic.rs`/`keyword.rs` ya exportan `classify` como `pub`.
  - Acceptance criteria: `cargo build` compila. `cargo test` pasa.

## Phase 2: Configuración del clasificador + trazabilidad DB
- [ ] Step 2.1: Añadir campos de config del clasificador
  - File(s): `src/config.rs`
  - Change: Tras el campo `pub llm_complex_keywords: Vec<String>;` (línea 175) añadir:
    ```rust
    // ── Intent Classifier (cascada C01) ───────────────────────────────
    /// Umbral de confianza [0.0,1.0]: si un nivel baja de éste, la cascada continúa.
    pub classifier_confidence_threshold: f32,            // CLASSIFIER_CONFIDENCE_THRESHOLD, default 0.6
    /// Activar Nivel 2/3 (embeddings) — requiere feature `classifier-embedding`.
    pub classifier_enable_embedding: bool,                // CLASSIFIER_ENABLE_EMBEDDING, default false
    /// Ruta al modelo de embeddings ONNX (dir o .onnx). Vacío → skip Nivel 2.
    pub classifier_model_path: String,                    // CLASSIFIER_MODEL_PATH
    /// Ruta al fichero JSON con centroides de referencia por categoría.
    pub classifier_centroids_path: String,                // CLASSIFIER_CENTROIDS_PATH
    /// Ruta al fichero JSON con pesos de la regresión logística (Nivel 3).
    pub classifier_weights_path: String,                  // CLASSIFIER_WEIGHTS_PATH
    /// Activar Nivel 4 (SLM fallback). Si false, la cascada termina en el último nivel disponible.
    pub classifier_enable_fallback: bool,                 // CLASSIFIER_ENABLE_FALLBACK, default false
    /// URL base del segundo endpoint LLM (SLM en vLLM) para el fallback.
    pub classifier_fallback_url: String,                  // CLASSIFIER_FALLBACK_URL, default ""
    /// Modelo del SLM fallback (campo `model` del payload).
    pub classifier_fallback_model: String,                // CLASSIFIER_FALLBACK_MODEL
    /// API key del SLM fallback (Bearer; vacío = sin auth).
    pub classifier_fallback_api_key: String,              // CLASSIFIER_FALLBACK_API_KEY
    /// Timeout en ms del fallback SLM (lo más corto posible).
    pub classifier_fallback_timeout_ms: u64,             // CLASSIFIER_FALLBACK_TIMEOUT_MS, default 800
    /// "Tools Strict": en SIMPLE force `tool_choice: "none"` explícito en el payload.
    pub llm_tools_strict: bool,                           // LLM_TOOLS_STRICT, default false
    ```
    Default de `Config::default()`/TOML embebido: ver Step 2.2.
  - Acceptance criteria: `cargo build` falla hasta Step 2.2 (campos sin inicializar en TOML). Esperado.

- [ ] Step 2.2: Defaults en TOML de entorno
  - File(s): `seneschal.pro.toml`, `seneschal.dev.toml`
  - Change: Tras `llm_complex_keywords = []` añadir (idéntico en ambos salvo nota):
    ```toml
    classifier_confidence_threshold = 0.6
    classifier_enable_embedding = false
    classifier_model_path = ""
    classifier_centroids_path = ""
    classifier_weights_path = ""
    classifier_enable_fallback = false
    classifier_fallback_url = ""
    classifier_fallback_model = ""
    classifier_fallback_api_key = ""
    classifier_fallback_timeout_ms = 800
    llm_tools_strict = false
    ```
    Sin diferencias entre `pro` y `dev` en estos campos.
  - Acceptance criteria: `cargo build` compila (todos los campos cubiertos por deserialize del TOML embebido). `cargo run -- --help` no rompe.

- [ ] Step 2.3: Cargar overrides por env var
  - File(s): `src/config.rs`
  - Change: En `apply_env_overrides()`, junto al bloque de `LLM_COMPLEX_KEYWORDS` (alrededor de `:735`), añadir:
    ```rust
    if let Ok(v) = env::var("CLASSIFIER_CONFIDENCE_THRESHOLD") {
        self.classifier_confidence_threshold = v.parse().context("Invalid CLASSIFIER_CONFIDENCE_THRESHOLD")?;
    }
    if let Ok(v) = env::var("CLASSIFIER_ENABLE_EMBEDDING") {
        self.classifier_enable_embedding = v == "1" || v.to_lowercase() == "true";
    }
    if let Ok(v) = env::var("CLASSIFIER_MODEL_PATH") { self.classifier_model_path = v; }
    if let Ok(v) = env::var("CLASSIFIER_CENTROIDS_PATH") { self.classifier_centroids_path = v; }
    if let Ok(v) = env::var("CLASSIFIER_WEIGHTS_PATH") { self.classifier_weights_path = v; }
    if let Ok(v) = env::var("CLASSIFIER_ENABLE_FALLBACK") {
        self.classifier_enable_fallback = v == "1" || v.to_lowercase() == "true";
    }
    if let Ok(v) = env::var("CLASSIFIER_FALLBACK_URL") { self.classifier_fallback_url = v; }
    if let Ok(v) = env::var("CLASSIFIER_FALLBACK_MODEL") { self.classifier_fallback_model = v; }
    if let Ok(v) = env::var("CLASSIFIER_FALLBACK_API_KEY") { self.classifier_fallback_api_key = v; }
    if let Ok(v) = env::var("CLASSIFIER_FALLBACK_TIMEOUT_MS") {
        self.classifier_fallback_timeout_ms = v.parse().context("Invalid CLASSIFIER_FALLBACK_TIMEOUT_MS")?;
    }
    if let Ok(v) = env::var("LLM_TOOLS_STRICT") {
        self.llm_tools_strict = v == "1" || v.to_lowercase() == "true";
    }
    ```
  - Acceptance criteria: `cargo test` pasa. Añadir un test `config.rs` que fija `LLM_TOOLS_STRICT=1` y `CLASSIFIER_CONFIDENCE_THRESHOLD=0.8` y verifica los campos (usando `temp-env` como hacen los tests existentes).

- [ ] Step 2.4: Documentar variables en `doc/env-vars.md`
  - File(s): `doc/env-vars.md`
  - Change: Tras las filas existentes de `LLM_*` (cerca de `:27-31`) añadir una sección "Intent Classifier (cascada C01)" con una fila por cada nueva env var (name, default, descripción), según los defaults del Step 2.2.
  - Acceptance criteria: `cargo test` sigue pasando. La doc refleja defaults reales del TOML.

- [ ] Step 2.5: Migración DB de trazabilidad `classification_log`
  - File(s): `src/db/database.rs`
  - Change:
    1. En `run_migrations()`, tras el bloque FTS5/triggers (justo antes de `Ok(())` en `:261`) añadir:
       ```rust
       sqlx::query(
           "CREATE TABLE IF NOT EXISTS classification_log (
               id              INTEGER PRIMARY KEY AUTOINCREMENT,
               session_id      TEXT NOT NULL,
               utterance_id    INTEGER NOT NULL,
               transcript      TEXT NOT NULL,
               intent          TEXT NOT NULL,
               confidence      REAL NOT NULL,
               level           TEXT NOT NULL,
               classifier      TEXT NOT NULL,
               matched_keyword TEXT
           )",
       ).execute(&self.pool).await?;
       sqlx::query(
           "CREATE INDEX IF NOT EXISTS idx_classification_log_session
            ON classification_log(session_id)",
       ).execute(&self.pool).await?;
       ```
    2. Añadir método `pub async fn save_classification(&self, ...) -> Result<i64>` que haga `INSERT INTO classification_log (...) VALUES (?,?,?,?,?,?,?,?)` (intent como `"SIMPLE"/"COMPLEX"`, level como nombre del enum) y devuelva el id. Considerar `utterance_id` como `i64`.
  - Acceptance criteria: `cargo build` compila. Un test `database.rs` (patrón temp dir + `Database::new`) crea una sesión, llama `save_classification`, y verifica la fila presente con un `SELECT`.

## Phase 3: Cascada dispatcher (`ClassifierPipeline`) + integración en el pipeline
- [ ] Step 3.1: Definir trait `ClassifierStage` y enum `ClassifierPipeline`
  - File(s): `src/classifier/pipeline.rs` (nuevo), `src/classifier/mod.rs`
  - Change:
    1. En `mod.rs` añadir `pub mod pipeline;` y re-exportar `pub use pipeline::ClassifierPipeline;`.
    2. En `src/classifier/pipeline.rs` definir:
       ```rust
       use async_trait::async_trait;
       use super::{ClassifyResult, Intent, ClassifierLevel};

       #[async_trait]
       pub trait ClassifierStage: Send + Sync {
           fn name(&self) -> &'static str;
           fn level(&self) -> ClassifierLevel;
           /// `Some(...)` si este nivel resuelve (confianza ≥ umbral); `None` si debe continuar la cascada.
           async fn try_classify(&self, text: &str, threshold: f32) -> Option<ClassifyResult>;
       }

       pub struct ClassifierPipeline {
           stages: Vec<Box<dyn ClassifierStage>>,
           threshold: f32,
       }
       impl ClassifierPipeline {
           pub fn new(threshold: f32) -> Self { Self { stages: Vec::new(), threshold } }
           pub fn with_stage(mut self, s: Box<dyn ClassifierStage>) -> Self { self.stages.push(s); self }
           /// Recorre niveles en orden. El primero que devuelve `Some` resuelve.
           /// Si ninguno resuelve → `Complex` (sesgo de seguridad) con level del último intentado y confidence 0.0.
           pub async fn classify(&self, text: &str) -> ClassifyResult {
               let mut last: Option<ClassifyResult> = None;
               for s in &self.stages {
                   if let Some(r) = s.try_classify(text, self.threshold).await { return r; }
                   last = s.try_classify(text, 1.0).await; // siempre ejecuta para trazabilidad (conf threshold alta=forzar return del nivel)
               }
               last.unwrap_or(ClassifyResult { intent: Intent::Complex, level: ClassifierLevel::Fallback, confidence: 0.0, matched_keyword: None })
           }
       }
       ```
       Nota: el segundo `try_classify(text, 1.0)` tras el `continue` garantiza que `last` refleje la salida bruta del último nivel para trazabilidad. Aclarar con comentario.
    3. NOTA: el `Box<dyn ClassifierStage>` con `async_trait` exige que el tipo sea `Send`. Los stages de Nivel 2 (ort) y Nivel 4 (reqwest) lo son.
  - Acceptance criteria: `cargo build` compila. Test `classifier::pipeline` con dos stages mock (uno siempre `None`, uno siempre `Some(Simple)`) verifica orden y que sin resolución → `Complex`.

- [ ] Step 3.2: Stage del Nivel 1 (Heuristic) como `ClassifierStage`
  - File(s): `src/classifier/heuristic.rs`
  - Change: Añadir un `pub struct HeuristicStage { keywords: Vec<String> }` con `impl HeuristicStage { pub fn new(keywords: Vec<String>) -> Self }` y `#[async_trait] impl ClassifierStage for HeuristicStage` donde `name()="heuristic"`, `level()=Heuristic`, y `try_classify` llama a `classify(text, &self.keywords)` y devuelve `Some(r)` si `r.confidence >= threshold` (1.0 siempre), `None` si `confidence == 0.0` (no resuelto). Importar `use async_trait::async_trait; use super::pipeline::ClassifierStage;`.
  - Acceptance criteria: `cargo build` compila. Test: con `threshold=0.6`, `"hola"` → `Some` Simple; `"dime algo largo sin keyword"` → `None`.

- [ ] Step 3.3: Factory `build_classifier` desde `Config`
  - File(s): `src/classifier/pipeline.rs`, `src/classifier/mod.rs`
  - Change: Añadir:
    ```rust
    pub fn build_classifier(config: &crate::config::Config) -> ClassifierPipeline {
        let resolved_keywords: Vec<String> = if config.llm_complex_keywords.is_empty() {
            super::DEFAULT_COMPLEX_KEYWORDS.iter().map(|s| s.to_string()).collect()
        } else { config.llm_complex_keywords.clone() };
        let mut p = ClassifierPipeline::new(config.classifier_confidence_threshold);
        p = p.with_stage(Box::new(super::heuristic::HeuristicStage::new(resolved_keywords)));
        #[cfg(feature = "classifier-embedding")]
        if config.classifier_enable_embedding {
            // Añadido en Phase 5 (embedding) y Phase 6 (logistic).
        }
        // Nivel 4 (fallback) añadido en Phase 7.
        p
    }
    ```
    Dejar TODOs comentados para las fases 5-7. `pub use pipeline::build_classifier;` en `mod.rs`.
  - Acceptance criteria: `cargo build` (default features) compila con solo el stage heurístico.

- [ ] Step 3.4: Integrar `ClassifierPipeline` en `llm_task`
  - File(s): `src/pipeline/llm_task.rs`, `src/main.rs`, `src/e2e_tests.rs`
  - Change:
    1. Firma de `llm_task`: **sustituir** los args `llm_temperature_simple/complex, llm_thinking_simple/complex, llm_complex_keywords` por:
       ```rust
       classifier: Arc<crate::classifier::ClassifierPipeline>,
       llm_temperature_simple: f32, llm_temperature_complex: f32,
       llm_thinking_simple: bool, llm_thinking_complex: bool,
       llm_tools_strict: bool,
       ```
       (mantiene los temperature/thinking porque `RequestOptions` se construye aquí; `llm_complex_keywords` ya no hace falta — vive dentro del `HeuristicStage`.) Eliminar el bloque de resolución de `resolved_keywords` (`:51-58`).
    2. Reemplazar el bloque de clasificación `:100-118` por:
       ```rust
       let is_complex_turn = tool_continuation || is_system_notification;
       let result = if is_complex_turn {
           crate::classifier::ClassifyResult { intent: Intent::Complex, level: crate::classifier::ClassifierLevel::Heuristic, confidence: 1.0, matched_keyword: None }
       } else {
           classifier.classify(&text).await
       };
       let intent = result.intent;
       let matched_kw = result.matched_keyword.clone();
       let options = match intent {
           Intent::Simple => RequestOptions::new().with_temperature(llm_temperature_simple).with_thinking(llm_thinking_simple),
           Intent::Complex => RequestOptions::new().with_temperature(llm_temperature_complex).with_thinking(llm_thinking_complex),
       };
       let tools_enabled = matches!(intent, Intent::Complex);
       info!(target: "classifier", "[pipe={}] intent={:?} level={:?} confidence={:.2} matched={:?} tools={} strict={}",
           pipeline_id, intent, result.level, result.confidence, matched_kw, tools_enabled, llm_tools_strict);
       ```
       Importar `use crate::classifier::{ClassifierPipeline, Intent, ClassifierLevel, ClassifyResult};`.
    3. En `src/main.rs`: construir `let classifier = Arc::new(crate::classifier::build_classifier(&config));` antes del spawn (en el bloque `:1077`), pasar `Arc::clone(&classifier)` y `config.llm_tools_strict` en la llamada `:1101-1128` (sustituyendo `llm_kw` por el `classifier`).
    4. En `src/e2e_tests.rs:222-251`: sustituir args actuales por `Arc::new(crate::classifier::build_classifier(&config))` (usar `config` del test; si el test no tiene `config` accesible, construir un `ClassifierPipeline::new(0.6).with_stage(Box::new(HeuristicStage::new(vec![])))` directamente) y el flag `false` (llm_tools_strict). **No** romper la firma en los tests: pasar 5 args (classifier + 4 escalares/flag) en el nuevo orden.
  - Acceptance criteria: `cargo build` compila (default features). `cargo test` pasa. `cargo build --features tui,remote,control` compila (test-ci). Logs del e2e muestran `level=Heuristic`.

- [ ] Step 3.5: Ampliar trazabilidad DB en el pipeline
  - File(s): `src/pipeline/llm_task.rs`, `src/main.rs`, `src/e2e_tests.rs`
  - Change:
    1. En `llm_task`, tras el `info!` de clasificación (Step 3.4) añadir (no awaiting de forma bloqueante: fire-and-forget con `tokio::spawn` O `let _ =` directo, ya que `db` es `Clone`). Construir `utterance_id` como `pipeline_id as i64` (si no hay mejor id del turno, usar ese). Insertar:
       ```rust
       let _ = db.save_classification(
           &session_id.to_string(),
           pipeline_id as i64,
           &text,
           match intent { Intent::Simple => "SIMPLE", Intent::Complex => "COMPLEX" },
           result.confidence,
           format!("{:?}", result.level).as_str(), // "Heuristic" | "Embedding" | "Logistic" | "Fallback"
           "cascade",
           matched_kw.as_deref(),
       ).await;
       ```
       Importar `use crate::db::Database;` (ya existe). Manejar el error con `if let Err(e)=... { warn!(...) }` para no romper el turno si la DB falla.
    2. No requiere cambios en el signature (ya recibe `db` y `session_id`).
  - Acceptance criteria: `cargo test e2e -- --ignored` (test `basic_conversation` u otro que inyecte transcripción) deja filas en `classification_log` (verificar con un `SELECT` en el test o manual). En la no-opt no se testea, basta compilar.

## Phase 4: "Tools Strict" en `RequestOptions` y payload
- [ ] Step 4.1: Añadir `tool_choice` a `RequestOptions`
  - File(s): `src/llm/provider.rs`
  - Change:
    1. Añadir enum:
       ```rust
       #[derive(Debug, Clone, Copy, PartialEq, Eq)]
       pub enum ToolChoice { Auto, Required, None }
       ```
    2. En `RequestOptions` añadir `pub tool_choice: Option<ToolChoice>;` y builder `pub fn with_tool_choice(mut self, c: ToolChoice) -> Self { self.tool_choice = Some(c); self }`. Actualizar `Default` (deriva sigue ok, `Option::None`).
    3. Re-exportar `ToolChoice` desde `src/llm/mod.rs` (`pub use provider::ToolChoice;`).
  - Acceptance criteria: `cargo build` falla solo si `client.rs` no lo consume (esperado hasta 4.2).

- [ ] Step 4.2: Honrar `tool_choice` en `OpenAIClient::stream()`
  - File(s): `src/llm/client.rs`
  - Change: En el payload `:236-263`:
    - Mantener la rama `!tools.is_empty()` actual pero, al calcular `tool_choice`, calcular primero `let choice = options.tool_choice.unwrap_or(match forced_tool { Some(_) => ToolChoice::Required, None => ToolChoice::Auto });` y serializar como `"required" | "auto" | "none"`. Cuando `choice == ToolChoice::None`, aunque `tools` no esté vacío, **omitir** el campo `tools` y `tool_choice` (igual que la rama `tools.is_empty()`): usar `else` con `chat_template_kwargs`. En la práctica, la mecánica de `llm_task` pasa `&[]` en SIMPLE, por lo que la rama vacía ya cobra; el `tool_choice: None` explícito se agrega **solo si** se envían tools (caso defensa). Cubrir también el caso `tools.is_empty() && options.tool_choice == Some(ToolChoice::None)` añadiendo `"tool_choice": "none"` al payload (en la rama else) **únicamente cuando `llm_tools_strict` lo pida** — pero el flag vive en `llm_task`; para no pasar el flag por aquí, se interpreta así: si `options.tool_choice == Some(ToolChoice::None)` → añadir `"tool_choice":"none"` al payload (en la rama `tools.is_empty()` también).
    - Implementar un helper local `fn tc_string(c: ToolChoice) -> &'static str`.
    - No romper `build_stream_payload` (`#[cfg(test)]`) existente: sincronizar el cambio ahí.
  - Acceptance criteria: `cargo build` compila. Test `client.rs` existente `build_stream_payload` se actualiza; nuevo test: con `RequestOptions::new().with_tool_choice(ToolChoice::None)` y `tools=&[]`, el payload contiene `"tool_choice":"none"`.

- [ ] Step 4.3: Aplicar `tools_strict` desde `llm_task`
  - File(s): `src/pipeline/llm_task.rs`
  - Change: En la construcción de `options` (Step 3.4), cuando `llm_tools_strict && !tools_enabled` (Simple), añadir `.with_tool_choice(crate::llm::ToolChoice::None)`:
    ```rust
    let mut options = match intent { Simple => ..., Complex => ... };
    if llm_tools_strict && !tools_enabled { options = options.with_tool_choice(crate::llm::ToolChoice::None); }
    ```
  - Acceptance criteria: `cargo build` compila. Test e2e/inject: con `llm_tools_strict=true` y un saludo (Simple), el payload enviado al wiremock incluye `"tool_choice":"none"`.

## Limit point — Checkpoint QA parcial
> Antes de continuar con los Niveles 2/3/4 (que requieren dependencias pesadas y modelos externos), ejecutar `make qa` y verificar verde. Esta es la línea base funcional (cascada con solo Nivel 1 + trazabilidad + tools_strict). Commit "verify: cascada Nivel 1 + DBQA" antes de Phase 5.

## Phase 5: Nivel 2 — Embeddings (feature flag `classifier-embedding`)
- [ ] Step 5.1: Añadir dependencias gated y feature
  - File(s): `Cargo.toml`
  - Change:
    1. En `[dependencies]` añadir (todos optional):
       ```toml
       ort = { version = "2", optional = true }
       tokenizers = { version = "0.21", default-features = false, features = ["onig"], optional = true }
       ndarray = { version = "0.16", optional = true }
       ```
       (Versiones a fijar a las que resuelva `cargo update`; si `ort 2` trae problemas de build en macOS, valorar `ort = { version = "2", features = ["download-binaries"] }`.)
    2. En `[features]` añadir: `classifier-embedding = ["dep:ort", "dep:tokenizers", "dep:ndarray"]`.
    3. **No** añadir el feature a los perfiles de CI por defecto (`make qa` no lo activa); se testea con `cargo test --features classifier-embedding ...` solo en pasos explícitos del plan.
  - Acceptance criteria: `cargo build` (default) sigue compilando. `cargo build --features classifier-embedding` descarga/compila `ort` (puede tardar la primera vez).

- [ ] Step 5.2: Script de descarga del modelo de embeddings
  - File(s): `scripts/download-embedding-model.sh` (nuevo), `models/bge-small-onnx/.gitignore` (opcional)
  - Change: Script bash idempotente que:
    1. Crea `models/bge-small-onnx/` si no existe.
    2. Descarga desde HuggingFace (URL estable: `https://huggingface.co/Qdrant/all-MiniLM-L6-v2-onnx/resolve/main/onnx/model.onnx` o, preferida, `https://huggingface.co/bge-small-en-v1.5` packaged como ONNX — si no resuelve, usar `all-MiniLM-L6-v2` que sí tiene mirror ONNX estable) los ficheros `model.onnx` y `tokenizer.json` y `config.json` (necesarios para `ort`+`tokenizers`).
    3. Verifica tamaño > 0; si falla la descarga, sale con error y mensaje claro.
    4. `chmod +x` recomendado en doc.
    Usar `curl -fsSL` y `set -euo pipefail`. Documentar al inicio del script qué descarga y dónde.
  - Acceptance criteria: Ejecutar `bash scripts/download-embedding-model.sh` deja `models/bge-small-onnx/{model.onnx,tokenizer.json}`. Re-ejecutar es no-op si los ficheros ya existen. Si no hay red, el script falla de forma clara (no corrompe rien).

- [ ] Step 5.3: Stage `EmbeddingClassifier`
  - File(s): `src/classifier/embedding.rs` (nuevo, todo `#[cfg(feature = "classifier-embedding")]`), `src/classifier/mod.rs`, `src/classifier/pipeline.rs`
  - Change:
    1. En `mod.rs` añadir `#[cfg(feature = "classifier-embedding")] pub mod embedding;`.
    2. En `embedding.rs`:
       - `pub struct EmbeddingClassifier { session: ort::session::Session, tokenizer: tokenizers::Tokenizer, centroids: Centroids, dim: usize }` donde `Centroids { simple: Vec<f32>, complex: Vec<f32> }` deserializable de JSON `{ "simple":[...], "complex":[...] }`.
       - `impl EmbeddingClassifier { pub fn load(model_path: &str, centroids_path: &str) -> anyhow::Result<Self> }`: carga el `Session::builder()?.commit_from_file(model_path)?`, `Tokenizer::from_file(tokenizer_path)?`, y los centroides desde `centroids_path` (si el fichero no existe, error claro).
       - `fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>>`: tokeniza (`self.tokenizer.encode(text, true)?`), construye `inputs` (input_ids + attention_mask como `ndarray`), corre la sesión, extrae el embedding (mean-pool sobre la secuencia si el modelo lo pide — para MiniLM/bge el output ya es pooled; usar el primer/único output vector). Normaliza a longitud unitaria (L2).
       - `fn cosine(a: &[f32], b: &[f32]) -> f32`: suma de productos (vectores ya normalizados) clampada a [0,1].
       - `#[async_trait] impl ClassifierStage for EmbeddingClassifier`: `name()="embedding"`, `level()=Embedding`, `try_classify(text, threshold)` → calcula `e = embed(text)`, `sim_simple = cosine(e, centroids.simple)`, `sim_complex = cosine(e, centroids.complex)`. Si `max(sim_simple, sim_complex) >= threshold` → `Some(ClassifyResult { intent: (sim_complex > sim_simple ? Complex : Simple), level: Embedding, confidence: max, matched_keyword: None })`; si no, `Some(result con confianza baja)`? No: el método debe devolver `Some` solo si conf≥threshold; si no, `None` (la cascada continúa). Pero para trazabilidad `pipeline.classify` necesita la salida bruta del último nivel → devolver vía `last` (ver Step 3.1, ya cubierto). Manejar errores con `?`-style: si `embed` falla → `warn!` y `None`.
    3. En `build_classifier` (Phase 3.3) dentro del bloque `#[cfg(feature = "classifier-embedding")] if config.classifier_enable_embedding { ... }`:
       ```rust
       if !config.classifier_model_path.is_empty() && !config.classifier_centroids_path.is_empty() {
           match super::embedding::EmbeddingClassifier::load(&config.classifier_model_path, &config.classifier_centroids_path) {
               Ok(e) => { p = p.with_stage(Box::new(e)); }
               Err(err) => warn!(target: "classifier", "Nivel 2 disabled: {err}"),
           }
       } else { warn!(target: "classifier", "Nivel 2 disabled: CLASSIFIER_MODEL_PATH/CENTROIDS_PATH vacíos"); }
       ```
  - Acceptance criteria: `cargo build --features classifier-embedding` compila. Tests `#[cfg(feature="classifier-embedding")] #[ignore]` en `embedding.rs`: si `models/bge-small-onnx/` existe (tras correr el script), `embed("Hola")` devuelve un `Vec<f32>` de la dimensión esperada y `cosine` ∈ [0,1]; si no existe el modelo, los tests se skipan (`#[ignore]`). Sin el feature, `cargo build` no incluye `ort`/`tokenizers` (`cargo build` default sigue verde).

- [ ] Step 5.4: Centroides por defecto shippeados
  - File(s): `models/classifier_centroids.json` (nuevo)
  - Change: JSON con dos vectores de la dimensión del modelo elegido (p.ej. MiniLM L6 → 384 dims) rellenos con `0.0` (placeholder). Documentar: el usuario debe regenerarlos con frases de referencia reales (script futuro fuera de scope). El placeholder produce `cosine≈0` → confianza baja → cae a Nivel 3/4, no rompe.
    Formato: `{ "dim": 384, "simple": [0.0,...], "complex": [0.0,...] }`. Al cargar, si todos los centroides son `0.0`, normalizar es NaN → tratar centroides `0.0` como **no válidos** y skipar el Nivel 2 con `warn!`.
  - Acceptance criteria: `.json` parseable por la deserialización de `embedding.rs`. Si todos ceros, `build_classifier` loguea `warn` y no añade el stage.

## Phase 6: Nivel 3 — Regresión logística (stub, sobre embeddings)
- [ ] Step 6.1: Stage `LogisticClassifier`
  - File(s): `src/classifier/logistic.rs` (nuevo, todo `#[cfg(feature = "classifier-embedding")]`), `src/classifier/mod.rs`, `src/classifier/pipeline.rs`
  - Change:
    1. En `mod.rs` añadir `#[cfg(feature = "classifier-embedding")] pub mod logistic;`.
    2. En `logistic.rs`:
       - `#[derive(serde::Deserialize)] pub struct LogisticWeights { dim: usize, weights: Vec<f32>, bias: f32 }` (shippeado en `models/classifier_weights.json` con `dim=384`, `weights=[0.0;384]`, `bias=0.0` → sigmoid 0.5).
       - `pub struct LogisticClassifier { w: LogisticWeights }` con `pub fn load(path: &str) -> anyhow::Result<Self>` (serde_json desde fichero; si no existe, error claro).
       - `fn score(&self, emb: &[f32]) -> f32`: `σ(w·x + b)` con `1/(1+exp(-z))`. `z` = dot. La salida se interpreta como `P(Complex)`.
       - **Acoplamiento con Nivel 2**: el Nivel 3 necesita el embedding del Nivel 2. Como la cascada actual pasa solo `text`, reestructurar el trait `ClassifierStage::try_classify` para recibir **opcionalmente** el embedding previo: añadir `async fn try_classify(&self, text: &str, emb: Option<&[f32]>, threshold: f32) -> Option<ClassifyResult>;`. Actualizar `HeuristicStage` y `EmbeddingStage` (el embedding stage guarda su embedding en un `Arc<Mutex<Option<Vec<f32>>>>` accesible al siguiente stage — o mejor: `ClassifierPipeline.classify` pasa el embedding entre stages). **Decisión**: en `pipeline.rs` hacer que `EmbeddingClassifier` almacene el último embedding en su interior (interior `Mutex<Option<Vec<f32>>>`) y exponga `pub fn last_embedding(&self) -> Option<Vec<f32>>` para que el `LogisticClassifier` lo lea. Más limpio: el `ClassifierPipeline` mantiene `last_embedding: Arc<Mutex<Option<Vec<f32>>>>` y los stages lo leen/escriben. Re-diseñar `try_classify(&self, text, shared_emb: &Arc<Mutex<Option<Vec<f32>>>>, threshold)`.
       - `#[async_trait] impl ClassifierStage for LogisticClassifier`: `name()="logistic"`, `level()=Logistic`, `try_classify`: lee `shared_emb.lock().unwrap().clone()`; si `None` → `return None` (sin embedding, se skipa); si `Some(e)` calcula `p=score(e)`; `intent = if p > 0.5 { Complex } else { Simple }`, `confidence = p` (o `max(p, 1-p)` para "margen"). Devolver `Some` si `|p-0.5|*2 >= threshold` (margen suficiente); `None` si no.
    3. En `build_classifier`, tras añadir `EmbeddingClassifier`, añadir `LogisticClassifier::load(&config.classifier_weights_path)` si el path no está vacío (con `match` + `warn!`).
  - Acceptance criteria: `cargo build --features classifier-embedding` compila. Con pesos stub `[0;N],0`, `score=0.5` → confianza 0 → `None` → cascada continúa a Nivel 4 (o finaliza en Complex). Test unit de `logistic.rs` con pesos conocidos (`w=[1,...], bias=-N/2`) verifica que un embedding cerca del "complejo" da `p>0.5`.

- [ ] Step 6.2: Re-exportar y ordenar stages en la cascada
  - File(s): `src/classifier/pipeline.rs`
  - Change: Confirmar el orden de inserción en `build_classifier`: Heuristic → Embedding → Logistic → Fallback. El `shared_emb` se pasa a cada `try_classify`. `EmbeddingClassifier` escribe `shared_emb`; `LogisticClassifier` lee. `Heuristic` y `Fallback` ignoran `shared_emb`.
  - Acceptance criteria: `cargo build --features classifier-embedding` compila. `cargo test --features classifier-embedding` (con modelo presente) pasa.

## Phase 7: Nivel 4 — SLM Fallback (segundo endpoint LLM dedicado)
- [ ] Step 7.1: Cliente del fallback SLM
  - File(s): `src/classifier/fallback.rs` (nuevo), `src/classifier/mod.rs`
  - Change:
    1. En `mod.rs` añadir `pub mod fallback;` (sin `cfg`, porque solo usa `reqwest`, ya disponible).
    2. En `fallback.rs`:
       ```rust
       use anyhow::{Context, Result};
       use std::time::Duration;

       pub struct FallbackClassifier {
           client: reqwest::Client,
           url: String,        // {fallback_url}/v1/chat/completions
           model: String,
           api_key: String,
       }
       impl FallbackClassifier {
           pub fn new(url: &str, model: &str, api_key: &str, timeout_ms: u64) -> Self {
               let mut b = reqwest::Client::builder().timeout(Duration::from_millis(timeout_ms));
               Self { client: b.build().expect("reqwest"), url: format!("{}/v1/chat/completions", url.trim_end_matches('/')), model: model.to_string(), api_key: api_key.to_string() }
           }
           pub async fn classify(&self, text: &str) -> Result<crate::classifier::Intent> {
               let sys = "You are an intent classifier. Respond with EXACTLY one token: \"SIMPLE\" for casual/greeting/trivial requests, or \"COMPLEX\" for requests requiring tools, reasoning, code, search, or multi-step tasks. No punctuation.";
               let payload = serde_json::json!({
                   "model": self.model,
                   "messages": [
                       {"role":"system","content":sys},
                       {"role":"user","content": text}
                   ],
                   "max_tokens": 1,
                   "temperature": 0.0,
                   "stream": false,
               });
               let mut req = self.client.post(&self.url).json(&payload);
               if !self.api_key.is_empty() { req = req.header("Authorization", format!("Bearer {}", self.api_key)); }
               let resp = req.send().await.context("fallback SLM request failed")?;
               if !resp.status().is_success() { anyhow::bail!("fallback SLM status {}", resp.status()); }
               let json: serde_json::Value = resp.json().await?;
               let txt = json["choices"][0]["message"]["content"].as_str().unwrap_or("").trim().to_string();
               let lower = txt.to_lowercase();
               if lower.starts_with("simple") { Ok(crate::classifier::Intent::Simple) }
               else if lower.starts_with("complex") { Ok(crate::classifier::Intent::Complex) }
               else { anyhow::bail!("fallback SLM ambiguous output: {txt:?}") }
           }
       }
       ```
       Nota: `logit_bias`/gramática omitido en esta fase (TODO en doc); el `max_tokens=1` + `temperature=0` + prompt restrictivo basta para la v0.
  - Acceptance criteria: `cargo build` compila. Test unit con wiremock: mock `/v1/chat/completions` responde `content:"SIMPLE"` → `classify()` devuelve `Intent::Simple`; responde `content:"COMPLEX"` → `Complex`; responde basura → `Err`. Test con timeout simulado (wiremock `.set_delay`) → `Err` tras `timeout_ms`.

- [ ] Step 7.2: Stage `FallbackStage`
  - File(s): `src/classifier/fallback.rs`, `src/classifier/pipeline.rs`
  - Change:
    1. En `fallback.rs` añadir `pub struct FallbackStage { inner: FallbackClassifier, enabled: bool }` con `#[async_trait] impl ClassifierStage for FallbackStage`: `name()="fallback"`, `level()=Fallback`, `try_classify` (sin usar `shared_emb`): si `!enabled` → `None`; si no, llama `inner.classify(text).await`, on `Ok` devolver `Some(ClassifyResult { intent, level: Fallback, confidence: 1.0, matched_keyword: None })`; on `Err(e)` → `warn!(target:"classifier","fallback failed: {e}")` y `None` (la cascada resolverá Complex por sesgo de seguridad en `pipeline.classify`).
    2. En `build_classifier`, si `config.classifier_enable_fallback && !config.classifier_fallback_url.is_empty() && !config.classifier_fallback_model.is_empty()` → añadir `FallbackStage`:
       ```rust
       p = p.with_stage(Box::new(crate::classifier::fallback::FallbackStage::new(
           crate::classifier::fallback::FallbackClassifier::new(
               &config.classifier_fallback_url,
               &config.classifier_fallback_model,
               &config.classifier_fallback_api_key,
               config.classifier_fallback_timeout_ms),
           config.classifier_enable_fallback)));
       ```
  - Acceptance criteria: `cargo build` (default features) compila. `cargo build --features classifier-embedding` compila. Con `CLASSIFIER_ENABLE_FALLBACK=false` (default), el stage no se añade y `pipeline.classify` termina resolviendo por sesgo de seguridad en `Complex` si ningún nivel previo resolvió.

## Phase 8: Tests de integración + QA
- [ ] Step 8.1: Test e2e de la cascada con fallback mock
  - File(s): `src/e2e_tests.rs`
  - Change: Añadir test `#[ignore]` `e2e::intent_cascade_fallback` que:
    1. Levanta `wiremock::MockServer` mockeando `/v1/chat/completions` del fallback SLM → responde `content:"COMPLEX"` y del LLM principal → responde SSE.
    2. Construye `ClassifierPipeline` con `HeuristicStage` (keywords=[]) + `FallbackStage` (apuntando al mock fallback, enabled).
    3. Inyecta `"hola"` (Simple → resuelto en Nivel 1, fallback no invocado — verificar que el mock fallback **no** recibió petición) y una frase larga sin keyword (caería a fallback → verificar mock recibió llamada y `intent=Complex`).
    4. Verifica filas en `classification_log` con `level` correcto.
  - Acceptance criteria: `cargo test e2e::intent_cascade_fallback -- --ignored --nocapture` pasa.

- [ ] Step 8.2: Test de trazabilidad y sesgo de seguridad
  - File(s): `src/e2e_tests.rs`
  - Change: Test que construye un `ClassifierPipeline` con **ningún stage que resuelva** (todos devuelven `None` — p.ej. solo `HeuristicStage` con keywords=[] y frase sin patrón trivial) y verifica `pipeline.classify(...)` → `Intent::Complex` (sesgo de seguridad) con `level` del último stage y `confidence=0.0`.
  - Acceptance criteria: pasa.

- [ ] Step 8.3: Test de "tools_strict" en payload
  - File(s): `src/llm/client.rs` (tests)
  - Change: Extender el test existente de `build_stream_payload` con un caso `RequestOptions::new().with_tool_choice(ToolChoice::None)` + `tools=&[]` → asertar `payload["tool_choice"] == "none"`. Un caso `ToolChoice::Required` con `forced_tool=None` + `tools` no vacío → `"required"`.
  - Acceptance criteria: `cargo test --features tui,remote,control` pasa el nuevo test.

- [ ] Step 8.4: QA completo local (sin feature embedding)
  - File(s): ninguno
  - Change: `make qa`. Corregir warnings de clippy del nuevo código. Verificar `RUST_LOG=classifier=info cargo run` muestra `level=Heuristic confidence=...`.
  - Acceptance criteria: `make qa` verde (`fmt`, `lint`, `test`, `test-ci`, `test-e2e`, `build`).

- [ ] Step 8.5: QA con feature `classifier-embedding` (mejor-effort)
  - File(s): ninguno
  - Change: `cargo build --features classifier-embedding` (verifica que compila). `bash scripts/download-embedding-model.sh` y `cargo test --features classifier-embedding -- --ignored` (skipan si no hay modelo/centroides válidos). Documentar en `doc/env-vars.md` cómo activar el Nivel 2/3.
  - Acceptance criteria: Build compila. Los tests se skipan limpiamente sin modelo. No se exige verde total en CI base por el peso de `ort`.

## Notas para el agente de build
- **Orden estricto**: Phase 1 → 2 → 3 → 4 → checkpoint QA → 5 → 6 → 7 → 8. Sin saltarse el checkpoint tras Phase 4.
- **Dependencias pesadas** (`ort`, `tokenizers`, `ndarray`) **solo** tras `#[cfg(feature = "classifier-embedding")]`. El build/Ci por defecto no las incluye.
- **No tocar** `complete`, `complete_short`, `complete_multimodal`, secondary LLM, Tools Asíncronas, Dream/consolidación. Solo `stream()` del pipeline conversacional y el módulo `classifier/`.
- **Mutual exclusividad thinking/tools** en `client.rs:236-263` se **preserva**; el nuevo `tool_choice` se añade sin alterar la rama `chat_template_kwargs`.
- **Sesgo de seguridad**: ante cualquier error/ambigüedad en niveles 2/3/4, la cascada debe **caer** a `Complex`. Verificar en tests.
- **Compat con issue #158**: el comportamiento con `classifier_enable_embedding=false` y `classifier_enable_fallback=false` **debe ser equivalente** al clasificador por keywords actual (Nivel 1). Cero regresión.
- **Commit por fase**; tras el checkpoint de Phase 4, commit "verify: cascada Nivel 1 + DBQA" antes de seguir.
- **Naming Seneschal** en logs/UI (no usar marca de terceros). "C01" solo informativo en docs/comentarios.
- **`logit_bias`/gramática** del fallback SLM queda como TODO documentado en `src/classifier/fallback.rs`; la v0 se apoya en `max_tokens=1` + `temperature=0` + prompt estricto.