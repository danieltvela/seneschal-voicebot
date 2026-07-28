pub mod heuristic;
pub mod keyword;

/// Classification of a user utterance: either casual interaction (Simple)
/// or a task that requires tool-calling and reasoning (Complex).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Simple,
    Complex,
}

/// Which level of the cascade resolved the classification.
#[allow(dead_code)] // consumed in Phase 3+
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifierLevel {
    Heuristic,
    Embedding,
    Logistic,
    Fallback,
}

/// Result of classification: the assigned intent, the level that resolved it,
/// the confidence [0.0, 1.0], plus the first keyword that triggered it (for logging).
#[derive(Debug, Clone)]
pub struct ClassifyResult {
    pub intent: Intent,
    /// Cascade level that made the final decision.
    pub level: ClassifierLevel,
    /// Confidence [0.0–1.0]. 1.0 = deterministic keyword match.
    /// 0.0 = "not resolved" → the cascade continues to the next level.
    pub confidence: f32,
    /// First keyword matched (for logging), if the heuristic level resolved.
    pub matched_keyword: Option<String>,
}

impl ClassifyResult {
    /// Split into (Intent, Option<String>) for destructuring.
    /// Preserved for backward compatibility with existing callers.
    pub fn into_parts(self) -> (Intent, Option<String>) {
        (self.intent, self.matched_keyword)
    }
}

/// Default keywords that mark a request as Complex.
///
/// Replaced entirely by `LLM_COMPLEX_KEYWORDS` (CSV) when the env var is set.
/// Matching is case-insensitive and substring-based.
pub const DEFAULT_COMPLEX_KEYWORDS: &[&str] = &[
    "investiga",
    "lanza",
    "ejecuta",
    "analiza",
    "crea",
    "busca",
    "abre",
    "calcula",
    "resume",
    "resumen",
    "traduce",
    "compara",
    "lee",
    "escribe",
    "instala",
    "configura",
    "diagnostica",
    "muestra",
    "lista",
    "diseña",
    "planifica",
    "busca en",
    "buscame",
];

/// Compatibility wrapper: classifies text using the heuristic (Nivel 1) rules.
///
/// Kept for backward compatibility with `llm_task.rs` until Phase 3 replaces it
/// with the full `ClassifierPipeline`.
pub fn classify(text: &str, complex_keywords: &[String]) -> ClassifyResult {
    heuristic::classify(text, complex_keywords)
}
