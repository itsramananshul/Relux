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

/// A deterministic, operator-named tool-call directive a [`Task`] can carry in its
/// `input` so a LOCAL run executes exactly ONE explicitly-named tool through the
/// kernel's gated `call_tool` path (permission + approval/grant + audit + run
/// transcript) instead of the default echo.
///
/// The directive is fixed in the task `input` when the task is created — the brain
/// never chooses the tool. `plugin` may be a real installed plugin id or a synthetic
/// `mcp:<server>` MCP server (see [`crate::mcp_synthetic_plugin_id`]); the kernel's
/// `call_tool` applies the identical gates to both, so a run-driven MCP `tools/call`
/// is no weaker than a plugin tool call. Spec: `docs/mcp.md` "Run-driven MCP tool
/// call"; `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §9.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskToolCall {
    /// The plugin id to invoke — a real plugin id or a synthetic `mcp:<server>`.
    pub plugin: String,
    /// The tool name to call on that plugin (e.g. an MCP server's `search`).
    pub tool: String,
    /// The JSON arguments forwarded verbatim to the tool. Defaults to `{}`.
    #[serde(default)]
    pub args: serde_json::Value,
}

impl TaskToolCall {
    /// Serialize this directive into the canonical Task `input` shape
    /// (`{ "tool_call": { plugin, tool, args } }`) that [`parse_task_tool_call`]
    /// reads back. A null/absent `args` is normalized to `{}`.
    pub fn to_input(&self) -> serde_json::Value {
        let args = if self.args.is_null() {
            serde_json::json!({})
        } else {
            self.args.clone()
        };
        serde_json::json!({
            "tool_call": { "plugin": self.plugin, "tool": self.tool, "args": args }
        })
    }
}

/// The CONSERVATIVE static default for the number of steps an operator may put in a
/// [`TaskToolPlan`] when no operator policy is threaded through (tests, static/CLI
/// contexts, and the no-arg [`TaskToolPlan::validate`]). The operator-facing surfaces
/// (task-create route, Prime tool-plan proposal) instead read the configured limit from
/// [`crate::PrimeAgentPolicy`] (`max_tool_plan_steps` standard / `extended_*` extended)
/// and call [`TaskToolPlan::validate_with_limit`]. This default is aligned with
/// [`crate::MAX_ORCHESTRATION_STEPS`] (16) — a serious multi-step plan, not the retired
/// toy `5`. It is still a real bound, enforced at task-creation time (over-long plans are
/// rejected, never silently truncated). Spec: `docs/ARTIFICIAL_CONSTRAINT_AUDIT.md`
/// (configurable tool-plan policy); `docs/mcp.md` "Run-driven multi-tool plan".
pub const MAX_TASK_TOOL_PLAN_STEPS: usize = 16;

/// The absolute hard backstop on a [`TaskToolPlan`]'s step count — the ceiling the
/// configurable per-plan limit ([`crate::PrimeAgentPolicy::tool_plan_steps`]) is clamped
/// to, and the bound the permissive READ path ([`parse_task_tool_plan`]) enforces so any
/// plan that legitimately passed create-time validation under an operator's (possibly
/// extended) limit still reads back, while a pathological list can never fan out without
/// bound. No operator setting and no `validate_with_limit` caller can exceed this.
pub const MAX_TASK_TOOL_PLAN_STEPS_CEIL: usize = 64;

/// The per-step args size cap (bytes of the serialized JSON). Mirrors the kernel's
/// `MAX_TOOL_INVOCATION_ARGS_BYTES` (256 KiB) loopback request cap so a plan step can
/// never carry args the gated `call_tool` path would itself reject — the bound is
/// applied up front at task-creation time, fail closed.
pub const MAX_TASK_TOOL_PLAN_ARGS_BYTES: usize = 256 * 1024;

/// A bounded, operator-authored SEQUENCE of tool-call steps a [`Task`] can carry in its
/// `input` (`{ "tool_plan": [ { plugin, tool, args }, … ] }`) so a LOCAL run executes
/// each named step in order through the gated `call_tool` chokepoint, **stopping on the
/// first failure/denial**. This is the bounded multi-tool sibling of the single
/// [`TaskToolCall`] directive: every step is the same gated `mcp:<server>`-or-plugin
/// call, and the whole list is validated strictly at task-creation time
/// ([`TaskToolPlan::validate`]). The brain never chooses a step — the sequence is fixed
/// when the task is created. Spec: `docs/mcp.md` "Run-driven multi-tool plan";
/// `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §9.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskToolPlan {
    /// The ordered tool-call steps. Validated non-empty and `≤ MAX_TASK_TOOL_PLAN_STEPS`.
    pub steps: Vec<TaskToolCall>,
}

/// A strict, create-time validation failure for a [`TaskToolPlan`]. Surfaced as an
/// honest `400` on the task-create route — never silently coerced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskToolPlanError {
    /// `tool_plan` was present but carried no steps.
    Empty,
    /// More than [`MAX_TASK_TOOL_PLAN_STEPS`] steps.
    TooManySteps { max: usize, got: usize },
    /// A step (0-based `index`) had an empty plugin or tool after trimming.
    EmptyStep { index: usize },
    /// A step's serialized args exceeded [`MAX_TASK_TOOL_PLAN_ARGS_BYTES`].
    ArgsTooLarge { index: usize, max: usize, got: usize },
}

impl std::fmt::Display for TaskToolPlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskToolPlanError::Empty => write!(f, "tool_plan must have at least one step"),
            TaskToolPlanError::TooManySteps { max, got } => write!(
                f,
                "tool_plan has {got} steps; the maximum is {max} — remove steps, or raise the \
                 tool-plan step limit in Prime Autonomy settings (standard/extended)"
            ),
            TaskToolPlanError::EmptyStep { index } => {
                write!(f, "tool_plan step {index} requires a non-empty plugin and tool")
            }
            TaskToolPlanError::ArgsTooLarge { index, max, got } => write!(
                f,
                "tool_plan step {index} args are {got} bytes; the maximum is {max}"
            ),
        }
    }
}

impl std::error::Error for TaskToolPlanError {}

impl TaskToolPlan {
    /// Validate the whole plan strictly against the CONSERVATIVE static default
    /// ([`MAX_TASK_TOOL_PLAN_STEPS`]). Use this in static/test/CLI contexts that carry no
    /// operator policy; the operator-facing surfaces call [`Self::validate_with_limit`]
    /// with the configured per-deployment limit. Fails closed (mirrors openclaw's
    /// `buildToolPlan` posture: every entry is checked BEFORE any execution rather than
    /// discovering invalidity mid-run).
    pub fn validate(&self) -> Result<(), TaskToolPlanError> {
        self.validate_with_limit(MAX_TASK_TOOL_PLAN_STEPS)
    }

    /// Validate the whole plan strictly against an operator-CONFIGURED step limit,
    /// fail closed. `max_steps` is itself clamped into `1..=MAX_TASK_TOOL_PLAN_STEPS_CEIL`
    /// so no caller — however the policy was set — can validate a plan past the absolute
    /// hard backstop. A plan must be non-empty, `≤` the clamped limit, every step must
    /// carry a non-empty plugin + tool, and every step's serialized args must be `≤
    /// MAX_TASK_TOOL_PLAN_ARGS_BYTES`. The configurable limit is the configurable-budget
    /// precedent from Hermes `agent/iteration_budget.py` (a tunable bound, not a tiny
    /// constant) applied to operator-authored plans; spec
    /// `docs/ARTIFICIAL_CONSTRAINT_AUDIT.md` (configurable tool-plan policy).
    pub fn validate_with_limit(&self, max_steps: usize) -> Result<(), TaskToolPlanError> {
        let max = max_steps.clamp(1, MAX_TASK_TOOL_PLAN_STEPS_CEIL);
        if self.steps.is_empty() {
            return Err(TaskToolPlanError::Empty);
        }
        if self.steps.len() > max {
            return Err(TaskToolPlanError::TooManySteps {
                max,
                got: self.steps.len(),
            });
        }
        for (index, step) in self.steps.iter().enumerate() {
            if step.plugin.trim().is_empty() || step.tool.trim().is_empty() {
                return Err(TaskToolPlanError::EmptyStep { index });
            }
            let bytes = serde_json::to_string(&step.args)
                .map(|s| s.len())
                .unwrap_or(0);
            if bytes > MAX_TASK_TOOL_PLAN_ARGS_BYTES {
                return Err(TaskToolPlanError::ArgsTooLarge {
                    index,
                    max: MAX_TASK_TOOL_PLAN_ARGS_BYTES,
                    got: bytes,
                });
            }
        }
        Ok(())
    }

    /// Serialize this plan into the canonical Task `input` shape
    /// (`{ "tool_plan": [ { plugin, tool, args }, … ] }`) that [`parse_task_tool_plan`]
    /// reads back. Each step's plugin/tool are trimmed and a null/absent `args` is
    /// normalized to `{}`.
    pub fn to_input(&self) -> serde_json::Value {
        let steps: Vec<serde_json::Value> = self
            .steps
            .iter()
            .map(|s| {
                let args = if s.args.is_null() {
                    serde_json::json!({})
                } else {
                    s.args.clone()
                };
                serde_json::json!({ "plugin": s.plugin.trim(), "tool": s.tool.trim(), "args": args })
            })
            .collect();
        serde_json::json!({ "tool_plan": steps })
    }
}

/// Parse a [`TaskToolPlan`]'s steps out of a Task `input`, returning `None` for an input
/// that carries no (well-formed) `tool_plan`. A plan requires a non-empty `tool_plan`
/// array of `≤ MAX_TASK_TOOL_PLAN_STEPS_CEIL` entries (the absolute hard backstop, NOT
/// the per-deployment operator limit — create-time validation already enforced the
/// operator's configured limit, so the read path bounds only at the ceiling and never
/// re-rejects a plan that was legitimately created under an extended limit), each with a
/// non-empty plugin + tool (trimmed); each step's `args` defaults to `{}`. Anything
/// malformed — wrong type, empty, past the ceiling, or an empty plugin/tool — yields
/// `None` so the local run falls back rather than guessing. This mirrors
/// [`parse_task_tool_call`]'s permissive read posture; the strict create-time gate is
/// [`TaskToolPlan::validate_with_limit`].
pub fn parse_task_tool_plan(input: &serde_json::Value) -> Option<Vec<TaskToolCall>> {
    let arr = input.get("tool_plan")?.as_array()?;
    if arr.is_empty() || arr.len() > MAX_TASK_TOOL_PLAN_STEPS_CEIL {
        return None;
    }
    let mut steps = Vec::with_capacity(arr.len());
    for step in arr {
        let plugin = step.get("plugin")?.as_str()?.trim().to_string();
        let tool = step.get("tool")?.as_str()?.trim().to_string();
        if plugin.is_empty() || tool.is_empty() {
            return None;
        }
        let args = step
            .get("args")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        steps.push(TaskToolCall { plugin, tool, args });
    }
    Some(steps)
}

/// Parse a [`TaskToolCall`] directive out of a Task `input`, returning `None` for an
/// ordinary (echo) task. A directive requires a non-empty `tool_call.plugin` and
/// `tool_call.tool`; `args` defaults to `{}`. Both are trimmed; if either is empty
/// after trimming it is NOT treated as a directive (the local run falls back to the
/// default echo rather than guessing). This is the only thing that turns a local run
/// into a gated tool call — there is no implicit brain-chosen tool selection.
pub fn parse_task_tool_call(input: &serde_json::Value) -> Option<TaskToolCall> {
    let tc = input.get("tool_call")?;
    let plugin = tc.get("plugin")?.as_str()?.trim().to_string();
    let tool = tc.get("tool")?.as_str()?.trim().to_string();
    if plugin.is_empty() || tool.is_empty() {
        return None;
    }
    let args = tc
        .get("args")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    Some(TaskToolCall { plugin, tool, args })
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

// --- Ad-hoc task subtrees (the `parent_task` edge) ------------------------------
//
// Section: `docs/relix-dashboard-design.md` §6.2 (the still-pending "ad-hoc
// (non-orchestration) task subtrees" target — the orchestration is the ONLY real
// parent→child link today; these pure helpers let the kernel populate the inert
// `Task::parent_task` edge safely).
//
// Reference-driven (`docs/reference-driven-development.md`): the same bounded,
// cycle-guarded parent-pointer walk the org-lattice uses (`hierarchy.rs`,
// Paperclip `agentIsInSubtree` / Hermes `delegate_tool` `MAX_DEPTH`), applied to the
// task tree rather than the agent tree. A parent pointer walked under a hard depth
// bound, fail-narrow by default.

/// Hard cap on how deep an ad-hoc task subtree is walked when validating a proposed
/// parent edge. Deep enough for any real plan decomposition, bounded so a malformed/
/// cyclic map can never loop forever (defence in depth — the kernel boundary rejects
/// a cycle before persisting an edge). Mirrors the org-lattice `MAX_HIERARCHY_DEPTH`.
pub const MAX_TASK_DEPTH: usize = 64;

/// Child task id → parent task id. One entry per task that carries a `parent_task`; a
/// top-level (standalone) task simply has no entry. Built from the live task set at
/// the call site. A hash map because every consumer walks the graph by following
/// pointers — the outputs depend on the edges, never on map iteration order.
pub type TaskParentMap = std::collections::HashMap<TaskId, TaskId>;

/// The ordered chain of ancestor tasks above `task` (nearest parent first). The walk
/// stops at a standalone task (no entry), a dangling parent id (still returned, the
/// walk just can't continue), a repeat (cycle guard), or [`MAX_TASK_DEPTH`]. `task`
/// itself is never included. Pure; total even on a cyclic map.
pub fn task_ancestors(task: &TaskId, parent_of: &TaskParentMap) -> Vec<TaskId> {
    let mut chain = Vec::new();
    let mut seen = std::collections::HashSet::new();
    seen.insert(task.clone());
    let mut current = task.clone();
    for _ in 0..MAX_TASK_DEPTH {
        match parent_of.get(&current) {
            // Stop the moment we'd revisit a node — the cycle guard keeps the walk
            // total even if a cyclic map ever reached this code.
            Some(parent) if !seen.contains(parent) => {
                chain.push(parent.clone());
                seen.insert(parent.clone());
                current = parent.clone();
            }
            _ => break,
        }
    }
    chain
}

/// True iff `ancestor` is a (transitive) parent of `task` — i.e. `task` sits somewhere
/// in `ancestor`'s subtree. Proper-descendant semantics: a task is NOT in its own
/// subtree, so `is_in_task_subtree(x, x)` is `false`. Bounded by [`MAX_TASK_DEPTH`].
pub fn is_in_task_subtree(ancestor: &TaskId, task: &TaskId, parent_of: &TaskParentMap) -> bool {
    if ancestor == task {
        return false;
    }
    task_ancestors(task, parent_of)
        .iter()
        .any(|a| a == ancestor)
}

/// True iff pointing `child` → `new_parent` would create a cycle in the task tree:
/// either a self-parent (`child == new_parent`) or `new_parent` already sits in
/// `child`'s subtree (so the new edge would close a loop). Pure; used by the kernel
/// boundary before persisting a `parent_task` edge.
///
/// The map passed in is the CURRENT graph (it does not yet contain the proposed
/// `child → new_parent` edge); the check walks up from `new_parent` and asks whether
/// it can already reach `child`, which is exactly the loop the new edge would close.
pub fn would_create_task_cycle(
    child: &TaskId,
    new_parent: &TaskId,
    parent_of: &TaskParentMap,
) -> bool {
    child == new_parent || is_in_task_subtree(child, new_parent, parent_of)
}

#[cfg(test)]
mod task_subtree_tests {
    use super::*;

    fn id(s: &str) -> TaskId {
        TaskId::new(s)
    }

    /// Build a child→parent map from `(child, parent)` pairs.
    fn map(edges: &[(&str, &str)]) -> TaskParentMap {
        edges.iter().map(|(c, p)| (id(c), id(p))).collect()
    }

    #[test]
    fn ancestors_walk_nearest_first_and_stop_at_the_root() {
        // leaf -> mid -> root ; root is top-level (standalone).
        let m = map(&[("leaf", "mid"), ("mid", "root")]);
        assert_eq!(task_ancestors(&id("leaf"), &m), vec![id("mid"), id("root")]);
        assert_eq!(task_ancestors(&id("mid"), &m), vec![id("root")]);
        // A standalone task has no ancestors.
        assert!(task_ancestors(&id("root"), &m).is_empty());
        // A task not in the map at all is treated as standalone.
        assert!(task_ancestors(&id("ghost"), &m).is_empty());
    }

    #[test]
    fn subtree_membership_is_proper_descendant_only() {
        let m = map(&[("leaf", "mid"), ("mid", "root"), ("peer", "root")]);
        assert!(is_in_task_subtree(&id("root"), &id("mid"), &m));
        assert!(is_in_task_subtree(&id("root"), &id("leaf"), &m));
        assert!(is_in_task_subtree(&id("root"), &id("peer"), &m));
        assert!(is_in_task_subtree(&id("mid"), &id("leaf"), &m));
        // Not a descendant: self, upward, and sideways.
        assert!(!is_in_task_subtree(&id("root"), &id("root"), &m), "self not in own subtree");
        assert!(!is_in_task_subtree(&id("leaf"), &id("root"), &m), "child is not above its parent");
        assert!(!is_in_task_subtree(&id("mid"), &id("peer"), &m), "siblings' subtrees don't overlap");
    }

    #[test]
    fn cycle_check_rejects_self_direct_and_transitive_loops() {
        let m = map(&[("leaf", "mid"), ("mid", "root")]);
        // Self-parent.
        assert!(would_create_task_cycle(&id("mid"), &id("mid"), &m));
        // root -> leaf would close root -> leaf -> mid -> root.
        assert!(would_create_task_cycle(&id("root"), &id("leaf"), &m));
        // root -> mid would close root -> mid -> root.
        assert!(would_create_task_cycle(&id("root"), &id("mid"), &m));
        // A safe edge (peer under mid) is fine; so is a fresh, unrelated child.
        assert!(!would_create_task_cycle(&id("peer"), &id("mid"), &m));
        assert!(!would_create_task_cycle(&id("brand_new"), &id("root"), &m));
        // leaf -> root (skip-level re-parent) is acyclic and allowed.
        assert!(!would_create_task_cycle(&id("leaf"), &id("root"), &m));
    }

    #[test]
    fn walks_stay_total_under_a_cyclic_map() {
        // a -> b -> a (a cycle that should never be persisted, but must not hang).
        let m = map(&[("a", "b"), ("b", "a")]);
        let chain = task_ancestors(&id("a"), &m);
        assert!(chain.len() <= MAX_TASK_DEPTH);
        assert_eq!(chain, vec![id("b")]);
    }

    #[test]
    fn deep_chain_is_capped_at_max_depth() {
        let edges: Vec<(String, String)> = (0..MAX_TASK_DEPTH + 5)
            .map(|i| (format!("n{i}"), format!("n{}", i + 1)))
            .collect();
        let m: TaskParentMap = edges.iter().map(|(c, p)| (id(c), id(p))).collect();
        assert_eq!(task_ancestors(&id("n0"), &m).len(), MAX_TASK_DEPTH);
    }
}

#[cfg(test)]
mod tool_call_directive_tests {
    use super::*;

    #[test]
    fn to_input_round_trips_through_parse() {
        let d = TaskToolCall {
            plugin: "mcp:fs".to_string(),
            tool: "search".to_string(),
            args: serde_json::json!({ "q": "files" }),
        };
        let input = d.to_input();
        assert_eq!(input["tool_call"]["plugin"], "mcp:fs");
        let parsed = parse_task_tool_call(&input).expect("a directive");
        assert_eq!(parsed, d);
    }

    #[test]
    fn null_args_normalize_to_empty_object() {
        let d = TaskToolCall {
            plugin: "mcp:fs".to_string(),
            tool: "search".to_string(),
            args: serde_json::Value::Null,
        };
        assert_eq!(d.to_input()["tool_call"]["args"], serde_json::json!({}));
        let parsed = parse_task_tool_call(&d.to_input()).unwrap();
        assert_eq!(parsed.args, serde_json::json!({}));
    }

    #[test]
    fn an_ordinary_input_is_not_a_directive() {
        assert!(parse_task_tool_call(&serde_json::json!({ "message": "hi" })).is_none());
        assert!(parse_task_tool_call(&serde_json::json!({})).is_none());
    }

    #[test]
    fn empty_plugin_or_tool_is_not_a_directive() {
        // Whitespace-only / missing fields fall back to echo rather than guessing.
        assert!(parse_task_tool_call(
            &serde_json::json!({ "tool_call": { "plugin": "  ", "tool": "search" } })
        )
        .is_none());
        assert!(parse_task_tool_call(
            &serde_json::json!({ "tool_call": { "plugin": "mcp:fs", "tool": "" } })
        )
        .is_none());
        assert!(
            parse_task_tool_call(&serde_json::json!({ "tool_call": { "plugin": "mcp:fs" } }))
                .is_none()
        );
    }

    #[test]
    fn missing_args_defaults_to_empty_object() {
        let parsed = parse_task_tool_call(
            &serde_json::json!({ "tool_call": { "plugin": "mcp:fs", "tool": "search" } }),
        )
        .unwrap();
        assert_eq!(parsed.args, serde_json::json!({}));
    }
}

#[cfg(test)]
mod tool_plan_tests {
    use super::*;

    fn step(plugin: &str, tool: &str, args: serde_json::Value) -> TaskToolCall {
        TaskToolCall { plugin: plugin.to_string(), tool: tool.to_string(), args }
    }

    #[test]
    fn plan_to_input_round_trips_through_parse() {
        let plan = TaskToolPlan {
            steps: vec![
                step("mcp:fs", "search", serde_json::json!({ "q": "a" })),
                step("relux-tools-echo", "echo.say", serde_json::json!({ "x": 1 })),
            ],
        };
        plan.validate().expect("valid plan");
        let input = plan.to_input();
        let parsed = parse_task_tool_plan(&input).expect("a plan");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed, plan.steps);
    }

    #[test]
    fn plan_step_null_args_normalize_to_empty_object() {
        let plan = TaskToolPlan { steps: vec![step("mcp:fs", "search", serde_json::Value::Null)] };
        assert_eq!(plan.to_input()["tool_plan"][0]["args"], serde_json::json!({}));
        let parsed = parse_task_tool_plan(&plan.to_input()).unwrap();
        assert_eq!(parsed[0].args, serde_json::json!({}));
    }

    #[test]
    fn empty_plan_is_rejected_and_not_parsed() {
        let plan = TaskToolPlan { steps: vec![] };
        assert_eq!(plan.validate(), Err(TaskToolPlanError::Empty));
        assert!(parse_task_tool_plan(&serde_json::json!({ "tool_plan": [] })).is_none());
    }

    #[test]
    fn too_many_steps_is_rejected_against_the_static_default() {
        // The no-arg validate() enforces the CONSERVATIVE static default (16) — over that
        // is rejected with the limit named, never silently truncated.
        let many: Vec<TaskToolCall> = (0..MAX_TASK_TOOL_PLAN_STEPS + 1)
            .map(|i| step("mcp:fs", &format!("t{i}"), serde_json::json!({})))
            .collect();
        let plan = TaskToolPlan { steps: many };
        assert_eq!(
            plan.validate(),
            Err(TaskToolPlanError::TooManySteps {
                max: MAX_TASK_TOOL_PLAN_STEPS,
                got: MAX_TASK_TOOL_PLAN_STEPS + 1
            })
        );
    }

    #[test]
    fn over_the_ceiling_is_not_parsed() {
        // The permissive read path bounds at the absolute hard backstop (the ceiling),
        // not the per-deployment limit — so a plan past the ceiling reads back as None.
        let over: Vec<TaskToolCall> = (0..MAX_TASK_TOOL_PLAN_STEPS_CEIL + 1)
            .map(|i| step("mcp:fs", &format!("t{i}"), serde_json::json!({})))
            .collect();
        let plan = TaskToolPlan { steps: over };
        assert!(parse_task_tool_plan(&plan.to_input()).is_none());
    }

    #[test]
    fn validate_with_limit_honors_a_configured_limit_and_clamps_to_the_ceiling() {
        let eight: Vec<TaskToolCall> = (0..8)
            .map(|i| step("mcp:fs", &format!("t{i}"), serde_json::json!({})))
            .collect();
        // A standard default (16) permits well over the retired toy 5.
        assert!(TaskToolPlan { steps: eight.clone() }.validate().is_ok());
        // A lower operator limit (5) rejects the same plan, naming that limit.
        assert_eq!(
            TaskToolPlan { steps: eight.clone() }.validate_with_limit(5),
            Err(TaskToolPlanError::TooManySteps { max: 5, got: 8 })
        );
        // An extended-sized plan validates under an extended-sized limit.
        let forty: Vec<TaskToolCall> = (0..40)
            .map(|i| step("mcp:fs", &format!("t{i}"), serde_json::json!({})))
            .collect();
        assert!(TaskToolPlan { steps: forty }.validate_with_limit(64).is_ok());
        // A limit above the ceiling is clamped DOWN to the ceiling — no caller can exceed it.
        let over: Vec<TaskToolCall> = (0..MAX_TASK_TOOL_PLAN_STEPS_CEIL + 1)
            .map(|i| step("mcp:fs", &format!("t{i}"), serde_json::json!({})))
            .collect();
        assert_eq!(
            TaskToolPlan { steps: over }.validate_with_limit(usize::MAX),
            Err(TaskToolPlanError::TooManySteps {
                max: MAX_TASK_TOOL_PLAN_STEPS_CEIL,
                got: MAX_TASK_TOOL_PLAN_STEPS_CEIL + 1
            })
        );
    }

    #[test]
    fn max_steps_exactly_is_accepted() {
        let plan = TaskToolPlan {
            steps: (0..MAX_TASK_TOOL_PLAN_STEPS)
                .map(|i| step("mcp:fs", &format!("t{i}"), serde_json::json!({})))
                .collect(),
        };
        plan.validate().expect("a full plan is valid");
        assert_eq!(parse_task_tool_plan(&plan.to_input()).unwrap().len(), MAX_TASK_TOOL_PLAN_STEPS);
    }

    #[test]
    fn empty_plugin_or_tool_step_is_rejected() {
        let plan = TaskToolPlan {
            steps: vec![step("mcp:fs", "search", serde_json::json!({})), step("  ", "x", serde_json::json!({}))],
        };
        assert_eq!(plan.validate(), Err(TaskToolPlanError::EmptyStep { index: 1 }));
        // The read path drops the whole plan rather than guess past a bad step.
        assert!(parse_task_tool_plan(
            &serde_json::json!({ "tool_plan": [{ "plugin": "mcp:fs", "tool": "" }] })
        )
        .is_none());
    }

    #[test]
    fn oversized_step_args_are_rejected() {
        let big = "x".repeat(MAX_TASK_TOOL_PLAN_ARGS_BYTES + 1);
        let plan = TaskToolPlan { steps: vec![step("mcp:fs", "search", serde_json::json!({ "blob": big }))] };
        assert!(matches!(
            plan.validate(),
            Err(TaskToolPlanError::ArgsTooLarge { index: 0, .. })
        ));
    }

    #[test]
    fn an_ordinary_input_is_not_a_plan() {
        assert!(parse_task_tool_plan(&serde_json::json!({ "message": "hi" })).is_none());
        assert!(parse_task_tool_plan(&serde_json::json!({ "tool_plan": "nope" })).is_none());
        // A single tool_call directive is not a plan (and vice versa).
        assert!(parse_task_tool_plan(
            &serde_json::json!({ "tool_call": { "plugin": "mcp:fs", "tool": "search" } })
        )
        .is_none());
    }
}
