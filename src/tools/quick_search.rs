use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tracing::info;

use super::Tool;
use crate::search::SearchProvider;

/// Fast-path web search tool.
///
/// Uses the configured [`SearchProvider`] (Tavily, Exa, or SearXNG) to fetch
/// results directly — no agent delegation overhead.
///
/// This tool is NOT background: it executes synchronously within the LLM turn
/// and returns results in ~1–3 seconds.  Use it for factual lookups, current
/// events, weather, quick definitions, etc.
pub struct QuickSearchTool {
    provider: Arc<dyn SearchProvider>,
}

impl QuickSearchTool {
    pub fn new(provider: Arc<dyn SearchProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl Tool for QuickSearchTool {
    fn name(&self) -> &str {
        "quick_search"
    }

    fn description(&self) -> &str {
        "Búsqueda web rápida para consultas factuales cortas. \
         Úsala cuando el usuario pregunte por información actual, \
         noticias, eventos recientes, datos concretos, definiciones, \
         o cualquier cosa que se pueda responder con una búsqueda simple. \
         NO la uses para investigación profunda o síntesis compleja \
         (usa deep_research para eso). \
         Respuesta en 1-3 segundos."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default 5)"
                }
            },
            "required": ["query"]
        })
    }

    fn is_background(&self) -> bool {
        false
    }

    async fn run(&self, args: &str) -> String {
        let (query, max_results) = match serde_json::from_str::<serde_json::Value>(args) {
            Ok(v) => {
                let q = v["query"].as_str().unwrap_or("").trim().to_string();
                let n = v["max_results"].as_u64().map(|n| n as usize).unwrap_or(5);
                (q, n)
            }
            Err(_) => (args.trim().to_string(), 5),
        };

        if query.is_empty() {
            return "Error: no search query provided.".to_string();
        }

        info!(target: "tools", "quick_search: query={:?} max_results={}", query, max_results);
        self.provider.search(&query, max_results).await
    }
}
