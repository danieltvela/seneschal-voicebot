# src/search/ — Search Providers

## Responsibility

Provides a **pluggable search backend** via the `SearchProvider` trait. Three concrete implementations (Tavily, Exa, SearXNG) allow the `quick_search` tool to fetch web results without depending on a single provider. Priority selection: Tavily > Exa > SearXNG, based on which API keys/URLs are configured.

## Design

### `SearchProvider` Trait

```rust
#[async_trait]
pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn search(&self, query: &str, max_results: usize) -> String;
}
```

- `name()` — Human-readable provider name for logs and tool descriptions.
- `search()` — Returns a formatted, LLM-ready string (numbered list of title/content/url). Pre-truncated to `MAX_OUTPUT_BYTES` (4 KB) by `format_results()`.

### `SearchResult`

Uniform result struct used by all providers:

```rust
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub content: String,
}
```

### `format_results()`

Shared helper that formats `&[SearchResult]` as a numbered list:
```
1. Title
   Content snippet
   URL

2. Title
   ...
```
Truncates at `MAX_OUTPUT_BYTES` (4 KB) to prevent overwhelming the LLM context.

### Provider Implementations

| Provider | API | Auth | Timeout | Special |
|----------|-----|------|---------|---------|
| `TavilyProvider` | `POST https://api.tavily.com/search` | `api_key` in body | 8s | Returns AI-generated `answer` field first; falls back to results |
| `ExaProvider` | `POST https://api.exa.ai/search` | `x-api-key` header | 10s | `useAutoprompt: true`, `type: auto`, extracts up to 2000 chars of text |
| `SearXngProvider` | `GET {base_url}/search?q=...&format=json` | `Authorization: Bearer` header | 10s | Self-hosted, no API key required |

### Provider Selection (`from_config()`)

```rust
pub fn from_config(config: &Config) -> Option<Box<dyn SearchProvider>>
```

Priority order:
1. **Tavily** — if `config.tavily_api_key` is set and non-empty.
2. **Exa** — if `config.exa_api_key` is set and non-empty.
3. **SearXNG** — if `config.searxng_url` is set.
4. **None** — no provider available; `quick_search` tool should not be registered.

### Response Handling Pattern

All providers follow the same error handling pattern:
1. Empty query → `"Error: no search query provided."`
2. HTTP error → timeout check → connection error message
3. Non-200 status → HTTP status code message
4. JSON parse error → parse error message
5. Empty results → `"No results found."`
6. Success → `format_results(&results, max_results)`

Tavily has an additional optimization: if the API returns a non-empty `answer` field (AI-generated concise answer) that fits within `MAX_OUTPUT_BYTES`, it returns the answer directly instead of formatting results.

## Flow

```
Config → search::from_config(config)
    → Check tavily_api_key → TavilyProvider::new(key, max_tokens)
    → Check exa_api_key → ExaProvider::new(key)
    → Check searxng_url → SearXngProvider::new(url, secret)
    → None (no provider)

QuickSearchTool.run(args)
    → Parse query, max_results from JSON args
    → provider.search(query, max_results)
        → HTTP request (POST or GET)
        → Parse response JSON
        → Convert to SearchResult[]
        → format_results(results, max_results)
    → Return String (synchronous, ~1-3 seconds)
```

## Integration

**Consumers**:
- `src/tools/quick_search.rs` — `QuickSearchTool` wraps `Arc<dyn SearchProvider>` and calls `search()`.
- `src/main.rs` — Calls `search::from_config()` to construct the provider, passes to `QuickSearchTool::new()`.

**Dependencies**:
- `src/config/Config` — `tavily_api_key`, `tavily_max_tokens`, `exa_api_key`, `searxng_url`, `searxng_secret`.
- `reqwest::Client` — HTTP client with per-provider timeout configuration.
- `serde::Deserialize` — Response parsing.
- `async_trait`, `tracing`.