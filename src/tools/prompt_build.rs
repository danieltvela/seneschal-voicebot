use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::tools::Tool;

/// Whether prompt-build mode is active and what the current prompt text is.
#[derive(Debug, Clone, PartialEq)]
pub enum PromptBuildState {
    Inactive,
    Active { prompt: String },
}

impl PromptBuildState {
    pub fn is_active(&self) -> bool {
        matches!(self, PromptBuildState::Active { .. })
    }

    pub fn prompt_text(&self) -> Option<&str> {
        match self {
            PromptBuildState::Active { prompt } => Some(prompt),
            _ => None,
        }
    }
}

/// Tool that lets the LLM activate, update, and deactivate prompt-build mode.
pub struct SetPromptBuildTool {
    state: Arc<Mutex<PromptBuildState>>,
}

impl SetPromptBuildTool {
    pub fn new(state: Arc<Mutex<PromptBuildState>>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Tool for SetPromptBuildTool {
    fn name(&self) -> &str {
        "set_prompt_build"
    }

    fn description(&self) -> &str {
        "Control the prompt-build mode. Actions: start (activate mode), update (replace the prompt text with a new version), cancel (deactivate mode). \
 While active, all user messages are instructions to modify the prompt — call update after each refinement. \
 IMPORTANT: Always call cancel after the prompt has been saved, copied, sent to another tool/agent, or otherwise dispatched."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["start", "update", "cancel"],
                    "description": "The action to perform: start activates the mode, update modifies the prompt text, cancel deactivates the mode"
                },
                "prompt": {
                    "type": "string",
                    "description": "The prompt text (required for 'update' action, ignored for 'start' and 'cancel')"
                }
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    fn should_force_for(&self, user_message: &str) -> bool {
        let lower = user_message.to_lowercase();
        // Detect phrases that should trigger prompt-build mode
        lower.contains("prompt build")
            || lower.contains("build a prompt")
            || lower.contains("construir un prompt")
            || lower.contains("construir prompt")
            || lower.contains("crear un prompt")
            || lower.contains("crear prompt")
            || lower.contains("prompt builder")
    }

    async fn run(&self, args: &str) -> String {
        let parsed: serde_json::Value = match serde_json::from_str(args) {
            Ok(v) => v,
            Err(e) => return format!("Error: failed to parse arguments: {e}"),
        };

        let action = match parsed["action"].as_str() {
            Some(a) => a,
            None => return "Error: missing 'action' field".to_string(),
        };

        let mut state = self.state.lock().unwrap();

        match action {
            "start" => {
                *state = PromptBuildState::Active {
                    prompt: String::new(),
                };
                "Prompt-build mode activated. Current prompt: (empty). \
                 The user can now give instructions to build a prompt. \
                 Remember: after saving/copying/sending the prompt, call set_prompt_build(action: \"cancel\") to deactivate."
                    .to_string()
            }
            "update" => {
                let new_prompt = match parsed["prompt"].as_str() {
                    Some(p) => p.to_string(),
                    None => {
                        return "Error: missing 'prompt' field for update action".to_string();
                    }
                };
                // Enforce max length
                if new_prompt.len() > 4000 {
                    return format!(
                        "Error: Prompt exceeds maximum length of 4000 characters (current: {})",
                        new_prompt.len()
                    );
                }
                match *state {
                    PromptBuildState::Active { ref mut prompt } => {
                        *prompt = new_prompt.clone();
                        format!(
                            "Prompt updated successfully. Current prompt:\n---\n{}\n---",
                            new_prompt
                        )
                    }
                    _ => {
                        *state = PromptBuildState::Active { prompt: new_prompt };
                        "Prompt-build mode activated and prompt set.".to_string()
                    }
                }
            }
            "cancel" => {
                *state = PromptBuildState::Inactive;
                "Prompt-build mode deactivated.".to_string()
            }
            _ => format!("Error: Unknown action: {action}"),
        }
    }
}
