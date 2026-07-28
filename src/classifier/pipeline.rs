use async_trait::async_trait;
use std::sync::Arc;
use std::sync::Mutex;

use super::{ClassifierLevel, ClassifyResult, Intent};

/// A single stage in the classification cascade.
///
/// Each stage receives the user's text, an optional shared embedding buffer,
/// and a confidence threshold. It returns `Some(ClassifyResult)` if it
/// determines the intent with confidence ≥ threshold, or `None` to continue
/// the cascade to the next stage.
#[async_trait]
pub trait ClassifierStage: Send + Sync {
    /// Human-readable name of this stage (for logs and DB).
    fn name(&self) -> &'static str;

    /// The `ClassifierLevel` enum variant for this stage.
    fn level(&self) -> ClassifierLevel;

    /// Attempt to classify. The optional shared embedding is written by the
    /// Embedding stage (Nivel 2) and read by the Logistic stage (Nivel 3).
    /// All other stages ignore it.
    async fn try_classify(
        &self,
        text: &str,
        shared_emb: &Arc<Mutex<Option<Vec<f32>>>>,
        threshold: f32,
    ) -> Option<ClassifyResult>;
}

/// Orchestrates the classification cascade.
///
/// Stages are tried in insertion order. The first stage whose confidence
/// meets or exceeds `threshold` resolves the pipeline. If none resolve,
/// the pipeline returns `Complex` (safety bias).
pub struct ClassifierPipeline {
    stages: Vec<Box<dyn ClassifierStage>>,
    threshold: f32,
}

impl ClassifierPipeline {
    pub fn new(threshold: f32) -> Self {
        Self {
            stages: Vec::new(),
            threshold,
        }
    }

    pub fn with_stage(mut self, s: Box<dyn ClassifierStage>) -> Self {
        self.stages.push(s);
        self
    }

    /// Run each stage in order. The first that resolves (`Some`) wins.
    ///
    /// If no stage resolves, the pipeline biases to `Complex` for safety.
    /// The returned `level` field reflects the last stage attempted.
    pub async fn classify(&self, text: &str) -> ClassifyResult {
        let shared_emb = Arc::new(Mutex::new(None));
        let mut last_result: Option<ClassifyResult> = None;

        for stage in &self.stages {
            match stage
                .try_classify(text, &shared_emb, self.threshold)
                .await
            {
                Some(r) => return r,
                None => {
                    // Re-run with threshold = 1.0 to ensure `last_result`
                    // captures this stage's raw output for traceability
                    // (the stage's confidence is below the real threshold
                    //  so it returned None, but we still want to know what
                    //  it *would* have decided for DB logging).
                    if let Some(r) = stage.try_classify(text, &shared_emb, 1.0).await {
                        last_result = Some(r);
                    }
                }
            }
        }

        // Safety bias: if nothing resolved, assume Complex.
        last_result.unwrap_or(ClassifyResult {
            intent: Intent::Complex,
            level: ClassifierLevel::Fallback,
            confidence: 0.0,
            matched_keyword: None,
        })
    }
}

/// Build a `ClassifierPipeline` from config.
///
/// On default features, only the `Heuristic` stage (Nivel 1) is added.
/// With `classifier-embedding` and when enabled via config, the Embedding
/// and Logistic stages (Niveles 2/3) are added. The Fallback stage (Nivel 4)
/// is added when enabled and configured.
pub fn build_classifier(config: &crate::config::Config) -> ClassifierPipeline {
    let resolved_keywords: Vec<String> = if config.llm_complex_keywords.is_empty() {
        super::DEFAULT_COMPLEX_KEYWORDS
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        config.llm_complex_keywords.clone()
    };

    let mut pipeline = ClassifierPipeline::new(config.classifier_confidence_threshold);

    // Nivel 1 – heuristic (always active)
    pipeline = pipeline.with_stage(Box::new(
        super::heuristic::HeuristicStage::new(resolved_keywords),
    ));

    #[cfg(feature = "classifier-embedding")]
    if config.classifier_enable_embedding {
        // Nivel 2 – embeddings (Phase 5)
        use tracing::warn;
        if !config.classifier_model_path.is_empty()
            && !config.classifier_centroids_path.is_empty()
        {
            match super::embedding::EmbeddingClassifier::load(
                &config.classifier_model_path,
                &config.classifier_centroids_path,
            ) {
                Ok(e) => {
                    pipeline = pipeline.with_stage(Box::new(e));
                }
                Err(err) => {
                    warn!(
                        target: "classifier",
                        "Nivel 2 (embedding) disabled: {err}"
                    );
                }
            }
        } else {
            warn!(
                target: "classifier",
                "Nivel 2 (embedding) disabled: CLASSIFIER_MODEL_PATH or CLASSIFIER_CENTROIDS_PATH empty"
            );
        }

        // Nivel 3 – logistic regression (Phase 6)
        if !config.classifier_weights_path.is_empty() {
            match super::logistic::LogisticClassifier::load(
                &config.classifier_weights_path,
            ) {
                Ok(lc) => {
                    pipeline = pipeline.with_stage(Box::new(lc));
                }
                Err(err) => {
                    warn!(
                        target: "classifier",
                        "Nivel 3 (logistic) disabled: {err}"
                    );
                }
            }
        }
    }

    // Nivel 4 – SLM fallback (Phase 7)
    if config.classifier_enable_fallback
        && !config.classifier_fallback_url.is_empty()
        && !config.classifier_fallback_model.is_empty()
    {
        let fc =
            super::fallback::FallbackClassifier::new(
                &config.classifier_fallback_url,
                &config.classifier_fallback_model,
                &config.classifier_fallback_api_key,
                config.classifier_fallback_timeout_ms,
            );
        let stage = super::fallback::FallbackStage::new(
            fc,
            config.classifier_enable_fallback,
        );
        pipeline = pipeline.with_stage(Box::new(stage));
    }

    pipeline
}
