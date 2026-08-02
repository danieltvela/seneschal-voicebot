//! Shared permission wire types for Control API and agent permission flows.
//!
//! Kept in `seneschal-common` so control, extras, and the main binary can share
//! the same serde shapes without cross-crate cycles. `PermissionGate` lands in a
//! later PR; this module only defines the wire option shape used by ControlEvent.

use serde::{Deserialize, Serialize};

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
}
