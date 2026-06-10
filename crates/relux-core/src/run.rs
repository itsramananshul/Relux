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
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 9.6 (Run).
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
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 9.6 (Run).
///
/// The timing fields work in two layers. `started_at`/`ended_at` come from the
/// kernel's deterministic logical clock (ordering, reproducible), so they are NOT
/// wall-clock instants. `duration_ms` is the **real** measured wall time of an
/// adapter subprocess (captured in the adapter spawn, which is the one place a
/// real process is touched); it is only present for CLI adapter runs. `usage` and
/// `cost` are only populated when an adapter emits a structured result envelope we
/// could parse (master plan section 9.6) - never fabricated.
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
    /// Real measured wall-clock duration of the adapter subprocess, in
    /// milliseconds. Only set for CLI adapter runs; `None` for the deterministic
    /// local echo path (which never touches a real process).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Structured token/usage data, only when the adapter reported it in a
    /// machine-readable result envelope. Never synthesized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<serde_json::Value>,
    /// Reported cost in USD, only when the adapter result envelope carried it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    /// When this run was created by retrying an earlier run, the id of that run
    /// (attempt lineage). Retry is a fresh run on the same task, not a resume of a
    /// partial CLI run (master plan section 10.2 `prime.retry_run`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retried_from: Option<RunId>,
}
