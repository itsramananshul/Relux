use serde::{Deserialize, Serialize};

use crate::agent::AgentId;
use crate::approval::ApprovalId;
use crate::namespace::NamespaceId;
use crate::permission::RiskLevel;
use crate::run::RunId;
use crate::task::{TaskId, TaskStatus};

/// Configuration for Prime's autonomous operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimeAutonomyConfig {
    /// Whether Prime's autonomy loop is enabled.
    pub enabled: bool,
    /// The interval in seconds between autonomy ticks.
    pub interval_seconds: u64,
    /// The maximum number of tasks Prime will process in a single tick.
    pub max_tasks_per_tick: u32,
    /// Whether Prime should automatically assign unassigned queued tasks.
    pub auto_assign_unassigned: bool,
    /// The kernel timestamp of the last autonomy tick.
    pub last_tick_at: Option<String>,
    /// A summary of what happened during the last autonomy tick.
    pub last_tick_summary: Option<String>,
}

impl Default for PrimeAutonomyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_seconds: 60, // Conservative default: tick every minute
            max_tasks_per_tick: 1, // Small per-tick limit
            auto_assign_unassigned: false, // Disabled by default
            last_tick_at: None,
            last_tick_summary: None,
        }
    }
}

/// The result of a single Prime autonomy tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimeAutonomyTickResult {
    /// Kernel timestamp of when this tick occurred.
    pub tick_at: String,
    /// Number of tasks successfully run in this tick.
    pub tasks_run: u32,
    /// Number of tasks successfully assigned in this tick.
    pub tasks_assigned: u32,
    /// Number of actions taken (e.g., runs started, tasks assigned).
    pub actions_taken: u32,
    /// A summary of what happened during this tick.
    pub summary: String,
    /// Reasons why some tasks were skipped or actions refused.
    pub skipped_reasons: Vec<String>,
}

impl Default for PrimeAutonomyTickResult {
    fn default() -> Self {
        Self {
            tick_at: String::new(),
            tasks_run: 0,
            tasks_assigned: 0,
            actions_taken: 0,
            summary: "No actions taken.".to_string(),
            skipped_reasons: Vec::new(),
        }
    }
}

/// What Prime understood the user to intend before taking any action.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 10.1 (Intent Layer).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimeIntent {
    Greeting,
    StatusQuestion,
    TaskCreation,
    CreateAndRunTask,
    TaskUpdate,
    AssignTask,
    RunStart,
    RunRetry,
    AgentCreation,
    PluginInstallation,
    PermissionChange,
    ApprovalResponse,
    ExplanationRequest,
    DashboardNavigation,
    Brainstorming,
    /// The user asked which tools Prime can use ("what tools can you use?").
    /// Answered with grounded capability discovery, never a fabricated list.
    ToolDiscovery,
    /// The user asked Prime to run a specific built-in tool ("echo hello",
    /// "use echo.say with {json}", "run the status tool"). Executed through the
    /// kernel's permission/audit path; an installed-but-unimplemented tool is
    /// reported honestly, never faked.
    ToolInvocation,
    DirectAnswer,
}

/// A concrete kernel action that Prime is authorized to invoke.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 10.2 (Action Layer).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum PrimeAction {
    InspectState,
    /// List the installed plugin tools and their honest executable status
    /// (`ready` / `not_implemented` / `missing_permission`). Read-only.
    DiscoverTools,
    /// Invoke one tool through the kernel's permission/audit path. `input_json`
    /// is the JSON-encoded tool input (kept as text so the action stays `Eq`);
    /// it is parsed back to a value immediately before invocation.
    InvokeTool {
        plugin_id: String,
        tool_name: String,
        input_json: String,
    },
    CreateTask {
        title: String,
    },
    CreateAndRunTask {
        title: String,
    },
    UpdateTask {
        task_id: String,
        patch: String,
    },
    AssignTask {
        task_id: String,
        agent_id: String,
    },
    StartRun {
        task_id: String,
    },
    RetryRun {
        run_id: String,
    },
    CreateAgent {
        name: String,
        adapter_plugin: String,
    },
    InstallPlugin {
        plugin_id: String,
    },
    ConfigurePlugin {
        plugin_id: String,
    },
    GrantPermission {
        subject_id: String,
        permission: String,
    },
    RequestApproval {
        action: String,
        reason: String,
    },
    SummarizeRun {
        run_id: String,
    },
    ExplainBlocker {
        task_id: String,
    },
}

/// A compact, grounded view of one task that Prime can reason and speak about.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 10.1 (Intent Layer) and section 11.2 (Board).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskBrief {
    pub id: TaskId,
    pub title: String,
    pub status: TaskStatus,
    pub assigned_agent: Option<AgentId>,
}

/// A grounded projection of control-plane state.
///
/// This is the "context window" Prime reasons over before deciding anything: a
/// real LLM-backed Prime would be handed the same shape. Keeping it explicit is
/// what stops Prime from inventing work or pretending plugins/runs exist.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 10.1, section 17.1 (Prime Must Be Smart And
/// Grounded), section 10.5 (Conversation Rules).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateSummary {
    pub plugins: usize,
    pub agents: usize,
    pub tasks_total: usize,
    /// Tasks not in a terminal state (not completed/failed/cancelled/expired).
    pub tasks_open: usize,
    /// Runs currently in `Running`.
    pub runs_active: usize,
    pub tasks_waiting_approval: usize,
    pub tasks_blocked: usize,
    pub tasks_failed: usize,
    pub pending_approvals: usize,
    /// All agents known to the system, by their ID.
    pub all_agent_ids: Vec<String>,
    /// All tasks known to the system, by their ID.
    pub all_task_ids: Vec<String>,
    /// Tasks assigned and ready to start, in id order.
    pub queued: Vec<TaskBrief>,
    /// The most recent tasks (newest first), used to ground explanations.
    pub recent: Vec<TaskBrief>,
}

/// What Prime decided to do with a message, before the kernel executes anything.
///
/// This is the pure output of Prime's "brain": the kernel turns a plan into real
/// state changes (or an approval request) when it runs the turn. Modelling the
/// decision separately from execution is what keeps risky actions from happening
/// silently.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 10.2 (Action Layer), section 10.3 (Approval
/// Rules), section 10.5 (Conversation Rules).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PrimePlan {
    /// Answer conversationally; take no kernel action.
    Reply { text: String },
    /// Execute a safe, in-scope action, then report back.
    Act { action: PrimeAction, text: String },
    /// Propose a risky action and request human approval before doing it.
    Propose {
        action: PrimeAction,
        reason: String,
        risk: RiskLevel,
        text: String,
    },
    /// Ask the user for missing information before acting.
    Clarify { text: String },
}

/// How a Prime turn resolved once the kernel acted on the plan.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 10.2, section 10.3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimeDisposition {
    /// Prime answered; no durable state changed.
    Answered,
    /// Prime executed a safe action.
    Executed,
    /// Prime queued a risky action behind a human approval.
    AwaitingApproval,
    /// Prime needs more information before it can act.
    NeedsClarification,
}

/// The full result of Prime handling one user message.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 10 (Prime Behavior Specification).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrimeTurn {
    pub intent: PrimeIntent,
    pub reply: String,
    pub disposition: PrimeDisposition,
    /// The action Prime took or proposed, if any.
    pub action: Option<PrimeAction>,
    pub created_task: Option<TaskId>,
    pub started_run: Option<RunId>,
    pub created_agent: Option<AgentId>,
    pub approval: Option<ApprovalId>,
    /// When Prime ran a tool this turn, its `"<plugin_id>/<tool_name>"` label.
    /// Set only on a real, kernel-executed invocation - never on a fabricated or
    /// refused one (`docs/RELUX_MASTER_PLAN.md` §11.1 plugin/action results).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invoked_tool: Option<String>,
    /// The deterministic JSON output of the invoked tool, when one ran. Carries
    /// only real kernel output; absent on a refusal or not-implemented reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_output: Option<serde_json::Value>,
    /// An honest, non-fabricated reason a requested tool did NOT run (installed
    /// but runtime not implemented, missing permission, or unknown tool).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_error: Option<String>,
}

/// The scope a Prime turn runs in: which namespace work lands in, which agent
/// identity Prime acts as, and which human actor is talking.
#[derive(Debug, Clone)]
pub struct PrimeContext {
    pub namespace: NamespaceId,
    pub agent: AgentId,
    pub actor: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prime_intent_serializes_cleanly() {
        let intent = PrimeIntent::TaskCreation;
        let json = serde_json::to_string(&intent).unwrap();
        assert_eq!(json, "\"task_creation\"");
        let back: PrimeIntent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PrimeIntent::TaskCreation);
    }

    #[test]
    fn prime_action_serializes_cleanly() {
        let action = PrimeAction::CreateTask {
            title: "Fix failing tests".to_string(),
        };
        let json = serde_json::to_string(&action).unwrap();
        let back: PrimeAction = serde_json::from_str(&json).unwrap();
        assert_eq!(back, action);
    }

    #[test]
    fn invoke_tool_action_round_trips() {
        let action = PrimeAction::InvokeTool {
            plugin_id: "relux-tools-echo".to_string(),
            tool_name: "echo.say".to_string(),
            input_json: "{\"message\":\"hi\"}".to_string(),
        };
        let json = serde_json::to_string(&action).unwrap();
        let back: PrimeAction = serde_json::from_str(&json).unwrap();
        assert_eq!(back, action);
    }

    #[test]
    fn all_prime_intents_round_trip() {
        let intents = [
            PrimeIntent::Greeting,
            PrimeIntent::StatusQuestion,
            PrimeIntent::TaskCreation,
            PrimeIntent::CreateAndRunTask,
            PrimeIntent::TaskUpdate,
            PrimeIntent::RunStart,
            PrimeIntent::RunRetry,
            PrimeIntent::AgentCreation,
            PrimeIntent::PluginInstallation,
            PrimeIntent::PermissionChange,
            PrimeIntent::ApprovalResponse,
            PrimeIntent::ExplanationRequest,
            PrimeIntent::DashboardNavigation,
            PrimeIntent::Brainstorming,
            PrimeIntent::ToolDiscovery,
            PrimeIntent::ToolInvocation,
            PrimeIntent::DirectAnswer,
        ];
        for intent in intents {
            let json = serde_json::to_string(&intent).unwrap();
            let back: PrimeIntent = serde_json::from_str(&json).unwrap();
            assert_eq!(back, intent);
        }
    }

    #[test]
    fn prime_plan_round_trips_with_nested_action() {
        let plan = PrimePlan::Propose {
            action: PrimeAction::GrantPermission {
                subject_id: "code-agent".to_string(),
                permission: "tool:relux-tools-github:access".to_string(),
            },
            reason: "Granting a permission widens what an actor can do.".to_string(),
            risk: RiskLevel::High,
            text: "I can grant GitHub access.".to_string(),
        };
        let json = serde_json::to_string(&plan).unwrap();
        let back: PrimePlan = serde_json::from_str(&json).unwrap();
        assert_eq!(back, plan);
    }
}
