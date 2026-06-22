use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::warn;

use super::Tool;
use crate::plugins::{PluginManager, PluginSwitchEvent};

pub struct SwitchPluginTool {
    manager: PluginManager,
    event_tx: Arc<mpsc::Sender<PluginSwitchEvent>>,
}

impl SwitchPluginTool {
    pub fn new(manager: PluginManager, event_tx: mpsc::Sender<PluginSwitchEvent>) -> Self {
        Self {
            manager,
            event_tx: Arc::new(event_tx),
        }
    }
}

#[async_trait]
impl Tool for SwitchPluginTool {
    fn name(&self) -> &str {
        "switch_plugin"
    }

    fn description(&self) -> &str {
        "Activa o cambia el plugin activo. Los plugins disponibles son: "
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "plugin_name": {
                    "type": "string",
                    "description": "Nombre del plugin a activar"
                }
            },
            "required": ["plugin_name"]
        })
    }

    async fn run(&self, args: &str) -> String {
        let plugin_name = serde_json::from_str::<serde_json::Value>(args)
            .ok()
            .and_then(|v| v["plugin_name"].as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| args.trim().to_string());

        let available = self.manager.list_available();

        if available.is_empty() {
            return "No hay plugins disponibles instalados.".to_string();
        }

        if !available.contains(&plugin_name) {
            let list = available.join(", ");
            return format!(
                "Plugin '{}' no encontrado. Plugins disponibles: {}",
                plugin_name, list
            );
        }

        let current = self.manager.get_active();
        if current.as_deref() == Some(plugin_name.as_str()) {
            return format!("Plugin '{}' ya está activo.", plugin_name);
        }

        if self
            .event_tx
            .try_send(PluginSwitchEvent::Activate {
                plugin_id: plugin_name.clone(),
            })
            .is_err()
        {
            warn!("Failed to send plugin switch event — channel full");
        }

        format!("Plugin '{}' activado con éxito.", plugin_name)
    }
}
