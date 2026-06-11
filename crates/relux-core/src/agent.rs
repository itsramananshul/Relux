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
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 9.3 (Agent).
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
    /// Bounded specialty tags (slugs) describing what this operative is good at, used
    /// to route work to a specialist during assignment matching. Validated/sanitized at
    /// the config boundary (`relux-kernel` `agent_config`). `#[serde(default)]` so agents
    /// stored before this field existed load with an empty list (backwards compatible).
    #[serde(default)]
    pub skills: Vec<String>,
    pub status: AgentStatus,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An agent serialized BEFORE the `skills` field existed (no `skills` key) must still
    /// deserialize — `#[serde(default)]` gives it an empty list (snapshot backwards compat).
    #[test]
    fn agent_without_skills_field_deserializes_to_empty() {
        let legacy = serde_json::json!({
            "id": "research-bot",
            "name": "Research Bot",
            "description": "does research",
            "adapter_plugin": "relux-adapter-local-prime",
            "adapter_config": null,
            "persona": null,
            "namespace_id": "default",
            "owner": "founder",
            "permissions": [],
            "status": "active",
            "created_at": "t0"
        });
        let agent: Agent = serde_json::from_value(legacy).expect("legacy agent deserializes");
        assert!(agent.skills.is_empty(), "missing skills => empty (backwards compatible)");
    }

    /// A skills list round-trips through serialization.
    #[test]
    fn agent_skills_round_trip() {
        let with_skills = serde_json::json!({
            "id": "a",
            "name": "A",
            "description": "",
            "adapter_plugin": "p",
            "adapter_config": null,
            "persona": null,
            "namespace_id": "default",
            "owner": "founder",
            "permissions": [],
            "skills": ["rust", "frontend"],
            "status": "active",
            "created_at": "t0"
        });
        let agent: Agent = serde_json::from_value(with_skills).expect("deserializes");
        assert_eq!(agent.skills, vec!["rust".to_string(), "frontend".to_string()]);
        let back = serde_json::to_value(&agent).expect("serializes");
        assert_eq!(back["skills"], serde_json::json!(["rust", "frontend"]));
    }
}
