use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NamespaceId(pub String);

impl NamespaceId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NamespaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The kind of isolation scope a namespace represents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamespaceKind {
    Company,
    Team,
    Project,
    Environment,
    Customer,
    Personal,
}

/// An isolated resource scope.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 9.2 (Namespace).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Namespace {
    pub id: NamespaceId,
    pub name: String,
    pub kind: NamespaceKind,
    pub parent_id: Option<NamespaceId>,
    pub settings: serde_json::Value,
    pub created_at: String,
}
