use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;
use tracing::{info, warn};

use super::{SearchProvider, SearchResult, format_results};

/// Adapter that adapts an existing SearXNG instance to the [`SearchProvider`] trait.
///
/// This is the same backend the existing `WebSearchTool` uses, exposed through
/// the unified trait so it can serve as a fallback when no native API key is set.
pub struct SearXngProvider {
    base_url: String,
    secret: String,
    client: reqwest::Client,
}

const SEARXNG_TIMEOUT_SECS: u64 = 10;

#[derive(Deserialize)]
struct SearxResponse {
    #[serde(default)]
    results: Vec<SearxResult>,
}

#[derive(Deserialize)]
struct SearxResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
}

impl SearXngProvider {
    pub fn new(base_url: String, secret: String) -> Self {
        Self {
            base_url,
            secret,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(SEARXNG_TIMEOUT_SECS))
                .build()
                .expect("failed to build HTTP client for SearXNG"),
        }
    }
}

#[async_trait]
impl SearchProvider for SearXngProvider {
    fn name(&self) -> &str {
        "searxng"
    }

    async fn search(&self, query: &str, max_results: usize) -> String {
        if query.is_empty() {
            return "Error: no search query provided.".to_string();
        }

        info!(target: "search", "SearXNG search: query={:?} max_results={}", query, max_results);

        let url = format!("{}/search", self.base_url.trim_end_matches('/'));
        let mut req = self
            .client
            .get(&url)
            .query(&[("q", query), ("format", "json")]);

        if !self.secret.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.secret));
        }

        let response = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                warn!(target: "search", "SearXNG HTTP error: {}", e);
                if e.is_timeout() {
                    return "Error: search request timed out.".to_string();
                }
                return format!("Error: could not reach SearXNG service: {e}");
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            warn!(target: "search", "SearXNG HTTP {}", status);
            return format!("Error: search service returned HTTP {status}");
        }

        let body: SearxResponse = match response.json().await {
            Ok(b) => b,
            Err(e) => {
                warn!(target: "search", "SearXNG JSON parse error: {}", e);
                return format!("Error: could not parse search results: {e}");
            }
        };

        if body.results.is_empty() {
            return "No results found.".to_string();
        }

        let results: Vec<SearchResult> = body
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
