use serde::{Deserialize, Serialize};

use crate::agent::AgentId;
use crate::plugin::PluginId;
use crate::task::TaskId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunId(pub String);

impl RunId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lifecycle states for one execution attempt of a task.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` §9.6 (Run).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    WaitingForApproval,
    Completed,
    Failed,
    Cancelled,
}

/// One execution attempt for a task.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` §9.6 (Run).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    pub task_id: TaskId,
    pub agent_id: AgentId,
    pub adapter_plugin: PluginId,
    pub status: RunStatus,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub summary: Option<String>,
    pub error: Option<String>,
}
