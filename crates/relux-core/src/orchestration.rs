//! Prime as a multi-agent orchestrator: the durable types and the deterministic
//! planner that turn one user goal into several briefs assigned to several agents.
//!
//! This is the first slice of multi-agent autonomy from `docs/RELUX_MASTER_PLAN.md`
//! section 10.4 (Delegation Rules) and section 15 ("Relux can support real
//! multi-agent workloads"). It deliberately stays in the same two layers as the
//! rest of Prime:
//!
//! - [`plan_orchestration`] is the pure planning brain: `(goal, StateSummary)`
//!   becomes an [`OrchestrationPlan`] of role-typed [`PlannedStep`]s, grounded in
//!   the live agent roster. No kernel access, no mutation, no clock, no network.
//! - The kernel turns a multi-agent plan into real tasks/assignments (an
//!   [`Orchestration`] record) and later runs them in a governed batch.
//!
//! The planner is conservative by construction: it only decomposes a goal that
//! actually spans multiple steps, so a greeting or a single piece of work never
//! becomes a task storm (section 10.5).

use serde::{Deserialize, Serialize};

use crate::agent::AgentId;
use crate::namespace::NamespaceId;
use crate::prime::StateSummary;
use crate::run::RunId;
use crate::task::TaskId;

/// Stable id for one Prime-coordinated multi-agent plan.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OrchestrationId(pub String);

impl OrchestrationId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for OrchestrationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The specialist role a brief needs. Prime maps each step of a goal to one role,
/// then resolves that role to a real agent on the roster (or falls back to Prime).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationRole {
    Research,
    Implementation,
    Testing,
    Review,
    Documentation,
    Operations,
    /// No specialist role recognized; safe to run on a general/local agent.
    General,
}

impl OrchestrationRole {
    /// A short human label used in replies and the dashboard.
    pub fn label(&self) -> &'static str {
        match self {
            OrchestrationRole::Research => "research",
            OrchestrationRole::Implementation => "implementation",
            OrchestrationRole::Testing => "testing",
            OrchestrationRole::Review => "review",
            OrchestrationRole::Documentation => "documentation",
            OrchestrationRole::Operations => "operations",
            OrchestrationRole::General => "general",
        }
    }

    /// Substrings that, when found in an agent id, mark that agent as a good fit
    /// for this role. Matched against the live roster so planning is grounded in
    /// agents that actually exist - Prime never invents an assignee.
    pub fn agent_keywords(&self) -> &'static [&'static str] {
        match self {
            OrchestrationRole::Research => &["research", "analyst", "investigat", "scout"],
            OrchestrationRole::Implementation => {
                &["code", "coding", "dev", "engineer", "implement", "build"]
            }
            OrchestrationRole::Testing => &["test", "qa", "quality"],
            OrchestrationRole::Review => &["review", "audit", "critic"],
            OrchestrationRole::Documentation => &["doc", "writer", "scribe"],
            OrchestrationRole::Operations => &["ops", "deploy", "release", "infra", "devops", "sre"],
            OrchestrationRole::General => &[],
        }
    }
}

/// One planned brief before anything is committed: a title, the role it needs, and
/// the existing agent Prime would assign it to (if a specialist is on the roster).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedStep {
    pub title: String,
    pub role: OrchestrationRole,
    /// The id of an existing agent that fits this role, or `None` when no
    /// specialist exists (the kernel falls back to Prime and records a note).
    pub agent_id: Option<String>,
    /// Indices (into the plan's `steps`) of the briefs this brief waits on. Only
    /// ever earlier indices, so the dependency graph is a DAG by construction.
    /// Empty for an independent step (the backward-compatible default).
    #[serde(default)]
    pub depends_on: Vec<usize>,
}

/// The pure decomposition of a goal into role-typed briefs, grounded in the
/// current roster. Produced by [`plan_orchestration`] before the kernel commits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestrationPlan {
    pub goal: String,
    pub steps: Vec<PlannedStep>,
    /// Honest notes about the plan (e.g. a role with no specialist agent).
    pub notes: Vec<String>,
}

impl OrchestrationPlan {
    /// True when the goal genuinely splits into multiple briefs. Prime only turns
    /// a multi-agent plan into work; a single-step "plan" is read as one task (or
    /// a clarifying question), never a storm (section 10.5).
    pub fn is_multi_agent(&self) -> bool {
        self.steps.len() >= 2
    }

    /// The distinct agent ids this plan would assign work to (including `prime`
    /// for unmatched roles, rendered as `"prime"`). Used for the preview line.
    pub fn agent_labels(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for s in &self.steps {
            let label = s.agent_id.clone().unwrap_or_else(|| "prime".to_string());
            if !out.contains(&label) {
                out.push(label);
            }
        }
        out
    }
}

/// The outcome of one step's most recent run inside a governed batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepOutcome {
    /// The brief exists and is assigned, but has not been run yet.
    Pending,
    /// The brief's run completed.
    Completed,
    /// The brief's run failed (e.g. the CLI returned an error). Retryable.
    Failed,
    /// The brief could not be run safely (e.g. the adapter runtime is disabled or
    /// a permission is missing). Needs a human action before it can run.
    Blocked,
}

impl StepOutcome {
    pub fn label(&self) -> &'static str {
        match self {
            StepOutcome::Pending => "pending",
            StepOutcome::Completed => "completed",
            StepOutcome::Failed => "failed",
            StepOutcome::Blocked => "blocked",
        }
    }
}

/// A committed step: a brief that became a real task assigned to a real agent,
/// linking goal -> task -> agent -> run so the user can trace the whole chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestrationStep {
    pub task_id: TaskId,
    pub agent_id: AgentId,
    pub role: OrchestrationRole,
    pub title: String,
    pub outcome: StepOutcome,
    /// Indices (into the orchestration's `steps`) of the briefs this brief waits
    /// on. The run loop only runs a brief once every dependency has `Completed`,
    /// and marks a brief `Blocked` when a dependency `Failed`/`Blocked`. Always
    /// earlier indices (a DAG); empty for an independent brief.
    #[serde(default)]
    pub depends_on: Vec<usize>,
    /// The most recent run for this step, once one has started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// A short, honest note about the last attempt (e.g. the failure reason).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// When this brief's most recent run started / finished (clock ticks), so the
    /// dashboard can render real per-brief progress after a batch completes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// The batch round this brief most recently ran in (1-based), for the
    /// "which briefs ran together" view. `None` until it has run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<u32>,
}

/// Overall lifecycle of an orchestration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationStatus {
    /// Briefs created and assigned; nothing has been run yet.
    Planned,
    /// At least one brief ran; more remain pending (run again to continue).
    Running,
    /// Every brief completed.
    Completed,
    /// No briefs are pending, but at least one failed or is blocked - a human
    /// needs to retry, reassign, or enable a runtime before it can finish.
    NeedsAttention,
}

impl OrchestrationStatus {
    pub fn label(&self) -> &'static str {
        match self {
            OrchestrationStatus::Planned => "planned",
            OrchestrationStatus::Running => "running",
            OrchestrationStatus::Completed => "completed",
            OrchestrationStatus::NeedsAttention => "needs_attention",
        }
    }
}

/// A durable record of one Prime-coordinated multi-agent plan.
///
/// This is the audit/trace anchor: it links the original goal to the briefs
/// (tasks), their assigned agents, and the runs that executed them, so a refresh
/// shows exactly what Prime did across the fleet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Orchestration {
    pub id: OrchestrationId,
    pub goal: String,
    pub created_by: String,
    pub namespace_id: NamespaceId,
    pub status: OrchestrationStatus,
    pub steps: Vec<OrchestrationStep>,
    pub notes: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    /// A one-line summary of the most recent governed batch run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_batch_summary: Option<String>,
}

/// The result of one governed multi-agent batch run.
///
/// Records per-agent outcomes and the next human action so the orchestration loop
/// is observable and stops safely instead of spinning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestrationBatchResult {
    pub orchestration_id: OrchestrationId,
    /// Number of briefs whose run was attempted in this batch.
    pub ran: u32,
    pub completed: u32,
    pub failed: u32,
    pub blocked: u32,
    /// Briefs still pending after this batch (capped out or waiting on a
    /// dependency; run again to continue).
    pub pending: u32,
    /// The round-size cap used this batch (clamped 1..=4): the most ready briefs
    /// the scheduler grouped into a single round.
    #[serde(default)]
    pub concurrency: u32,
    /// How many dependency-gated rounds the scheduler executed this batch.
    #[serde(default)]
    pub rounds: u32,
    /// Pending briefs that could not start because a dependency has not completed
    /// yet (a subset of `pending`); they become runnable once their upstream does.
    #[serde(default)]
    pub waiting: u32,
    /// Briefs marked `Blocked` this batch because an upstream brief they depend on
    /// failed or was blocked (distinct from a runtime/permission block).
    #[serde(default)]
    pub dependency_blocked: u32,
    /// Reasons individual briefs were skipped or could not run.
    pub skipped_reasons: Vec<String>,
    /// Per-agent outcome lines, e.g. `"round 1 code-agent: task_0007 completed"`.
    pub per_agent: Vec<String>,
    pub summary: String,
    pub next_action: String,
    pub status: OrchestrationStatus,
}

/// Decompose `goal` into a grounded multi-agent [`OrchestrationPlan`].
///
/// Deterministic and pure: it splits the goal into clauses on natural connectors,
/// classifies each clause to a [`OrchestrationRole`], and resolves each role to an
/// existing agent on the roster (`summary.all_agent_ids`). When no specialist
/// exists for a role the step's `agent_id` is `None` (the kernel assigns Prime and
/// records a hire suggestion). The number of steps is capped so a long sentence
/// cannot fan out without bound.
pub fn plan_orchestration(goal: &str, summary: &StateSummary) -> OrchestrationPlan {
    const MAX_STEPS: usize = 6;

    let goal = goal.trim().to_string();
    let clauses = split_into_clauses(&goal);

    // A sorted snapshot of the roster, so role->agent matching is deterministic
    // regardless of the StateSummary's (HashMap-derived) ordering.
    let mut roster: Vec<String> = summary.all_agent_ids.clone();
    roster.sort();

    let mut steps: Vec<PlannedStep> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    for clause in clauses {
        if steps.len() >= MAX_STEPS {
            notes.push(format!(
                "Goal had more than {MAX_STEPS} steps; only the first {MAX_STEPS} were planned."
            ));
            break;
        }
        let role = classify_role(&clause);
        let agent_id = match_agent_for_role(role, &roster);
        if agent_id.is_none() && role != OrchestrationRole::General {
            let note = format!(
                "No {} agent on the roster; assigning to Prime. Hire one (\"create a {} agent\") for a specialist.",
                role.label(),
                role.label()
            );
            if !notes.contains(&note) {
                notes.push(note);
            }
        }
        steps.push(PlannedStep {
            title: title_from_clause(&clause),
            role,
            agent_id,
            depends_on: Vec::new(),
        });
    }

    // Infer simple, obvious dependencies so the run loop can gate work in order:
    // implementation waits on research, and testing/review/documentation wait on
    // implementation, when those roles co-occur in the goal. Each dependency only
    // ever points at an *earlier* step, so the graph is a DAG by construction (no
    // cycles, no deadlock). A goal whose roles do not co-occur gets no
    // dependencies and runs exactly as before (backward compatible).
    let mut any_dependency = false;
    for i in 0..steps.len() {
        let prereqs = prerequisite_roles(steps[i].role);
        if prereqs.is_empty() {
            continue;
        }
        let deps: Vec<usize> = (0..i).filter(|&j| prereqs.contains(&steps[j].role)).collect();
        if !deps.is_empty() {
            any_dependency = true;
        }
        steps[i].depends_on = deps;
    }
    if any_dependency {
        notes.push(
            "Dependency order: implementation waits on research; testing, review, and \
             documentation wait on implementation."
                .to_string(),
        );
    }

    OrchestrationPlan { goal, steps, notes }
}

/// The roles a step of `role` must wait for when both appear in the same goal.
/// This is the planner's whole dependency policy: implementation waits on
/// research; testing, review, and documentation wait on implementation. Every
/// other role (research, operations, general) imposes no inferred prerequisite
/// and runs as soon as it is the next ready brief.
fn prerequisite_roles(role: OrchestrationRole) -> &'static [OrchestrationRole] {
    use OrchestrationRole::*;
    match role {
        Implementation => &[Research],
        Testing | Review | Documentation => &[Implementation],
        Research | Operations | General => &[],
    }
}

/// Natural-language connectors that separate steps within a goal, longest first
/// so a compound connector wins over its shorter prefix.
const CLAUSE_CONNECTORS: &[&str] = &[
    ", and then ",
    " and then ",
    ", then ",
    " then ",
    " after that, ",
    " after that ",
    "; ",
    ", and ",
    " and ",
    ", ",
];

/// Split a goal into trimmed clauses on the recognized connectors. Empty and
/// trivially-short fragments are dropped so punctuation noise never mints a brief.
fn split_into_clauses(goal: &str) -> Vec<String> {
    let mut fragments: Vec<String> = vec![goal.to_string()];
    for connector in CLAUSE_CONNECTORS {
        let mut next: Vec<String> = Vec::new();
        for fragment in fragments {
            next.extend(split_ci(&fragment, connector));
        }
        fragments = next;
    }
    fragments
        .into_iter()
        .map(|f| f.trim().trim_matches(|c: char| c == '.' || c == ',').trim().to_string())
        .filter(|f| f.chars().filter(|c| c.is_alphanumeric()).count() >= 3)
        .collect()
}

/// Case-insensitive split of `s` on `delim`. The delimiters are ASCII, so byte
/// offsets on the lowercased copy line up with the original.
fn split_ci(s: &str, delim: &str) -> Vec<String> {
    let lower = s.to_lowercase();
    let dl = delim.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    let mut start = 0usize;
    let mut search = 0usize;
    while let Some(pos) = lower[search..].find(&dl) {
        let abs = search + pos;
        out.push(s[start..abs].to_string());
        start = abs + delim.len();
        search = start;
    }
    out.push(s[start..].to_string());
    out
}

/// Classify a clause into the specialist role it most needs. Order matters: more
/// specific roles (research, testing, review, docs, ops) are checked before the
/// broad implementation catch so "write the docs" is documentation, not codework.
fn classify_role(clause: &str) -> OrchestrationRole {
    let c = clause.to_lowercase();
    let has = |kws: &[&str]| kws.iter().any(|k| c.contains(k));
    if has(&[
        "research", "investigat", "explore", "gather", "find out", "look into", "survey",
        "analyz", "analyse", "compare",
    ]) {
        OrchestrationRole::Research
    } else if has(&["test", "qa", "verif", "validat", "coverage"]) {
        OrchestrationRole::Testing
    } else if has(&["review", "audit", "inspect", "proofread", "evaluat", "critique"]) {
        OrchestrationRole::Review
    } else if has(&[
        "document", "docs", "readme", "changelog", "write up", "write-up", "summari", "draft",
    ]) {
        OrchestrationRole::Documentation
    } else if has(&["deploy", "release", "ship", "publish", "provision", "infra", "rollout"]) {
        OrchestrationRole::Operations
    } else if has(&[
        "implement", "build", "code", "develop", "refactor", "integrat", "set up", "add ",
        "create", "write ", "fix", "prototype",
    ]) {
        OrchestrationRole::Implementation
    } else {
        OrchestrationRole::General
    }
}

/// Find the first agent on the (sorted) roster whose id signals a fit for `role`.
fn match_agent_for_role(role: OrchestrationRole, roster: &[String]) -> Option<String> {
    let keywords = role.agent_keywords();
    if keywords.is_empty() {
        return None;
    }
    roster
        .iter()
        .find(|id| {
            // Prime is the general fallback, never a "specialist" match.
            id.as_str() != "prime" && {
                let lower = id.to_lowercase();
                keywords.iter().any(|k| lower.contains(k))
            }
        })
        .cloned()
}

/// Build a readable brief title from a clause: trim, uppercase the first letter,
/// cap the length.
fn title_from_clause(clause: &str) -> String {
    let trimmed = clause.trim();
    let mut chars = trimmed.chars();
    let title: String = match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    };
    title.chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary_with_agents(ids: &[&str]) -> StateSummary {
        StateSummary {
            plugins: 0,
            agents: ids.len(),
            tasks_total: 0,
            tasks_open: 0,
            runs_active: 0,
            tasks_waiting_approval: 0,
            tasks_blocked: 0,
            tasks_failed: 0,
            pending_approvals: 0,
            all_agent_ids: ids.iter().map(|s| s.to_string()).collect(),
            all_task_ids: vec![],
            queued: vec![],
            recent: vec![],
        }
    }

    #[test]
    fn decomposes_a_multi_step_goal_into_role_briefs() {
        let s = summary_with_agents(&["prime", "research-agent", "code-agent"]);
        let plan = plan_orchestration(
            "research the best rust web framework, implement a prototype, and write the docs",
            &s,
        );
        assert!(plan.is_multi_agent());
        let roles: Vec<OrchestrationRole> = plan.steps.iter().map(|s| s.role).collect();
        assert_eq!(
            roles,
            vec![
                OrchestrationRole::Research,
                OrchestrationRole::Implementation,
                OrchestrationRole::Documentation,
            ]
        );
        // Research and implementation map to the matching specialist agents.
        assert_eq!(plan.steps[0].agent_id.as_deref(), Some("research-agent"));
        assert_eq!(plan.steps[1].agent_id.as_deref(), Some("code-agent"));
        // No documentation specialist -> falls back to Prime, with an honest note.
        assert_eq!(plan.steps[2].agent_id, None);
        assert!(plan.notes.iter().any(|n| n.contains("documentation")));
    }

    #[test]
    fn infers_research_then_implementation_then_checks_dependencies() {
        let s = summary_with_agents(&["prime"]);
        let plan = plan_orchestration(
            "research the options, implement a prototype, test it, and document it",
            &s,
        );
        let roles: Vec<OrchestrationRole> = plan.steps.iter().map(|s| s.role).collect();
        assert_eq!(
            roles,
            vec![
                OrchestrationRole::Research,
                OrchestrationRole::Implementation,
                OrchestrationRole::Testing,
                OrchestrationRole::Documentation,
            ]
        );
        // research is independent; implementation waits on research; test + docs
        // wait on implementation.
        assert_eq!(plan.steps[0].depends_on, Vec::<usize>::new());
        assert_eq!(plan.steps[1].depends_on, vec![0]);
        assert_eq!(plan.steps[2].depends_on, vec![1]);
        assert_eq!(plan.steps[3].depends_on, vec![1]);
        assert!(plan.notes.iter().any(|n| n.contains("Dependency order")));
    }

    #[test]
    fn independent_steps_have_no_dependencies() {
        // Two research clauses: same tier, no prerequisite between them, so both
        // are independent and can run in one round.
        let s = summary_with_agents(&["prime"]);
        let plan = plan_orchestration(
            "research the rust options and research the go options",
            &s,
        );
        assert!(plan.is_multi_agent());
        assert!(plan.steps.iter().all(|st| st.depends_on.is_empty()));
        assert!(!plan.notes.iter().any(|n| n.contains("Dependency order")));
    }

    #[test]
    fn inferred_dependencies_always_point_at_earlier_steps() {
        // The DAG invariant the run loop relies on: no dependency index is >= its
        // own step index, so the graph can never contain a cycle.
        let s = summary_with_agents(&["prime"]);
        let plan = plan_orchestration(
            "research it, implement it, review it, test it, and document it",
            &s,
        );
        for (i, step) in plan.steps.iter().enumerate() {
            for &dep in &step.depends_on {
                assert!(dep < i, "step {i} depends on a later/self step {dep}");
            }
        }
    }

    #[test]
    fn legacy_step_without_dependency_fields_deserializes_to_defaults() {
        // A record written before this slice has no depends_on / timing / round
        // fields; it must load with empty deps and no timing (backward compatible).
        let json = r#"{
            "task_id": "task_0001",
            "agent_id": "code-agent",
            "role": "implementation",
            "title": "Build it",
            "outcome": "pending"
        }"#;
        let step: OrchestrationStep = serde_json::from_str(json).unwrap();
        assert!(step.depends_on.is_empty());
        assert!(step.run_id.is_none());
        assert!(step.started_at.is_none());
        assert!(step.finished_at.is_none());
        assert!(step.round.is_none());
    }

    #[test]
    fn single_step_goal_is_not_multi_agent() {
        let s = summary_with_agents(&["prime"]);
        let plan = plan_orchestration("summarize the README", &s);
        assert!(!plan.is_multi_agent(), "one clause must not fan out");
    }

    #[test]
    fn agent_matching_is_deterministic_regardless_of_roster_order() {
        let a = plan_orchestration(
            "implement the feature and test it",
            &summary_with_agents(&["zeta-code-agent", "alpha-code-agent", "prime"]),
        );
        let b = plan_orchestration(
            "implement the feature and test it",
            &summary_with_agents(&["alpha-code-agent", "prime", "zeta-code-agent"]),
        );
        // Sorted roster -> the lexically-first matching agent wins both times.
        assert_eq!(a.steps[0].agent_id, b.steps[0].agent_id);
        assert_eq!(a.steps[0].agent_id.as_deref(), Some("alpha-code-agent"));
    }

    #[test]
    fn step_count_is_capped() {
        let s = summary_with_agents(&["prime"]);
        let plan = plan_orchestration(
            "research a, research b, research c, research d, research e, research f, research g, research h",
            &s,
        );
        assert!(plan.steps.len() <= 6);
        assert!(plan.notes.iter().any(|n| n.contains("only the first")));
    }

    #[test]
    fn roles_round_trip_through_serde() {
        for role in [
            OrchestrationRole::Research,
            OrchestrationRole::Implementation,
            OrchestrationRole::Testing,
            OrchestrationRole::Review,
            OrchestrationRole::Documentation,
            OrchestrationRole::Operations,
            OrchestrationRole::General,
        ] {
            let json = serde_json::to_string(&role).unwrap();
            let back: OrchestrationRole = serde_json::from_str(&json).unwrap();
            assert_eq!(role, back);
        }
    }

    #[test]
    fn orchestration_record_round_trips() {
        let rec = Orchestration {
            id: OrchestrationId::new("orch_0001"),
            goal: "do the thing".to_string(),
            created_by: "founder".to_string(),
            namespace_id: NamespaceId::new("workspace"),
            status: OrchestrationStatus::Planned,
            steps: vec![OrchestrationStep {
                task_id: TaskId::new("task_0001"),
                agent_id: AgentId::new("code-agent"),
                role: OrchestrationRole::Implementation,
                title: "Build it".to_string(),
                outcome: StepOutcome::Pending,
                depends_on: vec![],
                run_id: None,
                note: None,
                started_at: None,
                finished_at: None,
                round: None,
            }],
            notes: vec![],
            created_at: "t0".to_string(),
            updated_at: "t0".to_string(),
            last_batch_summary: None,
        };
        let json = serde_json::to_string(&rec).unwrap();
        let back: Orchestration = serde_json::from_str(&json).unwrap();
        assert_eq!(rec, back);
    }
}
