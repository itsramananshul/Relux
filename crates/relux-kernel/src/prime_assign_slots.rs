//! Brain-assisted, VALIDATED resolution of a task ASSIGNMENT's slots
//! (`{task_id, agent_id}`) — the continuation-path counterpart of the create-slot
//! layer ([`crate::prime_slots`]).
//!
//! ## Why this exists
//!
//! The `AssignTask` arm now resolves a *fuzzy* assignee deterministically
//! ([`crate::prime::resolve_assignee`]), but the deterministic extractors still miss
//! cases a person would not: "give the readme task to our ML person", or a multi-turn
//! continuation where the original request and the answer only together name both the
//! task and the agent. Per the master plan (`docs/RELUX_MASTER_PLAN.md` §10.1 Intent
//! Layer, §10.2 Action Layer, §17.1) a real brain should *propose* the missing
//! `{task_id, agent_id}` from the full context, while the kernel validates every id
//! against the live state before any assignment happens.
//!
//! ## What makes this safe (binding)
//!
//! Unlike the create-slot layer (which only *sharpens* an action the deterministic path
//! already produced), an assignment slot can PROMOTE an under-specified `AssignTask`
//! turn into a real `AssignTask` action. That is allowed ONLY because:
//!
//! - **Assignment is a safe, in-scope action** — the deterministic path already produces
//!   it freely (no approval, no risk gate). The brain authors no risky action.
//! - **Both ids are validated against the live state** — `task_id` is honored ONLY when
//!   it names an EXISTING task (`summary.all_task_ids`); `agent_id` is resolved through
//!   the SAME [`crate::prime::resolve_assignee`] fuzzy matcher and is ALWAYS an existing
//!   agent. The brain can never invent a task or an assignee.
//! - **It fails closed** — a low-confidence, malformed, unknown-field proposal, or one
//!   whose ids do not BOTH validate, is dropped and the deterministic outcome (a
//!   clarify) stands. The deterministic resolution is always the fallback.
//!
//! ## Reference-driven design (see `docs/reference-driven-development.md`)
//!
//! - **Hermes** `model_tools.py` `coerce_tool_args` (L535-616) / `agent/
//!   message_sanitization.py` — parse the model's structured arguments, sanitize control
//!   chars, and CLAMP length; a bad field is dropped, not fatal.
//! - **openclaw** `src/agents/tools/sessions-spawn-tool.ts`
//!   (`UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS` rejected before any param is read) +
//!   `src/agents/tools/common.ts` (`readStringParam`/`ToolInputError`) — reject any field
//!   outside the allowlist (fail closed), require/trim the strings.
//! - **openclaw** `src/auto-reply/reply/subagents-utils.ts`
//!   `resolveSubagentTargetFromRuns` — resolve a fuzzy reference to an EXISTING target
//!   (exact → unique-prefix), reused here via [`crate::prime::resolve_assignee`]; a
//!   `task_id` is likewise honored only when it exists.
//! - **openclaw** `src/shared/balanced-json.ts` (`extractBalancedJsonPrefix`) — the JSON
//!   object is lifted from a noisy reply with a balanced-brace scan, reused via
//!   [`crate::prime_intent::extract_json_object`].

use crate::prime::{resolve_assignee, AssigneeResolution};
use crate::prime_intent::extract_json_object;
use relux_core::StateSummary;

/// Minimum confidence before a brain's proposed assignment slots are honored.
const CONFIDENCE_FLOOR: f32 = 0.6;

/// Max characters kept for a proposed id / phrase before validation.
const MAX_ID_CHARS: usize = 96;
/// Max characters kept from the brain's free-text rationale (audit/provenance only).
const MAX_RATIONALE_CHARS: usize = 240;
/// Max task entries listed in the grounding prompt, so a large board cannot bloat it.
const MAX_PROMPT_TASKS: usize = 24;
/// Max agent entries listed in the grounding prompt.
const MAX_PROMPT_AGENTS: usize = 24;

/// The only fields an assignment-slot proposal may carry. Any other key fails the
/// proposal closed (openclaw's `UNSUPPORTED_*_PARAM_KEYS` rejection) — the brain may not
/// smuggle a run/permission/tool key in as authority.
const ALLOWED_KEYS: &[&str] = &["task_id", "agent_id", "confidence", "rationale"];

/// A validated assignment proposal a brain offers for one `AssignTask` turn.
///
/// Only [`parse_assign_slots`] builds this, and only after rejecting unknown fields,
/// sanitizing the strings, and clamping lengths. `task_id`/`agent_id` are the raw
/// (sanitized but NOT yet existence-validated) references; [`reconcile_assign_slots`]
/// validates them against the live state.
#[derive(Debug, Clone, PartialEq)]
pub struct BrainAssignSlots {
    /// The task reference the brain extracted (a `task_…` id), if any.
    pub task_id: Option<String>,
    /// The agent reference the brain extracted (an id or a fuzzy phrase), if any.
    pub agent_id: Option<String>,
    pub confidence: f32,
    pub rationale: String,
}

/// The fully-validated assignment the kernel will actually apply: both ids exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAssignSlots {
    pub task_id: String,
    pub agent_id: String,
}

/// Build the JSON-only extraction prompt for an assignment, grounded in the live board so
/// the brain can map a natural reference ("the readme task", "the researcher") onto a real
/// id. The kernel still validates every id, so the listing is grounding, not authority.
pub fn build_assign_slots_prompt(message: &str, summary: &StateSummary) -> String {
    // A bounded catalog of "<task_id>: <title>" from the ready queue then recent tasks,
    // deduped by id, so the brain can resolve a task referenced by description.
    let mut seen: Vec<String> = Vec::new();
    let mut task_lines: Vec<String> = Vec::new();
    for b in summary.queued.iter().chain(summary.recent.iter()) {
        let id = b.id.0.clone();
        if seen.contains(&id) {
            continue;
        }
        seen.push(id.clone());
        task_lines.push(format!("  - {id}: {}", b.title));
        if task_lines.len() >= MAX_PROMPT_TASKS {
            break;
        }
    }
    if task_lines.is_empty() {
        task_lines.push("  (no tasks on the board)".to_string());
    }

    let agent_list = if summary.all_agent_ids.is_empty() {
        "  (no agents)".to_string()
    } else {
        summary
            .all_agent_ids
            .iter()
            .take(MAX_PROMPT_AGENTS)
            .map(|a| format!("  - {a}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "You assign an EXISTING task to an EXISTING agent. From the user's message, \
identify which task and which agent they mean, choosing ONLY from the lists below. \
Output ONLY compact JSON: {{\"task_id\": \"<task id from the list, or omit>\", \
\"agent_id\": \"<agent id from the list, or omit>\", \"confidence\": 0.0-1.0}}. \
Use the exact ids from the lists. If you cannot identify one with confidence, omit that \
field. No prose, no code fences.\n\nTasks:\n{tasks}\n\nAgents:\n{agents}\n\nMessage: {msg}",
        tasks = task_lines.join("\n"),
        agents = agent_list,
        msg = message,
    )
}

/// Parse a brain reply into a validated [`BrainAssignSlots`], or `Err(())` on any
/// failure (no JSON object, an unsupported field, etc.). Mirrors the other slot parsers:
/// lift the JSON with the shared balanced-brace scanner, reject any field outside the
/// allowlist (fail closed), sanitize every string, and clamp lengths.
pub fn parse_assign_slots(reply: &str) -> Result<BrainAssignSlots, String> {
    let json = extract_json_object(reply).ok_or_else(|| "no JSON object in reply".to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&json).map_err(|_| "reply was not valid JSON".to_string())?;
    let obj = value
        .as_object()
        .ok_or_else(|| "reply was not a JSON object".to_string())?;

    // Fail closed on ANY field outside the allowlist.
    for key in obj.keys() {
        if !ALLOWED_KEYS.contains(&key.as_str()) {
            return Err(format!("unsupported field '{key}'"));
        }
    }

    let task_id = obj
        .get("task_id")
        .and_then(|v| v.as_str())
        .map(sanitize_task_id)
        .filter(|s| !s.is_empty());

    let agent_id = obj
        .get("agent_id")
        .and_then(|v| v.as_str())
        .map(sanitize_phrase)
        .filter(|s| !s.is_empty());

    let confidence = obj
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.5)
        .clamp(0.0, 1.0) as f32;

    let rationale = obj
        .get("rationale")
        .and_then(|v| v.as_str())
        .map(|s| clamp_line(s, MAX_RATIONALE_CHARS))
        .unwrap_or_default();

    Ok(BrainAssignSlots {
        task_id,
        agent_id,
        confidence,
        rationale,
    })
}

/// Reconcile a brain assignment proposal against the deterministic references and the
/// live state, returning the fully-validated `{task_id, agent_id}` to apply, or `None` to
/// keep the deterministic outcome (a clarify).
///
/// Policy — every rule fails toward the deterministic / safer choice:
/// 1. Low confidence (`< CONFIDENCE_FLOOR`) → `None`.
/// 2. `task_id` is taken from the brain when present, else the deterministic reference,
///    and is honored ONLY when it names an EXISTING task (`summary.all_task_ids`).
/// 3. `agent_id` is taken from the brain when present, else the deterministic reference,
///    and is resolved through [`resolve_assignee`] — it is ALWAYS an existing agent, and
///    an ambiguous/unknown reference yields `None`.
/// 4. BOTH must resolve; otherwise `None` (a half-resolved assignment is never invented).
pub fn reconcile_assign_slots(
    deterministic_task_id: Option<&str>,
    deterministic_agent_ref: Option<&str>,
    proposal: &BrainAssignSlots,
    summary: &StateSummary,
) -> Option<ResolvedAssignSlots> {
    if proposal.confidence < CONFIDENCE_FLOOR {
        return None;
    }

    let task_ref = proposal
        .task_id
        .as_deref()
        .or(deterministic_task_id)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let task_id = summary
        .all_task_ids
        .iter()
        .find(|id| id.eq_ignore_ascii_case(task_ref))
        .cloned()?;

    let agent_ref = proposal
        .agent_id
        .as_deref()
        .or(deterministic_agent_ref)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let agent_id = match resolve_assignee(agent_ref, &summary.all_agent_ids, &summary.agent_skills) {
        AssigneeResolution::Resolved(id) => id,
        _ => return None,
    };

    Some(ResolvedAssignSlots { task_id, agent_id })
}

/// Sanitize a proposed task id: lowercase, keep only `[a-z0-9_-]`, clamp length.
fn sanitize_task_id(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(MAX_ID_CHARS)
        .collect()
}

/// Sanitize a proposed agent reference (kept as a phrase so [`resolve_assignee`] can do
/// the fuzzy matching): strip control chars, collapse whitespace, clamp length.
fn sanitize_phrase(s: &str) -> String {
    clamp_line(s, MAX_ID_CHARS)
}

/// Strip control chars, collapse whitespace, trim, and clamp to `max` chars.
fn clamp_line(s: &str, max: usize) -> String {
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
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use relux_core::{TaskBrief, TaskId, TaskStatus};

    fn summary(tasks: &[&str], agents: &[&str]) -> StateSummary {
        StateSummary {
            plugins: 0,
            agents: agents.len(),
            tasks_total: tasks.len(),
            tasks_open: 0,
            runs_active: 0,
            tasks_waiting_approval: 0,
            tasks_blocked: 0,
            tasks_failed: 0,
            pending_approvals: 0,
            all_agent_ids: agents.iter().map(|s| s.to_string()).collect(),
            agent_skills: vec![],
            all_task_ids: tasks.iter().map(|s| s.to_string()).collect(),
            available_adapter_ids: Vec::new(),
            queued: tasks
                .iter()
                .map(|id| TaskBrief {
                    id: TaskId::new(*id),
                    title: format!("title for {id}"),
                    status: TaskStatus::Queued,
                    assigned_agent: None,
                })
                .collect(),
            recent: vec![],
        }
    }

    #[test]
    fn parses_a_clean_proposal() {
        let s = parse_assign_slots(r#"{"task_id":"task_0001","agent_id":"researcher","confidence":0.9}"#)
            .expect("clean parse");
        assert_eq!(s.task_id.as_deref(), Some("task_0001"));
        assert_eq!(s.agent_id.as_deref(), Some("researcher"));
        assert!(s.confidence > 0.8);
    }

    #[test]
    fn lifts_json_from_a_noisy_reply() {
        let s = parse_assign_slots("Sure! Here you go:\n```\n{\"task_id\":\"task_0002\",\"agent_id\":\"the researcher\",\"confidence\":0.8}\n```")
            .expect("noisy parse");
        assert_eq!(s.task_id.as_deref(), Some("task_0002"));
        assert_eq!(s.agent_id.as_deref(), Some("the researcher"));
    }

    #[test]
    fn rejects_an_unsupported_field_fail_closed() {
        assert!(parse_assign_slots(
            r#"{"task_id":"task_0001","agent_id":"researcher","run_now":true,"confidence":0.9}"#
        )
        .is_err());
    }

    #[test]
    fn rejects_objectless_text() {
        assert!(parse_assign_slots("no json here").is_err());
    }

    #[test]
    fn reconcile_validates_both_ids_against_live_state() {
        let sum = summary(&["task_0001"], &["prime", "researcher"]);
        // A fuzzy "the researcher" + an existing task resolves both.
        let p = BrainAssignSlots {
            task_id: Some("task_0001".to_string()),
            agent_id: Some("the researcher".to_string()),
            confidence: 0.9,
            rationale: String::new(),
        };
        assert_eq!(
            reconcile_assign_slots(None, None, &p, &sum),
            Some(ResolvedAssignSlots {
                task_id: "task_0001".to_string(),
                agent_id: "researcher".to_string(),
            })
        );
    }

    #[test]
    fn reconcile_fails_closed_on_unknown_or_low_confidence() {
        let sum = summary(&["task_0001"], &["researcher"]);
        // Unknown task.
        let p = BrainAssignSlots {
            task_id: Some("task_9999".to_string()),
            agent_id: Some("researcher".to_string()),
            confidence: 0.9,
            rationale: String::new(),
        };
        assert_eq!(reconcile_assign_slots(None, None, &p, &sum), None);
        // Unknown agent.
        let p2 = BrainAssignSlots {
            task_id: Some("task_0001".to_string()),
            agent_id: Some("nobody".to_string()),
            confidence: 0.9,
            rationale: String::new(),
        };
        assert_eq!(reconcile_assign_slots(None, None, &p2, &sum), None);
        // Low confidence.
        let p3 = BrainAssignSlots {
            task_id: Some("task_0001".to_string()),
            agent_id: Some("researcher".to_string()),
            confidence: 0.2,
            rationale: String::new(),
        };
        assert_eq!(reconcile_assign_slots(None, None, &p3, &sum), None);
        // Only one id -> never half-resolved.
        let p4 = BrainAssignSlots {
            task_id: Some("task_0001".to_string()),
            agent_id: None,
            confidence: 0.9,
            rationale: String::new(),
        };
        assert_eq!(reconcile_assign_slots(None, None, &p4, &sum), None);
    }

    #[test]
    fn reconcile_falls_back_to_deterministic_references() {
        let sum = summary(&["task_0001"], &["researcher"]);
        // The brain offered nothing, but the deterministic extractors found both.
        let p = BrainAssignSlots {
            task_id: None,
            agent_id: None,
            confidence: 0.9,
            rationale: String::new(),
        };
        assert_eq!(
            reconcile_assign_slots(Some("task_0001"), Some("the researcher"), &p, &sum),
            Some(ResolvedAssignSlots {
                task_id: "task_0001".to_string(),
                agent_id: "researcher".to_string(),
            })
        );
    }

    #[test]
    fn prompt_lists_the_live_board() {
        let sum = summary(&["task_0001"], &["researcher"]);
        let prompt = build_assign_slots_prompt("assign it to the researcher", &sum);
        assert!(prompt.contains("task_0001"));
        assert!(prompt.contains("researcher"));
        assert!(prompt.contains("ONLY compact JSON"));
    }
}
