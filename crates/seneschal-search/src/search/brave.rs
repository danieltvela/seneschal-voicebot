use async_trait::async_trait;
use std::time::Duration;
use tracing::{info, warn};

use super::{SearchProvider, SearchResult, format_results};

/// Brave Search public scraper.
///
/// This provider queries the public `search.brave.com` HTML page and extracts
/// results via CSS selectors.  **No API key required.**
///
/// Because Brave currently serves fully server-rendered HTML, a simple GET
/// request with a desktop User-Agent is sufficient.  The selectors target
/// stable SvelteKit class names that have not changed since early 2025.
///
/// When Brave adds bot detection or changes their markup the provider will
/// degrade gracefully: errors are surfaced as formatted error strings rather
/// than panics, and the caller can fall back to another provider.
pub struct BraveProvider {
    client: reqwest::Client,
}

const BRAVE_TIMEOUT_SECS: u64 = 15;
const BRAVE_SEARCH_URL: &str = "https://search.brave.com/search";

/// Desktop Chrome UA — required for Brave to return server-rendered HTML
/// instead of a SPA shell.  Matches the user-agent used by the reference
/// implementation in `badlogic/agent-tools`.
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

impl BraveProvider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(BRAVE_TIMEOUT_SECS))
                .build()
                .expect("failed to build HTTP client for Brave"),
        }
    }
}

impl Default for BraveProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl BraveProvider {
    /// Fetch the Brave search results page and parse out title/url/content.
    async fn fetch(&self, query: &str, max_results: usize) -> anyhow::Result<Vec<SearchResult>> {
        let response = self
            .client
            .get(BRAVE_SEARCH_URL)
            .query(&[("q", query)])
            .header("User-Agent", USER_AGENT)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("Accept-Language", "en-US,en;q=0.9")
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(anyhow::anyhow!("Brave returned HTTP {status}"));
        }

        let html = response.text().await?;
        let doc = scraper::Html::parse_document(&html);

        let result_sel = scraper::Selector::parse("div.result-wrapper").unwrap();
        let link_sel = scraper::Selector::parse("a.svelte-14r20fy[href]").unwrap();
        let title_sel = scraper::Selector::parse("div.title").unwrap();
        let snippet_sel = scraper::Selector::parse("div.generic-snippet > div.content").unwrap();

        let mut hits = Vec::new();
        for r in doc.select(&result_sel) {
            if hits.len() >= max_results {
                break;
            }

            // Extract link
            let link = match r.select(&link_sel).next() {
                Some(a) => a.value().attr("href").unwrap_or("").to_string(),
                None => continue,
            };
            // Skip internal Brave links and empty hrefs.
            if link.is_empty() || link.contains("brave.com") {
                continue;
            }

            // Extract title
            let title: String = match r.select(&title_sel).next() {
                Some(el) => el.text().collect::<String>().trim().to_string(),
                None => continue,
            };
            if title.is_empty() {
                continue;
            }

            // Extract snippet
            let snippet: String = match r.select(&snippet_sel).next() {
                Some(el) => el.text().collect::<String>().trim().to_string(),
                None => String::new(),
            };

            hits.push(SearchResult {
                title,
                url: link,
                content: snippet,
            });
        }

        Ok(hits)
    }
}

#[async_trait]
impl SearchProvider for BraveProvider {
    fn name(&self) -> &str {
        "brave"
    }

    async fn search(&self, query: &str, max_results: usize) -> String {
        if query.is_empty() {
            return "Error: no search query provided.".to_string();
        }

        info!(
            target: "search",
            "Brave search: query={:?} max_results={}",
            query,
            max_results
        );

        match self.fetch(query, max_results).await {
            Ok(results) if results.is_empty() => "No results found.".to_string(),
            Ok(results) => format_results(&results, max_results),
            Err(e) => {
                warn!(target: "search", "Brave HTTP error: {}", e);
                if e.to_string().contains("timeout") || e.to_string().contains("Timeout") {
                    return "Error: search request timed out.".to_string();
                }
                format!("Error: could not reach Brave Search: {e}")
            }
        }
    }
}
