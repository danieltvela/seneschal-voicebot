# Search Providers — Architecture Reference

The search module (`src/search/`) defines a pluggable `SearchProvider` trait used by the `quick_search` tool. Four backends are available, selected by priority in a factory function.

## Core API

### SearchResult

```rust
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub content: String,
}
```

### SearchProvider Trait

```rust
#[async_trait]
pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn search(&self, query: &str, max_results: usize) -> String;
}
```

- `name()` — short identifier (`"brave"`, `"tavily"`, `"exa"`, `"searxng"`).
- `search()` — executes the query and returns a **pre-truncated, formatted string** suitable for LLM context injection. The caller does no additional truncation.

### Factory

```rust
pub fn from_config(config: &crate::config::Config) -> Option<Box<dyn SearchProvider>>;
```

Returns `None` when no provider is configured. Selection priority (first match wins):

| Priority | Provider | Config field | Requires key? |
|----------|----------|-------------|---------------|
| 1 | Brave (public scraper) | `brave_public_search_enabled` (default `true`) | No |
| 2 | Tavily | `tavily_api_key` is set | Yes |
| 3 | Exa | `exa_api_key` is set | Yes |
| 4 | SearXNG (self-hosted) | `searxng_url` is set | No |

**Brave is the default free provider.** Disabled via `BRAVE_PUBLIC_SEARCH=0`.

### Helper

```rust
const MAX_OUTPUT_BYTES: usize = 4_000;

pub fn format_results(results: &[SearchResult], max: usize) -> String;
```

Formats up to `max` results as a numbered list. Truncates the full string to 4 000 bytes. Each entry:
```
1. <title>
   <content>
   <url>
```

## Backend Details

### Brave (public scraper)

```rust
// src/search/brave.rs
pub struct BraveProvider;
```

- **Endpoint:** `GET https://search.brave.com/search?q={query}`
- **Auth:** None. Uses a desktop Chrome `User-Agent` header.
- **Parsing:** HTML via `scraper` crate. CSS selectors: `div.result-wrapper`, `a[href]`, `div.title`, `div.generic-snippet > div.content`.
- **Timeout:** 15 s
- **Rate limiting:** None built-in.
- **Env var:** `BRAVE_PUBLIC_SEARCH=0` to disable.

### Tavily

```rust
// src/search/tavily.rs
pub struct TavilyProvider { api_key: String }
```

- **Endpoint:** `POST https://api.tavily.com/search`
- **Auth:** `api_key` field in JSON body.
- **Body:** `{api_key, query, max_results, include_answer: true, max_tokens}`
- **Response:** Returns Tavily's AI-generated `answer` directly if short enough; otherwise formats results.
- **Timeout:** 8 s
- **Max results:** 10 (Tavily hard cap)
- **Env var:** `TAVILY_API_KEY`

### Exa

```rust
// src/search/exa.rs
pub struct ExaProvider { api_key: String }
```

- **Endpoint:** `POST https://api.exa.ai/search`
- **Auth:** `x-api-key` header.
- **Body:** `{query, numResults, useAutoprompt: true, type: "auto", contents: {text: {maxCharacters: 2000}}}`
- **Features:** Semantic search + content extraction.
- **Timeout:** 10 s
- **Env var:** `EXA_API_KEY`

### SearXNG (self-hosted)

```rust
// src/search/searxng.rs
pub struct SearXngProvider { base_url: String, secret: Option<String> }
```

- **Endpoint:** `GET {base_url}/search?q={query}&format=json`
- **Auth:** Optional `Authorization: Bearer {secret}` header.
- **Timeout:** 10 s
- **Env vars:** `SEARXNG_URL`, `SEARXNG_SECRET`

## Error Handling

All providers return errors as formatted strings rather than panicking. A failure to search produces a text response like `"[brave] search failed: <error>"` which the LLM can relay to the user gracefully.
