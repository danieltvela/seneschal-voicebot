/// Embedding-based intent classifier (Nivel 2 de la cascada C01).
///
/// Uses an ONNX embedding model (e.g. bge-small or all-MiniLM-L6-v2) to compute
/// a sentence embedding and compare via cosine similarity against pre-computed
/// centroids for `SIMPLE` and `COMPLEX` categories.
///
/// ## Requirements
/// - `ort` crate (ONNX Runtime) + `tokenizers` + `ndarray`
/// - Model files in the path specified by `CLASSIFIER_MODEL_PATH`
/// - Centroids JSON at `CLASSIFIER_CENTROIDS_PATH`
///
/// Without these, `load()` returns an error and the cascade skips this level gracefully.

#[derive(serde::Deserialize)]
struct Centroids {
    dim: usize,
    simple: Vec<f32>,
    complex: Vec<f32>,
}

pub struct EmbeddingClassifier {
    // Placeholder — real implementation requires ort + tokenizers + ndarray
    _private: (),
}

impl EmbeddingClassifier {
    pub fn load(model_path: &str, centroids_path: &str) -> anyhow::Result<Self> {
        // Verify paths exist
        let model_meta = std::fs::metadata(model_path)
            .map_err(|e| anyhow::anyhow!("CLASSIFIER_MODEL_PATH '{}': {}", model_path, e))?;
        if !model_meta.is_dir() && !model_path.ends_with(".onnx") {
            anyhow::bail!("CLASSIFIER_MODEL_PATH must be a directory or .onnx file");
        }
        let _ = std::fs::metadata(centroids_path).map_err(|e| {
            anyhow::anyhow!("CLASSIFIER_CENTROIDS_PATH '{}': {}", centroids_path, e)
        })?;

        // Check that centroids are not all zeros (placeholder)
        let data = std::fs::read_to_string(centroids_path)?;
        let c: Centroids = serde_json::from_str(&data)?;
        let all_zero = c.simple.iter().all(|&x| x == 0.0) && c.complex.iter().all(|&x| x == 0.0);
        if all_zero {
            anyhow::bail!(
                "centroids are all zeros (placeholder) — populate {} with real vectors",
                centroids_path
            );
        }

        // TODO: load ONNX model via ort::Session::builder()?.commit_from_file(...)
        // TODO: load tokenizer via tokenizers::Tokenizer::from_file(tokenizer_path)
        anyhow::bail!(
            "EmbeddingClassifier not yet integrated — requires ort crate. See doc/env-vars.md"
        );
    }
}

use super::pipeline::ClassifierStage;
use super::{ClassifierLevel, ClassifyResult};
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::Mutex;

#[async_trait]
impl ClassifierStage for EmbeddingClassifier {
    fn name(&self) -> &'static str {
        "embedding"
    }

    fn level(&self) -> ClassifierLevel {
        ClassifierLevel::Embedding
    }

    async fn try_classify(
        &self,
        _text: &str,
        _shared_emb: &Arc<Mutex<Option<Vec<f32>>>>,
        _threshold: f32,
    ) -> Option<ClassifyResult> {
        // TODO: compute embedding, compare with centroids, return if confident
        None
    }
}
