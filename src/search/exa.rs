use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;
use tracing::{info, warn};

use super::{SearchProvider, SearchResult, format_results};

/// Exa (formerly Metaphor) Search API client.
///
/// Exa (https://exa.ai) provides a semantic search engine with high-quality
/// content extraction.  Requires an API key.
pub struct ExaProvider {
    api_key: String,
    client: reqwest::Client,
}

const EXA_TIMEOUT_SECS: u64 = 10;
const EXA_API_URL: &str = "https://api.exa.ai/search";

#[derive(Deserialize)]
struct ExaResponse {
    #[serde(default)]
    results: Vec<ExaResult>,
}

#[derive(Deserialize)]
struct ExaResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    text: String,
}

impl ExaProvider {
    pub fn new(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(EXA_TIMEOUT_SECS))
                .build()
                .expect("failed to build HTTP client for Exa"),
        }
    }
}

#[async_trait]
impl SearchProvider for ExaProvider {
    fn name(&self) -> &str {
        "exa"
    }

    async fn search(&self, query: &str, max_results: usize) -> String {
        if query.is_empty() {
            return "Error: no search query provided.".to_string();
        }

        info!(target: "search", "Exa search: query={:?} max_results={}", query, max_results);

        let body = serde_json::json!({
            "query": query,
            "numResults": max_results.min(10),
            "useAutoprompt": true,
            "type": "auto",
            "contents": {
                "text": { "maxCharacters": 2000 }
            }
        });

        let response = match self
            .client
            .post(EXA_API_URL)
            .header("x-api-key", &self.api_key)
            .header("accept", "application/json")
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!(target: "search", "Exa HTTP error: {}", e);
                if e.is_timeout() {
                    return "Error: search request timed out.".to_string();
                }
                return format!("Error: could not reach Exa API: {e}");
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            warn!(target: "search", "Exa HTTP {} — {}", status, body_text);
            return format!("Error: Exa API returned HTTP {status}");
        }

        let exa: ExaResponse = match response.json().await {
            Ok(b) => b,
            Err(e) => {
                warn!(target: "search", "Exa JSON parse error: {}", e);
                return format!("Error: could not parse Exa response: {e}");
            }
        };

        if exa.results.is_empty() {
            return "No results found.".to_string();
        }

        let results: Vec<SearchResult> = exa
            .results
            .into_iter()
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                content: r.text,
            })
            .collect();

        format_results(&results, max_results)
    }
}
