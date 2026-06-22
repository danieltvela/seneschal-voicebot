use crate::config::Config;
use serde::Deserialize;
use tracing::info;

#[derive(Debug, Default, Clone, Deserialize)]
pub struct PluginConfigOverrides {
    pub llm_temperature: Option<f32>,
    pub llm_max_tokens: Option<u32>,
    pub llm_system_prompt: Option<String>,
    pub llm_context_tokens: Option<usize>,
    pub language: Option<String>,
}

impl PluginConfigOverrides {
    pub fn apply_overrides(&self, config: &mut Config) {
        if let Some(v) = self.llm_temperature {
            info!(value = v, "plugin override: llm_temperature");
            config.llm_temperature = v;
        }
        if let Some(v) = self.llm_max_tokens {
            info!(value = v, "plugin override: llm_max_tokens");
            config.llm_max_tokens = v;
        }
        if let Some(ref v) = self.llm_system_prompt {
            info!(length = v.len(), "plugin override: llm_system_prompt");
            config.llm_system_prompt = v.clone();
        }
        if let Some(v) = self.llm_context_tokens {
            info!(value = v, "plugin override: llm_context_tokens");
            config.llm_context_tokens = v;
        }
        if let Some(ref v) = self.language {
            info!(value = v, "plugin override: language");
            config.language = v.clone();
        }
    }

    pub fn revert_overrides(&self, config: &mut Config, baseline: &OriginalConfigSnapshot) {
        if self.llm_temperature.is_some() {
            info!("plugin revert: llm_temperature");
            config.llm_temperature = baseline.llm_temperature;
        }
        if self.llm_max_tokens.is_some() {
            info!("plugin revert: llm_max_tokens");
            config.llm_max_tokens = baseline.llm_max_tokens;
        }
        if self.llm_system_prompt.is_some() {
            info!("plugin revert: llm_system_prompt");
            config.llm_system_prompt = baseline.llm_system_prompt.clone();
        }
        if self.llm_context_tokens.is_some() {
            info!("plugin revert: llm_context_tokens");
            config.llm_context_tokens = baseline.llm_context_tokens;
        }
        if self.language.is_some() {
            info!("plugin revert: language");
            config.language = baseline.language.clone();
        }
    }
}

#[derive(Debug, Clone)]
pub struct OriginalConfigSnapshot {
    pub llm_temperature: f32,
    pub llm_max_tokens: u32,
    pub llm_system_prompt: String,
    pub llm_context_tokens: usize,
    pub language: String,
}

impl OriginalConfigSnapshot {
    pub fn from_config(config: &Config) -> Self {
        Self {
            llm_temperature: config.llm_temperature,
            llm_max_tokens: config.llm_max_tokens,
            llm_system_prompt: config.llm_system_prompt.clone(),
            llm_context_tokens: config.llm_context_tokens,
            language: config.language.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config::from_env().expect("Config::from_env should succeed")
    }

    // ── apply_overrides ───────────────────────────────────────────────────────

    #[test]
    fn apply_overrides_sets_specified_fields() {
        let mut config = test_config();

        let overrides = PluginConfigOverrides {
            llm_temperature: Some(0.9),
            llm_max_tokens: Some(2048),
            llm_system_prompt: Some("custom prompt".to_string()),
            llm_context_tokens: Some(4096),
            language: Some("en".to_string()),
        };

        overrides.apply_overrides(&mut config);
        assert_eq!(config.llm_temperature, 0.9);
        assert_eq!(config.llm_max_tokens, 2048);
        assert_eq!(config.llm_system_prompt, "custom prompt");
        assert_eq!(config.llm_context_tokens, 4096);
        assert_eq!(config.language, "en");
    }

    #[test]
    fn apply_overrides_leaves_unset_fields_unchanged() {
        let mut config = test_config();
        let original_temp = config.llm_temperature;
        let original_tokens = config.llm_max_tokens;
        let original_lang = config.language.clone();

        let overrides = PluginConfigOverrides {
            llm_temperature: None,
            llm_max_tokens: None,
            llm_system_prompt: None,
            llm_context_tokens: None,
            language: None,
        };

        overrides.apply_overrides(&mut config);
        assert_eq!(config.llm_temperature, original_temp);
        assert_eq!(config.llm_max_tokens, original_tokens);
        assert_eq!(config.language, original_lang);
    }

    #[test]
    fn apply_overrides_partial_only_changes_set_fields() {
        let mut config = test_config();
        let original_tokens = config.llm_max_tokens;
        let original_lang = config.language.clone();

        let overrides = PluginConfigOverrides {
            llm_temperature: Some(0.7),
            llm_max_tokens: None,
            llm_system_prompt: None,
            llm_context_tokens: None,
            language: None,
        };

        overrides.apply_overrides(&mut config);
        assert_eq!(config.llm_temperature, 0.7);
        assert_eq!(config.llm_max_tokens, original_tokens);
        assert_eq!(config.language, original_lang);
    }

    // ── revert_overrides ──────────────────────────────────────────────────────

    #[test]
    fn revert_overrides_restores_original_values() {
        let config = test_config();
        let baseline = OriginalConfigSnapshot::from_config(&config);

        let mut config = config.clone();
        let overrides = PluginConfigOverrides {
            llm_temperature: Some(0.9),
            llm_max_tokens: Some(2048),
            llm_system_prompt: Some("custom prompt".to_string()),
            llm_context_tokens: Some(4096),
            language: Some("en".to_string()),
        };

        overrides.apply_overrides(&mut config);
        overrides.revert_overrides(&mut config, &baseline);

        assert_eq!(config.llm_temperature, baseline.llm_temperature);
        assert_eq!(config.llm_max_tokens, baseline.llm_max_tokens);
        assert_eq!(config.llm_system_prompt, baseline.llm_system_prompt);
        assert_eq!(config.llm_context_tokens, baseline.llm_context_tokens);
        assert_eq!(config.language, baseline.language);
    }

    #[test]
    fn revert_overrides_only_restores_fields_that_were_overridden() {
        let config = test_config();
        let baseline = OriginalConfigSnapshot::from_config(&config);

        let mut config = config.clone();
        let overrides = PluginConfigOverrides {
            llm_temperature: Some(0.9),
            llm_max_tokens: None,
            llm_system_prompt: None,
            llm_context_tokens: None,
            language: None,
        };

        overrides.apply_overrides(&mut config);
        assert_eq!(config.llm_temperature, 0.9);

        /* Manually change a field that wasn't overridden */
        config.llm_max_tokens = 9999;

        overrides.revert_overrides(&mut config, &baseline);

        assert_eq!(config.llm_temperature, baseline.llm_temperature);
        assert_eq!(config.llm_max_tokens, 9999);
    }

    // ── from_toml ─────────────────────────────────────────────────────────────

    #[test]
    fn deserialize_from_toml_all_fields() {
        let toml_str = r#"
llm_temperature = 0.8
llm_max_tokens = 4096
llm_system_prompt = "override prompt"
llm_context_tokens = 16384
language = "fr"
"#;
        let overrides: PluginConfigOverrides = toml::from_str(toml_str).unwrap();

        assert_eq!(overrides.llm_temperature, Some(0.8));
        assert_eq!(overrides.llm_max_tokens, Some(4096));
        assert_eq!(
            overrides.llm_system_prompt,
            Some("override prompt".to_string())
        );
        assert_eq!(overrides.llm_context_tokens, Some(16384));
        assert_eq!(overrides.language, Some("fr".to_string()));
    }

    #[test]
    fn deserialize_from_toml_partial_config() {
        let toml_str = r#"
llm_temperature = 0.5
"#;
        let overrides: PluginConfigOverrides = toml::from_str(toml_str).unwrap();

        assert_eq!(overrides.llm_temperature, Some(0.5));
        assert!(overrides.llm_max_tokens.is_none());
        assert!(overrides.language.is_none());
    }

    // ── OriginalConfigSnapshot ────────────────────────────────────────────────

    #[test]
    fn snapshot_from_config_captures_values() {
        let config = test_config();
        let snapshot = OriginalConfigSnapshot::from_config(&config);

        assert_eq!(snapshot.llm_temperature, config.llm_temperature);
        assert_eq!(snapshot.llm_max_tokens, config.llm_max_tokens);
        assert_eq!(snapshot.llm_system_prompt, config.llm_system_prompt);
        assert_eq!(snapshot.llm_context_tokens, config.llm_context_tokens);
        assert_eq!(snapshot.language, config.language);
    }
}
