/// Logistic regression classifier (Nivel 3 de la cascada C01).
///
/// Applies a learned linear model on top of the embedding from Nivel 2.
/// Weights are loaded from a JSON file (`CLASSIFIER_WEIGHTS_PATH`).
///
/// ## Shape
/// - `weights`: `[f32; N]` (one per embedding dimension)
/// - `bias`: `f32`
/// - Score: `σ(w·x + b)` where `σ` is sigmoid, interpreted as P(Complex)
///
/// ## Placeholder
/// The shipped `models/classifier_weights.json` contains all-zero weights,
/// producing sigmoid(0) = 0.5 → confidence = 0 → Nivel 3 skips.
/// Replace with trained weights to activate this level.

#[derive(serde::Deserialize)]
struct LogisticWeights {
    dim: usize,
    weights: Vec<f32>,
    bias: f32,
}

pub struct LogisticClassifier {
    w: LogisticWeights,
}

impl LogisticClassifier {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let data = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("CLASSIFIER_WEIGHTS_PATH '{}': {}", path, e))?;
        let w: LogisticWeights = serde_json::from_str(&data)?;
        if w.dim == 0 || w.weights.is_empty() {
            anyhow::bail!(
                "invalid weights: dim={} weights.len={}",
                w.dim,
                w.weights.len()
            );
        }
        if w.weights.len() != w.dim {
            anyhow::bail!("weights len {} != dim {}", w.weights.len(), w.dim);
        }
        // Placeholder: all-zero weights → skip this level
        let all_zero = w.weights.iter().all(|&x| x == 0.0) && w.bias == 0.0;
        if all_zero {
            anyhow::bail!(
                "weights are all zeros (placeholder) — replace {} with trained weights",
                path
            );
        }
        Ok(Self { w })
    }

    fn score(&self, emb: &[f32]) -> f32 {
        let z: f32 = emb
            .iter()
            .zip(self.w.weights.iter())
            .map(|(&e, &wt)| e * wt)
            .sum::<f32>()
            + self.w.bias;
        // Sigmoid: 1 / (1 + exp(-z))
        // Clamp to avoid overflow
        let z = z.clamp(-20.0, 20.0);
        1.0 / (1.0 + (-z).exp())
    }
}

use super::pipeline::ClassifierStage;
use super::{ClassifierLevel, ClassifyResult, Intent};
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::Mutex;

#[async_trait]
impl ClassifierStage for LogisticClassifier {
    fn name(&self) -> &'static str {
        "logistic"
    }

    fn level(&self) -> ClassifierLevel {
        ClassifierLevel::Logistic
    }

    async fn try_classify(
        &self,
        _text: &str,
        shared_emb: &Arc<Mutex<Option<Vec<f32>>>>,
        threshold: f32,
    ) -> Option<ClassifyResult> {
        let emb = shared_emb.lock().unwrap().clone()?;
        let p = self.score(&emb); // P(Complex)
        let confidence = (p - 0.5).abs() * 2.0; // map to [0,1] margin
        if confidence >= threshold {
            Some(ClassifyResult {
                intent: if p > 0.5 {
                    Intent::Complex
                } else {
                    Intent::Simple
                },
                level: ClassifierLevel::Logistic,
                confidence,
                matched_keyword: None,
            })
        } else {
            None
        }
    }
}
