//! LLM tool: `respond_agent_permission`.
//!
//! Allows the frontend LLM to answer a pending backend-agent permission
//! request that was injected as a `<backend_permission_request>` conversation
//! item by the voice pipeline (issue #167).
//!
//! The tool resolves a `PermissionGate` slot by its `authorization_id` and
//! maps the LLM's `decision` ("always" | "reject") to the ACP `optionId`
//! using the slot's option list. Mirrors the qwen-audio-agent
//! `respondAgentPermission` flow.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::{info, warn};

use seneschal_common::permission::{PermissionDecision, PermissionGate, ResolveOutcome};
use seneschal_common::tools::Tool;

pub struct RespondAgentPermissionTool {
    gate: Arc<PermissionGate>,
}

impl RespondAgentPermissionTool {
    pub fn new(gate: Arc<PermissionGate>) -> Self {
        Self { gate }
    }
}

#[async_trait]
impl Tool for RespondAgentPermissionTool {
    fn name(&self) -> &str {
        "respond_agent_permission"
    }

    fn description(&self) -> &str {
        "Responde a una solicitud de permiso pendiente del agente backend. \
         Usa exactamente el `authorization_id` del contexto de la solicitud \
         pendiente (<backend_permission_request>). Combina la pregunta que \
         acabas de hacer al usuario con su expresión natural de esta vuelta \
         para interpretar la intención: afirmaciones claras como 'sí', 'claro', \
         'adelante', 'ok', 'permiso', 'autorizo' → decision='always'; \
         negativas claras como 'no', 'ni loco', 'cancela' → decision='reject'. \
         Si el usuario no se pronuncia claro, NO llames a esta herramienta y \
         vuelve a preguntar en una frase corta. Nunca inventes el \
         `authorization_id`."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "authorization_id": {
                    "type": "string",
                    "description": "Identificador de la solicitud pendiente. Debe coincidir exactamente con el authorization_id inyectado en el contexto de la solicitud actual.",
                },
                "decision": {
                    "type": "string",
                    "enum": ["always", "reject"],
                    "description": "'always' permite la operación y, por defecto, también las siguientes solicitudes equivalentes en esta sesión de voz. 'reject' rechaza esta operación; las solicitudes equivalentes volverán a preguntar.",
                },
            },
            "required": ["authorization_id", "decision"],
            "additionalProperties": false,
        })
    }

    async fn run(&self, args: &str) -> String {
        let parsed: serde_json::Value = match serde_json::from_str(args) {
            Ok(v) => v,
            Err(e) => {
                warn!(target: "tools", "respond_agent_permission: invalid JSON args: {e}");
                return error_response("invalid_arguments", "Los argumentos no son JSON válido.");
            }
        };

        let authorization_id = parsed
            .get("authorization_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("");
        let decision_str = parsed
            .get("decision")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("");

        if authorization_id.is_empty() {
            return error_response(
                "missing_authorization_id",
                "Falta el campo `authorization_id`.",
            );
        }
        let decision = match decision_str {
            "always" => PermissionDecision::Always,
            "reject" => PermissionDecision::Reject,
            other => {
                return error_response(
                    "invalid_decision",
                    &format!("decision '{other}' no es válido (usa 'always' o 'reject')."),
                );
            }
        };

        let outcome = self
            .gate
            .resolve_by_authorization_id(authorization_id, decision);
        match outcome {
            ResolveOutcome::Sent => {
                info!(
                    target: "tools",
                    "respond_agent_permission: auth={authorization_id} decision={decision:?} → sent",
                );
                if matches!(decision, PermissionDecision::Always) {
                    "Permiso concedido: el agente continuará con la operación.".to_string()
                } else {
                    "Permiso denegado: el agente no ejecutará esta operación.".to_string()
                }
            }
            ResolveOutcome::ReceiverDropped => {
                info!(
                    target: "tools",
                    "respond_agent_permission: auth={authorization_id} → receiver dropped (ACP timeout)",
                );
                error_response(
                    "permission_lapsed",
                    "La solicitud de permiso ya expiró (timeout del backend).",
                )
            }
            ResolveOutcome::Unknown => {
                warn!(
                    target: "tools",
                    "respond_agent_permission: auth={authorization_id} → unknown (not pending)",
                );
                error_response(
                    "unknown_authorization_id",
                    "No hay ninguna solicitud pendiente con ese authorization_id. \
                     Puede que ya se haya respondido o que el id sea incorrecto.",
                )
            }
        }
    }
}

fn error_response(code: &str, message: &str) -> String {
    serde_json::json!({
        "status": "error",
        "error_code": code,
        "error": message,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use seneschal_common::permission::PermissionOptionWire;
    use tokio::sync::oneshot;

    fn options() -> Vec<PermissionOptionWire> {
        vec![
            PermissionOptionWire::with_kind("allow", "Allow", "allow"),
            PermissionOptionWire::with_kind("deny", "Deny", "reject"),
        ]
    }

    #[tokio::test]
    async fn run_resolves_always() {
        let gate = Arc::new(PermissionGate::new());
        let tool = RespondAgentPermissionTool::new(Arc::clone(&gate));
        let (tx, mut rx) = oneshot::channel();
        let auth = gate.enqueue("t1".into(), "a".into(), "q".into(), options(), tx);

        let result = tool
            .run(&format!(
                r#"{{"authorization_id":"{auth}","decision":"always"}}"#
            ))
            .await;
        assert!(result.contains("Permiso concedido"));
        assert_eq!(rx.try_recv().unwrap(), "allow");
        assert!(!gate.has_pending_or_resolving());
    }

    #[tokio::test]
    async fn run_resolves_reject() {
        let gate = Arc::new(PermissionGate::new());
        let tool = RespondAgentPermissionTool::new(Arc::clone(&gate));
        let (tx, mut rx) = oneshot::channel();
        let auth = gate.enqueue("t1".into(), "a".into(), "q".into(), options(), tx);

        let result = tool
            .run(&format!(
                r#"{{"authorization_id":"{auth}","decision":"reject"}}"#
            ))
            .await;
        assert!(result.contains("Permiso denegado"));
        assert_eq!(rx.try_recv().unwrap(), "deny");
    }

    #[tokio::test]
    async fn run_unknown_authorization_id_returns_error() {
        let gate = Arc::new(PermissionGate::new());
        let tool = RespondAgentPermissionTool::new(Arc::clone(&gate));
        let result = tool
            .run(r#"{"authorization_id":"auth_nonexistent","decision":"always"}"#)
            .await;
        assert!(result.contains("unknown_authorization_id"));
        assert!(result.contains("error"));
    }

    #[tokio::test]
    async fn run_invalid_decision_returns_error() {
        let gate = Arc::new(PermissionGate::new());
        let tool = RespondAgentPermissionTool::new(Arc::clone(&gate));
        let result = tool
            .run(r#"{"authorization_id":"auth_x","decision":"maybe"}"#)
            .await;
        assert!(result.contains("invalid_decision"));
    }

    #[tokio::test]
    async fn run_missing_authorization_id_returns_error() {
        let gate = Arc::new(PermissionGate::new());
        let tool = RespondAgentPermissionTool::new(Arc::clone(&gate));
        let result = tool.run(r#"{"decision":"always"}"#).await;
        assert!(result.contains("missing_authorization_id"));
    }

    #[tokio::test]
    async fn run_malformed_json_returns_error() {
        let gate = Arc::new(PermissionGate::new());
        let tool = RespondAgentPermissionTool::new(Arc::clone(&gate));
        let result = tool.run("not-json").await;
        assert!(result.contains("invalid_arguments"));
    }

    #[test]
    fn name_and_parameters_shape() {
        let gate = Arc::new(PermissionGate::new());
        let tool = RespondAgentPermissionTool::new(gate);
        assert_eq!(tool.name(), "respond_agent_permission");
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["authorization_id"].is_object());
        assert!(params["properties"]["decision"].is_object());
        assert_eq!(
            params["properties"]["decision"]["enum"],
            serde_json::json!(["always", "reject"])
        );
        let required = params["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "authorization_id"));
        assert!(required.iter().any(|v| v == "decision"));
        assert_eq!(params["additionalProperties"], serde_json::json!(false));
    }
}
