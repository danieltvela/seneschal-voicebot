use async_trait::async_trait;
use serde_json::json;
use tracing::info;
use uuid::Uuid;

use super::Tool;
use crate::db::Database;

/// Searches the L2 (long-term) message archive using FTS5 full-text search.
///
/// Provides access to historical conversations consolidated out of the active
/// context window. Queries match against message content, memory entries,
/// and session summaries. Results are ranked by BM25 relevance and formatted
/// as markdown with rank, role, session, timestamp, snippet, and content.
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

        // Parse session_id: empty string -> None, valid UUID -> Some, otherwise error ignored.
        let session_uuid: Option<Uuid> = match &params.session_id {
            Some(s) if !s.trim().is_empty() => Uuid::parse_str(s.trim()).ok(),
            _ => None,
        };

        match &self.db {
            Some(db) => {
                let results = match db
                    .search_messages(&params.query, session_uuid, params.limit, 0)
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        return format!("Error de base de datos: {e}");
                    }
                };

                if results.is_empty() {
                    return "No se encontraron mensajes hist\u{00f3}ricos que coincidan con la b\u{00fa}squeda.".to_string();
                }

                let mut output = "## Resultados de búsqueda histórica\n\n".to_string();
                output += &format!("**Consulta:** {}\n\n", params.query);
                output += &format!("**Total:** {} resultado(s)\n\n---\n\n", results.len());

                for (i, res) in results.iter().enumerate() {
                    output += &format!(
                        "{}. **Rango:** {:.4} | **Rol:** {} | **Sesi\u{00f3}n:** {} | **Fecha:** {}\n\n",
                        i + 1,
                        res.rank,
                        res.role,
                        &res.session_id[..8],
                        res.timestamp
                    );
                    output += &format!("**Fragmento:** {}\n\n", res.snippet);
                    output += &format!("**Contenido:** {}\n\n---\n\n", res.content);
                }
                output
            }
            None => {
                "Base de datos no disponible. No se puede buscar en el archivo hist\u{00f3}rico."
                    .to_string()
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
    async fn run_returns_no_results_for_empty_db() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::new(path.to_str().unwrap())
            .await
            .expect("failed to create test database");
        let tool = RecoverHistoricalContextTool::new(Some(db));
        let result = tool
            .run(r#"{"query": "nonexistent_query_xyz", "limit": 5}"#)
            .await;
        assert!(
            result.contains("No se encontraron mensajes"),
            "should return no results message: {result:?}"
        );
    }

    #[tokio::test]
    async fn run_uses_default_limit_when_omitted() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::new(path.to_str().unwrap())
            .await
            .expect("failed to create test database");
        let sid = db.get_or_create_session().await.unwrap();
        db.save_message(sid, "user", "test message content")
            .await
            .unwrap();

        let tool = RecoverHistoricalContextTool::new(Some(db));
        let result = tool.run(r#"{"query": "test"}"#).await;
        assert!(
            result.contains("Total:"),
            "should contain total results count: {result:?}"
        );
        assert!(
            result.contains("resultado"),
            "should contain resultado(s): {result:?}"
        );
    }

    #[tokio::test]
    async fn run_accepts_session_id() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::new(path.to_str().unwrap())
            .await
            .expect("failed to create test database");
        let sid = db.get_or_create_session().await.unwrap();
        let sid_str = sid.to_string();
        db.save_message(sid, "user", "session test message")
            .await
            .unwrap();

        let tool = RecoverHistoricalContextTool::new(Some(db));
        let result = tool
            .run(&format!(
                r#"{{"query": "session", "session_id": "{}", "limit": 5}}"#,
                sid
            ))
            .await;
        assert!(
            result.contains(&sid_str[..8]),
            "should include session ID prefix: {result:?}"
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
            result.contains("No se encontraron mensajes"),
            "empty DB should return no results: {result:?}"
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
            result.contains("hola que tal"),
            "response should contain matching message content: {result:?}"
        );
        assert!(
            !result.contains("PLACEHOLDER"),
            "response must not contain PLACEHOLDER: {result:?}"
        );
    }

    #[tokio::test]
    async fn run_finds_messages_with_fts() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::new(path.to_str().unwrap())
            .await
            .expect("failed to create test database");
        let sid = db.get_or_create_session().await.unwrap();
        db.save_message(sid, "user", "mensaje sobre machine learning")
            .await
            .unwrap();
        db.save_message(sid, "assistant", "keyword para testing de busqueda")
            .await
            .unwrap();

        let tool = RecoverHistoricalContextTool::new(Some(db));
        let result = tool.run(r#"{"query": "keyword", "limit": 5}"#).await;
        assert!(
            result.contains("keyword"),
            "result should contain matching keyword: {result:?}"
        );
        assert!(
            result.contains("<b>"),
            "snippet should contain <b> highlight tags from FTS5: {result:?}"
        );
    }

    #[tokio::test]
    async fn run_with_session_id_filter() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::new(path.to_str().unwrap())
            .await
            .expect("failed to create test database");
        let sid_a = db.get_or_create_session().await.unwrap();
        let sid_b = Uuid::new_v4();
        db.create_session(sid_b).await.unwrap();
        db.save_message(sid_a, "user", "unique alpha message content")
            .await
            .unwrap();
        db.save_message(sid_b, "user", "unique beta message content")
            .await
            .unwrap();

        let tool = RecoverHistoricalContextTool::new(Some(db));
        let result = tool
            .run(&format!(
                r#"{{"query": "unique", "session_id": "{}", "limit": 5}}"#,
                sid_a
            ))
            .await;
        assert!(
            result.contains("alpha"),
            "should contain messages from session A: {result:?}"
        );
        assert!(
            !result.contains("beta"),
            "should NOT contain messages from session B: {result:?}"
        );
    }
}
