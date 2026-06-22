use crate::plugins::{PluginPromptConfig, PromptMode};

#[derive(Debug, Default, Clone)]
pub struct PluginPromptSections {
    pub replace: String,
    pub prepend: String,
    pub append: String,
}

pub fn build_plugin_prompt_section(configs: &[&PluginPromptConfig]) -> PluginPromptSections {
    let mut sections = PluginPromptSections::default();

    for cfg in configs {
        match cfg.mode {
            PromptMode::Replace => {
                sections.replace.push_str(&cfg.content);
                sections.replace.push('\n');
            }
            PromptMode::Append => {
                sections.append.push_str(&cfg.content);
                sections.append.push('\n');
            }
            PromptMode::Both => {
                if cfg.prepend {
                    sections.prepend.push_str(&cfg.content);
                    sections.prepend.push('\n');
                } else {
                    sections.append.push_str(&cfg.content);
                    sections.append.push('\n');
                }
            }
        }
    }

    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cfg(mode: PromptMode, content: &str, prepend: bool) -> PluginPromptConfig {
        PluginPromptConfig {
            mode,
            content: content.to_string(),
            prepend,
        }
    }

    #[test]
    fn replace_mode_populates_replace_field() {
        let cfg = make_cfg(PromptMode::Replace, "replacement content", false);
        let sections = build_plugin_prompt_section(&[&cfg]);

        assert_eq!(sections.replace, "replacement content\n");
        assert!(sections.prepend.is_empty());
        assert!(sections.append.is_empty());
    }

    #[test]
    fn append_mode_populates_append_field() {
        let cfg = make_cfg(PromptMode::Append, "appended content", false);
        let sections = build_plugin_prompt_section(&[&cfg]);

        assert!(sections.replace.is_empty());
        assert!(sections.prepend.is_empty());
        assert_eq!(sections.append, "appended content\n");
    }

    #[test]
    fn both_mode_with_prepend_true_goes_to_prepend() {
        let cfg = make_cfg(PromptMode::Both, "prelude", true);
        let sections = build_plugin_prompt_section(&[&cfg]);

        assert!(sections.replace.is_empty());
        assert_eq!(sections.prepend, "prelude\n");
        assert!(sections.append.is_empty());
    }

    #[test]
    fn both_mode_with_prepend_false_goes_to_append() {
        let cfg = make_cfg(PromptMode::Both, "postlude", false);
        let sections = build_plugin_prompt_section(&[&cfg]);

        assert!(sections.replace.is_empty());
        assert!(sections.prepend.is_empty());
        assert_eq!(sections.append, "postlude\n");
    }

    #[test]
    fn multiple_configs_accumulate() {
        let c1 = make_cfg(PromptMode::Replace, "r1", false);
        let c2 = make_cfg(PromptMode::Replace, "r2", false);
        let c3 = make_cfg(PromptMode::Append, "a1", false);
        let c4 = make_cfg(PromptMode::Both, "p1", true);
        let c5 = make_cfg(PromptMode::Both, "a2", false);
        let sections = build_plugin_prompt_section(&[&c1, &c2, &c3, &c4, &c5]);

        assert_eq!(sections.replace, "r1\nr2\n");
        assert_eq!(sections.prepend, "p1\n");
        assert_eq!(sections.append, "a1\na2\n");
    }

    #[test]
    fn empty_configs_returns_defaults() {
        let sections = build_plugin_prompt_section(&[]);

        assert!(sections.replace.is_empty());
        assert!(sections.prepend.is_empty());
        assert!(sections.append.is_empty());
    }
}
