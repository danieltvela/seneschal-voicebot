// Intent classification types shared across crates.
// Extracted from src/classifier/mod.rs.

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
