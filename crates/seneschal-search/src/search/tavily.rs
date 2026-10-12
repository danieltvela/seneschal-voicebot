use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;
use tracing::{info, warn};

use super::{MAX_OUTPUT_BYTES, SearchProvider, SearchResult, format_results};

/// Tavily Search API client.
///
/// Tavily (https://tavily.com) is a search engine purpose-built for LLMs.
/// It returns concise, structured results that require minimal post-processing.
pub struct TavilyProvider {
    api_key: String,
    max_tokens: usize,
    client: reqwest::Client,
}

/// Hard timeout for Tavily HTTP requests.
const TAVILY_TIMEOUT_SECS: u64 = 8;

/// Tavily Search API endpoint.
const TAVILY_API_URL: &str = "https://api.tavily.com/search";

/// Default max tokens for Tavily answer generation (0 = disabled).
const DEFAULT_MAX_TOKENS: usize = 512;

// ── API response shapes ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct TavilyResponse {
    #[serde(default)]
    answer: Option<String>,
    #[serde(default)]
    results: Vec<TavilyResult>,
}

#[derive(Deserialize)]
struct TavilyResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
}

impl TavilyProvider {
    pub fn new(api_key: &str, max_tokens: usize) -> Self {
        let max_tokens = if max_tokens == 0 {
            DEFAULT_MAX_TOKENS
        } else {
            max_tokens
        };
        Self {
            api_key: api_key.to_string(),
            max_tokens,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(TAVILY_TIMEOUT_SECS))
                .build()
                .expect("failed to build HTTP client for Tavily"),
        }
    }
}

#[async_trait]
impl SearchProvider for TavilyProvider {
    fn name(&self) -> &str {
        "tavily"
    }

    async fn search(&self, query: &str, max_results: usize) -> String {
        if query.is_empty() {
            return "Error: no search query provided.".to_string();
        }

        info!(target: "search", "Tavily search: query={:?} max_results={}", query, max_results);

        let body = serde_json::json!({
            "api_key": self.api_key,
            "query": query,
            "max_results": max_results.min(10),    // Tavily hard cap
            "include_answer": true,
            "include_raw_content": false,
            "max_tokens": self.max_tokens,
        });

        let response = match self.client.post(TAVILY_API_URL).json(&body).send().await {
            Ok(r) => r,
            Err(e) => {
                warn!(target: "search", "Tavily HTTP error: {}", e);
                if e.is_timeout() {
                    return "Error: search request timed out.".to_string();
                }
                return format!("Error: could not reach Tavily API: {e}");
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            warn!(target: "search", "Tavily HTTP {} — {}", status, body_text);
            return format!("Error: Tavily API returned HTTP {status}");
        }

        let tavily: TavilyResponse = match response.json().await {
            Ok(b) => b,
            Err(e) => {
                warn!(target: "search", "Tavily JSON parse error: {}", e);
                return format!("Error: could not parse Tavily response: {e}");
            }
        };

        // If Tavily generated a concise answer, return it directly — it's
        // exactly what the LLM needs for quick facts.
        if let Some(answer) = &tavily.answer
            && !answer.is_empty()
            && answer.len() < MAX_OUTPUT_BYTES
        {
            info!(target: "search", "Tavily: returned AI-generated answer ({} chars)", answer.len());
            return answer.clone();
        }

        if tavily.results.is_empty() {
            return "No results found.".to_string();
        }

        // Convert to uniform SearchResult and format.
        let results: Vec<SearchResult> = tavily
            .results
            .into_iter()
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                content: r.content,
            })
            .collect();

        format_results(&results, max_results)
    }
}
