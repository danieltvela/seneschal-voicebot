// Intent classification cascade — shared across crates.
// Moved from src/classifier/ to break the core pipeline's dependency on the main binary.

pub mod fallback;
pub mod heuristic;
pub mod keyword;
pub mod pipeline;

pub use pipeline::ClassifierPipeline;
pub use pipeline::build_classifier;

/// Classification of a user utterance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Simple,
    Complex,
}

/// Which cascade level resolved the classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifierLevel {
    Heuristic,
    Embedding,
    Logistic,
    Fallback,
}

/// Result of the classifier cascade.
#[derive(Debug, Clone)]
pub struct ClassifyResult {
    pub intent: Intent,
    pub level: ClassifierLevel,
    pub confidence: f32,
    pub matched_keyword: Option<String>,
}

impl ClassifyResult {
    pub fn into_parts(self) -> (Intent, Option<String>) {
        (self.intent, self.matched_keyword)
    }
}

/// Default keywords that mark a request as Complex.
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
pub fn classify(text: &str, complex_keywords: &[String]) -> ClassifyResult {
    heuristic::classify(text, complex_keywords)
}
