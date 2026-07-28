/// Keyword-based intent classification (Nivel 1 interno de la cascada).
///
/// Returns `Complex` with confidence 1.0 if any keyword from `complex_keywords`
/// is a substring (case-insensitive) of `text`. Otherwise `Simple` confidence 0.0.
pub fn classify(text: &str, complex_keywords: &[String]) -> super::ClassifyResult {
    use super::{ClassifierLevel, ClassifyResult, Intent};

    if complex_keywords.is_empty() {
        return ClassifyResult {
            intent: Intent::Simple,
            level: ClassifierLevel::Heuristic,
            confidence: 0.0,
            matched_keyword: None,
        };
    }

    let lower = text.to_lowercase();
    for kw in complex_keywords {
        if lower.contains(kw.as_str()) {
            return ClassifyResult {
                intent: Intent::Complex,
                level: ClassifierLevel::Heuristic,
                confidence: 1.0,
                matched_keyword: Some(kw.clone()),
            };
        }
    }

    ClassifyResult {
        intent: Intent::Simple,
        level: ClassifierLevel::Heuristic,
        confidence: 0.0,
        matched_keyword: None,
    }
}

#[cfg(test)]
mod tests {
    use crate::classifier::{ClassifierLevel, Intent, keyword};

    fn kw_list(kw: &[&str]) -> Vec<String> {
        kw.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn simple_greeting() {
        let r = keyword::classify("Hola, ¿cómo estás?", &kw_list(&["investiga", "lanza"]));
        assert_eq!(r.intent, Intent::Simple);
        assert_eq!(r.level, ClassifierLevel::Heuristic);
        assert_eq!(r.confidence, 0.0);
        assert!(r.matched_keyword.is_none());
    }

    #[test]
    fn complex_by_keyword() {
        let r = keyword::classify("Investiga la API de OpenAI", &kw_list(&["investiga", "lanza"]));
        assert_eq!(r.intent, Intent::Complex);
        assert_eq!(r.confidence, 1.0);
        assert_eq!(r.matched_keyword.as_deref(), Some("investiga"));
    }

    #[test]
    fn case_insensitive() {
        let r = keyword::classify("INVESTIGA eso por favor", &kw_list(&["investiga"]));
        assert_eq!(r.intent, Intent::Complex);
        assert_eq!(r.confidence, 1.0);
    }

    #[test]
    fn empty_keyword_list_always_simple() {
        let r = keyword::classify("Ejecuta algo", &[]);
        assert_eq!(r.intent, Intent::Simple);
        assert_eq!(r.confidence, 0.0);
        assert!(r.matched_keyword.is_none());
    }

    #[test]
    fn keyword_with_space() {
        let r = keyword::classify("busca en Google las noticias", &kw_list(&["busca en"]));
        assert_eq!(r.intent, Intent::Complex);
        assert_eq!(r.confidence, 1.0);
        assert_eq!(r.matched_keyword.as_deref(), Some("busca en"));
    }

    #[test]
    fn first_keyword_wins() {
        let r = keyword::classify("lanza e investiga algo", &kw_list(&["investiga", "lanza"]));
        assert_eq!(r.intent, Intent::Complex);
        assert_eq!(r.matched_keyword.as_deref(), Some("investiga"));
        let r2 = keyword::classify("lanza e investiga algo", &kw_list(&["lanza", "investiga"]));
        assert_eq!(r2.intent, Intent::Complex);
        assert_eq!(r2.matched_keyword.as_deref(), Some("lanza"));
    }

    #[test]
    fn no_match_returns_simple() {
        let r = keyword::classify(
            "¿Qué tal el clima hoy?",
            &kw_list(&["investiga", "ejecuta", "analiza"]),
        );
        assert_eq!(r.intent, Intent::Simple);
        assert_eq!(r.confidence, 0.0);
        assert!(r.matched_keyword.is_none());
    }

    #[test]
    fn keyword_substring_in_longer_word() {
        let r = keyword::classify("Puedes crear un documento", &kw_list(&["crea"]));
        assert_eq!(r.intent, Intent::Complex);
        assert_eq!(r.confidence, 1.0);
    }

    #[test]
    fn real_sentences_with_defaults() {
        let kw = kw_list(crate::classifier::DEFAULT_COMPLEX_KEYWORDS);
        assert_eq!(keyword::classify("Buenos días", &kw).intent, Intent::Simple);
        assert_eq!(keyword::classify("Hola ¿cómo estás?", &kw).intent, Intent::Simple);
        assert_eq!(keyword::classify("Gracias, muy amable", &kw).intent, Intent::Simple);
        assert_eq!(
            keyword::classify("Ejecuta el script de backup", &kw).intent,
            Intent::Complex
        );
        assert_eq!(
            keyword::classify("Investiga el modelo Gemma-4 para ver su rendimiento", &kw).intent,
            Intent::Complex
        );
        assert_eq!(
            keyword::classify("Analiza este archivo de logs y dime si hay errores", &kw).intent,
            Intent::Complex
        );
    }
}
