/// Heuristic intent classification (Nivel 1 de la cascada C01).
/// ...
pub fn classify(text: &str, complex_keywords: &[String]) -> super::ClassifyResult {
    use super::{ClassifierLevel, ClassifyResult, Intent};

    // (a) Empty text
    if text.trim().is_empty() {
        return ClassifyResult {
            intent: Intent::Simple,
            level: ClassifierLevel::Heuristic,
            confidence: 1.0,
            matched_keyword: None,
        };
    }

    // (b) Trivial greetings / confirmations
    const TRIVIAL: &[&str] = &[
        "hola",
        "buenos días",
        "buenas tardes",
        "buenas noches",
        "hey",
        "ola",
        "ok",
        "vale",
        "sí",
        "si",
        "no",
        "gracias",
        "adiós",
        "adios",
        "claro",
        "chao",
        "bye",
    ];

    let trimmed = text.trim();
    let lower = trimmed.to_lowercase();
    let cleaned = lower.trim_end_matches(|c: char| !c.is_alphanumeric() && c != ' ');
    let cleaned = cleaned.trim();

    for pat in TRIVIAL {
        if cleaned == *pat || cleaned.starts_with(*pat) {
            let after = &cleaned[pat.len()..];
            if after.is_empty()
                || after.starts_with(' ')
                || after.starts_with(',')
                || after.starts_with('.')
                || after.starts_with('!')
                || after.starts_with('?')
            {
                return ClassifyResult {
                    intent: Intent::Simple,
                    level: ClassifierLevel::Heuristic,
                    confidence: 1.0,
                    matched_keyword: Some(pat.to_string()),
                };
            }
        }
    }

    // Also handle very short phrases (≤ 2 words without keywords = casual)
    {
        let word_count = trimmed.split_whitespace().count();
        if word_count <= 2 && !trimmed.contains('?') && !trimmed.contains('¿') {
            let kw = super::keyword::classify(text, complex_keywords);
            if kw.intent == Intent::Simple {
                return ClassifyResult {
                    intent: Intent::Simple,
                    level: ClassifierLevel::Heuristic,
                    confidence: 0.0,
                    matched_keyword: None,
                };
            }
        }
    }

    // (c) Keyword match
    let kw = super::keyword::classify(text, complex_keywords);
    if kw.intent == Intent::Complex {
        return kw;
    }

    // No keyword match, not trivial, not empty → Simple with 0 confidence
    ClassifyResult {
        intent: Intent::Simple,
        level: ClassifierLevel::Heuristic,
        confidence: 0.0,
        matched_keyword: None,
    }
}

use std::sync::Arc;
use std::sync::Mutex;
use async_trait::async_trait;
use super::pipeline::ClassifierStage;
use super::{ClassifierLevel, ClassifyResult};

/// Wraps the heuristic classifier as a cascade stage.
pub struct HeuristicStage {
    keywords: Vec<String>,
}

impl HeuristicStage {
    pub fn new(keywords: Vec<String>) -> Self {
        Self { keywords }
    }
}

#[async_trait]
impl ClassifierStage for HeuristicStage {
    fn name(&self) -> &'static str {
        "heuristic"
    }

    fn level(&self) -> ClassifierLevel {
        ClassifierLevel::Heuristic
    }

    async fn try_classify(
        &self,
        text: &str,
        _shared_emb: &Arc<Mutex<Option<Vec<f32>>>>,
        threshold: f32,
    ) -> Option<ClassifyResult> {
        let r = classify(text, &self.keywords);
        if r.confidence >= threshold {
            Some(r)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::classifier::{ClassifierLevel, Intent, heuristic};

    fn kw(k: &[&str]) -> Vec<String> {
        k.iter().map(|s| s.to_string()).collect()
    }

    fn default_kw() -> Vec<String> {
        kw(crate::classifier::DEFAULT_COMPLEX_KEYWORDS)
    }

    #[test]
    fn empty_text_is_simple_full_confidence() {
        let r = heuristic::classify("", &default_kw());
        assert_eq!(r.intent, Intent::Simple);
        assert_eq!(r.level, ClassifierLevel::Heuristic);
        assert_eq!(r.confidence, 1.0);
    }

    #[test]
    fn greeting_is_simple_full_confidence() {
        for g in &["hola", "buenos días", "hey", "ok", "gracias!", "adiós."] {
            let r = heuristic::classify(g, &default_kw());
            assert_eq!(
                r.intent,
                Intent::Simple,
                "expected Simple for '{g}', got {:?}",
                r.intent
            );
            assert_eq!(r.confidence, 1.0, "confidence mismatch for '{g}'");
        }
    }

    #[test]
    fn confirmation_is_simple() {
        let r = heuristic::classify("sí", &default_kw());
        assert_eq!(r.intent, Intent::Simple);
        assert_eq!(r.confidence, 1.0);
        let r2 = heuristic::classify("no", &default_kw());
        assert_eq!(r2.intent, Intent::Simple);
        assert_eq!(r2.confidence, 1.0);
    }

    #[test]
    fn keyword_marks_complex_full_confidence() {
        let r = heuristic::classify(
            "Investiga el modelo Gemma-4 para ver su rendimiento",
            &default_kw(),
        );
        assert_eq!(r.intent, Intent::Complex);
        assert_eq!(r.confidence, 1.0);
        assert!(r.matched_keyword.is_some());
    }

    #[test]
    fn long_sentence_without_keyword_is_simple_zero_confidence() {
        let r = heuristic::classify(
            "dime algo interesante sobre la historia de españa en la edad media",
            &default_kw(),
        );
        assert_eq!(r.intent, Intent::Simple);
        assert_eq!(r.confidence, 0.0);
        assert_eq!(r.level, ClassifierLevel::Heuristic);
    }

    #[test]
    fn short_question_without_keyword_is_simple_zero() {
        let r = heuristic::classify("¿cómo funciona esto?", &default_kw());
        assert_eq!(r.intent, Intent::Simple);
        assert_eq!(r.confidence, 0.0);
    }

    #[test]
    fn whitespace_only() {
        let r = heuristic::classify("   ", &default_kw());
        assert_eq!(r.intent, Intent::Simple);
        assert_eq!(r.confidence, 1.0);
    }
}
