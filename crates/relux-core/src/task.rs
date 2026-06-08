use serde::{Deserialize, Serialize};

use crate::agent::AgentId;
use crate::namespace::NamespaceId;
use crate::permission::Permission;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub String);

impl TaskId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lifecycle states for a durable unit of work.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 9.5 (Task) and section 7.9 (Task).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Created,
    Queued,
    Leased,
    Running,
    WaitingForTool,
    WaitingForApproval,
    Blocked,
    Completed,
    Failed,
    Cancelled,
    Expired,
}

/// A durable unit of work.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 9.5 (Task).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub input: serde_json::Value,
    pub status: TaskStatus,
    pub priority: u8,
    pub created_by: String,
    pub assigned_agent: Option<AgentId>,
    pub namespace_id: NamespaceId,
    pub required_permissions: Vec<Permission>,
    pub parent_task: Option<TaskId>,
    pub deadline: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}
