use async_trait::async_trait;

use super::Tool;

/// A NOOP tool that suppresses any LLM response.
///
/// When called, the pipeline stops the current query and the user does not
/// receive any audio response. The tool's description is dynamically set from
/// the `NOOP_TOOL_INSTRUCTIONS` environment variable to tell the LLM when
/// this tool should be invoked (e.g. when the user asks something to Siri
/// or Alexa).
pub struct NoopTool {
    instructions: String,
}

impl NoopTool {
    pub fn new(instructions: String) -> Self {
        Self { instructions }
    }
}

#[async_trait]
impl Tool for NoopTool {
    fn name(&self) -> &str {
        "noop"
    }

    fn description(&self) -> &str {
        &self.instructions
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    fn is_silent(&self) -> bool {
        true
    }

    async fn run(&self, _args: &str) -> String {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> NoopTool {
        NoopTool::new("Call this tool when the user asks something to Siri or Alexa.".to_string())
    }

    #[test]
    fn name_is_noop() {
        assert_eq!(tool().name(), "noop");
    }

    #[test]
    fn description_is_non_empty() {
        assert!(!tool().description().is_empty());
    }

    #[test]
    fn is_silent_returns_true() {
        assert!(tool().is_silent());
    }

    #[tokio::test]
    async fn run_returns_empty_string() {
        let result = tool().run("").await;
        assert!(result.is_empty());
    }
}
