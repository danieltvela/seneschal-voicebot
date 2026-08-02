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

impl Intent {
    /// Short status-bar label (`SIMPLE` / `COMPLEX`).
    pub fn as_str(self) -> &'static str {
        match self {
            Intent::Simple => "SIMPLE",
            Intent::Complex => "COMPLEX",
        }
    }
}

/// Which cascade level resolved the classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifierLevel {
    Heuristic,
    Embedding,
    Logistic,
    Fallback,
}

/// Debug override for the intent classifier (shared between TUI and pipeline).
///
/// When not `Auto`, `llm_task` skips the cascade and uses the forced intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClassifierForceMode {
    #[default]
    Auto,
    ForceSimple,
    ForceComplex,
}

impl ClassifierForceMode {
    /// Cycle `Auto → ForceSimple → ForceComplex → Auto`.
    pub fn cycle(self) -> Self {
        match self {
            ClassifierForceMode::Auto => ClassifierForceMode::ForceSimple,
            ClassifierForceMode::ForceSimple => ClassifierForceMode::ForceComplex,
            ClassifierForceMode::ForceComplex => ClassifierForceMode::Auto,
        }
    }

    /// Forced intent when override is active; `None` means run the cascade.
    pub fn as_intent(self) -> Option<Intent> {
        match self {
            ClassifierForceMode::Auto => None,
            ClassifierForceMode::ForceSimple => Some(Intent::Simple),
            ClassifierForceMode::ForceComplex => Some(Intent::Complex),
        }
    }

    /// Short status-bar / notification label.
    pub fn as_str(self) -> &'static str {
        match self {
            ClassifierForceMode::Auto => "AUTO",
            ClassifierForceMode::ForceSimple => "SIMPLE",
            ClassifierForceMode::ForceComplex => "COMPLEX",
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{ClassifierForceMode, Intent};

    #[test]
    fn force_mode_cycles() {
        assert_eq!(
            ClassifierForceMode::Auto.cycle(),
            ClassifierForceMode::ForceSimple
        );
        assert_eq!(
            ClassifierForceMode::ForceSimple.cycle(),
            ClassifierForceMode::ForceComplex
        );
        assert_eq!(
            ClassifierForceMode::ForceComplex.cycle(),
            ClassifierForceMode::Auto
        );
    }

    #[test]
    fn force_mode_as_intent() {
        assert_eq!(ClassifierForceMode::Auto.as_intent(), None);
        assert_eq!(
            ClassifierForceMode::ForceSimple.as_intent(),
            Some(Intent::Simple)
        );
        assert_eq!(
            ClassifierForceMode::ForceComplex.as_intent(),
            Some(Intent::Complex)
        );
    }
}
