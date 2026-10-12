use async_trait::async_trait;

use super::Tool;

/// Maximum entries returned from a single directory listing.
const MAX_ENTRIES: usize = 200;

/// Lists the entries of a directory (names plus file/dir marker).
///
/// Output is capped at `MAX_ENTRIES` entries with a truncation notice.
pub struct ListDirTool;

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "Lists the entries of a directory, marking files and directories. \
         Use when the user asks what is inside a folder or to explore a path. \
         Output is capped at 200 entries."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute or home-relative (~) path to the directory."
                }
            },
            "required": ["path"]
        })
    }

    async fn run(&self, args: &str) -> String {
        let raw_path = match serde_json::from_str::<serde_json::Value>(args) {
            Ok(v) => match v["path"].as_str() {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => return "Error: no directory path provided.".to_string(),
            },
            Err(_) => args.trim().to_string(),
        };
        if raw_path.is_empty() {
            return "Error: no directory path provided.".to_string();
        }
        let path = expand_tilde(&raw_path);

        let mut entries = match tokio::fs::read_dir(&path).await {
            Ok(e) => e,
            Err(e) => return format!("Cannot list '{path}': {e}"),
        };

        let mut lines: Vec<String> = Vec::new();
        let mut truncated = false;
        while let Some(entry) = match entries.next_entry().await {
            Ok(e) => e,
            Err(e) => return format!("Error reading '{path}': {e}"),
        } {
            if lines.len() >= MAX_ENTRIES {
                truncated = true;
                break;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let marker = if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                "/"
            } else {
                ""
            };
            lines.push(format!("{marker}{name}"));
        }

        if lines.is_empty() {
            return format!("'{path}' is empty.");
        }
        lines.sort();
        let mut out = lines.join("\n");
        if truncated {
            out.push_str(&format!("\n[... truncated at {MAX_ENTRIES} entries]"));
        }
        out
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
    fn name_is_list_dir() {
        assert_eq!(ListDirTool.name(), "list_dir");
    }

    #[tokio::test]
    async fn lists_entries_with_dir_marker() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("a.txt"), "x")
            .await
            .unwrap();
        tokio::fs::create_dir(dir.path().join("sub")).await.unwrap();

        let result = ListDirTool
            .run(&format!(r#"{{"path": "{}"}}"#, dir.path().display()))
            .await;
        assert!(result.contains("a.txt"), "{result:?}");
        assert!(result.contains("/sub"), "{result:?}");
        assert!(!result.contains("truncated"), "{result:?}");
    }

    #[tokio::test]
    async fn empty_dir_message() {
        let dir = tempfile::tempdir().unwrap();
        let result = ListDirTool
            .run(&format!(r#"{{"path": "{}"}}"#, dir.path().display()))
            .await;
        assert!(result.contains("is empty"), "{result:?}");
    }

    #[tokio::test]
    async fn missing_dir_error() {
        let result = ListDirTool
            .run(r#"{"path": "/tmp/nonexistent_seneschal_dir_xyz"}"#)
            .await;
        assert!(result.to_lowercase().contains("cannot list"), "{result:?}");
    }

    #[tokio::test]
    async fn truncates_large_listings() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..(MAX_ENTRIES + 10) {
            tokio::fs::write(dir.path().join(format!("f{i}.txt")), "x")
                .await
                .unwrap();
        }
        let result = ListDirTool
            .run(&format!(r#"{{"path": "{}"}}"#, dir.path().display()))
            .await;
        assert!(result.contains("truncated"), "{result:?}");
    }
}
