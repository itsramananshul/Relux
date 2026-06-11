//! The first SAFE *write-capable* Prime tool surface — a governed, allowlisted
//! contract by which a configured brain may *request* a known mutating tool, which
//! Relux converts into an EXISTING Prime action/proposal and routes through every
//! current validation and approval gate. The brain never writes state directly.
//!
//! ## Why this exists
//!
//! The read-only context loop ([`crate::prime_tools`]) proved the governed-tool shape:
//! the brain *requests* an allowlisted tool, the kernel validates the name fail-closed
//! and executes it deterministically. But every tool there is a pure READ — the brain
//! still cannot ask Prime to *do* anything through a tool contract. That is the gap the
//! audit names next (`docs/prime-processing-audit.md` "A WRITE-capable tool surface"):
//! letting the brain request a mutating tool that STILL flows through the existing
//! fail-closed `decide` → `prime_execute` (safe `Act`) / human-approval (`Propose`)
//! path, with openclaw's `isMutatingToolCall` unknown-⇒-mutating default as the gate.
//!
//! This module is that surface. It defines a small allowlist of write tools, each
//! mapping ONLY to an existing safe action or an existing approval-gated proposal, and
//! validates the brain's requested args by REUSING the existing slot validators (no
//! weaker duplicate parsing). The result is an intent proposal + a validated slot that
//! the kernel feeds through its UNCHANGED chokepoint — so the fail-closed intent gate,
//! every existence/sanitization check, and every approval gate apply exactly as before.
//!
//! ## Reference-driven design (see `docs/reference-driven-development.md`)
//!
//! - **Hermes** `agent/conversation_loop.py` `run_conversation(...)` (L3114-3162) — the
//!   model's chosen tool name is validated against `agent.valid_tool_names` BEFORE
//!   execution; an off-list name is fed back for self-correction, never executed.
//!   [`classify_write_tool`] is that name-allowlist gate: a name not in [`WRITE_TOOLS`]
//!   is `None` (refused), so a creative / mutating-sounding name the brain invents can
//!   never reach an action.
//! - **Paperclip (openclaw)** `src/agents/tool-mutation.ts` `isMutatingToolCall` — a
//!   single fail-closed classifier where an UNKNOWN action defaults to *mutating*. We
//!   keep the same posture: the write allowlist is explicit and tiny; anything off it is
//!   refused, and a write tool is honored ONLY when the deterministic intent gate
//!   ([`crate::prime_intent::reconcile_intent`]) agrees the user asked for action.
//! - **openclaw** `src/agents/tools/update-plan-tool.ts` `readPlanSteps` + `common.ts`
//!   `readStringParam`/`ToolInputError` + `sessions-spawn-tool.ts`
//!   `UNSUPPORTED_*_PARAM_KEYS` — validate a structured payload field-by-field against an
//!   explicit schema/allowlist, require the mandatory string, reject unsupported keys. We
//!   adopt it by REUSING the existing per-action slot validators
//!   ([`crate::prime_slots::parse_task_slots`], [`crate::prime_update_slots::parse_update_slots`],
//!   [`crate::prime_assign_slots::parse_assign_slots`], [`crate::prime_agent_slots::parse_agent_slots`],
//!   [`crate::prime_admin_slots::parse_plugin_ref`]/`parse_permission_slots`) on the tool's
//!   `args` — so a write tool inherits the same allowlist, sanitization, clamping, and
//!   existing-target validation, with no weaker duplicate parsing.
//!
//! ## The safety contract (binding)
//!
//! - **The brain writes nothing.** A write tool request is converted into a
//!   [`crate::prime_intent::BrainIntentProposal`] + a validated slot; the kernel reconciles
//!   the intent behind the fail-closed gate and applies the slot at its single chokepoint.
//!   Every durable change still flows through `decide` → [`crate::state::KernelState::prime_execute`]
//!   (safe `Act`) or a human approval (risky `Propose`). There is NO path from this module
//!   to a mutation.
//! - **Fail closed on the tool name.** [`classify_write_tool`] admits ONLY an allowlisted
//!   name; anything else is refused.
//! - **The deterministic gate still decides.** A write tool's intent is SENSITIVE
//!   ([`crate::prime_intent`] `is_sensitive_intent`), so on guarded chat / ideation the
//!   gate vetoes it and keeps the deterministic (non-work) intent — casual chat can NEVER
//!   trigger a mutating tool, even if the brain requests one.
//! - **Risky actions stay approval-gated.** `plugin.install` / `permission.grant` map to
//!   the SAME `Propose` the deterministic path produces; the brain only sharpens the
//!   subject the human reviews — it can never execute an install or a grant.
//! - **One mutating tool per turn.** [`crate::prime_decision`] carries at most ONE
//!   `action_request`; a multi-tool / batched request is dropped (the turn falls back to
//!   the deterministic path, which clarifies) rather than batch-executed.

use serde::{Deserialize, Serialize};

use relux_core::{PrimeIntent, StateSummary};

/// Confidence stamped on a write tool's synthesized intent proposal and injected into
/// its slot args when the brain omits one. Naming a write tool IS an explicit, committed
/// action request, so the brain is confident in the *intent* — but the value is kept
/// below 1.0 and, crucially, the fail-closed intent gate still vetoes it on guarded chat
/// (a sensitive intent + guarded chat is always kept deterministic), so this only lets a
/// genuinely-commanded action through the validators, never casual chat.
pub const WRITE_TOOL_CONFIDENCE: f64 = 0.9;

/// Max characters kept from the requested tool name (an id is short; this only guards a
/// pathological argument before the allowlist check).
const MAX_NAME_CHARS: usize = 80;

/// Max characters kept from a sanitized run-start `task_id` argument.
const MAX_ID_CHARS: usize = 80;

/// One write-capable Prime tool descriptor. Presentation/grounding only — the executable
/// mapping is the [`PrimeIntent`] it produces, which the kernel runs through its unchanged
/// `decide` → execute / approval path.
#[derive(Debug, Clone)]
pub struct WriteTool {
    /// The tool's wire name (what the brain puts in `{"tool": "..."}`), dotted to read as a
    /// governed capability (`task.create`, `plugin.install`).
    pub name: &'static str,
    /// The existing Prime intent this tool maps to. The kernel reconciles it behind the
    /// fail-closed gate and `decide` turns it into the action/proposal.
    pub intent: PrimeIntent,
    /// `true` when the mapped action is a risky, APPROVAL-GATED `Propose` (a plugin install
    /// or a permission grant); `false` for a safe, in-scope `Act`. Advisory/UI only — the
    /// kernel's `decide` is the real authority on whether the action is gated.
    pub gated: bool,
    /// A one-line description shown in the decision prompt.
    pub summary: &'static str,
    /// A one-line hint of the accepted `args` shape (JSON), shown in the decision prompt.
    pub args_hint: &'static str,
}

/// The explicit allowlist of WRITE-capable Prime tools. [`classify_write_tool`] admits ONLY
/// a name in this list; everything else is refused (openclaw's `isMutatingToolCall`
/// unknown-⇒-unsafe default). Each maps to exactly one EXISTING action/proposal: the safe
/// creates/updates/assignments/starts are `Act`s through `prime_execute`; the plugin install
/// and permission grant are the SAME approval-gated `Propose` the deterministic path produces.
pub const WRITE_TOOLS: &[WriteTool] = &[
    WriteTool {
        name: "task.create",
        intent: PrimeIntent::TaskCreation,
        gated: false,
        summary: "Create a new task. Maps to the existing CreateTask action.",
        args_hint: "{\"title\":\"<imperative title>\",\"details\":\"<optional>\",\"assignee\":\"<optional existing agent id>\",\"priority\":<optional 1-9>}",
    },
    WriteTool {
        name: "task.update",
        intent: PrimeIntent::TaskUpdate,
        gated: false,
        summary: "Update an existing task by id (title/details/priority/status/assignee). Maps to UpdateTask.",
        args_hint: "{\"task_id\":\"<existing task id>\",\"title\":\"<optional>\",\"details\":\"<optional>\",\"priority\":<optional 1-9>,\"status\":\"<optional blocked|cancelled>\",\"assignee\":\"<optional existing agent id>\"}",
    },
    WriteTool {
        name: "task.assign",
        intent: PrimeIntent::AssignTask,
        gated: false,
        summary: "Assign an existing task to an existing agent. Maps to AssignTask.",
        args_hint: "{\"task_id\":\"<existing task id>\",\"agent_id\":\"<existing agent id>\"}",
    },
    WriteTool {
        name: "task.start",
        intent: PrimeIntent::RunStart,
        gated: false,
        summary: "Start a run for a task that is assigned and ready. Maps to StartRun.",
        args_hint: "{\"task_id\":\"<existing, ready task id>\"}",
    },
    WriteTool {
        name: "agent.create",
        intent: PrimeIntent::AgentCreation,
        gated: false,
        summary: "Create a new operative (agent). Maps to CreateAgent.",
        args_hint: "{\"name\":\"<agent name>\",\"role\":\"<optional>\",\"adapter\":\"<optional existing adapter id>\",\"persona\":\"<optional>\"}",
    },
    WriteTool {
        name: "orchestration.create",
        intent: PrimeIntent::Orchestration,
        gated: false,
        summary: "Decompose a multi-step goal into briefs across agents. Maps to the existing OrchestrateGoal action; the deterministic planner owns the step/agent decomposition and the multi-agent gate.",
        args_hint: "{\"goal\":\"<the multi-step goal>\",\"steps\":[\"<optional distinct step>\", ...]}",
    },
    WriteTool {
        name: "plugin.install",
        intent: PrimeIntent::PluginInstallation,
        gated: true,
        summary: "Request a plugin install. APPROVAL-GATED: maps to a Propose a human must approve.",
        args_hint: "{\"plugin_id\":\"<plugin id, e.g. relux-tools-github>\"}",
    },
    WriteTool {
        name: "permission.grant",
        intent: PrimeIntent::PermissionChange,
        gated: true,
        summary: "Request a permission grant to an existing agent. APPROVAL-GATED: maps to a Propose a human must approve.",
        args_hint: "{\"subject_kind\":\"agent\",\"subject_id\":\"<existing agent id>\",\"permission\":\"<optional permission label>\"}",
    },
];

/// Classify a requested write tool name against the allowlist. Fail-closed: a name not in
/// [`WRITE_TOOLS`] is `None` (refused — never mapped to an action). Mirrors Hermes's
/// validate-against-`valid_tool_names` and openclaw's unknown-⇒-unsafe default.
pub fn classify_write_tool(name: &str) -> Option<&'static WriteTool> {
    WRITE_TOOLS.iter().find(|t| t.name == name)
}

/// A validated run-start reference (the `task.start` tool's only arg). The `task_id` is
/// sanitized but NOT yet existence-checked; [`reconcile_run_start`] validates it against
/// the live ready queue (it must EXIST and be ready to start).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrainRunStart {
    pub task_id: String,
}

/// The validated slot a write tool produced, one variant per mapped action. Each inner
/// value is built by the EXISTING per-action validator (no weaker duplicate parsing), so it
/// carries the same allowlist/sanitization/clamping guarantees as the specialized slot path.
#[derive(Debug, Clone, PartialEq)]
pub enum WriteToolSlot {
    /// `task.create` → [`crate::prime_slots::BrainTaskSlots`] (CreateTask).
    Task(crate::prime_slots::BrainTaskSlots),
    /// `task.update` → [`crate::prime_update_slots::BrainUpdateSlots`] (UpdateTask).
    Update(crate::prime_update_slots::BrainUpdateSlots),
    /// `task.assign` → [`crate::prime_assign_slots::BrainAssignSlots`] (AssignTask).
    Assign(crate::prime_assign_slots::BrainAssignSlots),
    /// `agent.create` → [`crate::prime_agent_slots::BrainAgentSlots`] (CreateAgent).
    Agent(crate::prime_agent_slots::BrainAgentSlots),
    /// `plugin.install` → [`crate::prime_admin_slots::BrainPluginRef`] (Propose: InstallPlugin).
    Plugin(crate::prime_admin_slots::BrainPluginRef),
    /// `permission.grant` → [`crate::prime_admin_slots::BrainPermissionSlots`] (Propose: GrantPermission).
    Permission(crate::prime_admin_slots::BrainPermissionSlots),
    /// `task.start` → [`BrainRunStart`] (StartRun).
    RunStart(BrainRunStart),
    /// `orchestration.create` → [`crate::prime_orchestration_slots::BrainOrchestrationSlots`]
    /// (OrchestrateGoal). The validated goal flows into the EXISTING deterministic planner +
    /// orchestration-creation path; the brain proposes only the goal text.
    Orchestration(crate::prime_orchestration_slots::BrainOrchestrationSlots),
}

/// A validated write tool request: the allowlisted tool name, the existing intent it maps
/// to, whether that action is approval-gated, and the validated slot. Only
/// [`parse_write_tool_request`] builds this; it executes nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedWriteTool {
    /// The allowlisted tool name (already passed [`classify_write_tool`]).
    pub tool: String,
    /// The existing Prime intent this tool maps to (fed to the fail-closed intent gate).
    pub intent: PrimeIntent,
    /// `true` when the mapped action is approval-gated (`Propose`), advisory/UI only.
    pub gated: bool,
    /// The validated slot, built by the existing per-action validator.
    pub slot: WriteToolSlot,
}

impl ParsedWriteTool {
    /// Synthesize the [`crate::prime_intent::BrainIntentProposal`] this write tool implies, to
    /// feed the kernel's UNCHANGED fail-closed intent gate. The confidence is
    /// [`WRITE_TOOL_CONFIDENCE`] (an explicit tool request is a committed action), but the
    /// gate still vetoes a sensitive intent on guarded chat — so this proposal can act only
    /// when the deterministic gate agrees the user asked for action.
    pub fn intent_proposal(&self) -> crate::prime_intent::BrainIntentProposal {
        crate::prime_intent::BrainIntentProposal {
            intent: self.intent.clone(),
            confidence: WRITE_TOOL_CONFIDENCE as f32,
            rationale: format!("requested via the {} tool", self.tool),
        }
    }
}

/// Strip control chars, collapse whitespace, and clamp a string to `max` chars.
fn sanitize_line(s: &str, max: usize) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

/// Validate ONE write tool request object (`{"tool":..,"args":..}`) the brain proposed in its
/// UNIFIED decision envelope ([`crate::prime_decision`]), returning a validated
/// [`ParsedWriteTool`] or `None`.
///
/// Fail-closed: the name is sanitized and run through [`classify_write_tool`]; a name NOT on
/// the write allowlist (a mutating-sounding / unknown / made-up tool such as `task.delete` or
/// `shell.run`) is refused (`None`). The `args` object is validated by the EXISTING per-action
/// validator for the tool's mapped intent (no weaker duplicate parsing); an args object that
/// fails that validator (missing required field, unsupported key, unknown status, …) fails the
/// whole request closed. A missing `confidence` in `args` is stamped at [`WRITE_TOOL_CONFIDENCE`]
/// so the (committed) request clears the slot validators' confidence floors — every other field
/// is still validated exactly as on the specialized path.
///
/// Pure. This is the parse-time gate; the kernel still reconciles the intent behind the
/// fail-closed gate and validates the slot against the live state at its single chokepoint.
pub fn parse_write_tool_request(value: &serde_json::Value) -> Option<ParsedWriteTool> {
    // Accept exactly one request object. A single-element array is unwrapped; a batched
    // (multi-element) array — or any other shape — is refused (this slice executes at most
    // ONE mutating tool per turn; multiple ⇒ fall back to the deterministic path, which asks).
    let obj = match value {
        serde_json::Value::Object(o) => o,
        serde_json::Value::Array(a) if a.len() == 1 => a[0].as_object()?,
        _ => return None,
    };

    let raw_name = obj.get("tool").and_then(|v| v.as_str())?;
    let name = sanitize_line(raw_name, MAX_NAME_CHARS);
    let descriptor = classify_write_tool(&name)?; // fail closed on an off-allowlist name

    let mut args = obj
        .get("args")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    // Stamp a committed confidence when the brain omitted one, so the slot validators (which
    // default a missing confidence below their honor floor) honor an explicit tool request.
    args.entry("confidence".to_string())
        .or_insert_with(|| serde_json::json!(WRITE_TOOL_CONFIDENCE));
    let args_json = serde_json::to_string(&serde_json::Value::Object(args.clone())).ok()?;

    let slot = match descriptor.intent {
        PrimeIntent::TaskCreation => {
            WriteToolSlot::Task(crate::prime_slots::parse_task_slots(&args_json).ok()?)
        }
        PrimeIntent::TaskUpdate => {
            WriteToolSlot::Update(crate::prime_update_slots::parse_update_slots(&args_json).ok()?)
        }
        PrimeIntent::AssignTask => {
            WriteToolSlot::Assign(crate::prime_assign_slots::parse_assign_slots(&args_json).ok()?)
        }
        PrimeIntent::AgentCreation => {
            WriteToolSlot::Agent(crate::prime_agent_slots::parse_agent_slots(&args_json).ok()?)
        }
        PrimeIntent::PluginInstallation => {
            WriteToolSlot::Plugin(crate::prime_admin_slots::parse_plugin_ref(&args_json).ok()?)
        }
        PrimeIntent::PermissionChange => WriteToolSlot::Permission(
            crate::prime_admin_slots::parse_permission_slots(&args_json).ok()?,
        ),
        PrimeIntent::RunStart => WriteToolSlot::RunStart(parse_run_start(&args)?),
        PrimeIntent::Orchestration => WriteToolSlot::Orchestration(
            crate::prime_orchestration_slots::parse_orchestration_slots(&args_json).ok()?,
        ),
        // The allowlist above never maps to any other intent; defensive fail-closed.
        _ => return None,
    };

    Some(ParsedWriteTool {
        tool: descriptor.name.to_string(),
        intent: descriptor.intent.clone(),
        gated: descriptor.gated,
        slot,
    })
}

/// Read the required `task_id` arg for `task.start` (openclaw `readStringParam(required)`):
/// trim, sanitize, length-clamp; a missing/empty id fails the request closed.
fn parse_run_start(args: &serde_json::Map<String, serde_json::Value>) -> Option<BrainRunStart> {
    let raw = args.get("task_id").and_then(|v| v.as_str())?;
    let task_id = sanitize_line(raw, MAX_ID_CHARS);
    if task_id.is_empty() {
        None
    } else {
        Some(BrainRunStart { task_id })
    }
}

/// Reconcile a `task.start` reference against the live ready queue, returning the existing,
/// ready task id to start or `None`. Mirrors the deterministic `RunStart` arm
/// ([`crate::prime`]): a run starts ONLY for a task that EXISTS and is ready (`summary.queued`).
/// An unknown / not-ready id yields `None` (the deterministic outcome — a clarify or an honest
/// "not ready" — stands). The id is taken verbatim from the queue, so the brain can never start
/// a task that is not real and runnable.
pub fn reconcile_run_start(proposal: &BrainRunStart, summary: &StateSummary) -> Option<String> {
    summary
        .queued
        .iter()
        .find(|b| b.id.0 == proposal.task_id)
        .map(|b| b.id.0.clone())
}

/// The write tool names, comma-joined — grounding for the unified decision prompt's
/// `action_request` section so the brain names only a real write tool. Pure.
pub fn write_tool_names() -> String {
    WRITE_TOOLS
        .iter()
        .map(|t| t.name)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary_with(agents: &[&str], tasks: &[&str], queued: &[(&str, &str)]) -> StateSummary {
        use relux_core::{TaskBrief, TaskId, TaskStatus};
        StateSummary {
            plugins: 0,
            agents: agents.len(),
            tasks_total: tasks.len(),
            tasks_open: tasks.len(),
            runs_active: 0,
            tasks_waiting_approval: 0,
            tasks_blocked: 0,
            tasks_failed: 0,
            pending_approvals: 0,
            all_agent_ids: agents.iter().map(|s| s.to_string()).collect(),
            all_task_ids: tasks.iter().map(|s| s.to_string()).collect(),
            queued: queued
                .iter()
                .map(|(id, title)| TaskBrief {
                    id: TaskId(id.to_string()),
                    title: title.to_string(),
                    status: TaskStatus::Queued,
                    assigned_agent: None,
                })
                .collect(),
            recent: vec![],
        }
    }

    #[test]
    fn classify_is_fail_closed_on_unknown_names() {
        assert!(classify_write_tool("task.create").is_some());
        assert!(classify_write_tool("task.update").is_some());
        assert!(classify_write_tool("task.assign").is_some());
        assert!(classify_write_tool("task.start").is_some());
        assert!(classify_write_tool("agent.create").is_some());
        assert!(classify_write_tool("orchestration.create").is_some());
        assert!(classify_write_tool("plugin.install").is_some());
        assert!(classify_write_tool("permission.grant").is_some());
        // Anything off the allowlist is refused — including a plausible-sounding write.
        assert!(classify_write_tool("task.delete").is_none());
        assert!(classify_write_tool("shell.run").is_none());
        assert!(classify_write_tool("run_shell").is_none());
        assert!(classify_write_tool("plugin.remove").is_none());
        assert!(classify_write_tool("").is_none());
    }

    #[test]
    fn gated_flag_marks_only_plugin_and_permission() {
        for t in WRITE_TOOLS {
            let gated_expected =
                matches!(t.name, "plugin.install" | "permission.grant");
            assert_eq!(t.gated, gated_expected, "gated mismatch for {}", t.name);
        }
    }

    #[test]
    fn parses_task_create_through_the_existing_task_validator() {
        let v = serde_json::json!({
            "tool": "task.create",
            "args": {"title": "Fix the login redirect bug", "priority": 7}
        });
        let parsed = parse_write_tool_request(&v).expect("a valid task.create");
        assert_eq!(parsed.tool, "task.create");
        assert_eq!(parsed.intent, PrimeIntent::TaskCreation);
        assert!(!parsed.gated);
        match parsed.slot {
            WriteToolSlot::Task(s) => {
                assert_eq!(s.title, "Fix the login redirect bug");
                assert_eq!(s.priority, Some(7));
                // The committed-request confidence cleared the validator's floor.
                assert!(s.confidence >= 0.6);
            }
            other => panic!("expected a task slot, got {other:?}"),
        }
    }

    #[test]
    fn parses_each_supported_write_tool_to_its_intent_and_slot() {
        let cases = [
            (
                serde_json::json!({"tool":"task.update","args":{"task_id":"task_0001","priority":8}}),
                PrimeIntent::TaskUpdate,
            ),
            (
                serde_json::json!({"tool":"task.assign","args":{"task_id":"task_0001","agent_id":"researcher"}}),
                PrimeIntent::AssignTask,
            ),
            (
                serde_json::json!({"tool":"task.start","args":{"task_id":"task_0001"}}),
                PrimeIntent::RunStart,
            ),
            (
                serde_json::json!({"tool":"agent.create","args":{"name":"Research Agent"}}),
                PrimeIntent::AgentCreation,
            ),
            (
                serde_json::json!({"tool":"orchestration.create","args":{"goal":"research, implement, and document the feature"}}),
                PrimeIntent::Orchestration,
            ),
            (
                serde_json::json!({"tool":"plugin.install","args":{"plugin_id":"relux-tools-github"}}),
                PrimeIntent::PluginInstallation,
            ),
            (
                serde_json::json!({"tool":"permission.grant","args":{"subject_kind":"agent","subject_id":"researcher"}}),
                PrimeIntent::PermissionChange,
            ),
        ];
        for (v, intent) in cases {
            let parsed = parse_write_tool_request(&v)
                .unwrap_or_else(|| panic!("expected {intent:?} to parse"));
            assert_eq!(parsed.intent, intent);
        }
    }

    #[test]
    fn parses_orchestration_create_through_the_existing_orchestration_validator() {
        let v = serde_json::json!({
            "tool": "orchestration.create",
            "args": {"goal": "ship the launch", "steps": ["research the market", "build the page"]}
        });
        let parsed = parse_write_tool_request(&v).expect("a valid orchestration.create");
        assert_eq!(parsed.tool, "orchestration.create");
        assert_eq!(parsed.intent, PrimeIntent::Orchestration);
        assert!(!parsed.gated);
        match parsed.slot {
            WriteToolSlot::Orchestration(s) => {
                assert_eq!(s.goal, "ship the launch");
                assert_eq!(s.steps.len(), 2);
                // The committed-request confidence cleared the validator's floor.
                assert!(s.confidence >= 0.6);
            }
            other => panic!("expected an orchestration slot, got {other:?}"),
        }
        // A missing required goal fails closed through the reused validator.
        assert!(parse_write_tool_request(&serde_json::json!({
            "tool": "orchestration.create",
            "args": {"steps": ["a", "b"]}
        }))
        .is_none());
        // A smuggled unsupported field (an agent reference) fails closed.
        assert!(parse_write_tool_request(&serde_json::json!({
            "tool": "orchestration.create",
            "args": {"goal": "do it", "agent_id": "researcher"}
        }))
        .is_none());
    }

    #[test]
    fn an_unknown_tool_name_is_refused() {
        assert!(parse_write_tool_request(
            &serde_json::json!({"tool":"task.delete","args":{"task_id":"task_0001"}})
        )
        .is_none());
        assert!(parse_write_tool_request(&serde_json::json!({"tool":"shell.run","args":{}})).is_none());
        // A missing/empty tool name is refused.
        assert!(parse_write_tool_request(&serde_json::json!({"args":{}})).is_none());
        assert!(parse_write_tool_request(&serde_json::json!({"tool":""})).is_none());
    }

    #[test]
    fn unsupported_args_fail_closed_through_the_reused_validator() {
        // A smuggled unsupported field is rejected by the EXISTING task validator (no weaker
        // duplicate parsing), so the whole request fails closed.
        assert!(parse_write_tool_request(&serde_json::json!({
            "tool": "task.create",
            "args": {"title": "Fix it", "run_tool": "shell"}
        }))
        .is_none());
        // An empty/missing required field (task.create needs a title) fails closed.
        assert!(parse_write_tool_request(&serde_json::json!({
            "tool": "task.create",
            "args": {"priority": 5}
        }))
        .is_none());
        // task.start needs a task_id.
        assert!(parse_write_tool_request(&serde_json::json!({
            "tool": "task.start",
            "args": {}
        }))
        .is_none());
    }

    #[test]
    fn a_batched_multi_tool_request_is_refused() {
        // More than one tool in a turn is not supported this slice: refuse (fall back to the
        // deterministic path) rather than batch-execute.
        let batched = serde_json::json!([
            {"tool":"task.create","args":{"title":"A"}},
            {"tool":"task.create","args":{"title":"B"}}
        ]);
        assert!(parse_write_tool_request(&batched).is_none());
        // A single-element array is unwrapped and honored.
        let single = serde_json::json!([{"tool":"task.create","args":{"title":"A"}}]);
        assert!(parse_write_tool_request(&single).is_some());
    }

    #[test]
    fn run_start_reconciles_only_against_a_ready_task() {
        let summary = summary_with(&["researcher"], &["task_0001", "task_0002"], &[("task_0001", "Fix login")]);
        // A ready task id resolves to itself (taken verbatim from the queue).
        assert_eq!(
            reconcile_run_start(&BrainRunStart { task_id: "task_0001".into() }, &summary).as_deref(),
            Some("task_0001")
        );
        // An existing-but-not-ready task (not in the queue) does not resolve.
        assert!(reconcile_run_start(&BrainRunStart { task_id: "task_0002".into() }, &summary).is_none());
        // An unknown task does not resolve.
        assert!(reconcile_run_start(&BrainRunStart { task_id: "task_9999".into() }, &summary).is_none());
    }

    #[test]
    fn intent_proposal_is_confident_and_names_the_tool() {
        let parsed = parse_write_tool_request(&serde_json::json!({
            "tool": "task.create",
            "args": {"title": "Tidy the docs"}
        }))
        .unwrap();
        let proposal = parsed.intent_proposal();
        assert_eq!(proposal.intent, PrimeIntent::TaskCreation);
        assert!(proposal.confidence >= 0.6, "an explicit tool request is confident");
        assert!(proposal.rationale.contains("task.create"));
    }

    #[test]
    fn write_tool_names_lists_the_allowlist() {
        let names = write_tool_names();
        for t in WRITE_TOOLS {
            assert!(names.contains(t.name), "{} must be listed", t.name);
        }
    }
}
