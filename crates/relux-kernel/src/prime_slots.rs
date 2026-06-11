//! Brain-assisted, VALIDATED extraction of a task's *slots* — the next brittle
//! part of Prime moved off keyword string-slicing and onto a brain proposal that
//! the kernel validates hard before any task is created.
//!
//! ## Why this exists
//!
//! Even after intent classification became brain-mediated
//! ([`crate::prime_intent`]), the *slots* of a created task were still derived by
//! string slicing: [`crate::prime::task_title`] strips a fixed list of polite
//! lead-ins and takes whatever is left as the title, with no normalization, no
//! details, no assignee, no priority. So "could you please take care of the thing
//! where users land on a blank page after SSO, and have code-agent do it" becomes
//! a task titled with the whole run-on clause. The master plan asks for Prime to
//! *understand* the request and produce clean, structured work
//! (`docs/RELUX_MASTER_PLAN.md` §10.1 Intent Layer, §10.2 Action Layer, §17.1).
//! A real brain produces a clean title, optional details, a suggested assignee,
//! and a priority; keyword slicing cannot.
//!
//! ## Reference-driven design (see `docs/reference-driven-development.md`)
//!
//! This is the "model proposes structured arguments, server validates against a
//! schema/allowlist before acting" pattern, read first from Hermes and Paperclip:
//!
//! - **Hermes** `agent/conversation_loop.py` (~L3166-3251) + `model_tools.py`
//!   `coerce_tool_args` / `_coerce_number` / `_coerce_boolean` + `tools/
//!   schema_sanitizer.py`: the model's tool-call ARGUMENTS are parsed, repaired,
//!   and COERCED against the registered parameter schema before execution; a
//!   malformed object degrades to `{}`, an out-of-type value is coerced or
//!   rejected, and control chars / lone surrogates are sanitized
//!   (`agent/message_sanitization.py`). We mirror that: [`parse_task_slots`] lifts
//!   the JSON out of a noisy reply, **rejects any unsupported field**, sanitizes
//!   every string (control chars stripped, length-clamped), and coerces the
//!   priority — anything malformed fails closed.
//! - **Paperclip/openclaw** `src/agents/tools/update-plan-tool.ts` (`readPlanSteps`,
//!   the `PLAN_STEP_STATUSES` allowlist) + `src/agents/tools/sessions-spawn-tool.ts`
//!   (`UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS`, `readStringParam`, `maxItems`,
//!   `Math.max(0, Math.floor(...))` clamps) + `src/agents/tools/common.ts`
//!   (`ToolInputError`): structured payloads are validated field-by-field against
//!   an explicit schema, statuses are checked against an ALLOWLIST, unsupported
//!   keys are rejected outright, and every value is length/range clamped before the
//!   payload is honored. We mirror that field discipline here, and — the key
//!   safety adaptation — the assignee is honored **only when it names an agent that
//!   actually exists** (the brain can never invent an assignee or smuggle a
//!   plugin/tool name in as one).
//! - **openclaw** `src/shared/balanced-json.ts` (`extractBalancedJsonPrefix`): pull
//!   the JSON object out of a noisy reply with a balanced-brace scan rather than
//!   trusting the whole stdout. We reuse the same scanner via
//!   [`crate::prime_intent::extract_json_object`].
//!
//! ## The contract (binding)
//!
//! The brain only *proposes* slots; it executes nothing. Slots are computed ONLY
//! when the (already brain-reconciled, fail-closed-gated) intent is a task-creation
//! intent **and** the deterministic path already produced a real create (a title) —
//! so this layer *sharpens* an existing create, it never mints work from nothing
//! and never runs anything. Casual chat / ideation still cannot reach this layer:
//! the intent gate keeps it `Brainstorming`. Every slot is validated:
//!
//! - **title** — sanitized (control chars stripped, single line) and length-clamped;
//!   an empty/missing title fails the whole proposal.
//! - **details** — sanitized and length-clamped; folded into the task input only.
//! - **assignee** — honored ONLY when it matches an EXISTING agent id; an unknown id
//!   is dropped and the task stays assigned to Prime.
//! - **priority** — coerced to a number and clamped to the supported range.
//! - any **unsupported field** → reject the whole proposal (fail closed).
//! - **low confidence**, **invalid JSON**, or an **empty title** → fall back to the
//!   deterministic slots.
//!
//! On any failure the deterministic [`crate::prime::task_title`] slots stand. The
//! brain is strictly additive: it sharpens a create it could not mint.

use relux_core::StateSummary;

use crate::prime_intent::extract_json_object;

/// Minimum confidence before a brain's proposed slots are honored. Below this the
/// deterministic title stands — a hesitant brain never reshapes the task.
const CONFIDENCE_FLOOR: f32 = 0.6;

/// Max characters kept for a task title. Matches the deterministic
/// [`crate::prime::task_title`] cap so brain and keyword titles share one bound.
const MAX_TITLE_CHARS: usize = 120;
/// Max characters kept for optional details/notes folded into the task input.
const MAX_DETAILS_CHARS: usize = 600;
/// Max characters kept for a normalized assignee id before allowlist validation.
const MAX_ASSIGNEE_CHARS: usize = 64;
/// Max characters kept from the brain's free-text rationale (audit/provenance only).
const MAX_RATIONALE_CHARS: usize = 240;

/// Inclusive priority range the kernel supports. The kernel default is `5`; the
/// brain may nudge a task up or down within `[1, 9]` (1 low … 9 high).
const PRIORITY_MIN: u8 = 1;
const PRIORITY_MAX: u8 = 9;

/// The only fields a slot proposal may carry. Any other key fails the proposal
/// closed (Paperclip's `UNSUPPORTED_*_PARAM_KEYS` rejection) — the brain may not
/// smuggle a tool/plugin/action key in as authority.
const ALLOWED_KEYS: &[&str] = &[
    "title",
    "details",
    "assignee",
    "priority",
    "confidence",
    "rationale",
];

/// A validated set of task slots a brain *proposes* for one create turn.
///
/// Only [`parse_task_slots`] builds this, and only after rejecting unknown fields,
/// sanitizing every string, and clamping lengths/ranges. `title` is guaranteed
/// non-empty; the rationale is audit text only.
#[derive(Debug, Clone, PartialEq)]
pub struct BrainTaskSlots {
    pub title: String,
    pub details: Option<String>,
    /// The raw (normalized but NOT yet allowlist-validated) assignee id. It is
    /// honored only if [`reconcile_task_slots`] finds it among the live agents.
    pub assignee: Option<String>,
    pub priority: Option<u8>,
    pub confidence: f32,
    pub rationale: String,
}

/// The slots the kernel will actually apply to a created task, after reconciling a
/// brain proposal against the deterministic title and the live control-plane state.
///
/// `assignee` here is always an EXISTING agent id (or `None`); `title` is the
/// effective title the task carries. Built only by [`reconcile_task_slots`], and
/// only when the brain genuinely contributed something.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTaskSlots {
    pub title: String,
    pub details: Option<String>,
    pub assignee: Option<String>,
    pub priority: Option<u8>,
}

/// The strict, self-contained prompt handed to a brain to extract the slots of ONE
/// task the user clearly asked to create. Mirrors the intent prompt: the schema is
/// spelled out, the safety rules are explicit (never invent an assignee/tool/
/// plugin, never add other fields, never claim an action), and JSON-only output is
/// demanded so nothing un-validated leaks downstream.
pub fn build_task_slots_prompt(message: &str) -> String {
    format!(
        "You are extracting the structured slots of a SINGLE task the user has clearly asked Prime \
to create on a local Relux control plane. You perform no action and create nothing; you only \
describe the task's slots so the kernel can create it.\n\n\
Respond with JSON ONLY (no prose, no code fences) in EXACTLY this shape:\n\
{{\"title\":\"<short imperative task title>\",\"details\":\"<optional notes, or omit>\",\
\"assignee\":\"<optional existing agent id, or omit>\",\"priority\":<optional integer 1-9, or omit>,\
\"confidence\":<0.0-1.0>}}\n\n\
Rules:\n\
- title: a concise, imperative, single-line title (e.g. \"Fix the login redirect bug\"). REQUIRED.\n\
- details: include ONLY if the message carries extra specifics worth recording; otherwise omit it.\n\
- assignee: include ONLY an agent id the user explicitly named; if unsure, omit it. NEVER invent an \
agent, tool, or plugin name.\n\
- priority: include ONLY if the user signaled urgency. 1 (low) to 9 (high). Otherwise omit it.\n\
- Do NOT add any field other than these. Do NOT claim the task was created.\n\n\
User message:\n{message}"
    )
}

/// Parse a brain's raw reply into validated [`BrainTaskSlots`], or `Err` with a
/// short reason on anything malformed/unsupported.
///
/// This is the schema/allowlist gate: the reply must contain a balanced JSON
/// object, every key must be in [`ALLOWED_KEYS`] (an unsupported field fails the
/// whole proposal), the title must sanitize to a non-empty single line, and every
/// value is sanitized and clamped. The brain's raw text never flows anywhere else —
/// a parse failure simply drops the caller to the deterministic slots.
pub fn parse_task_slots(raw: &str) -> Result<BrainTaskSlots, String> {
    let json = extract_json_object(raw).ok_or_else(|| "no JSON object in reply".to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&json).map_err(|_| "reply was not valid JSON".to_string())?;
    let obj = value
        .as_object()
        .ok_or_else(|| "reply was not a JSON object".to_string())?;

    // Reject unknown / unsupported fields outright (fail closed). The brain may not
    // smuggle a tool/plugin/action/tags key in as authority.
    for key in obj.keys() {
        if !ALLOWED_KEYS.contains(&key.as_str()) {
            return Err(format!("unsupported field '{key}'"));
        }
    }

    let title = sanitize_line(
        obj.get("title").and_then(|v| v.as_str()).unwrap_or(""),
        MAX_TITLE_CHARS,
    );
    if title.is_empty() {
        return Err("empty or missing title".to_string());
    }

    let details = obj
        .get("details")
        .and_then(|v| v.as_str())
        .map(|s| sanitize_block(s, MAX_DETAILS_CHARS))
        .filter(|s| !s.is_empty());

    let assignee = obj
        .get("assignee")
        .and_then(|v| v.as_str())
        .map(sanitize_assignee)
        .filter(|s| !s.is_empty());

    let priority = coerce_priority(obj.get("priority"));

    let confidence = obj
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.5)
        .clamp(0.0, 1.0) as f32;

    let rationale = obj
        .get("rationale")
        .and_then(|v| v.as_str())
        .map(|s| sanitize_line(s, MAX_RATIONALE_CHARS))
        .unwrap_or_default();

    Ok(BrainTaskSlots {
        title,
        details,
        assignee,
        priority,
        confidence,
        rationale,
    })
}

/// Reconcile a brain slot proposal against the deterministic title and the live
/// control-plane state, returning the slots to apply, or `None` to keep the
/// deterministic slots.
///
/// Policy — each rule fails toward the deterministic / safer choice:
/// 1. Low confidence (`< CONFIDENCE_FLOOR`) → `None` (deterministic slots stand).
/// 2. The assignee is honored ONLY when it names an EXISTING agent (case-insensitive
///    match against `summary.all_agent_ids`); an unknown id is dropped so the task
///    stays assigned to Prime. The brain can never invent an assignee.
/// 3. The priority is taken as already clamped by [`parse_task_slots`].
/// 4. The result is reported as brain-assisted ONLY when the brain actually
///    contributed something beyond echoing the deterministic title (a changed
///    title, or any details/assignee/priority); otherwise `None`, so a no-op
///    proposal shows no provenance and changes nothing.
pub fn reconcile_task_slots(
    deterministic_title: &str,
    proposal: &BrainTaskSlots,
    summary: &StateSummary,
) -> Option<ResolvedTaskSlots> {
    if proposal.confidence < CONFIDENCE_FLOOR {
        return None;
    }

    // The assignee is honored only when it names an existing agent; otherwise it is
    // dropped (fail closed) and the task stays assigned to Prime.
    let assignee = proposal.assignee.as_ref().and_then(|a| {
        summary
            .all_agent_ids
            .iter()
            .find(|id| id.eq_ignore_ascii_case(a))
            .cloned()
    });

    let title = proposal.title.clone();
    let details = proposal.details.clone();
    let priority = proposal.priority;

    // Only report brain assistance when it genuinely sharpened the slots — a brain
    // that merely echoes the deterministic title with nothing else is a no-op.
    let changed_title = title.trim() != deterministic_title.trim();
    if !changed_title && details.is_none() && assignee.is_none() && priority.is_none() {
        return None;
    }

    Some(ResolvedTaskSlots {
        title,
        details,
        assignee,
        priority,
    })
}

/// Coerce a JSON priority value (number or numeric string) to a clamped `u8`, or
/// `None` when absent or not a usable number. Mirrors Hermes' `_coerce_number` /
/// openclaw's `Math.max(0, Math.floor(...))` clamp: a non-numeric or out-of-range
/// value is dropped rather than failing the whole proposal.
fn coerce_priority(value: Option<&serde_json::Value>) -> Option<u8> {
    let raw = match value? {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }?;
    if !raw.is_finite() {
        return None;
    }
    let rounded = raw.round();
    let clamped = rounded.clamp(PRIORITY_MIN as f64, PRIORITY_MAX as f64);
    Some(clamped as u8)
}

/// Sanitize a single-line string: replace every control char (including newlines
/// and tabs) with a space, collapse whitespace runs, trim, and clamp to `max`
/// characters. Strips lone control chars an envelope could carry.
fn sanitize_line(s: &str, max: usize) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(max).collect::<String>().trim().to_string()
}

/// Sanitize a multi-line block: drop control chars except `\n`, collapse intra-line
/// whitespace, drop blank lines, trim, and clamp to `max` characters. Newlines are
/// preserved (JSON-escaped on the wire); other control chars are removed.
fn sanitize_block(s: &str, max: usize) -> String {
    let lines: Vec<String> = s
        .lines()
        .map(|line| {
            let cleaned: String = line
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
        })
        .filter(|line| !line.is_empty())
        .collect();
    let joined = lines.join("\n");
    let truncated: String = joined.chars().take(max).collect();
    truncated.trim().to_string()
}

/// Normalize a proposed assignee id the way agent ids are formed: lowercase, keep
/// only `[a-z0-9-_]` (other chars become hyphens), collapse repeats, trim hyphens,
/// and clamp. The result is still only a CANDIDATE — [`reconcile_task_slots`] keeps
/// it only if it matches an existing agent.
fn sanitize_assignee(s: &str) -> String {
    let lowered = s.trim().to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut last_hyphen = false;
    for c in lowered.chars() {
        let mapped = if c.is_ascii_alphanumeric() || c == '_' {
            last_hyphen = false;
            c
        } else if c == '-' || c.is_whitespace() {
            if last_hyphen {
                continue;
            }
            last_hyphen = true;
            '-'
        } else {
            // Drop anything else (punctuation an injected name might carry).
            continue;
        };
        out.push(mapped);
        if out.chars().count() >= MAX_ASSIGNEE_CHARS {
            break;
        }
    }
    out.trim_matches('-').to_string()
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
            all_task_ids: Vec::new(),
            queued: Vec::new(),
            recent: Vec::new(),
        }
    }

    // --- parse_task_slots: the schema / allowlist gate ----------------------

    #[test]
    fn parses_a_clean_slot_object() {
        let p = parse_task_slots(
            r#"{"title":"Fix the login redirect bug","details":"Blank page after SSO.","assignee":"code-agent","priority":8,"confidence":0.9}"#,
        )
        .unwrap();
        assert_eq!(p.title, "Fix the login redirect bug");
        assert_eq!(p.details.as_deref(), Some("Blank page after SSO."));
        assert_eq!(p.assignee.as_deref(), Some("code-agent"));
        assert_eq!(p.priority, Some(8));
        assert_eq!(p.confidence, 0.9);
    }

    #[test]
    fn extracts_slots_from_noisy_reply_with_prose_and_fences() {
        let raw = "Sure, here you go:\n```json\n{\"title\": \"Summarize the README\", \
                   \"confidence\": 0.8}\n```\nLet me know!";
        let p = parse_task_slots(raw).unwrap();
        assert_eq!(p.title, "Summarize the README");
        assert!(p.details.is_none());
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(parse_task_slots("this is not json at all").is_err());
        assert!(parse_task_slots("{ title: unquoted }").is_err());
    }

    #[test]
    fn rejects_an_unsupported_field_fail_closed() {
        // A brain that tries to smuggle authority (a tool/plugin/action/tags key)
        // fails the WHOLE proposal closed, so the caller keeps the deterministic
        // slots rather than honoring a field we never validate.
        let err = parse_task_slots(
            r#"{"title":"x","run_tool":"relux-tools-shell","confidence":0.9}"#,
        )
        .unwrap_err();
        assert!(err.contains("unsupported field"), "got: {err}");
        assert!(parse_task_slots(r#"{"title":"x","tags":["a"],"confidence":0.9}"#).is_err());
    }

    #[test]
    fn rejects_empty_or_missing_title() {
        assert!(parse_task_slots(r#"{"confidence":0.9}"#).is_err());
        assert!(parse_task_slots(r#"{"title":"   ","confidence":0.9}"#).is_err());
    }

    #[test]
    fn clamps_an_overlong_title_and_strips_control_chars() {
        let long = "a".repeat(400);
        let raw = format!(r#"{{"title":"{long}","confidence":0.9}}"#);
        let p = parse_task_slots(&raw).unwrap();
        assert_eq!(p.title.chars().count(), MAX_TITLE_CHARS);

        // Embedded newlines/tabs are collapsed to a single-line title — no raw
        // control chars survive into the slot.
        let p2 = parse_task_slots(
            "{\"title\":\"Fix\\tthe\\nlogin\\nbug\",\"confidence\":0.9}",
        )
        .unwrap();
        assert_eq!(p2.title, "Fix the login bug");
        assert!(!p2.title.contains('\n') && !p2.title.contains('\t'));
    }

    #[test]
    fn coerces_priority_from_float_and_string_and_clamps_range() {
        assert_eq!(
            parse_task_slots(r#"{"title":"x","priority":8.0,"confidence":0.9}"#)
                .unwrap()
                .priority,
            Some(8)
        );
        assert_eq!(
            parse_task_slots(r#"{"title":"x","priority":"7","confidence":0.9}"#)
                .unwrap()
                .priority,
            Some(7)
        );
        // Out-of-range clamps; a non-numeric priority is simply dropped (the slot
        // still parses — only the bad field is ignored).
        assert_eq!(
            parse_task_slots(r#"{"title":"x","priority":99,"confidence":0.9}"#)
                .unwrap()
                .priority,
            Some(PRIORITY_MAX)
        );
        assert_eq!(
            parse_task_slots(r#"{"title":"x","priority":"high","confidence":0.9}"#)
                .unwrap()
                .priority,
            None
        );
    }

    // --- reconcile_task_slots: the validation / fail-closed gate --------------

    fn prop(title: &str, confidence: f32) -> BrainTaskSlots {
        BrainTaskSlots {
            title: title.to_string(),
            details: None,
            assignee: None,
            priority: None,
            confidence,
            rationale: String::new(),
        }
    }

    #[test]
    fn reconcile_keeps_a_normalized_title_over_the_deterministic_one() {
        let summary = summary_with_agents(&["code-agent"]);
        let r = reconcile_task_slots(
            "take care of the login redirect thing",
            &prop("Fix the login redirect bug", 0.9),
            &summary,
        )
        .unwrap();
        assert_eq!(r.title, "Fix the login redirect bug");
    }

    #[test]
    fn reconcile_falls_back_on_low_confidence() {
        let summary = summary_with_agents(&["code-agent"]);
        assert!(reconcile_task_slots(
            "take care of the login bug",
            &prop("Fix the login bug", 0.4),
            &summary
        )
        .is_none());
    }

    #[test]
    fn reconcile_honors_an_existing_assignee_and_drops_an_unknown_one() {
        let summary = summary_with_agents(&["code-agent", "research-agent"]);

        let mut known = prop("Fix the login bug", 0.9);
        known.assignee = Some("code-agent".to_string());
        let r = reconcile_task_slots("fix the login bug", &known, &summary).unwrap();
        assert_eq!(r.assignee.as_deref(), Some("code-agent"));

        // An unknown agent is dropped (fail closed). With nothing else contributed
        // and an unchanged title, the whole proposal resolves to None — the task
        // stays deterministically assigned to Prime.
        let mut unknown = prop("fix the login bug", 0.9);
        unknown.assignee = Some("ghost-agent".to_string());
        assert!(reconcile_task_slots("fix the login bug", &unknown, &summary).is_none());
    }

    #[test]
    fn reconcile_reports_no_assistance_for_a_pure_echo() {
        // Brain echoes the deterministic title with nothing else: a no-op, so no
        // provenance and no override.
        let summary = summary_with_agents(&["code-agent"]);
        assert!(
            reconcile_task_slots("fix the login bug", &prop("fix the login bug", 0.95), &summary)
                .is_none()
        );
    }

    #[test]
    fn reconcile_carries_details_and_clamped_priority() {
        let summary = summary_with_agents(&["code-agent"]);
        let mut p = prop("Fix the login bug", 0.9);
        p.details = Some("Blank page after SSO.".to_string());
        p.priority = Some(8);
        let r = reconcile_task_slots("fix the login bug", &p, &summary).unwrap();
        assert_eq!(r.details.as_deref(), Some("Blank page after SSO."));
        assert_eq!(r.priority, Some(8));
    }

    #[test]
    fn build_prompt_carries_the_schema_and_safety_rules() {
        let prompt = build_task_slots_prompt("could you fix the login bug");
        assert!(prompt.contains("\"title\""));
        assert!(prompt.contains("JSON ONLY"));
        assert!(prompt.contains("NEVER invent"));
        assert!(prompt.contains("could you fix the login bug"));
    }
}
