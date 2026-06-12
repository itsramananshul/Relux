//! Brain-assisted, VALIDATED extraction of an *orchestration goal* — the slot behind
//! the governed `orchestration.create` write tool ([`crate::prime_write_tools`]).
//!
//! ## Why this exists
//!
//! Prime already has a deterministic multi-agent planner
//! ([`relux_core::plan_orchestration`]) and an executable `OrchestrateGoal` action
//! ([`crate::state::KernelState::prime_orchestrate`]): one user goal becomes several
//! role-typed briefs assigned across the live roster. But the goal that reaches that
//! planner was, until now, only ever derived by keyword string-slicing the raw message
//! ([`crate::prime`] `orchestration_goal`) — so a user who explicitly asked Prime to
//! coordinate work ("split the launch across the team") but phrased the goal as a single
//! clause got a clarifying question, not a plan, because the connector-splitter saw one
//! clause. The master plan asks for Prime to *understand* a coordination request and
//! shape it into distinct steps (`docs/RELUX_MASTER_PLAN.md` §10.1 Intent Layer, §10.4
//! Delegation Rules, §17.1). A real brain can name the distinct steps; keyword slicing
//! cannot.
//!
//! This module lets a configured brain *propose* the orchestration goal (and, optionally,
//! the distinct steps) — validated hard before anything is created. Crucially the brain
//! proposes only the goal TEXT: the deterministic [`relux_core::plan_orchestration`] still
//! owns the decomposition (role classification, agent grounding against the live roster,
//! the step cap, and the dependency DAG), and [`crate::state::KernelState::prime_orchestrate`]
//! re-checks `is_multi_agent` at execution. So the brain can never fan out a goal the
//! planner would not, never invent an agent or a role, and never exceed the planner's cap.
//!
//! ## Reference-driven design (see `docs/reference-driven-development.md`)
//!
//! This is the same "model proposes structured arguments, server validates against a
//! schema/allowlist before acting" pattern the other slot modules use, read first from
//! Hermes and Paperclip:
//!
//! - **openclaw** `src/agents/tools/update-plan-tool.ts` `readPlanSteps` (L39-74) +
//!   `src/agents/tools/sessions-spawn-tool.ts` `UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS`
//!   (rejected before any param is read) + `src/agents/tools/common.ts` `readStringParam`
//!   (`required`) / `ToolInputError`: validate a structured payload field-by-field against
//!   an explicit schema, require the mandatory string, reject unsupported keys, clamp
//!   ranges. We mirror it: [`parse_orchestration_slots`] accepts ONLY [`ALLOWED_KEYS`]
//!   (any other key fails the proposal closed), requires a non-empty `goal`, validates the
//!   optional `steps` array STRICTLY (a non-array `steps`, or any non-string element, fails
//!   the whole proposal), sanitizes every string, and clamps the step count.
//! - **Hermes** `model_tools.py` `coerce_tool_args` / `_coerce_number` + `agent/
//!   message_sanitization.py`: coerce each value to its schema type (a non-coercible value
//!   is dropped, not fatal) and sanitize control chars + clamp length on every
//!   model-produced string. Mirrored in the goal/step sanitizers and the confidence coerce.
//! - **openclaw** `src/shared/balanced-json.ts` `extractBalancedJsonPrefix`: lift the JSON
//!   object out of a noisy reply with a balanced-brace scan. We reuse the same scanner via
//!   [`crate::prime_intent::extract_json_object`].
//!
//! ## The contract (binding)
//!
//! The brain only *proposes* the goal; it executes nothing. The proposal is honored only
//! when the (already brain-reconciled, fail-closed-gated) intent is `Orchestration` — and
//! `Orchestration` is a SENSITIVE intent ([`crate::prime_intent`]), so casual chat / a
//! question / musing can NEVER reach this layer (the intent gate keeps it `Brainstorming`).
//! Every field is validated:
//!
//! - **goal** — sanitized (control chars stripped, single line) and length-clamped; an
//!   empty/missing goal fails the whole proposal.
//! - **steps** — OPTIONAL; when present must be an array of non-empty strings (a non-array,
//!   or any non-string element, fails the proposal closed). Each step is sanitized + clamped
//!   and the count is clamped to the planner's own cap. The steps are joined into the goal
//!   text the deterministic planner then decomposes — they are HINTS, never the authority on
//!   the final step/agent set.
//! - any **unsupported field** → reject the whole proposal (fail closed).
//! - **low confidence** → fall back to the deterministic outcome.
//!
//! And the deterministic planner is the final gate: [`reconcile_orchestration_slots`] runs
//! [`relux_core::plan_orchestration`] on the composed goal and returns `None` unless it
//! genuinely splits into a multi-agent plan. So a brain goal that does not decompose leaves
//! the deterministic clarify in place — the planner's "must be multi-agent to fan out"
//! safety constraint can never be bypassed.

use relux_core::{plan_orchestration, OrchestrationPlan, StateSummary};

use crate::prime_intent::extract_json_object;

/// Minimum confidence before a brain's proposed goal is honored. Below this the
/// deterministic outcome stands — a hesitant brain never shapes an orchestration.
const CONFIDENCE_FLOOR: f32 = 0.6;

/// Max characters kept for the orchestration goal (a goal is a sentence, not a title,
/// so this is wider than the task-title cap but still bounded against a runaway reply).
const MAX_GOAL_CHARS: usize = 400;
/// Max characters kept for one proposed step before it is folded into the goal text.
const MAX_STEP_CHARS: usize = 160;
/// Max proposed steps kept. Bound DIRECTLY to the deterministic planner's own
/// [`relux_core::MAX_ORCHESTRATION_STEPS`] cap (no duplicated literal) so a brain can
/// never propose more briefs than the planner would itself plan, and the two stay in
/// lock-step if the ceiling is ever retuned.
const MAX_STEPS: usize = relux_core::MAX_ORCHESTRATION_STEPS;
/// Max characters kept from the brain's free-text rationale (audit/provenance only).
const MAX_RATIONALE_CHARS: usize = 240;

/// The natural connector the deterministic clause splitter recognizes, used to join the
/// brain's proposed steps into a single goal string the planner then re-decomposes. Picking
/// a connector the planner actually splits on keeps the brain's steps and the planner's
/// briefs aligned, while the planner still owns role/agent/cap/DAG.
const STEP_CONNECTOR: &str = ", and then ";

/// The only fields an orchestration slot proposal may carry. Any other key fails the
/// proposal closed (openclaw's `UNSUPPORTED_*_PARAM_KEYS` rejection) — the brain may not
/// smuggle an agent/role/tool key in as authority.
const ALLOWED_KEYS: &[&str] = &["goal", "steps", "confidence", "rationale"];

/// A validated orchestration goal a brain *proposes* for one `Orchestration` turn.
///
/// Only [`parse_orchestration_slots`] builds this, and only after rejecting unknown fields,
/// validating the steps strictly, sanitizing every string, and clamping lengths/counts.
/// `goal` is guaranteed non-empty; `steps` are advisory hints (already sanitized); the
/// rationale is audit text only.
#[derive(Debug, Clone, PartialEq)]
pub struct BrainOrchestrationSlots {
    pub goal: String,
    /// Optional distinct steps the brain named. Advisory: they are joined into the goal text
    /// the deterministic planner decomposes — never the authority on the final brief/agent set.
    pub steps: Vec<String>,
    pub confidence: f32,
    pub rationale: String,
}

/// The orchestration the kernel will actually create, after reconciling a brain proposal
/// against the live roster through the deterministic planner.
///
/// `goal` is the effective goal text the `OrchestrateGoal` action carries; `plan` is the
/// deterministic [`OrchestrationPlan`] (already grounded in the roster, capped, and a DAG),
/// used only to render the honest preview line. Built only by
/// [`reconcile_orchestration_slots`], and only when the goal genuinely splits multi-agent.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedOrchestration {
    pub goal: String,
    pub plan: OrchestrationPlan,
}

/// The strict, self-contained prompt describing the `orchestration.create` slot. The write
/// tool is requested through the unified decision envelope ([`crate::prime_decision`]), so
/// this prompt is grounding/documentation for the brain rather than a separate call; it
/// mirrors the other slot prompts (schema spelled out, safety rules explicit, JSON only).
pub fn build_orchestration_slots_prompt(message: &str) -> String {
    format!(
        "You are extracting the GOAL of a multi-agent orchestration the user has clearly asked \
Prime to coordinate on a local Relux control plane. You perform no action and create nothing; \
you only describe the goal so the deterministic planner can decompose it into briefs across \
agents.\n\n\
Respond with JSON ONLY (no prose, no code fences) in EXACTLY this shape:\n\
{{\"goal\":\"<the overall goal>\",\"steps\":[\"<optional distinct step>\", ...],\
\"confidence\":<0.0-1.0>}}\n\n\
Rules:\n\
- goal: a concise statement of the whole goal. REQUIRED.\n\
- steps: include ONLY if the goal genuinely splits into distinct steps; each a short phrase. \
The planner decides the final briefs, the agents, and the order - you only HINT the steps. \
NEVER name an agent, role, tool, or plugin.\n\
- Do NOT add any field other than these. Do NOT claim anything was created.\n\n\
User message:\n{message}"
    )
}

/// Parse a brain's raw reply into validated [`BrainOrchestrationSlots`], or `Err` with a
/// short reason on anything malformed/unsupported.
///
/// This is the schema/allowlist gate: the reply must contain a balanced JSON object, every
/// key must be in [`ALLOWED_KEYS`] (an unsupported field fails the whole proposal), the goal
/// must sanitize to a non-empty single line, and the optional `steps` must be an array of
/// strings (a non-array, or any non-string element, fails the proposal closed). The brain's
/// raw text never flows anywhere else — a parse failure simply drops the caller to the
/// deterministic outcome.
pub fn parse_orchestration_slots(raw: &str) -> Result<BrainOrchestrationSlots, String> {
    let json = extract_json_object(raw).ok_or_else(|| "no JSON object in reply".to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&json).map_err(|_| "reply was not valid JSON".to_string())?;
    let obj = value
        .as_object()
        .ok_or_else(|| "reply was not a JSON object".to_string())?;

    // Reject unknown / unsupported fields outright (fail closed). The brain may not smuggle
    // an agent/role/tool/action key in as authority.
    for key in obj.keys() {
        if !ALLOWED_KEYS.contains(&key.as_str()) {
            return Err(format!("unsupported field '{key}'"));
        }
    }

    let goal = sanitize_line(
        obj.get("goal").and_then(|v| v.as_str()).unwrap_or(""),
        MAX_GOAL_CHARS,
    );
    if goal.is_empty() {
        return Err("empty or missing goal".to_string());
    }

    // The optional steps are validated STRICTLY: present ⇒ must be an array, and every element
    // must be a string (a non-array, or any non-string element, fails the whole proposal — the
    // brain may not smuggle a structured object/agent reference in as a "step"). Each is
    // sanitized + clamped; empties are dropped and the count is clamped to the planner's cap.
    let steps = match obj.get("steps") {
        None => Vec::new(),
        Some(serde_json::Value::Array(items)) => {
            let mut out: Vec<String> = Vec::new();
            for item in items {
                let s = item
                    .as_str()
                    .ok_or_else(|| "every step must be a string".to_string())?;
                let cleaned = sanitize_line(s, MAX_STEP_CHARS);
                if !cleaned.is_empty() {
                    out.push(cleaned);
                }
                if out.len() >= MAX_STEPS {
                    break;
                }
            }
            out
        }
        Some(_) => return Err("steps must be an array of strings".to_string()),
    };

    let confidence = coerce_confidence(obj.get("confidence"));

    let rationale = obj
        .get("rationale")
        .and_then(|v| v.as_str())
        .map(|s| sanitize_line(s, MAX_RATIONALE_CHARS))
        .unwrap_or_default();

    Ok(BrainOrchestrationSlots {
        goal,
        steps,
        confidence,
        rationale,
    })
}

/// Reconcile a brain orchestration proposal against the live roster through the deterministic
/// planner, returning the orchestration to create, or `None` to keep the deterministic outcome.
///
/// Policy — each rule fails toward the deterministic / safer choice:
/// 1. Low confidence (`< CONFIDENCE_FLOOR`) → `None` (deterministic outcome stands).
/// 2. The goal is composed from the proposal: when the brain named distinct `steps` they are
///    joined with the planner's own connector so the planner decomposes them into briefs;
///    otherwise the proposed `goal` is used verbatim.
/// 3. The composed goal is run through the deterministic [`relux_core::plan_orchestration`].
///    Only a plan that GENUINELY splits multi-agent is honored; a goal that does not split
///    returns `None`, so the deterministic clarify stands. This is the planner's safety
///    constraint the brain can never bypass — it owns the role classification, the agent
///    grounding (an agent is matched only against the live roster), the step cap, and the DAG.
pub fn reconcile_orchestration_slots(
    proposal: &BrainOrchestrationSlots,
    summary: &StateSummary,
) -> Option<ResolvedOrchestration> {
    if proposal.confidence < CONFIDENCE_FLOOR {
        return None;
    }

    let goal = if proposal.steps.is_empty() {
        proposal.goal.clone()
    } else {
        proposal.steps.join(STEP_CONNECTOR)
    };

    // The deterministic planner is the final authority: it grounds roles against the live
    // roster, caps the step count, and builds the DAG. A goal that does not genuinely split
    // is not orchestratable — the deterministic clarify stands.
    let plan = plan_orchestration(&goal, summary);
    if !plan.is_multi_agent() {
        return None;
    }

    Some(ResolvedOrchestration { goal, plan })
}

/// Coerce a JSON confidence value (number or numeric string) to a clamped `f32`, defaulting
/// to a neutral 0.5 (below the override floor) when absent or not a usable number. Mirrors
/// Hermes' `_coerce_number`: a non-numeric value degrades to the neutral default.
fn coerce_confidence(value: Option<&serde_json::Value>) -> f32 {
    let raw = match value {
        Some(serde_json::Value::Number(n)) => n.as_f64(),
        Some(serde_json::Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    };
    raw.filter(|v| v.is_finite()).unwrap_or(0.5).clamp(0.0, 1.0) as f32
}

/// Sanitize a single-line string: replace every control char with a space, collapse
/// whitespace runs, trim, and clamp to `max` characters. Mirrors the other slot sanitizers.
fn sanitize_line(s: &str, max: usize) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .chars()
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
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
            agent_skills: vec![],
            all_task_ids: vec![],
            queued: vec![],
            recent: vec![],
        }
    }

    #[test]
    fn parses_a_clean_goal_with_steps() {
        let parsed = parse_orchestration_slots(
            r#"{"goal":"Ship the dashboard","steps":["research the options","implement it","write the docs"],"confidence":0.9}"#,
        )
        .expect("a clean proposal");
        assert_eq!(parsed.goal, "Ship the dashboard");
        assert_eq!(parsed.steps.len(), 3);
        assert_eq!(parsed.steps[0], "research the options");
        assert_eq!(parsed.confidence, 0.9);
    }

    #[test]
    fn parses_a_goal_with_no_steps() {
        let parsed = parse_orchestration_slots(
            r#"{"goal":"research, implement, and document the feature","confidence":0.8}"#,
        )
        .expect("a goal alone is valid");
        assert!(parsed.steps.is_empty());
    }

    #[test]
    fn extracts_from_a_noisy_reply_with_prose_and_fences() {
        let raw = "Here is the plan:\n```json\n{\"goal\":\"do the thing\",\"confidence\":0.9}\n```\nDone.";
        let parsed = parse_orchestration_slots(raw).unwrap();
        assert_eq!(parsed.goal, "do the thing");
    }

    #[test]
    fn an_empty_goal_fails_closed() {
        assert!(parse_orchestration_slots(r#"{"goal":"","confidence":0.9}"#).is_err());
        assert!(parse_orchestration_slots(r#"{"confidence":0.9}"#).is_err());
        // A goal of only control chars sanitizes to empty and fails.
        assert!(parse_orchestration_slots("{\"goal\":\"\\u0000\\u0007\",\"confidence\":0.9}").is_err());
    }

    #[test]
    fn an_unsupported_field_fails_closed() {
        // A smuggled agent/role/action key fails the whole proposal closed.
        assert!(parse_orchestration_slots(
            r#"{"goal":"do it","agent_id":"researcher","confidence":0.9}"#
        )
        .is_err());
        assert!(parse_orchestration_slots(r#"{"goal":"do it","run":true}"#).is_err());
    }

    #[test]
    fn non_string_steps_fail_closed() {
        // A step that is not a string (an object smuggling an agent reference, a number)
        // fails the whole proposal closed — the brain may not name authority as a "step".
        assert!(parse_orchestration_slots(
            r#"{"goal":"do it","steps":[{"agent":"researcher"}],"confidence":0.9}"#
        )
        .is_err());
        assert!(parse_orchestration_slots(r#"{"goal":"do it","steps":[1,2,3],"confidence":0.9}"#).is_err());
        // A non-array steps value is rejected.
        assert!(parse_orchestration_slots(r#"{"goal":"do it","steps":"research","confidence":0.9}"#).is_err());
    }

    #[test]
    fn steps_are_sanitized_and_count_clamped() {
        // Feed MORE steps than the cap so the clamp is exercised regardless of the
        // configured ceiling (built from the constant, not a hard-coded count).
        let steps = (0..MAX_STEPS + 3)
            .map(|i| format!("\"step{i}\""))
            .collect::<Vec<_>>()
            .join(",");
        let json = format!("{{\"goal\":\"g\",\"steps\":[{steps}],\"confidence\":0.9}}");
        let parsed = parse_orchestration_slots(&json).unwrap();
        // Clamped to the planner's cap so the brain can never propose more briefs than the
        // planner would plan.
        assert_eq!(parsed.steps.len(), MAX_STEPS);
        // Control chars are stripped from each step.
        let cleaned = parse_orchestration_slots(
            "{\"goal\":\"g\",\"steps\":[\"line\\nbreak\"],\"confidence\":0.9}",
        )
        .unwrap();
        assert_eq!(cleaned.steps[0], "line break");
    }

    #[test]
    fn overlong_goal_is_clamped() {
        let long = "x".repeat(MAX_GOAL_CHARS + 50);
        let raw = format!("{{\"goal\":\"{long}\",\"confidence\":0.9}}");
        let parsed = parse_orchestration_slots(&raw).unwrap();
        assert!(parsed.goal.chars().count() <= MAX_GOAL_CHARS);
    }

    #[test]
    fn confidence_is_coerced_and_clamped() {
        assert_eq!(
            parse_orchestration_slots(r#"{"goal":"g","confidence":"0.8"}"#)
                .unwrap()
                .confidence,
            0.8
        );
        assert_eq!(
            parse_orchestration_slots(r#"{"goal":"g","confidence":5.0}"#)
                .unwrap()
                .confidence,
            1.0
        );
        // Absent confidence defaults to a neutral 0.5 (below the override floor).
        assert_eq!(
            parse_orchestration_slots(r#"{"goal":"g"}"#).unwrap().confidence,
            0.5
        );
    }

    #[test]
    fn reconcile_honors_a_multi_agent_goal() {
        let summary = summary_with_agents(&["prime", "research-agent", "code-agent"]);
        let proposal = BrainOrchestrationSlots {
            goal: "research the framework, implement a prototype, and write the docs".to_string(),
            steps: vec![],
            confidence: 0.9,
            rationale: String::new(),
        };
        let resolved = reconcile_orchestration_slots(&proposal, &summary)
            .expect("a multi-step goal orchestrates");
        assert!(resolved.plan.is_multi_agent());
        // The planner grounds roles against the live roster - the brain never named an agent.
        assert!(resolved
            .plan
            .steps
            .iter()
            .any(|s| s.agent_id.as_deref() == Some("research-agent")));
    }

    #[test]
    fn reconcile_composes_steps_into_a_multi_agent_plan() {
        let summary = summary_with_agents(&["prime"]);
        let proposal = BrainOrchestrationSlots {
            goal: "ship the launch".to_string(),
            steps: vec![
                "research the market".to_string(),
                "build the landing page".to_string(),
                "test it".to_string(),
            ],
            confidence: 0.9,
            rationale: String::new(),
        };
        let resolved =
            reconcile_orchestration_slots(&proposal, &summary).expect("steps compose to a plan");
        // The steps drove the decomposition (3 distinct briefs), but the planner owns the set.
        assert_eq!(resolved.plan.steps.len(), 3);
        assert!(resolved.goal.contains("research the market"));
    }

    #[test]
    fn reconcile_drops_a_single_clause_goal() {
        // A goal that does not genuinely split is not orchestratable: the deterministic
        // planner's "must be multi-agent" constraint stands, and the brain cannot bypass it.
        let summary = summary_with_agents(&["prime"]);
        let proposal = BrainOrchestrationSlots {
            goal: "summarize the README".to_string(),
            steps: vec![],
            confidence: 0.9,
            rationale: String::new(),
        };
        assert!(reconcile_orchestration_slots(&proposal, &summary).is_none());
    }

    #[test]
    fn reconcile_drops_a_low_confidence_proposal() {
        let summary = summary_with_agents(&["prime"]);
        let proposal = BrainOrchestrationSlots {
            goal: "research it, implement it, and document it".to_string(),
            steps: vec![],
            confidence: 0.4,
            rationale: String::new(),
        };
        assert!(reconcile_orchestration_slots(&proposal, &summary).is_none());
    }

    #[test]
    fn prompt_carries_schema_and_safety_rules() {
        let p = build_orchestration_slots_prompt("split the launch across the team");
        assert!(p.contains("\"goal\""));
        assert!(p.contains("\"steps\""));
        assert!(p.contains("JSON ONLY"));
        assert!(p.contains("NEVER name an agent"));
    }
}
