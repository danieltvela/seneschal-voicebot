use anyhow::Result;

/// Parser for Gemma 4 tool calls.
///
/// Gemma 4 uses a proprietary tool-calling format with special tokens:
/// - `<|channel>thought\n<channel|>` — reasoning blocks
/// - `<|tool_call>` — start of a tool call
///
/// Known issues handled:
/// - **Derailment**: the model may start a tool call, reconsider, reason again,
///   and emit a new tool call. We discard partial/invalid calls and keep only
///   the last valid one.
/// - **Trailing spaces**: the 26B variant occasionally emits a trailing space
///   before the `<|tool_call>` tag.
/// - **Multiple `<|channel>` tokens**: consume all leading ones.
///
/// Strategy: accumulate the full response and parse it post-generation rather
/// than streaming interactive tool calls. This avoids the "intractable" problem
/// of sending deltas to the client before the model finalizes its decision.
pub struct Gemma4ToolCallParser;

impl Gemma4ToolCallParser {
    /// Parse a complete response string looking for a tool call.
    ///
    /// Returns `Some((name, args_json))` if a valid tool call is found,
    /// otherwise `None` (the entire text should be treated as content).
    pub fn parse(text: &str) -> Option<(String, String)> {
        let trimmed = text.trim();

        // Find the last occurrence of <|tool_call> — if the model derailed,
        // the last one is the one it "committed" to.
        let tool_call_marker = "<|tool_call>";
        let pos = trimmed.rfind(tool_call_marker)?;

        // Everything after the marker is the tool call JSON.
        let after_marker = trimmed[pos + tool_call_marker.len()..].trim();

        // Try to extract a JSON object from the beginning of the remaining text.
        let json_str = Self::extract_first_json_object(after_marker)?;

        // Parse the JSON to ensure it has the required fields.
        let parsed: serde_json::Value = serde_json::from_str(json_str).ok()?;
        let name = parsed["name"].as_str()?.to_string();
        let args = parsed["arguments"].as_object()?;

        if name.is_empty() {
            return None;
        }

        let args_json = serde_json::to_string(args).ok()?;
        Some((name, args_json))
    }

    /// Strip reasoning blocks from the text so they don't reach TTS.
    ///
    /// Removes `<|channel>...<channel|>` blocks (including multiple leading
    /// `<|channel>` tokens) and trailing spaces before `<|tool_call>`.
    pub fn strip_reasoning(text: &str) -> String {
        let mut result = text.to_string();

        // Remove <|channel>...<channel|> blocks.
        loop {
            match (result.find("<|channel>"), result.find("<channel|>")) {
                (Some(start), Some(end)) if start <= end => {
                    let before = &result[..start];
                    let after = &result[end + "<channel|>".len()..];
                    result = format!("{}{}", before, after);
                }
                _ => break,
            }
        }

        // Clean up trailing space before tool call marker.
        result = result.replace(" <|tool_call>", "<|tool_call>");

        result.trim().to_string()
    }

    /// Extract the first JSON object from a string.
    ///
    /// Handles simple cases where the object is followed by more text.
    fn extract_first_json_object(s: &str) -> Option<&str> {
        let start = s.find('{')?;
        let mut brace_count = 0;
        let mut in_string = false;
        let mut escape = false;

        for (i, c) in s[start..].char_indices() {
            match c {
                '\\' if !escape => escape = true,
                '"' if !escape => in_string = !in_string,
                '{' if !in_string => brace_count += 1,
                '}' if !in_string => {
                    brace_count -= 1;
                    if brace_count == 0 {
                        return Some(&s[start..=start + i]);
                    }
                }
                _ => escape = false,
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tool_call() {
        let text = r#"I'll check the weather for you.<|tool_call>{"name": "get_weather", "arguments": {"city": "Paris"}}"#;
        let result = Gemma4ToolCallParser::parse(text);
        assert!(result.is_some());
        let (name, args) = result.unwrap();
        assert_eq!(name, "get_weather");
        assert_eq!(args, r#"{"city":"Paris"}"#);
    }

    #[test]
    fn test_parse_with_reasoning() {
        let text = r#"<|channel>thought
Let me check the weather.<channel|><|tool_call>{"name": "get_weather", "arguments": {"city": "Paris"}}"#;
        let result = Gemma4ToolCallParser::parse(text);
        assert!(result.is_some());
        let (name, args) = result.unwrap();
        assert_eq!(name, "get_weather");
        assert_eq!(args, r#"{"city":"Paris"}"#);
    }

    #[test]
    fn test_parse_no_tool_call() {
        let text = "The weather in Paris is sunny today.";
        let result = Gemma4ToolCallParser::parse(text);
        assert!(result.is_none());
    }

    #[test]
    fn test_strip_reasoning() {
        let text = r#"<|channel>thought
I need to check this.<channel|> The answer is 42."#;
        let cleaned = Gemma4ToolCallParser::strip_reasoning(text);
        assert!(!cleaned.contains("<|channel>"));
        assert!(!cleaned.contains("<channel|>"));
        assert!(cleaned.contains("The answer is 42."));
    }

    #[test]
    fn test_derailment_uses_last_tool_call() {
        let text = r#"<|tool_call>{"name": "old_tool", "arguments": {}} some text <|tool_call>{"name": "new_tool", "arguments": {"x": 1}}"#;
        let result = Gemma4ToolCallParser::parse(text);
        assert!(result.is_some());
        let (name, _) = result.unwrap();
        assert_eq!(name, "new_tool");
    }
}
