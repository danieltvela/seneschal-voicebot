use async_trait::async_trait;
use serde_json::json;
use tracing::info;

use super::Tool;
use crate::db::Database;

/// Searches the L2 (long-term) message archive using FTS5 full-text search.
///
/// Provides access to historical conversations consolidated out of the active
/// context window. Queries match against message content, memory entries,
/// and session summaries.
///
/// Actual FTS5 integration will be implemented in T13. For now, this tool
/// validates parameters and returns a placeholder response.
pub struct RecoverHistoricalContextTool {
    db: Option<Database>,
}

impl RecoverHistoricalContextTool {
    /// Creates a new `RecoverHistoricalContextTool`.
    ///
    /// `db` is an optional database handle. When `Some`, the tool can query
    /// the L2 (long-term archive) through FTS5 full-text search. When `None`,
    /// it returns a helpful message indicating the database is not available.
    pub fn new(db: Option<Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Tool for RecoverHistoricalContextTool {
    fn name(&self) -> &str {
        "recover_historical_context"
    }

    fn description(&self) -> &str {
        "Busca mensajes hist\u{00f3}ricos en el archivo L2 (conversaciones antiguas consolidadas). \
         \u{00da}til cuando el usuario pregunta sobre algo que se habl\u{00f3} en el pasado lejano. \
         Recibe un texto de b\u{00fa}squeda y opcionalmente un l\u{00ed}mite de resultados y \
         un session_id para acotar la b\u{00fa}squeda a una sesi\u{00f3}n espec\u{00ed}fica."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Texto de b\u{00fa}squeda en el archivo hist\u{00f3}rico"
                },
                "session_id": {
                    "type": "string",
                    "description": "Opcional: ID de sesi\u{00f3}n para acotar la b\u{00fa}squeda"
                },
                "limit": {
                    "type": "integer",
                    "description": "N\u{00fa}mero m\u{00e1}ximo de resultados (por defecto 10)"
                }
            },
            "required": ["query"]
        })
    }

    async fn run(&self, args: &str) -> String {
        #[derive(serde::Deserialize)]
        struct Params {
            query: String,
            #[serde(default)]
            session_id: Option<String>,
            #[serde(default = "default_limit")]
            limit: usize,
        }

        fn default_limit() -> usize {
            10
        }

        let params: Params = match serde_json::from_str(args) {
            Ok(p) => p,
            Err(e) => {
                return format!("Error: no se pudieron analizar los par\u{00e1}metros: {e}");
            }
        };

        if params.query.trim().is_empty() {
            return "Error: no se proporcion\u{00f3} un texto de b\u{00fa}squeda.".to_string();
        }

        info!(
            target: "tools",
            "recover_historical_context: query={:?} session_id={:?} limit={}",
            params.query,
            params.session_id,
            params.limit
        );

        // TODO(T13): Implement actual FTS5 search against L2 archive.
        // When `db` is Some, call db.search_messages(query, session_id, limit).
        match &self.db {
            Some(_db) => {
                format!(
                    "[PLACEHOLDER] recover_historical_context searched for {:?} \
                     (limit={}, session_id={:?}). La integraci\u{00f3}n con la base \
                     de datos se implementar\u{00e1} en una versi\u{00f3}n futura.",
                    params.query, params.limit, params.session_id
                )
            }
            None => {
                format!(
                    "[PLACEHOLDER] recover_historical_context: base de datos no disponible. \
                     B\u{00fa}squeda solicitada: {:?} (limit={}, session_id={:?})",
                    params.query, params.limit, params.session_id
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_correct() {
        let tool = RecoverHistoricalContextTool::new(None);
        assert_eq!(tool.name(), "recover_historical_context");
    }

    #[test]
    fn description_is_non_empty() {
        let tool = RecoverHistoricalContextTool::new(None);
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn parameters_include_query_session_id_and_limit() {
        let tool = RecoverHistoricalContextTool::new(None);
        let params = tool.parameters();
        let properties = params["properties"].as_object().unwrap();
        assert!(properties.contains_key("query"), "must have query param");
        assert!(
            properties.contains_key("session_id"),
            "must have session_id param"
        );
        assert!(properties.contains_key("limit"), "must have limit param");
        assert_eq!(
            params["required"].as_array().unwrap(),
            &[serde_json::json!("query")],
            "only query should be required"
        );
    }

    #[tokio::test]
    async fn run_returns_error_for_empty_query() {
        let tool = RecoverHistoricalContextTool::new(None);
        let result = tool.run(r#"{"query": "", "limit": 5}"#).await;
        assert!(
            result.starts_with("Error:"),
            "should return error for empty query: {result:?}"
        );
    }

    #[tokio::test]
    async fn run_returns_error_for_missing_query() {
        let tool = RecoverHistoricalContextTool::new(None);
        let result = tool.run(r#"{"limit": 5}"#).await;
        assert!(
            result.starts_with("Error:"),
            "should return error when query is missing: {result:?}"
        );
    }

    #[tokio::test]
    async fn run_returns_placeholder_with_query() {
        let tool = RecoverHistoricalContextTool::new(None);
        let result = tool
            .run(r#"{"query": "machine learning", "limit": 5}"#)
            .await;
        assert!(
            result.contains("machine learning"),
            "response must contain the query: {result:?}"
        );
        assert!(
            result.contains("PLACEHOLDER"),
            "response must indicate placeholder: {result:?}"
        );
    }

    #[tokio::test]
    async fn run_uses_default_limit_when_omitted() {
        let tool = RecoverHistoricalContextTool::new(None);
        let result = tool.run(r#"{"query": "test"}"#).await;
        assert!(result.contains("limit=10"), "default limit should be 10");
    }

    #[tokio::test]
    async fn run_accepts_session_id() {
        let tool = RecoverHistoricalContextTool::new(None);
        let result = tool
            .run(r#"{"query": "test", "session_id": "abc-123"}"#)
            .await;
        assert!(
            result.contains("abc-123"),
            "should include session_id: {result:?}"
        );
    }

    #[tokio::test]
    async fn run_with_database_available() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::new(path.to_str().unwrap())
            .await
            .expect("failed to create test database");
        let tool = RecoverHistoricalContextTool::new(Some(db));
        let result = tool.run(r#"{"query": "hello", "limit": 3}"#).await;
        assert!(
            result.contains("hello"),
            "response should contain query: {result:?}"
        );
        assert!(
            result.contains("PLACEHOLDER"),
            "response should indicate placeholder even with DB: {result:?}"
        );
    }

    #[tokio::test]
    async fn run_with_database_and_messages() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::new(path.to_str().unwrap())
            .await
            .expect("failed to create test database");
        let sid = db.get_or_create_session().await.unwrap();
        db.save_message(sid, "user", "hola que tal").await.unwrap();
        db.save_message(sid, "assistant", "muy bien gracias")
            .await
            .unwrap();

        let tool = RecoverHistoricalContextTool::new(Some(db));
        let result = tool
            .run(r#"{"query": "hola", "limit": 5, "session_id": ""}"#)
            .await;
        assert!(
            result.contains("hola"),
            "response should contain query: {result:?}"
        );
        assert!(
            result.contains("PLACEHOLDER"),
            "response should indicate placeholder: {result:?}"
        );
    }
}
