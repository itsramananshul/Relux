use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::permission::{Permission, PermissionError, ToolDefinition};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PluginId(pub String);

impl PluginId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PluginId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// First-class plugin kinds from the Relux plugin model.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` §8 (Plugin Model) and
/// `docs/Relux spec.md` §9.3 (Plugin Types).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum PluginKind {
    Adapter,
    ToolSet,
    ServiceProvider,
    ExecutionEnvironment,
    MemoryProvider,
    PolicyProvider,
    ObservabilityProvider,
    UiExtension,
    WorkflowExtension,
    IntegrationBridge,
}

/// How much the kernel should trust an installed plugin.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` §8 (Plugin Model).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel {
    Official,
    Community,
    Private,
    Unverified,
}

/// Runtime health state reported by a plugin.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` §7.4 (Plugin Kernel Layer).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginHealth {
    Unknown,
    Healthy,
    Degraded,
    Unhealthy,
}

/// Capabilities a plugin declares in its manifest.
///
/// Spec ref: `docs/Relux spec.md` §9.2 (Plugin Manifest).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginCapability {
    pub tools: Vec<ToolDefinition>,
    pub permissions: Vec<Permission>,
}

/// The manifest every plugin must provide for kernel registration.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` §8 and `docs/Relux spec.md` §9.2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: PluginId,
    pub name: String,
    pub version: String,
    pub kind: PluginKind,
    pub description: String,
    pub author: String,
    pub trust_level: TrustLevel,
    pub capabilities: PluginCapability,
    pub health: PluginHealth,
}

/// Errors produced by [`validate_manifest`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("plugin id is empty")]
    EmptyId,
    #[error("plugin name is empty")]
    EmptyName,
    #[error("plugin version is empty")]
    EmptyVersion,
    #[error("tool '{tool}' has no permission string")]
    ToolMissingPermission { tool: String },
    #[error("tool '{tool}' has invalid permission: {source}")]
    ToolInvalidPermission {
        tool: String,
        source: PermissionError,
    },
    #[error("declared permission is invalid: {0}")]
    InvalidDeclaredPermission(#[from] PermissionError),
}

/// Validate a [`PluginManifest`] against the Relux manifest contract.
///
/// Checks:
/// - id, name, version must not be empty
/// - every `ToolDefinition` must carry a `Permission` (already enforced by type,
///   but we re-validate the prefix here to catch deserialized manifests)
/// - every declared `Permission` in `capabilities.permissions` must have a valid prefix
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` §8 and `docs/Relux spec.md` §9.2.
pub fn validate_manifest(manifest: &PluginManifest) -> Result<(), ManifestError> {
    if manifest.id.0.is_empty() {
        return Err(ManifestError::EmptyId);
    }
    if manifest.name.is_empty() {
        return Err(ManifestError::EmptyName);
    }
    if manifest.version.is_empty() {
        return Err(ManifestError::EmptyVersion);
    }

    for tool in &manifest.capabilities.tools {
        if tool.name.is_empty() {
            return Err(ManifestError::ToolMissingPermission {
                tool: "(unnamed)".to_string(),
            });
        }
        // Re-validate that the permission string still has a valid prefix.
        Permission::new(tool.permission.as_str()).map_err(|e| {
            ManifestError::ToolInvalidPermission {
                tool: tool.name.clone(),
                source: e,
            }
        })?;
    }

    for perm in &manifest.capabilities.permissions {
        Permission::new(perm.as_str())?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::{ApprovalRequirement, RiskLevel};

    fn minimal_manifest() -> PluginManifest {
        PluginManifest {
            id: PluginId::new("relux-tools-github"),
            name: "Relux GitHub Tools".to_string(),
            version: "0.1.0".to_string(),
            kind: PluginKind::ToolSet,
            description: "GitHub tools for agents.".to_string(),
            author: "Relux Labs".to_string(),
            trust_level: TrustLevel::Official,
            capabilities: PluginCapability {
                tools: vec![ToolDefinition {
                    name: "github.create_pr".to_string(),
                    description: "Open a pull request.".to_string(),
                    risk: RiskLevel::Medium,
                    permission: Permission::new("tool:relux-tools-github:create_pr").unwrap(),
                    approval: ApprovalRequirement::Never,
                    timeout_secs: Some(30),
                }],
                permissions: vec![Permission::new("tool:relux-tools-github:create_pr").unwrap()],
            },
            health: PluginHealth::Unknown,
        }
    }

    #[test]
    fn valid_manifest_passes() {
        assert!(validate_manifest(&minimal_manifest()).is_ok());
    }

    #[test]
    fn empty_id_fails() {
        let mut m = minimal_manifest();
        m.id = PluginId::new("");
        assert_eq!(validate_manifest(&m), Err(ManifestError::EmptyId));
    }

    #[test]
    fn empty_name_fails() {
        let mut m = minimal_manifest();
        m.name = String::new();
        assert_eq!(validate_manifest(&m), Err(ManifestError::EmptyName));
    }

    #[test]
    fn empty_version_fails() {
        let mut m = minimal_manifest();
        m.version = String::new();
        assert_eq!(validate_manifest(&m), Err(ManifestError::EmptyVersion));
    }

    #[test]
    fn tool_without_valid_permission_prefix_fails() {
        // Construct a manifest that somehow carries an invalid permission string.
        // We bypass Permission::new by deserializing directly.
        let raw = serde_json::json!({
            "id": "relux-tools-github",
            "name": "GitHub Tools",
            "version": "0.1.0",
            "kind": "ToolSet",
            "description": "test",
            "author": "test",
            "trust_level": "official",
            "capabilities": {
                "tools": [{
                    "name": "github.create_pr",
                    "description": "open pr",
                    "risk": "medium",
                    "permission": "badprefix:resource:action",
                    "approval": "never",
                    "timeout_secs": null
                }],
                "permissions": []
            },
            "health": "unknown"
        });
        let m: PluginManifest = serde_json::from_value(raw).unwrap();
        let err = validate_manifest(&m).unwrap_err();
        assert!(
            matches!(err, ManifestError::ToolInvalidPermission { .. }),
            "expected ToolInvalidPermission, got {err}"
        );
    }
}
