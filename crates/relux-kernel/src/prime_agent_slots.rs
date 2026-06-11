//! Brain-assisted, VALIDATED extraction of an *agent's* creation slots — the next
//! brittle Prime path moved off keyword string-slicing, after task slots
//! ([`crate::prime_slots`]).
//!
//! ## Why this exists
//!
//! `AgentCreation` turns still derive the agent's name with
//! [`crate::prime::derive_agent_name`]: it scans for `" named "`/`" called "`/`" as "`
//! and otherwise maps a few hard-coded keywords (`browser`/`research`/`code`/…) to a
//! fixed id, falling back to `"new-agent"`. So "spin up an operative that watches the
//! CI and files a brief when a build breaks" becomes `new-agent` with the generic
//! description "Agent created by Prime" — no role, no normalized name, no adapter
//! preference. The master plan asks Prime to *understand* the request and produce
//! clean, structured crew (`docs/RELUX_MASTER_PLAN.md` §10.1 Intent Layer, §10.2
//! Action Layer, §17.1). A real brain produces a clean name, a role/description, and
//! (when the user named one) an existing adapter; keyword slicing cannot.
//!
//! ## Reference-driven design (see `docs/reference-driven-development.md`)
//!
//! Same "model proposes structured arguments, server validates against a
//! schema/allowlist before acting" shape as the task-slot layer, read first from
//! Paperclip's session-spawn tool — the closest reference analogue to "create a new
//! worker from a conversational request":
//!
//! - **Paperclip/openclaw** `src/agents/tools/sessions-spawn-tool.ts`
//!   (`UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS` rejected at L277-284 BEFORE any param is
//!   read; `readStringParam(params, "task", { required: true })`; the
//!   default-the-rest pattern `params.cleanup === "keep" | "delete" ? … : "keep"`)
//!   and `src/agents/tools/common.ts` (`readStringParam` / `ToolInputError`, L91-122
//!   — a required string THROWS on bad input rather than coercing silently). We mirror
//!   that exactly: [`parse_agent_slots`] rejects any field outside the allowlist
//!   (fail closed), requires a non-empty `name`, and defaults/drops the optionals.
//! - **Paperclip/openclaw** `src/acp/approval-classifier.ts`
//!   (`normalizeToolName`, L57-63: lowercase, length-bound, and only a
//!   `^[a-z0-9._-]+$` shape is accepted; an off-shape name returns `undefined`). We
//!   mirror that name discipline in [`agent_id_form`] and [`sanitize_plugin_id`]:
//!   an id is lowercased, reduced to `[a-z0-9-]`, and clamped; an empty result is
//!   rejected.
//! - **The key safety adaptation** — like a task assignee, the `adapter` is honored
//!   ONLY when it names an adapter that actually EXISTS (validated against the live
//!   adapter roster), and the derived id may NOT collide with an existing agent: the
//!   brain can never invent/enable an adapter, and can never reshape a create into a
//!   duplicate.
//! - **openclaw** `src/shared/balanced-json.ts` (`extractBalancedJsonPrefix`): the
//!   JSON object is lifted from a noisy reply with a balanced-brace scan. We reuse the
//!   same scanner via [`crate::prime_intent::extract_json_object`].
//!
//! ## The contract (binding)
//!
//! The brain only *proposes* slots; it executes nothing. Slots are computed ONLY when
//! the (already brain-reconciled, fail-closed-gated) intent is `AgentCreation` and the
//! deterministic path already produced a real `CreateAgent` — so this layer *sharpens*
//! a create the deterministic path already decided. Every slot is validated; on any
//! failure (no brain, low confidence, invalid JSON, unsupported field, empty name,
//! duplicate id, unknown adapter with nothing else contributed) the deterministic
//! [`crate::prime::derive_agent_name`] name stands.

use crate::prime_intent::extract_json_object;

/// Minimum confidence before a brain's proposed agent slots are honored.
const CONFIDENCE_FLOOR: f32 = 0.6;

/// Max characters kept for an agent display name before id normalization.
const MAX_NAME_CHARS: usize = 64;
/// Max characters kept for an agent id (the normalized handle).
const MAX_ID_CHARS: usize = 64;
/// Max characters kept for the role/description.
const MAX_DESC_CHARS: usize = 240;
/// Max characters kept for free-text notes.
const MAX_NOTES_CHARS: usize = 600;
/// Max characters kept for a normalized adapter plugin id before allowlist validation.
const MAX_ADAPTER_CHARS: usize = 96;
/// Max characters kept from the brain's free-text rationale (audit/provenance only).
const MAX_RATIONALE_CHARS: usize = 240;

/// The only fields an agent-slot proposal may carry. Any other key fails the proposal
/// closed (Paperclip's `UNSUPPORTED_*_PARAM_KEYS` rejection) — the brain may not
/// smuggle a permission/tool/run key in as authority.
const ALLOWED_KEYS: &[&str] = &[
    "name",
    "role",
    "adapter",
    "notes",
    "confidence",
    "rationale",
];

/// A validated set of agent-creation slots a brain *proposes* for one create turn.
///
/// Only [`parse_agent_slots`] builds this, and only after rejecting unknown fields,
/// sanitizing every string, and clamping lengths. `name` is guaranteed non-empty; the
/// rationale is audit text only.
#[derive(Debug, Clone, PartialEq)]
pub struct BrainAgentSlots {
    pub name: String,
    pub role: Option<String>,
    /// The raw (normalized but NOT yet allowlist-validated) adapter plugin id. It is
    /// honored only if [`reconcile_agent_slots`] finds it among the live adapters.
    pub adapter: Option<String>,
    pub notes: Option<String>,
    pub confidence: f32,
    pub rationale: String,
}

/// The slots the kernel will actually apply to a created agent, after reconciling a
/// brain proposal against the deterministic name, the live agent roster, and the live
/// adapter roster.
///
/// `id` is always a non-colliding, normalized handle; `adapter` here is always an
/// EXISTING adapter plugin id (or `None`, meaning the deterministic default). Built
/// only by [`reconcile_agent_slots`], and only when the brain genuinely contributed.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedAgentSlots {
    pub name: String,
    pub id: String,
    pub description: Option<String>,
    pub adapter: Option<String>,
    pub notes: Option<String>,
}

/// The strict, self-contained prompt handed to a brain to extract the slots of ONE
/// agent the user clearly asked Prime to create. Mirrors the task-slot prompt: the
/// schema is spelled out, the safety rules are explicit (never invent an adapter,
/// never add other fields, never claim an action), and JSON-only output is demanded.
pub fn build_agent_slots_prompt(message: &str) -> String {
    format!(
        "You are extracting the structured slots of a SINGLE agent (a worker) the user has \
clearly asked Prime to create on a local Relux control plane. You perform no action and create \
nothing; you only describe the agent's slots so the kernel can create it.\n\n\
Respond with JSON ONLY (no prose, no code fences) in EXACTLY this shape:\n\
{{\"name\":\"<short human name, e.g. Research Agent>\",\"role\":\"<optional one-line role/description, or omit>\",\
\"adapter\":\"<optional existing adapter plugin id, or omit>\",\"notes\":\"<optional notes, or omit>\",\
\"confidence\":<0.0-1.0>}}\n\n\
Rules:\n\
- name: a concise human name for the agent (e.g. \"Research Agent\", \"CI Watcher\"). REQUIRED.\n\
- role: a single-line description of what the agent does. Include ONLY if the message implies one.\n\
- adapter: include ONLY an adapter plugin id the user explicitly named; if unsure, omit it. NEVER \
invent an adapter, plugin, or tool name.\n\
- notes: include ONLY extra context worth recording; otherwise omit it.\n\
- Do NOT add any field other than these. Do NOT grant permissions. Do NOT claim the agent was created.\n\n\
User message:\n{message}"
    )
}

/// Parse a brain's raw reply into validated [`BrainAgentSlots`], or `Err` with a short
/// reason on anything malformed/unsupported. The schema/allowlist gate.
pub fn parse_agent_slots(raw: &str) -> Result<BrainAgentSlots, String> {
    let json = extract_json_object(raw).ok_or_else(|| "no JSON object in reply".to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&json).map_err(|_| "reply was not valid JSON".to_string())?;
    let obj = value
        .as_object()
        .ok_or_else(|| "reply was not a JSON object".to_string())?;

    for key in obj.keys() {
        if !ALLOWED_KEYS.contains(&key.as_str()) {
            return Err(format!("unsupported field '{key}'"));
        }
    }

    let name = sanitize_line(
        obj.get("name").and_then(|v| v.as_str()).unwrap_or(""),
        MAX_NAME_CHARS,
    );
    if name.is_empty() {
        return Err("empty or missing name".to_string());
    }

    let role = obj
        .get("role")
        .and_then(|v| v.as_str())
        .map(|s| sanitize_line(s, MAX_DESC_CHARS))
        .filter(|s| !s.is_empty());

    let adapter = obj
        .get("adapter")
        .and_then(|v| v.as_str())
        .map(sanitize_plugin_id)
        .filter(|s| !s.is_empty());

    let notes = obj
        .get("notes")
        .and_then(|v| v.as_str())
        .map(|s| sanitize_block(s, MAX_NOTES_CHARS))
        .filter(|s| !s.is_empty());

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

    Ok(BrainAgentSlots {
        name,
        role,
        adapter,
        notes,
        confidence,
        rationale,
    })
}

/// Reconcile a brain agent-slot proposal against the deterministic name and the live
/// control-plane state, returning the slots to apply, or `None` to keep the
/// deterministic name.
///
/// Policy — each rule fails toward the deterministic / safer choice:
/// 1. Low confidence (`< CONFIDENCE_FLOOR`) → `None`.
/// 2. The proposed name must normalize to a non-empty id; otherwise `None`.
/// 3. That id may NOT collide (case-insensitive) with an EXISTING agent id — a
///    duplicate proposal is rejected wholesale so the brain can never reshape a create
///    into a clash (`existing_agent_ids` is `summary.all_agent_ids`).
/// 4. The adapter is honored ONLY when it names an EXISTING adapter plugin
///    (case-insensitive match against `adapter_ids`); an unknown adapter is dropped
///    and the deterministic default stands. The brain can never invent/enable one.
/// 5. The result is reported as brain-assisted ONLY when the brain actually
///    contributed something beyond echoing the deterministic name (a changed id, or
///    any role/adapter/notes); otherwise `None`.
pub fn reconcile_agent_slots(
    deterministic_name: &str,
    proposal: &BrainAgentSlots,
    existing_agent_ids: &[String],
    adapter_ids: &[String],
) -> Option<ResolvedAgentSlots> {
    if proposal.confidence < CONFIDENCE_FLOOR {
        return None;
    }

    let id = agent_id_form(&proposal.name);
    if id.is_empty() {
        return None;
    }
    // Reject a duplicate id outright (fail closed): the brain may never reshape a
    // create into a collision with an existing agent.
    if existing_agent_ids.iter().any(|e| e.eq_ignore_ascii_case(&id)) {
        return None;
    }

    // The adapter is honored only when it names an existing adapter plugin; otherwise
    // it is dropped and the deterministic default adapter stands.
    let adapter = proposal.adapter.as_ref().and_then(|a| {
        adapter_ids
            .iter()
            .find(|id| id.eq_ignore_ascii_case(a))
            .cloned()
    });

    // Only report brain assistance when it genuinely sharpened the slots — a brain
    // that merely echoes the deterministic name with nothing else is a no-op.
    let changed_id = id != agent_id_form(deterministic_name);
    if !changed_id && proposal.role.is_none() && adapter.is_none() && proposal.notes.is_none() {
        return None;
    }

    Some(ResolvedAgentSlots {
        name: proposal.name.clone(),
        id,
        description: proposal.role.clone(),
        adapter,
        notes: proposal.notes.clone(),
    })
}

/// Normalize a display name into an agent id: lowercase, keep only `[a-z0-9-]` (spaces
/// and other separators become a single hyphen), collapse repeats, trim hyphens, and
/// clamp. Mirrors openclaw's `normalizeToolName` discipline (lowercase + bounded +
/// a strict id shape) and the kernel's own `name.to_lowercase().replace(" ", "-")`.
fn agent_id_form(s: &str) -> String {
    let lowered = s.trim().to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut last_hyphen = false;
    for c in lowered.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_hyphen = false;
        } else if c == '-' || c == '_' || c.is_whitespace() {
            if last_hyphen {
                continue;
            }
            last_hyphen = true;
            out.push('-');
        }
        // Drop anything else.
        if out.chars().count() >= MAX_ID_CHARS {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

/// Normalize a proposed adapter plugin id: lowercase, keep `[a-z0-9-]`, collapse and
/// trim hyphens, clamp. Still only a CANDIDATE — [`reconcile_agent_slots`] keeps it
/// only if it matches an existing adapter.
fn sanitize_plugin_id(s: &str) -> String {
    let lowered = s.trim().to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut last_hyphen = false;
    for c in lowered.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_hyphen = false;
        } else if c == '-' || c == '_' || c == '.' || c.is_whitespace() {
            if last_hyphen {
                continue;
            }
            last_hyphen = true;
            out.push('-');
        }
        if out.chars().count() >= MAX_ADAPTER_CHARS {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

/// Sanitize a single-line string: control chars → space, collapse whitespace, trim,
/// clamp. Shared shape with [`crate::prime_slots`].
fn sanitize_line(s: &str, max: usize) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(max).collect::<String>().trim().to_string()
}

/// Sanitize a multi-line block: drop control chars except `\n`, collapse intra-line
/// whitespace, drop blank lines, trim, clamp. Shared shape with [`crate::prime_slots`].
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

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_agent_slots: the schema / allowlist gate ----------------------

    #[test]
    fn parses_a_clean_agent_object() {
        let p = parse_agent_slots(
            r#"{"name":"CI Watcher","role":"Watches CI and files a brief on failure","adapter":"relux-adapter-local-prime","confidence":0.9}"#,
        )
        .unwrap();
        assert_eq!(p.name, "CI Watcher");
        assert_eq!(
            p.role.as_deref(),
            Some("Watches CI and files a brief on failure")
        );
        assert_eq!(p.adapter.as_deref(), Some("relux-adapter-local-prime"));
        assert_eq!(p.confidence, 0.9);
    }

    #[test]
    fn extracts_from_noisy_reply_with_prose_and_fences() {
        let raw = "Sure:\n```json\n{\"name\": \"Research Agent\", \"confidence\": 0.8}\n```\n";
        let p = parse_agent_slots(raw).unwrap();
        assert_eq!(p.name, "Research Agent");
        assert!(p.role.is_none());
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(parse_agent_slots("not json").is_err());
        assert!(parse_agent_slots("{ name: unquoted }").is_err());
    }

    #[test]
    fn rejects_an_unsupported_field_fail_closed() {
        // A brain that tries to smuggle authority (a permission/run key) fails the
        // WHOLE proposal closed.
        let err = parse_agent_slots(
            r#"{"name":"x","permissions":["tool:relux-tools-shell:exec"],"confidence":0.9}"#,
        )
        .unwrap_err();
        assert!(err.contains("unsupported field"), "got: {err}");
        assert!(parse_agent_slots(r#"{"name":"x","run":true,"confidence":0.9}"#).is_err());
    }

    #[test]
    fn rejects_empty_or_missing_name() {
        assert!(parse_agent_slots(r#"{"confidence":0.9}"#).is_err());
        assert!(parse_agent_slots(r#"{"name":"   ","confidence":0.9}"#).is_err());
    }

    #[test]
    fn clamps_and_strips_control_chars_in_name() {
        let p = parse_agent_slots("{\"name\":\"Re\\tsearch\\nAgent\",\"confidence\":0.9}").unwrap();
        assert_eq!(p.name, "Re search Agent");
        assert!(!p.name.contains('\n') && !p.name.contains('\t'));
    }

    // --- reconcile_agent_slots: the validation / fail-closed gate -------------

    fn prop(name: &str, confidence: f32) -> BrainAgentSlots {
        BrainAgentSlots {
            name: name.to_string(),
            role: None,
            adapter: None,
            notes: None,
            confidence,
            rationale: String::new(),
        }
    }

    #[test]
    fn reconcile_keeps_a_normalized_name_over_the_deterministic_one() {
        let mut p = prop("CI Watcher", 0.9);
        p.role = Some("Watches CI".to_string());
        let r = reconcile_agent_slots("new-agent", &p, &[], &[]).unwrap();
        assert_eq!(r.id, "ci-watcher");
        assert_eq!(r.name, "CI Watcher");
        assert_eq!(r.description.as_deref(), Some("Watches CI"));
    }

    #[test]
    fn reconcile_falls_back_on_low_confidence() {
        let mut p = prop("CI Watcher", 0.4);
        p.role = Some("Watches CI".to_string());
        assert!(reconcile_agent_slots("new-agent", &p, &[], &[]).is_none());
    }

    #[test]
    fn reconcile_rejects_a_duplicate_id_fail_closed() {
        // The brain proposes a name that normalizes to an existing agent id: rejected
        // wholesale so a create can never be reshaped into a collision.
        let existing = vec!["research-agent".to_string()];
        let mut p = prop("Research Agent", 0.9);
        p.role = Some("Does research".to_string());
        assert!(reconcile_agent_slots("new-agent", &p, &existing, &[]).is_none());
    }

    #[test]
    fn reconcile_honors_an_existing_adapter_and_drops_an_unknown_one() {
        let adapters = vec!["relux-adapter-local-prime".to_string()];

        let mut known = prop("CI Watcher", 0.9);
        known.adapter = Some("relux-adapter-local-prime".to_string());
        let r = reconcile_agent_slots("new-agent", &known, &[], &adapters).unwrap();
        assert_eq!(r.adapter.as_deref(), Some("relux-adapter-local-prime"));

        // An unknown adapter is dropped; with nothing else contributed and an
        // unchanged id, the whole proposal resolves to None.
        let mut unknown = prop("new-agent", 0.9);
        unknown.adapter = Some("relux-adapter-ghost".to_string());
        assert!(reconcile_agent_slots("new-agent", &unknown, &[], &adapters).is_none());
    }

    #[test]
    fn reconcile_reports_no_assistance_for_a_pure_echo() {
        // Brain echoes the deterministic name with nothing else: a no-op.
        assert!(reconcile_agent_slots("new-agent", &prop("new agent", 0.95), &[], &[]).is_none());
    }

    #[test]
    fn build_prompt_carries_the_schema_and_safety_rules() {
        let prompt = build_agent_slots_prompt("create a CI watcher agent");
        assert!(prompt.contains("\"name\""));
        assert!(prompt.contains("JSON ONLY"));
        assert!(prompt.contains("NEVER invent"));
        assert!(prompt.contains("create a CI watcher agent"));
    }
}
