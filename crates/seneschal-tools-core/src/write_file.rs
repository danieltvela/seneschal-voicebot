use async_trait::async_trait;

use super::Tool;

/// Maximum bytes accepted for a single file write (~32 KB).
const MAX_BYTES: usize = 32 * 1024;

/// Writes text content to a file, creating parent directories as needed.
///
/// Truncates or creates the target file. Output is capped at `MAX_BYTES`
/// to keep the LLM from dumping huge blobs through the tool channel.
pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Writes text content to a file at the given path (creates parent directories). \
         Use when the user asks to create, save, overwrite, or edit a file. \
         Content is capped at 32 KB per call."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute or home-relative (~) path to the file."
                },
                "content": {
                    "type": "string",
                    "description": "The full text content to write."
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn run(&self, args: &str) -> String {
        let v = match serde_json::from_str::<serde_json::Value>(args) {
            Ok(v) => v,
            Err(_) => return "Error: expected JSON args {\"path\", \"content\"}".to_string(),
        };
        let path = match v["path"].as_str() {
            Some(s) if !s.is_empty() => expand_tilde(s),
            _ => return "Error: no file path provided.".to_string(),
        };
        let content = match v["content"].as_str() {
            Some(s) => s,
            None => return "Error: no content provided.".to_string(),
        };

        if content.len() > MAX_BYTES {
            return format!(
                "Error: content is {} KB, exceeding the {} KB per-call limit. Write it in chunks.",
                content.len() / 1024,
                MAX_BYTES / 1024
            );
        }

        if let Some(parent) = std::path::Path::new(&path).parent()
            && !parent.as_os_str().is_empty()
            && let Err(e) = tokio::fs::create_dir_all(parent).await
        {
            return format!("Cannot create parent directory for '{path}': {e}");
        }

        match tokio::fs::write(&path, content).await {
            Ok(()) => format!("Wrote {} bytes to '{path}'.", content.len()),
            Err(e) => format!("Cannot write '{path}': {e}"),
        }
    }
}

/// Expands a leading `~` to the user's home directory.
fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return format!("{home}/{rest}");
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_write_file() {
        assert_eq!(WriteFileTool.name(), "write_file");
    }

    #[tokio::test]
    async fn writes_content_and_creates_parents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/out.txt");

        let result = WriteFileTool
            .run(&format!(
                r#"{{"path": "{}", "content": "hola"}}"#,
                path.display()
            ))
            .await;
        assert!(result.contains("Wrote 4 bytes"), "{result:?}");

        let read = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(read, "hola");
    }

    #[tokio::test]
    async fn overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        tokio::fs::write(&path, "old").await.unwrap();

        WriteFileTool
            .run(&format!(
                r#"{{"path": "{}", "content": "new"}}"#,
                path.display()
            ))
            .await;
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "new");
    }

    #[tokio::test]
    async fn rejects_oversized_content() {
        let big = "x".repeat(MAX_BYTES + 10);
        let args = serde_json::json!({"path": "/tmp/dw_test_big.txt", "content": big}).to_string();
        let result = WriteFileTool.run(&args).await;
        assert!(result.contains("exceeding"), "{result:?}");
    }

    #[tokio::test]
    async fn missing_fields_return_error() {
        assert!(
            WriteFileTool
                .run(r#"{"path": "/tmp/x"}"#)
                .await
                .contains("no content")
        );
        assert!(
            WriteFileTool
                .run(r#"{"content": "x"}"#)
                .await
                .contains("no file path")
        );
    }
}
