use anyhow::{Context, Result};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use super::{ClassifierLevel, ClassifyResult, Intent};
use super::pipeline::ClassifierStage;

/// Calls a second LLM endpoint (SLM) to classify the request as SIMPLE or COMPLEX.
pub struct FallbackClassifier {
    client: reqwest::Client,
    url: String,
    model: String,
    api_key: String,
}

impl FallbackClassifier {
    pub fn new(url: &str, model: &str, api_key: &str, timeout_ms: u64) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_millis(timeout_ms))
                .build()
                .expect("reqwest Client builder"),
            url: format!("{}/v1/chat/completions", url.trim_end_matches('/')),
            model: model.to_string(),
            api_key: api_key.to_string(),
        }
    }

    /// Calls the SLM with a restrictive prompt and `max_tokens=1`.
    ///
    /// TODO: Add `logit_bias` or grammar to force output to exactly "SIMPLE" or "COMPLEX".
    pub async fn classify(&self, text: &str) -> Result<Intent> {
        let sys = "You are an intent classifier. Respond with EXACTLY one token: \"SIMPLE\" for casual/greeting/trivial requests, or \"COMPLEX\" for requests requiring tools, reasoning, code, search, or multi-step tasks. No punctuation, no explanation.";
        let payload = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": sys},
                {"role": "user", "content": text}
            ],
            "max_tokens": 1,
            "temperature": 0.0,
            "stream": false,
        });
        let mut req = self.client.post(&self.url).json(&payload);
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }
        let resp = req
            .send()
            .await
            .context("fallback SLM request failed")?;
        if !resp.status().is_success() {
            anyhow::bail!("fallback SLM status {}", resp.status());
        }
        let json: serde_json::Value = resp.json().await?;
        let txt = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
        let lower = txt.to_lowercase();
        if lower.starts_with("simple") {
            Ok(Intent::Simple)
        } else if lower.starts_with("complex") {
            Ok(Intent::Complex)
        } else {
            anyhow::bail!("fallback SLM ambiguous output: {txt:?}")
        }
    }
}

/// Wraps `FallbackClassifier` as a cascade stage.
pub struct FallbackStage {
    inner: FallbackClassifier,
    enabled: bool,
}

impl FallbackStage {
    pub fn new(inner: FallbackClassifier, enabled: bool) -> Self {
        Self { inner, enabled }
    }
}

#[async_trait]
impl ClassifierStage for FallbackStage {
    fn name(&self) -> &'static str {
        "fallback"
    }

    fn level(&self) -> ClassifierLevel {
        ClassifierLevel::Fallback
    }

    async fn try_classify(
        &self,
        text: &str,
        _shared_emb: &Arc<Mutex<Option<Vec<f32>>>>,
        _threshold: f32,
    ) -> Option<ClassifyResult> {
        if !self.enabled {
            return None;
        }
        match self.inner.classify(text).await {
            Ok(intent) => Some(ClassifyResult {
                intent,
                level: ClassifierLevel::Fallback,
                confidence: 1.0,
                matched_keyword: None,
            }),
            Err(e) => {
                tracing::warn!(
                    target: "classifier",
                    "fallback stage failed: {e}"
                );
                None
            }
        }
    }
}
