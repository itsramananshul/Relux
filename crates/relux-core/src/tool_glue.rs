//! The `execute_code` FOUNDATION: grounding a brain-AUTHORED multi-step "tool glue"
//! program into an INERT, validated preview.
//!
//! ## What this is (and what it is NOT)
//!
//! Hermes' `reference/hermes-agent-main/tools/code_execution_tool.py` lets the model
//! write one script that RPC-calls tools back through the SAME tool gate, collapsing a
//! multi-step chain into a single inference turn. Its load-bearing safety move is
//! **allowlist-before-dispatch**: the RPC server validates every tool name against
//! `SANDBOX_ALLOWED_TOOLS` *before* anything executes (`code_execution_tool.py`
//! `_rpc_server_loop`, "Enforce the allow-list"). `docs/HERMES_OPENCLAW_DEEP_AUDIT.md`
//! §2 records this as the P1 `execute_code` gap and the openclaw precedent
//! (`update-plan-tool.ts` `readPlanSteps`: per-entry validation + status allowlist,
//! never silent acceptance/truncation).
//!
//! This module is the FIRST, safe slice of that gap. It does NOT run a sandboxed script
//! and it spawns no child process. It implements the half that is pure and high-leverage:
//! taking a **structured, model-authored program** (an ordered list of `(plugin, tool,
//! args)` steps the brain wrote) and **grounding every step against the live tool
//! catalog** — the Relux equivalent of `SANDBOX_ALLOWED_TOOLS` — fail-closed, bounded,
//! and producing only an INERT [`PrimeToolPlanProposal`] preview. Nothing here executes a
//! tool, creates a task, or mutates state; an unknown tool is flagged, never fabricated.
//!
//! The committed program rides the EXISTING `tool_plan` task path
//! ([`crate::task::TaskToolPlan`]) and its unchanged permission/approval/grant/audit
//! gates — there is no second execution model. The difference from the keyword-driven
//! [`PrimeToolPlanProposal`] builder (kernel `build_tool_plan_proposal`, which splits
//! natural-language segments) is the input: there the operator's prose is parsed by a
//! fallback keyword rail; here the **brain authors the structured program directly**, so
//! the primary brain is the model and the kernel is the deterministic validator (the
//! posture `docs/reference-driven-development.md` records as binding — "validate the
//! model's choice against an allowlist/schema before acting").

use serde::{Deserialize, Serialize};

use crate::prime::{PrimeToolPlanProposal, PrimeToolPlanStep};
use crate::task::{TaskToolCall, TaskToolPlan, MAX_TASK_TOOL_PLAN_STEPS_CEIL};
use crate::tool::{ToolDescriptor, ToolExecutability};

/// One step of a brain-AUTHORED tool-glue program: the `(plugin, tool, args)` triple the
/// model wrote. It is a PROPOSAL, never a command — [`ground_tool_glue_plan`] resolves it
/// against the live catalog before anything is offered for commit, and even then the only
/// path that runs it is the explicit operator commit through the gated `tool_plan` task.
///
/// `args` defaults to `{}` when the model supplied none, mirroring [`TaskToolCall`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposedGlueStep {
    /// The plugin id the step targets (a real installed plugin id, or `mcp:<server>` for
    /// an MCP-backed tool — the SAME namespacing the gated `call_tool` path uses).
    pub plugin: String,
    /// The tool name on that plugin.
    pub tool: String,
    /// The JSON arguments to forward verbatim. `{}` when absent.
    #[serde(default)]
    pub args: serde_json::Value,
}

/// Map a tool's live [`ToolExecutability`] to the honest `readiness` label and an
/// optional note carried on a [`PrimeToolPlanStep`]. The labels match the gated
/// `call_tool` reality — a higher-risk or unpermitted tool is surfaced as such, never as
/// "ready". Shared by BOTH grounding paths (the kernel's keyword `build_tool_plan_proposal`
/// and [`ground_tool_glue_plan`]) so they agree on what each readiness means.
pub fn tool_plan_readiness(executable: &ToolExecutability) -> (&'static str, Option<String>) {
    match executable {
        ToolExecutability::Ready => ("ready", None),
        ToolExecutability::NeedsApproval => (
            "needs_approval",
            Some("higher-risk tool — the run is gated on approval".to_string()),
        ),
        ToolExecutability::MissingPermission => (
            "missing_permission",
            Some("the acting agent lacks this tool's permission".to_string()),
        ),
        ToolExecutability::RuntimeNotConfigured => (
            "not_runnable",
            Some("installed, but no runtime is configured yet".to_string()),
        ),
        ToolExecutability::RuntimeDisabled => (
            "not_runnable",
            Some("installed, but its runtime is disabled".to_string()),
        ),
        ToolExecutability::NotImplemented => (
            "not_runnable",
            Some("installed, but the runtime is not implemented yet".to_string()),
        ),
    }
}

/// One-line, human-readable summary of a grounded tool-plan / tool-glue preview. Shared by
/// both grounding paths so the prose never diverges from the structured card.
pub fn tool_plan_summary(steps: &[PrimeToolPlanStep], ready: bool) -> String {
    if steps.is_empty() {
        return "I couldn't map this to any installed tools.".to_string();
    }
    let n = steps.len();
    let noun = if n == 1 { "step" } else { "steps" };
    if ready {
        let all_ready = steps.iter().all(|s| s.readiness == "ready");
        if all_ready {
            format!("{n} tool {noun}, all ready to run.")
        } else {
            format!("{n} tool {noun}; some are gated (see each step).")
        }
    } else {
        let unresolved = steps.iter().filter(|s| s.readiness == "unknown").count();
        if unresolved > 0 {
            format!("{n} tool {noun}, but {unresolved} couldn't be grounded — see below.")
        } else {
            format!("{n} tool {noun}; the plan needs a fix before it can be created.")
        }
    }
}

/// Ground a brain-AUTHORED tool-glue program into an INERT [`PrimeToolPlanProposal`].
///
/// READ-ONLY and PURE: it resolves every proposed step against `catalog` (the live tool
/// catalog the kernel passes in — installed plugin tools, Plugin Lens source tools,
/// governed command tools, and live MCP tools, all carrying their honest
/// [`ToolExecutability`]), validates the whole bounded plan with the SAME
/// [`TaskToolPlan::validate_with_limit`] the task-create route enforces, and returns the
/// preview. It creates nothing, runs nothing, and mutates no state.
///
/// ## The safety contract (binding)
///
/// - **Fail closed on unknown tools.** A `(plugin, tool)` that is not in `catalog` is
///   flagged `readiness: "unknown"` with an honest issue and `ready_to_create` is forced
///   `false`. The model can never fabricate a tool into existence — this is the Relux
///   analogue of Hermes' `SANDBOX_ALLOWED_TOOLS` allow-list check.
/// - **Gated tools stay gated.** A resolved tool keeps its real readiness
///   (`needs_approval` / `missing_permission` / `not_runnable`); it is included in the
///   committable plan but its eventual run still passes the unchanged approval/permission
///   gates of the `tool_plan` task path. Grounding never downgrades a gate.
/// - **Bounded, never truncated.** At most `max_steps` steps (itself clamped to
///   [`MAX_TASK_TOOL_PLAN_STEPS_CEIL`] — the configurable [`crate::PrimeAgentPolicy`]
///   limit, NOT a toy constant). A longer program is reported as too-long with an honest
///   issue rather than silently clipped.
///
/// `max_steps` is the operator-configured tool-plan step limit
/// ([`crate::PrimeAgentPolicy::tool_plan_steps`]); the caller picks the standard or
/// extended profile.
pub fn ground_tool_glue_plan(
    goal: &str,
    proposed: &[ProposedGlueStep],
    catalog: &[ToolDescriptor],
    max_steps: usize,
) -> PrimeToolPlanProposal {
    // Clamp the configured limit into the absolute hard backstop so no caller — however
    // the policy was set — can ground a program past the ceiling.
    let max = max_steps.clamp(1, MAX_TASK_TOOL_PLAN_STEPS_CEIL);

    let mut steps: Vec<PrimeToolPlanStep> = Vec::new();
    let mut resolved_calls: Vec<TaskToolCall> = Vec::new();
    let mut issues: Vec<String> = Vec::new();
    let mut all_resolved = true;
    let mut over_cap = false;

    for (i, step) in proposed.iter().enumerate() {
        // Cap previewed steps at the same bound the task-create route enforces; a longer
        // program is reported as too-long below rather than silently truncated.
        if steps.len() >= max {
            over_cap = true;
            break;
        }
        let index = (i + 1) as u32;
        let plugin = step.plugin.trim().to_string();
        let tool = step.tool.trim().to_string();
        let args = if step.args.is_null() {
            serde_json::json!({})
        } else {
            step.args.clone()
        };

        // A step the model left half-specified is never guessed — fail closed.
        if plugin.is_empty() || tool.is_empty() {
            all_resolved = false;
            issues.push(format!(
                "step {index}: a glue step needs both a plugin and a tool"
            ));
            steps.push(PrimeToolPlanStep {
                index,
                plugin,
                tool,
                args,
                readiness: "unknown".to_string(),
                risk: None,
                note: Some("missing plugin or tool".to_string()),
            });
            continue;
        }

        match catalog
            .iter()
            .find(|d| d.plugin_id == plugin && d.tool_name == tool)
        {
            // Resolved against the live catalog: carry its HONEST readiness/risk.
            Some(desc) => {
                let (readiness, note) = tool_plan_readiness(&desc.executable);
                steps.push(PrimeToolPlanStep {
                    index,
                    plugin: plugin.clone(),
                    tool: tool.clone(),
                    args: args.clone(),
                    readiness: readiness.to_string(),
                    risk: Some(format!("{:?}", desc.risk).to_lowercase()),
                    note,
                });
                resolved_calls.push(TaskToolCall { plugin, tool, args });
            }
            // FAIL CLOSED: an unknown tool is never accepted, never fabricated. This is
            // the allow-list gate — the model cannot invent a capability it was not shown.
            None => {
                all_resolved = false;
                issues.push(format!(
                    "step {index}: \"{plugin}/{tool}\" is not a known tool — it is not in \
                     the live catalog"
                ));
                steps.push(PrimeToolPlanStep {
                    index,
                    plugin,
                    tool,
                    args,
                    readiness: "unknown".to_string(),
                    risk: None,
                    note: Some("no installed or live tool matches this reference".to_string()),
                });
            }
        }
    }

    if proposed.is_empty() {
        all_resolved = false;
        issues.push("a tool-glue program needs at least one step".to_string());
    }

    if over_cap || proposed.len() > max {
        all_resolved = false;
        issues.push(format!(
            "a tool-glue program can have at most {max} steps; you proposed {} — raise the \
             tool-plan step limit in Prime Autonomy settings to plan more",
            proposed.len()
        ));
    }

    // Reuse the EXACT create-time validation so a preview can never advertise a program
    // the task-create route would reject (empty, over-long, oversized args).
    let mut validates = !resolved_calls.is_empty();
    if !resolved_calls.is_empty() {
        if let Err(e) = (TaskToolPlan {
            steps: resolved_calls.clone(),
        })
        .validate_with_limit(max)
        {
            validates = false;
            all_resolved = false;
            issues.push(e.to_string());
        }
    }

    let ready_to_create =
        all_resolved && validates && !steps.is_empty() && resolved_calls.len() == steps.len();
    let summary = tool_plan_summary(&steps, ready_to_create);

    PrimeToolPlanProposal {
        goal: goal.trim().to_string(),
        summary,
        steps,
        ready_to_create,
        issues,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::RiskLevel;

    fn desc(plugin: &str, tool: &str, executable: ToolExecutability, risk: RiskLevel) -> ToolDescriptor {
        ToolDescriptor {
            plugin_id: plugin.to_string(),
            tool_name: tool.to_string(),
            description: String::new(),
            permission: format!("{plugin}:{tool}"),
            risk,
            source_kind: "LocalDir".to_string(),
            installed: true,
            enabled: true,
            protected: false,
            executable,
        }
    }

    fn step(plugin: &str, tool: &str) -> ProposedGlueStep {
        ProposedGlueStep {
            plugin: plugin.to_string(),
            tool: tool.to_string(),
            args: serde_json::json!({}),
        }
    }

    #[test]
    fn unknown_tool_fails_closed_and_is_not_fabricated() {
        // A catalog with one real tool; the program names a tool that does not exist.
        let catalog = vec![desc("acme", "build", ToolExecutability::Ready, RiskLevel::Low)];
        let proposed = vec![step("acme", "build"), step("acme", "deploy")];
        let plan = ground_tool_glue_plan("ship it", &proposed, &catalog, 16);

        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].readiness, "ready");
        // The unknown step is flagged, NOT fabricated as runnable.
        assert_eq!(plan.steps[1].readiness, "unknown");
        assert!(
            !plan.ready_to_create,
            "an unknown tool must block the one-click commit"
        );
        assert!(
            plan.issues.iter().any(|i| i.contains("acme/deploy")),
            "the honest issue must name the unknown tool: {:?}",
            plan.issues
        );
    }

    #[test]
    fn plugin_lens_source_tools_ground_as_read_only_steps() {
        // The Plugin Lens source tools are ordinary catalog entries; a glue program may
        // include them as Ready read-only steps.
        let catalog = vec![
            desc("user-plugin", "plugin.summary", ToolExecutability::Ready, RiskLevel::Low),
            desc("user-plugin", "plugin.search", ToolExecutability::Ready, RiskLevel::Low),
        ];
        let proposed = vec![step("user-plugin", "plugin.summary"), step("user-plugin", "plugin.search")];
        let plan = ground_tool_glue_plan("inspect the plugin", &proposed, &catalog, 16);

        assert!(plan.ready_to_create, "{:?}", plan.issues);
        assert!(plan.steps.iter().all(|s| s.readiness == "ready"));
        assert_eq!(plan.steps.len(), 2);
    }

    #[test]
    fn approval_gated_tool_stays_gated_in_the_plan() {
        // A resolved-but-gated tool is included (it commits) but keeps its honest gate;
        // grounding never downgrades it to "ready".
        let catalog = vec![
            desc("acme", "read", ToolExecutability::Ready, RiskLevel::Low),
            desc("acme", "write", ToolExecutability::NeedsApproval, RiskLevel::High),
        ];
        let proposed = vec![step("acme", "read"), step("acme", "write")];
        let plan = ground_tool_glue_plan("read then write", &proposed, &catalog, 16);

        assert_eq!(plan.steps[1].readiness, "needs_approval");
        // Resolved gated tools still produce a committable plan; the run is gated later.
        assert!(plan.ready_to_create, "{:?}", plan.issues);
        assert!(plan.summary.contains("gated"));
    }

    #[test]
    fn step_count_is_bounded_by_the_configured_limit_not_a_toy_cap() {
        let catalog = vec![desc("acme", "build", ToolExecutability::Ready, RiskLevel::Low)];
        // Six identical valid steps, but the configured limit is 3.
        let proposed: Vec<ProposedGlueStep> = (0..6).map(|_| step("acme", "build")).collect();
        let plan = ground_tool_glue_plan("loop", &proposed, &catalog, 3);

        assert!(!plan.ready_to_create);
        assert!(plan.steps.len() <= 3, "never grounds past the configured limit");
        assert!(
            plan.issues.iter().any(|i| i.contains("at most 3 steps")),
            "the over-cap must be reported honestly, not silently truncated: {:?}",
            plan.issues
        );
    }

    #[test]
    fn configured_limit_above_default_grounds_a_wider_program() {
        // A higher (extended-style) configured limit lets a wider program ground — proving
        // the bound is the policy value, not a hard-coded constant.
        let catalog = vec![desc("acme", "build", ToolExecutability::Ready, RiskLevel::Low)];
        let proposed: Vec<ProposedGlueStep> = (0..20).map(|_| step("acme", "build")).collect();
        let plan = ground_tool_glue_plan("wide", &proposed, &catalog, 32);

        assert_eq!(plan.steps.len(), 20);
        assert!(plan.ready_to_create, "{:?}", plan.issues);
    }

    #[test]
    fn empty_program_is_rejected_honestly() {
        let catalog = vec![desc("acme", "build", ToolExecutability::Ready, RiskLevel::Low)];
        let plan = ground_tool_glue_plan("do nothing", &[], &catalog, 16);
        assert!(!plan.ready_to_create);
        assert!(plan.steps.is_empty());
        assert!(plan.issues.iter().any(|i| i.contains("at least one step")));
    }

    #[test]
    fn half_specified_step_is_not_guessed() {
        let catalog = vec![desc("acme", "build", ToolExecutability::Ready, RiskLevel::Low)];
        let proposed = vec![ProposedGlueStep {
            plugin: "acme".to_string(),
            tool: "  ".to_string(),
            args: serde_json::json!({}),
        }];
        let plan = ground_tool_glue_plan("bare", &proposed, &catalog, 16);
        assert!(!plan.ready_to_create);
        assert_eq!(plan.steps[0].readiness, "unknown");
    }
}
