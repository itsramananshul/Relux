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
    /// The user asked Prime to coordinate a goal across multiple agents ("orchestrate",
    /// "split this across agents", "have the team..."). Prime decomposes the goal
    /// into role-typed briefs and assigns them; running is a separate governed step.
    /// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 10.4 (Delegation Rules).
    Orchestration,
    /// The user asked Prime to lay an idea out as a reviewable PLAN before any
    /// work is created ("plan this out", "draft a plan for X", "make a plan").
    /// This is the explicit "idea -> plan -> tasks" rung: Prime previews the
    /// proposed steps, creating nothing, and the user commits the plan with one
    /// click. The preview is action-free; it never mints or runs work on its own.
    /// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 10 (planning layer),
    /// section 10.5 (Conversation Rules), section 11.1 (Prime suggested next actions),
    /// section 17.1 (Prime must not blindly turn every message into a plan).
    PlanRequest,
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
    /// Decompose a goal into multiple role-typed briefs and assign each to a fitting
    /// agent, recording a durable orchestration. Creates work but does not run it;
    /// running is a separate governed batch (`docs/RELUX_MASTER_PLAN.md` section 10.4).
    OrchestrateGoal {
        goal: String,
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

/// A suggested next action Prime offers as a one-click button in the chat
/// surface (`docs/RELUX_MASTER_PLAN.md` §11.1 "Prime suggested next actions").
///
/// A suggestion is never a privileged path: it is just a pre-written user
/// message. Acting on it sends `message` through the SAME grounded `prime_turn`,
/// so a button can do nothing the user could not type. When `send` is `true` the
/// dashboard sends `message` immediately; when `false` it pre-fills the input
/// with `message` for the user to complete or edit before sending (used when
/// Prime can only offer the start of a command — e.g. promoting a half-formed
/// idea into a task, where the user still names the work).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimeSuggestion {
    /// The button label shown to the user.
    pub label: String,
    /// The message routed through `prime_turn` when the user acts on it.
    pub message: String,
    /// `true` to send immediately; `false` to pre-fill the input for editing.
    pub send: bool,
}

/// One proposed step of a reviewable plan, as a card row the dashboard renders.
///
/// Carries ONLY descriptive data - a 1-based position, the brief title, the
/// specialist role it needs, and the agent it would land on. There is no
/// executable action here: a proposal is a preview, not a command. The kernel
/// builds these from the pure [`crate::OrchestrationPlan`] decomposition so the
/// card shows exactly what the "Create these tasks" commit would create - never
/// an invented step.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 10 (planning layer),
/// section 11.1 (Prime Chat - plugin/action results / suggested next actions).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimeProposalStep {
    /// The 1-based position shown in the card.
    pub index: u32,
    /// The brief's title.
    pub title: String,
    /// The specialist role label this step needs (e.g. `"research"`).
    pub role: String,
    /// The agent this step would be assigned to, or `"prime"` when no specialist
    /// is on the roster (the kernel's grounded fallback - never an invented name).
    pub agent: String,
}

/// A reviewable, ACTION-FREE plan preview attached to a `PlanRequest` turn so the
/// dashboard can render the proposed shape as a card instead of parsing the prose
/// reply (`docs/RELUX_MASTER_PLAN.md` section 10 planning layer, section 11.1).
///
/// It is purely informational: titles, role labels, and the agents work would land
/// on, grounded in the planner's real decomposition. It carries NO `PrimeAction`
/// and commits nothing - the only path that materializes it is the explicit
/// "Create these tasks" suggestion, which routes the normal grounded turn
/// (section 10.5, section 17.1: Prime must not turn musing into work on its own).
/// Omitted from the wire on every non-plan turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimeProposal {
    /// The goal this plan decomposes, exactly as the commit suggestion re-wraps it
    /// (so the previewed and committed plans come from identical input).
    pub goal: String,
    /// `true` when the goal genuinely splits into multiple briefs (a real plan);
    /// `false` when it reads as one piece of work and is steered to the one-task
    /// path. A single-step proposal carries an empty `steps` list.
    pub multi_step: bool,
    /// The proposed steps in order. Empty for a single-step goal.
    pub steps: Vec<PrimeProposalStep>,
    /// The distinct agents this plan would assign work to, including the `"prime"`
    /// fallback for unmatched roles. Empty for a single-step goal.
    pub agents: Vec<String>,
    /// An OPTIONAL, advisory presentation overlay produced by the LLM brain when
    /// it is enabled (see [`PrimeProposalPolish`]). Absent on every unpolished
    /// turn, so the wire is byte-for-byte unchanged for existing clients. It NEVER
    /// alters what the plan does: step count, order, agent grounding, the
    /// `multi_step` flag, and `goal` (which the commit re-wraps as
    /// `orchestrate <goal>`) all stay exactly as the deterministic planner set
    /// them. Polish refines wording only and is never read by any action/commit
    /// path (§10 planning layer, §11.1, §17.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polish: Option<PrimeProposalPolish>,
}

/// An advisory, PRESENTATION-ONLY overlay the optional LLM brain may attach to a
/// plan preview to make it read better (`docs/RELUX_MASTER_PLAN.md` §10 planning
/// layer, §11.1, §17.1 "Prime must be smart and grounded").
///
/// ## The safety contract (binding)
///
/// The LLM has ZERO action authority here. This overlay can only refine *wording*
/// the operator reads — a friendlier one-line summary, clearer per-step titles, a
/// couple of clarifying questions, and advisory risk notes. It can NEVER:
///
/// - change the number of steps, their order, or the agent each lands on,
/// - change the `goal` (the commit re-wraps it as `orchestrate <goal>`),
/// - introduce a step or assignee the deterministic planner did not produce, or
/// - feed back into any action: nothing in the commit path ever reads `polish`.
///
/// The kernel VALIDATES a model suggestion against the authoritative
/// [`PrimeProposal`] before attaching it: `step_titles` is accepted only when its
/// indexes match the authoritative steps exactly (same count, same set, no
/// extras); otherwise that part is dropped and the deterministic titles stand.
/// `questions`/`risks` are pure additive advisory text (trimmed and bounded). If
/// the model is unavailable or errors, no overlay is attached and the preview is
/// exactly the deterministic one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimeProposalPolish {
    /// A refined one-line summary of the plan (presentation only). The
    /// deterministic "N steps across M agents" line still stands behind it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Per-step refined titles, each keyed to an authoritative step `index`. Only
    /// ever populated when the model's indexes matched the authoritative steps
    /// exactly; the authoritative title remains the source of truth and is shown
    /// when no polished title is present for a step.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub step_titles: Vec<PrimePolishedStep>,
    /// Clarifying questions the operator may want to resolve before committing.
    /// Advisory only — answering them is not required and they commit nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<String>,
    /// Advisory risk notes about the plan. Presentation only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risks: Vec<String>,
    /// The model id that produced this overlay, for provenance on the card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// One polished step title, keyed to the authoritative step it refines.
///
/// `index` is the 1-based position of an existing [`PrimeProposalStep`]; the
/// kernel only ever emits this with an `index` that matches a real step, so it can
/// never name a step the planner did not produce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimePolishedStep {
    /// The authoritative step this title refines (1-based, matches a real step).
    pub index: u32,
    /// The refined, presentation-only title.
    pub title: String,
}

/// The brain-assisted, VALIDATED task slots that produced a created task, attached
/// to a `TaskCreation` / `CreateAndRunTask` turn ONLY when a configured brain
/// actually sharpened the slots (`docs/RELUX_MASTER_PLAN.md` section 10.1 Intent
/// Layer, section 10.2 Action Layer, section 17.1 "Prime must be smart and
/// grounded"). It is provenance/presentation only — every field here was already
/// validated by the kernel before the task was created, so this just reports what
/// the brain contributed, never a fresh authority.
///
/// ## The safety contract (binding)
///
/// The brain only ever *proposes* these slots; the kernel validates each one
/// against a strict schema and the live control-plane state before any task is
/// created (`crates/relux-kernel/src/prime_slots.rs`):
///
/// - `title` is sanitized (control chars stripped, single line) and length-clamped;
/// - `assignee` is honored ONLY when it names an EXISTING agent — an unknown id is
///   dropped and the task stays assigned to Prime (the brain can never invent an
///   assignee or smuggle a plugin/tool name in as one);
/// - `priority` is clamped to the supported range;
/// - a low-confidence, malformed, unknown-field, or empty-title proposal is
///   rejected wholesale and the deterministic slots stand (this struct is absent).
///
/// Omitted from the wire on every turn the brain did not sharpen, so existing
/// clients see exactly the JSON they did before.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimeTaskSlots {
    /// The normalized task title the brain produced and the kernel accepted. This
    /// is the title the created task actually carries.
    pub title: String,
    /// Optional sanitized details/notes the brain extracted, folded into the task
    /// input. Absent when the brain offered none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    /// The suggested assignee, present ONLY when it named an existing agent the
    /// kernel honored. Absent when the brain named none or named an unknown agent
    /// (in which case the task stayed assigned to Prime).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// The clamped priority the brain suggested and the kernel applied. Absent when
    /// the brain offered none (the task kept the default priority).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    /// The model id / CLI brain label that produced these slots, for provenance on
    /// the card. Absent degrades to a generic "AI brain" label client-side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// The brain-assisted, validated slots that shaped a *created agent*, present ONLY
/// on an `AgentCreation` turn the brain genuinely sharpened (the counterpart of
/// [`PrimeTaskSlots`] for crew creation).
///
/// The brain only ever *proposes* these slots; the kernel validates each one before
/// the agent is created (`crates/relux-kernel/src/prime_agent_slots.rs`):
///
/// - `name`/`id` are normalized (id is lowercased and reduced to `[a-z0-9-]`) and
///   length-clamped; an id that collides with an EXISTING agent is rejected wholesale
///   (the brain can never reshape a create into a duplicate);
/// - `adapter` is honored ONLY when it names an EXISTING adapter plugin — an unknown
///   adapter is dropped and the deterministic default stands (the brain can never
///   invent or enable an adapter);
/// - `description`/`notes` are sanitized and length-clamped;
/// - a low-confidence, malformed, unknown-field, empty-name, or duplicate proposal is
///   rejected wholesale and the deterministic name stands (this struct is absent).
///
/// Omitted from the wire on every turn the brain did not sharpen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimeAgentSlots {
    /// The normalized display name the brain produced and the kernel accepted.
    pub name: String,
    /// The agent id derived from the name (lowercase, `[a-z0-9-]`). This is the id
    /// the created agent actually carries.
    pub id: String,
    /// Optional sanitized role/description the brain extracted. Absent when none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The adapter plugin id, present ONLY when it named an existing adapter the
    /// kernel honored. Absent when the brain named none or named an unknown adapter
    /// (in which case the deterministic default adapter was used).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    /// Optional sanitized free-text notes the brain extracted (advisory/UI only;
    /// not applied to a durable kernel field). Absent when none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// The model id / CLI brain label that produced these slots, for provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// The brain-assisted, validated subject of a *risky, approval-gated* admin action
/// (a plugin install or a permission grant), present ONLY on a `Propose` turn the
/// brain sharpened.
///
/// This is purely advisory provenance: the action it describes is ALWAYS gated
/// behind a human approval (`PrimePlan::Propose`), so a brain slot can never execute
/// a plugin install or a permission grant by itself — it only sharpens the subject
/// the human reviews. The kernel validates each field
/// (`crates/relux-kernel/src/prime_admin_slots.rs`): a plugin id is normalized to a
/// plausible `[a-z0-9-]` id; a permission grant's `subject_id` is honored ONLY when
/// it names an EXISTING agent (validated against the live roster, exactly like a task
/// assignee), and the permission label is sanitized. A low-confidence, malformed,
/// unknown-field, or unvalidated-subject proposal is rejected and the deterministic
/// subject stands (this struct is absent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimeAdminSlots {
    /// Which admin action this sharpened: `"plugin_install"` or `"permission_grant"`.
    pub kind: String,
    /// The normalized plugin id, present on a `plugin_install` sharpening.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    /// The subject kind (today always `"agent"`), present on a `permission_grant`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_kind: Option<String>,
    /// The validated subject id (an EXISTING agent), present on a `permission_grant`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    /// The sanitized permission label, present on a `permission_grant`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
    /// The model id / CLI brain label that produced these slots, for provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
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
    /// Suggested next actions the chat surface renders as one-click buttons
    /// (`docs/RELUX_MASTER_PLAN.md` §11.1). Each is a pre-written user message
    /// routed through the normal turn path — never a privileged shortcut.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested_actions: Vec<PrimeSuggestion>,
    /// A reviewable, action-free plan preview, present ONLY on a `PlanRequest`
    /// turn so the dashboard can render the proposed shape as a card
    /// (`docs/RELUX_MASTER_PLAN.md` section 10 planning layer, section 11.1).
    /// Omitted on every other turn, so existing clients see the same JSON they
    /// did before. It carries no action - the commit is the separate explicit
    /// "Create these tasks" suggestion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<PrimeProposal>,
    /// The brain-assisted, validated task slots that shaped a created task, present
    /// ONLY on a `TaskCreation` / `CreateAndRunTask` turn the brain genuinely
    /// sharpened (see [`PrimeTaskSlots`]). Omitted on every other turn — including a
    /// deterministically-titled create — so existing clients see the same JSON they
    /// did before. Provenance/presentation only; the kernel already validated every
    /// slot before the task was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slots: Option<PrimeTaskSlots>,
    /// The brain-assisted, validated agent slots that shaped a created agent, present
    /// ONLY on an `AgentCreation` turn the brain genuinely sharpened (see
    /// [`PrimeAgentSlots`]). Omitted on every other turn, so existing clients see the
    /// same JSON they did before. Provenance/presentation only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_slots: Option<PrimeAgentSlots>,
    /// The brain-assisted, validated subject of a risky admin action (plugin install
    /// or permission grant), present ONLY on a `Propose` turn the brain sharpened (see
    /// [`PrimeAdminSlots`]). The action stays gated behind a human approval; this is
    /// advisory provenance only. Omitted on every other turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_slots: Option<PrimeAdminSlots>,
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
            PrimeIntent::Orchestration,
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
    fn prime_suggestion_round_trips_and_is_omitted_when_empty() {
        // A turn with no suggestions must not carry the field on the wire, so
        // existing clients see exactly the JSON they did before (§11.1).
        let turn = PrimeTurn {
            intent: PrimeIntent::Greeting,
            reply: "hi".to_string(),
            disposition: PrimeDisposition::Answered,
            action: None,
            created_task: None,
            started_run: None,
            created_agent: None,
            approval: None,
            invoked_tool: None,
            tool_output: None,
            tool_error: None,
            suggested_actions: vec![],
            proposal: None,
            slots: None,
            agent_slots: None,
            admin_slots: None,
        };
        let json = serde_json::to_string(&turn).unwrap();
        assert!(
            !json.contains("suggested_actions"),
            "empty suggestions must be omitted: {json}"
        );
        // The brain-assisted slot provenance is present ONLY on a sharpened create
        // turn: a turn the brain did not shape must omit `slots` on the wire.
        assert!(
            !json.contains("slots"),
            "a turn with no brain-assisted slots must omit the field: {json}"
        );
        // The agent/admin slot provenance is likewise present ONLY on a sharpened
        // agent-creation / approval turn; a plain turn must omit both on the wire.
        assert!(
            !json.contains("agent_slots"),
            "a turn with no brain-assisted agent slots must omit the field: {json}"
        );
        assert!(
            !json.contains("admin_slots"),
            "a turn with no brain-assisted admin slots must omit the field: {json}"
        );
        // The plan preview is present ONLY on a PlanRequest turn: a normal turn must
        // not carry `proposal` on the wire, so existing clients are unaffected (§11.1).
        assert!(
            !json.contains("proposal"),
            "an action-free turn must omit the proposal field: {json}"
        );

        let s = PrimeSuggestion {
            label: "Start the run".to_string(),
            message: "start it".to_string(),
            send: true,
        };
        let back: PrimeSuggestion = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn prime_proposal_round_trips_and_carries_only_descriptive_data() {
        // A proposal is a reviewable PREVIEW: it round-trips on the wire and carries
        // only descriptive rows (title/role/agent) - never a PrimeAction. The kernel
        // builds it from the planner's real decomposition; the commit is a separate
        // explicit suggestion (§10 planning layer, §11.1, §17.1).
        let proposal = PrimeProposal {
            goal: "ship the beta".to_string(),
            multi_step: true,
            steps: vec![
                PrimeProposalStep {
                    index: 1,
                    title: "research the options".to_string(),
                    role: "research".to_string(),
                    agent: "research-agent".to_string(),
                },
                PrimeProposalStep {
                    index: 2,
                    title: "build a prototype".to_string(),
                    role: "implementation".to_string(),
                    agent: "prime".to_string(),
                },
            ],
            agents: vec!["research-agent".to_string(), "prime".to_string()],
            polish: None,
        };
        let json = serde_json::to_string(&proposal).unwrap();
        // No action verbs leak into a preview - it is informational only.
        assert!(
            !json.contains("\"type\""),
            "a proposal must not embed an action: {json}"
        );
        let back: PrimeProposal = serde_json::from_str(&json).unwrap();
        assert_eq!(back, proposal);

        // A single-step proposal carries the goal but no fanned-out steps.
        let single = PrimeProposal {
            goal: "summarize the README".to_string(),
            multi_step: false,
            steps: vec![],
            agents: vec![],
            polish: None,
        };
        let back: PrimeProposal =
            serde_json::from_str(&serde_json::to_string(&single).unwrap()).unwrap();
        assert_eq!(back, single);
        assert!(back.steps.is_empty());
    }

    #[test]
    fn proposal_polish_is_advisory_and_omitted_when_absent() {
        // The unpolished wire is byte-for-byte unchanged: no `polish` key appears.
        let proposal = PrimeProposal {
            goal: "ship the beta".to_string(),
            multi_step: true,
            steps: vec![PrimeProposalStep {
                index: 1,
                title: "research the options".to_string(),
                role: "research".to_string(),
                agent: "research-agent".to_string(),
            }],
            agents: vec!["research-agent".to_string()],
            polish: None,
        };
        let json = serde_json::to_string(&proposal).unwrap();
        assert!(
            !json.contains("polish"),
            "an unpolished proposal must not carry a polish key: {json}"
        );

        // A polished proposal round-trips and still carries the AUTHORITATIVE
        // steps untouched; the overlay only adds presentation strings.
        let polished = PrimeProposal {
            polish: Some(PrimeProposalPolish {
                summary: Some("A clear three-stage path to a shippable beta.".to_string()),
                step_titles: vec![PrimePolishedStep {
                    index: 1,
                    title: "Survey the available options".to_string(),
                }],
                questions: vec!["Which platform are we targeting first?".to_string()],
                risks: vec!["Scope may grow past the beta cutoff.".to_string()],
                model: Some("openai/gpt-4o-mini".to_string()),
            }),
            ..proposal.clone()
        };
        let back: PrimeProposal =
            serde_json::from_str(&serde_json::to_string(&polished).unwrap()).unwrap();
        assert_eq!(back, polished);
        // The overlay never mutates the authoritative steps/agents/goal.
        assert_eq!(back.steps, proposal.steps);
        assert_eq!(back.agents, proposal.agents);
        assert_eq!(back.goal, proposal.goal);
    }

    #[test]
    fn prime_task_slots_round_trip_and_omit_empty_optionals() {
        // A minimal slot carries only the title; every optional the brain did not
        // contribute is omitted from the wire.
        let minimal = PrimeTaskSlots {
            title: "Fix the login redirect bug".to_string(),
            details: None,
            assignee: None,
            priority: None,
            source: None,
        };
        let json = serde_json::to_string(&minimal).unwrap();
        for absent in ["details", "assignee", "priority", "source"] {
            assert!(
                !json.contains(absent),
                "an unset optional must be omitted ({absent}): {json}"
            );
        }
        let back: PrimeTaskSlots = serde_json::from_str(&json).unwrap();
        assert_eq!(back, minimal);

        // A fully populated slot round-trips with every validated field intact.
        let full = PrimeTaskSlots {
            title: "Fix the login redirect bug".to_string(),
            details: Some("Users land on a blank page after SSO.".to_string()),
            assignee: Some("code-agent".to_string()),
            priority: Some(8),
            source: Some("anthropic/claude-3.5-haiku".to_string()),
        };
        let back: PrimeTaskSlots =
            serde_json::from_str(&serde_json::to_string(&full).unwrap()).unwrap();
        assert_eq!(back, full);
    }

    #[test]
    fn prime_agent_slots_round_trip_and_omit_empty_optionals() {
        // A minimal agent slot carries only name+id; every optional the brain did not
        // contribute is omitted from the wire.
        let minimal = PrimeAgentSlots {
            name: "Research Agent".to_string(),
            id: "research-agent".to_string(),
            description: None,
            adapter: None,
            notes: None,
            source: None,
        };
        let json = serde_json::to_string(&minimal).unwrap();
        for absent in ["description", "adapter", "notes", "source"] {
            assert!(
                !json.contains(absent),
                "an unset optional must be omitted ({absent}): {json}"
            );
        }
        let back: PrimeAgentSlots = serde_json::from_str(&json).unwrap();
        assert_eq!(back, minimal);

        let full = PrimeAgentSlots {
            name: "Research Agent".to_string(),
            id: "research-agent".to_string(),
            description: Some("Surveys options and writes briefs.".to_string()),
            adapter: Some("relux-adapter-local-prime".to_string()),
            notes: Some("Prefers concise output.".to_string()),
            source: Some("anthropic/claude-3.5-haiku".to_string()),
        };
        let back: PrimeAgentSlots =
            serde_json::from_str(&serde_json::to_string(&full).unwrap()).unwrap();
        assert_eq!(back, full);
    }

    #[test]
    fn prime_admin_slots_round_trip_and_omit_empty_optionals() {
        // A permission-grant sharpening carries the validated subject; the plugin
        // fields are omitted (and vice versa), so the wire stays compact.
        let perm = PrimeAdminSlots {
            kind: "permission_grant".to_string(),
            plugin_id: None,
            subject_kind: Some("agent".to_string()),
            subject_id: Some("code-agent".to_string()),
            permission: Some("tool:relux-tools-github:access".to_string()),
            source: Some("Claude CLI".to_string()),
        };
        let json = serde_json::to_string(&perm).unwrap();
        assert!(!json.contains("plugin_id"), "unset plugin_id must be omitted: {json}");
        let back: PrimeAdminSlots = serde_json::from_str(&json).unwrap();
        assert_eq!(back, perm);

        let plugin = PrimeAdminSlots {
            kind: "plugin_install".to_string(),
            plugin_id: Some("relux-tools-github".to_string()),
            subject_kind: None,
            subject_id: None,
            permission: None,
            source: None,
        };
        let json = serde_json::to_string(&plugin).unwrap();
        for absent in ["subject_kind", "subject_id", "permission", "source"] {
            assert!(!json.contains(absent), "unset {absent} must be omitted: {json}");
        }
        let back: PrimeAdminSlots = serde_json::from_str(&json).unwrap();
        assert_eq!(back, plugin);
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
