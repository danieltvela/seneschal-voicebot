//! # Search Provider — pluggable search backends
//!
//! Defines the [`SearchProvider`] trait that `quick_search` uses.
//! Backends: Brave (default, free), Tavily, Exa, SearXNG.

pub mod brave;
pub mod exa;
pub mod searxng;
pub mod tavily;

#[cfg(test)]
mod tests;

use async_trait::async_trait;

/// Result from a search query — structured for LLM consumption.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub content: String,
}

/// Pluggable search backend.
///
/// Every provider implements `search()` which returns a formatted string
/// ready for the LLM (or the user). The string is pre-truncated so the
/// caller does not need to worry about output size.
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Human-readable name shown in logs / tool descriptions.
    fn name(&self) -> &str;

    /// Execute a search and return results as a formatted, LLM-ready string.
    ///
    /// `query` — the user's search string.  
    /// `max_results` — maximum number of results the caller wants (the provider
    /// may return fewer).
    async fn search(&self, query: &str, max_results: usize) -> String;
}

/// Maximum output bytes for a single search response.
const MAX_OUTPUT_BYTES: usize = 4_000;

/// Helper: format a slice of [`SearchResult`] as a numbered list.
pub fn format_results(results: &[SearchResult], max: usize) -> String {
    let mut out = String::new();
    for (i, r) in results.iter().take(max).enumerate() {
        let entry = format!("{}. {}\n   {}\n   {}\n\n", i + 1, r.title, r.content, r.url);
        if out.len() + entry.len() > MAX_OUTPUT_BYTES {
            break;
        }
        out.push_str(&entry);
    }
    out.trim_end().to_string()
}

// ── Convenience ──────────────────────────────────────────────────────────────

/// Build the search provider from the application config.
///
/// Returns `None` when no search provider is configured (the `quick_search`
/// tool should not be registered).
///
/// ## Priority (first match wins)
///
/// 1. **Brave** (public scraper, no API key) — the default, free provider.
///    Disabled when `BRAVE_PUBLIC_SEARCH=0`.
/// 2. **Tavily** (API key) — low-latency, LLM-optimised.
/// 3. **Exa** (API key) — semantic search with content extraction.
/// 4. **SearXNG** (self-hosted) — fallback for self-hosted deployments.
pub fn from_config(config: &crate::config::Config) -> Option<Box<dyn SearchProvider>> {
    // 1. Brave public search — always available, no key required.
    if config.brave_public_search_enabled {
        let prov = brave::BraveProvider::new();
        return Some(Box::new(prov));
    }

    // 2. Native API providers (Tavily, Exa) when API keys are set.
    if let Some(key) = &config.tavily_api_key
        && !key.is_empty()
    {
        let prov = tavily::TavilyProvider::new(key, config.tavily_max_tokens);
        return Some(Box::new(prov));
    }

    if let Some(key) = &config.exa_api_key
        && !key.is_empty()
    {
        let prov = exa::ExaProvider::new(key);
        return Some(Box::new(prov));
    }

    // 3. Fall back to SearXNG (self-hosted, no API key).
    if let Some(url) = &config.searxng_url {
        let prov = searxng::SearXngProvider::new(url.clone(), config.searxng_secret.clone());
        return Some(Box::new(prov));
    }

    None
}
