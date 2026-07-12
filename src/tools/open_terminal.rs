use async_trait::async_trait;
use tracing::info;

use super::Tool;

/// Tool that opens macOS Terminal.app with OpenCode TUI for the user to see progress.
///
/// This tool is only registered on macOS when remote agents exist. It launches
/// a Terminal window running `opencode --dir {directory}` so the user can watch
/// the agent's progress in real time.
pub struct OpenTerminalTool {
    /// The working directory to open in OpenCode.
    pub directory: String,
}

#[async_trait]
impl Tool for OpenTerminalTool {
    fn name(&self) -> &str {
        "open_terminal"
    }

    fn description(&self) -> &str {
        "Abre OpenCode en una terminal para que el usuario vea el progreso"
    }

    fn should_force_for(&self, query: &str) -> bool {
        let lower = query.to_lowercase();
        lower.contains("abre opencode")
            || lower.contains("muestra la terminal")
            || lower.contains("open opencode terminal")
            || lower.contains("abre la terminal")
    }

    async fn run(&self, _args: &str) -> String {
        #[cfg(target_os = "macos")]
        {
            let escaped_dir = self.directory.replace('"', "\\\"");
            let osacmd = format!(
                r#"tell application "Terminal" to do script "clear && opencode --dir {}""#,
                escaped_dir,
            );

            info!(
                target: "tools",
                directory = %self.directory,
                "Launching Terminal with OpenCode TUI"
            );

            match std::process::Command::new("osascript")
                .arg("-e")
                .arg(&osacmd)
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(_) => {
                    info!(
                        target: "tools",
                        "Terminal launched successfully for OpenCode TUI"
                    );
                    "Abriendo OpenCode en la terminal...".to_string()
                }
                Err(e) => {
                    let msg = format!("Error al abrir la terminal: {e}");
                    tracing::warn!(target: "tools", "{msg}");
                    msg
                }
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            "Abrir terminal solo está disponible en macOS.".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_name_and_description() {
        let tool = OpenTerminalTool {
            directory: "/tmp/test".to_string(),
        };
        assert_eq!(tool.name(), "open_terminal");
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn should_force_for_abre_opencode() {
        let tool = OpenTerminalTool {
            directory: "/tmp".to_string(),
        };
        assert!(tool.should_force_for("abre opencode"));
    }

    #[test]
    fn should_force_for_muestra_la_terminal() {
        let tool = OpenTerminalTool {
            directory: "/tmp".to_string(),
        };
        assert!(tool.should_force_for("muestra la terminal"));
    }

    #[test]
    fn should_force_for_open_opencode_terminal() {
        let tool = OpenTerminalTool {
            directory: "/tmp".to_string(),
        };
        assert!(tool.should_force_for("open opencode terminal"));
    }

    #[test]
    fn should_force_for_abre_la_terminal() {
        let tool = OpenTerminalTool {
            directory: "/tmp".to_string(),
        };
        assert!(tool.should_force_for("abre la terminal"));
    }

    #[test]
    fn should_not_force_for_unrelated_query() {
        let tool = OpenTerminalTool {
            directory: "/tmp".to_string(),
        };
        assert!(!tool.should_force_for("¿qué hora es?"));
    }

    #[test]
    fn should_not_force_for_empty_query() {
        let tool = OpenTerminalTool {
            directory: "/tmp".to_string(),
        };
        assert!(!tool.should_force_for(""));
    }

    #[test]
    fn run_returns_acknowledgment() {
        let tool = OpenTerminalTool {
            directory: "/tmp/test".to_string(),
        };
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(tool.run(""));
        // On non-macOS, returns the "only on macOS" message.
        // On macOS, returns the acknowledgment (or error if opencode not in PATH).
        assert!(!result.is_empty());
    }
}
