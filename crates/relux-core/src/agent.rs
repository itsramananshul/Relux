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
    /// Optional **Lead** (`reports_to`) — the id of this operative's manager in the org
    /// lattice (the Paperclip-style chain-of-command). `None` = top-level (reports to no
    /// one). The product term is "Lead"; the internal id stays `reports_to` per the
    /// two-layer rule (`docs/relix-lexicon.md`). `#[serde(default)]` so agents stored
    /// before this field existed load as top-level (backwards compatible). The graph is
    /// validated acyclic at the config boundary (`relux-kernel` `agent_config` +
    /// kernel `create`/`update`); the pure subtree/chain helpers live in
    /// [`crate::hierarchy`]. It does NOT (yet) widen any permission — enforcement is
    /// unchanged; the helpers exist for a future scoped-grant slice.
    #[serde(default)]
    pub reports_to: Option<AgentId>,
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
        assert!(
            agent.reports_to.is_none(),
            "missing reports_to => top-level None (backwards compatible)"
        );
    }

    /// An agent serialized BEFORE the `reports_to` field existed (no `reports_to` key)
    /// must still deserialize — `#[serde(default)]` makes it a top-level operative.
    #[test]
    fn agent_without_reports_to_field_deserializes_to_none() {
        let legacy = serde_json::json!({
            "id": "ops",
            "name": "Ops",
            "description": "",
            "adapter_plugin": "p",
            "adapter_config": null,
            "persona": null,
            "namespace_id": "default",
            "owner": "founder",
            "permissions": [],
            "skills": ["rust"],
            "status": "active",
            "created_at": "t0"
        });
        let agent: Agent = serde_json::from_value(legacy).expect("legacy agent deserializes");
        assert!(agent.reports_to.is_none(), "missing reports_to => None");
    }

    /// A `reports_to` Lead pointer round-trips through serialization.
    #[test]
    fn agent_reports_to_round_trip() {
        let with_lead = serde_json::json!({
            "id": "ic",
            "name": "IC",
            "description": "",
            "adapter_plugin": "p",
            "adapter_config": null,
            "persona": null,
            "namespace_id": "default",
            "owner": "founder",
            "permissions": [],
            "skills": [],
            "reports_to": "lead-1",
            "status": "active",
            "created_at": "t0"
        });
        let agent: Agent = serde_json::from_value(with_lead).expect("deserializes");
        assert_eq!(agent.reports_to, Some(AgentId::new("lead-1")));
        let back = serde_json::to_value(&agent).expect("serializes");
        assert_eq!(back["reports_to"], serde_json::json!("lead-1"));
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
