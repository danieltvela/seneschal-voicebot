//! Shared permission wire types and gate for ACP permission flows.
//!
//! Lives in `seneschal-common` so control, extras, and main share the same shapes
//! without cross-crate cycles. Works with `feature = "control"` off (voice path).

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

/// One ACP permission option as exposed on the Control API wire.
///
/// `id` is the ACP `optionId` (e.g. `"allow"`, `"deny"`). `label` is for UI only.
/// Never send `label` back as the chosen outcome — POST must use `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionOptionWire {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl PermissionOptionWire {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: None,
        }
    }

    pub fn with_kind(
        id: impl Into<String>,
        label: impl Into<String>,
        kind: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: Some(kind.into()),
        }
    }

    /// Display string for TUI / TTS (`"Allow once (allow)"` when label present).
    pub fn display_label(&self) -> String {
        if self.label.is_empty() || self.label == self.id {
            self.id.clone()
        } else {
            format!("{} ({})", self.label, self.id)
        }
    }
}

/// Parse ACP `session/request_permission` options into wire options.
pub fn permission_options_from_acp_json(options: &serde_json::Value) -> Vec<PermissionOptionWire> {
    options
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|o| {
                    let id = o["optionId"].as_str().unwrap_or("?").to_string();
                    let label = o["label"]
                        .as_str()
                        .or_else(|| o["description"].as_str())
                        .or_else(|| o["name"].as_str())
                        .filter(|l| !l.is_empty())
                        .unwrap_or(id.as_str())
                        .to_string();
                    let kind = o["kind"].as_str().map(str::to_string);
                    PermissionOptionWire { id, label, kind }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Map a spoken yes/no transcript to an ACP `optionId` from the pending options.
pub fn map_transcript_to_option_id(transcript: &str, options: &[PermissionOptionWire]) -> String {
    let t = transcript.to_lowercase();
    let yes = t.contains("sí")
        || t.contains("si")
        || t.contains("yes")
        || t.contains("claro")
        || t.contains("dale")
        || t.contains("ok")
        || t.contains("adelante")
        || t.contains("permite")
        || t.contains("permiso")
        || t.contains("autorizo");
    if yes {
        find_allow_option_id(options).unwrap_or_else(|| "cancelled".to_string())
    } else {
        find_deny_option_id(options).unwrap_or_else(|| "cancelled".to_string())
    }
}

/// First allow-kind option, or first option (find_allow_option style).
pub fn find_allow_option_id(options: &[PermissionOptionWire]) -> Option<String> {
    options
        .iter()
        .find(|o| {
            o.kind.as_deref() == Some("allow")
                || matches!(
                    o.id.as_str(),
                    "allow" | "always_allow" | "allow_once" | "allow_always"
                )
        })
        .or_else(|| options.first())
        .map(|o| o.id.clone())
}

/// First deny/reject-kind option.
pub fn find_deny_option_id(options: &[PermissionOptionWire]) -> Option<String> {
    options
        .iter()
        .find(|o| {
            matches!(o.kind.as_deref(), Some("reject") | Some("deny"))
                || matches!(o.id.as_str(), "deny" | "reject" | "reject_once" | "cancel")
        })
        .map(|o| o.id.clone())
}

/// Phase of a permission slot in the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionPhase {
    Pending,
    Resolving,
}

/// Public view of a gate slot (no oneshot) for GET `/control/permissions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionSlotView {
    pub task_id: String,
    pub agent_name: String,
    pub description: String,
    pub options: Vec<PermissionOptionWire>,
    pub phase: PermissionPhase,
}

struct PermissionSlot {
    task_id: String,
    agent_name: String,
    description: String,
    options: Vec<PermissionOptionWire>,
    response_tx: Option<oneshot::Sender<String>>,
    phase: PermissionPhase,
}

/// Snapshot returned when voice claims the front Pending slot.
#[derive(Debug, Clone)]
pub struct VoiceClaim {
    pub task_id: String,
    pub agent_name: String,
    pub description: String,
    pub options: Vec<PermissionOptionWire>,
}

/// Result of `PermissionGate::resolve` after a claim (voice or internal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveOutcome {
    /// Oneshot delivered; slot removed. Emit `AgentPermissionResolved` with option_id.
    Sent,
    /// Receiver dropped (ACP timeout); slot removed. Emit resolved with `"cancelled"`.
    ReceiverDropped,
    /// No matching slot (already resolved / never existed).
    Unknown,
}

/// Result of HTTP POST `/control/permission`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpPermissionResult {
    /// 204 — oneshot sent with option_id.
    Ok,
    /// 404 — unknown task_id.
    NotFound,
    /// 409 — already Resolving, already gone, or oneshot closed.
    Conflict,
    /// 400 — option_id not in the pending options list.
    BadOption,
    /// 409 after closed channel — slot cleaned; emit cancelled Resolved.
    ClosedCancelled,
}

/// FIFO permission gate shared by voice path and Control API.
pub struct PermissionGate {
    inner: Mutex<VecDeque<PermissionSlot>>,
}

impl Default for PermissionGate {
    fn default() -> Self {
        Self::new()
    }
}

impl PermissionGate {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(VecDeque::new()),
        }
    }

    pub fn enqueue(
        &self,
        task_id: String,
        agent_name: String,
        description: String,
        options: Vec<PermissionOptionWire>,
        response_tx: oneshot::Sender<String>,
    ) {
        let mut q = self.inner.lock().expect("permission gate lock");
        q.push_back(PermissionSlot {
            task_id,
            agent_name,
            description,
            options,
            response_tx: Some(response_tx),
            phase: PermissionPhase::Pending,
        });
    }

    pub fn list(&self) -> Vec<PermissionSlotView> {
        let q = self.inner.lock().expect("permission gate lock");
        q.iter()
            .map(|s| PermissionSlotView {
                task_id: s.task_id.clone(),
                agent_name: s.agent_name.clone(),
                description: s.description.clone(),
                options: s.options.clone(),
                phase: s.phase,
            })
            .collect()
    }

    pub fn has_pending_or_resolving(&self) -> bool {
        !self.inner.lock().expect("permission gate lock").is_empty()
    }

    /// Claim the front Pending slot for the voice path (Pending → Resolving).
    pub fn claim_front_for_voice(&self) -> Option<VoiceClaim> {
        let mut q = self.inner.lock().expect("permission gate lock");
        let slot = q.front_mut()?;
        if slot.phase != PermissionPhase::Pending {
            return None;
        }
        slot.phase = PermissionPhase::Resolving;
        Some(VoiceClaim {
            task_id: slot.task_id.clone(),
            agent_name: slot.agent_name.clone(),
            description: slot.description.clone(),
            options: slot.options.clone(),
        })
    }

    /// Complete a previously claimed (or still pending) slot by task_id.
    pub fn resolve(&self, task_id: &str, option_id: &str) -> ResolveOutcome {
        let mut q = self.inner.lock().expect("permission gate lock");
        let idx = match q.iter().position(|s| s.task_id == task_id) {
            Some(i) => i,
            None => return ResolveOutcome::Unknown,
        };
        let mut slot = q.remove(idx).expect("index valid");
        let Some(tx) = slot.response_tx.take() else {
            return ResolveOutcome::Unknown;
        };
        match tx.send(option_id.to_string()) {
            Ok(()) => ResolveOutcome::Sent,
            Err(_) => ResolveOutcome::ReceiverDropped,
        }
    }

    /// HTTP claim+resolve in one step. Pending only; Resolving → Conflict.
    pub fn try_resolve_http(&self, task_id: &str, option_id: &str) -> HttpPermissionResult {
        let mut q = self.inner.lock().expect("permission gate lock");
        let idx = match q.iter().position(|s| s.task_id == task_id) {
            Some(i) => i,
            None => return HttpPermissionResult::NotFound,
        };
        if q[idx].phase == PermissionPhase::Resolving {
            return HttpPermissionResult::Conflict;
        }
        if !q[idx].options.iter().any(|o| o.id == option_id) && option_id != "cancelled" {
            return HttpPermissionResult::BadOption;
        }
        let mut slot = q.remove(idx).expect("index valid");
        let Some(tx) = slot.response_tx.take() else {
            return HttpPermissionResult::NotFound;
        };
        match tx.send(option_id.to_string()) {
            Ok(()) => HttpPermissionResult::Ok,
            Err(_) => HttpPermissionResult::ClosedCancelled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_option_wire_serde_roundtrip() {
        let opt = PermissionOptionWire::with_kind("allow", "Allow once", "allow");
        let json = serde_json::to_string(&opt).unwrap();
        assert!(json.contains("\"id\":\"allow\""));
        assert!(json.contains("\"kind\":\"allow\""));
        let back: PermissionOptionWire = serde_json::from_str(&json).unwrap();
        assert_eq!(back, opt);
    }

    #[test]
    fn permission_option_wire_skips_none_kind() {
        let opt = PermissionOptionWire::new("deny", "Deny");
        let json = serde_json::to_string(&opt).unwrap();
        assert!(!json.contains("kind"));
    }

    #[test]
    fn parse_acp_options_structured() {
        let options = serde_json::json!([
            {"optionId": "allow", "label": "Allow once", "kind": "allow"},
            {"optionId": "deny", "label": "Deny", "kind": "reject"}
        ]);
        let parsed = permission_options_from_acp_json(&options);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, "allow");
        assert_eq!(parsed[0].kind.as_deref(), Some("allow"));
        assert_eq!(parsed[1].id, "deny");
    }

    #[test]
    fn map_yes_to_allow_option_id() {
        let opts = vec![
            PermissionOptionWire::with_kind("allow", "Allow once", "allow"),
            PermissionOptionWire::with_kind("deny", "Deny", "reject"),
        ];
        assert_eq!(map_transcript_to_option_id("sí, adelante", &opts), "allow");
        assert_eq!(map_transcript_to_option_id("no gracias", &opts), "deny");
    }

    #[test]
    fn gate_enqueue_list_http_resolve() {
        let gate = PermissionGate::new();
        let (tx, mut rx) = oneshot::channel();
        gate.enqueue(
            "t1".into(),
            "hermes".into(),
            "bash: ls".into(),
            vec![PermissionOptionWire::with_kind("allow", "Allow", "allow")],
            tx,
        );
        assert!(gate.has_pending_or_resolving());
        let list = gate.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].phase, PermissionPhase::Pending);

        assert_eq!(
            gate.try_resolve_http("t1", "allow"),
            HttpPermissionResult::Ok
        );
        assert_eq!(rx.try_recv().unwrap(), "allow");
        assert!(!gate.has_pending_or_resolving());
        assert_eq!(
            gate.try_resolve_http("t1", "allow"),
            HttpPermissionResult::NotFound
        );
    }

    #[test]
    fn gate_voice_claim_then_http_conflict() {
        let gate = PermissionGate::new();
        let (tx, _rx) = oneshot::channel();
        gate.enqueue(
            "t1".into(),
            "a".into(),
            "q".into(),
            vec![PermissionOptionWire::new("allow", "Allow")],
            tx,
        );
        let claim = gate.claim_front_for_voice().expect("claim");
        assert_eq!(claim.task_id, "t1");
        assert_eq!(
            gate.try_resolve_http("t1", "allow"),
            HttpPermissionResult::Conflict
        );
        assert_eq!(gate.resolve("t1", "allow"), ResolveOutcome::Sent);
        assert!(!gate.has_pending_or_resolving());
    }

    #[test]
    fn gate_resolve_after_receiver_dropped() {
        let gate = PermissionGate::new();
        let (tx, rx) = oneshot::channel();
        gate.enqueue(
            "t1".into(),
            "a".into(),
            "q".into(),
            vec![PermissionOptionWire::new("allow", "Allow")],
            tx,
        );
        drop(rx); // simulate ACP 60s timeout
        assert_eq!(
            gate.try_resolve_http("t1", "allow"),
            HttpPermissionResult::ClosedCancelled
        );
        assert!(!gate.has_pending_or_resolving());
        assert_eq!(
            gate.try_resolve_http("t1", "allow"),
            HttpPermissionResult::NotFound
        );
    }

    #[test]
    fn gate_http_bad_option() {
        let gate = PermissionGate::new();
        let (tx, _rx) = oneshot::channel();
        gate.enqueue(
            "t1".into(),
            "a".into(),
            "q".into(),
            vec![PermissionOptionWire::new("allow", "Allow")],
            tx,
        );
        assert_eq!(
            gate.try_resolve_http("t1", "not-a-real-id"),
            HttpPermissionResult::BadOption
        );
        assert!(gate.has_pending_or_resolving());
    }

    #[test]
    fn gate_voice_resolve_unknown_after_http() {
        let gate = PermissionGate::new();
        let (tx, mut rx) = oneshot::channel();
        gate.enqueue(
            "t1".into(),
            "a".into(),
            "q".into(),
            vec![PermissionOptionWire::new("allow", "Allow")],
            tx,
        );
        let claim = gate.claim_front_for_voice().unwrap();
        // Simulate HTTP somehow got it first by resolve after we force remove via resolve from another path —
        // voice claimed, so HTTP conflicts; resolve with voice wins.
        assert_eq!(gate.resolve(&claim.task_id, "allow"), ResolveOutcome::Sent);
        assert_eq!(rx.try_recv().unwrap(), "allow");
        assert_eq!(gate.resolve("t1", "allow"), ResolveOutcome::Unknown);
    }
}
