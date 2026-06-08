use serde::{Deserialize, Serialize};

use crate::namespace::NamespaceId;
use crate::permission::Permission;
use crate::plugin::PluginId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentId(pub String);

impl AgentId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Status of a configured agent actor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Draft,
    Active,
    Paused,
    Disabled,
    Error,
}

/// A configured agent actor inside Relux.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` §9.3 (Agent).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: AgentId,
    pub name: String,
    pub description: String,
    pub adapter_plugin: PluginId,
    pub adapter_config: serde_json::Value,
    pub persona: Option<String>,
    pub namespace_id: NamespaceId,
    pub owner: String,
    pub permissions: Vec<Permission>,
    pub status: AgentStatus,
    pub created_at: String,
}
