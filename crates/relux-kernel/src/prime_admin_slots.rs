//! Brain-assisted, VALIDATED extraction of the *subject* of a risky, approval-gated
//! admin action — a plugin install or a permission grant. The advisory counterpart of
//! [`crate::prime_slots`] / [`crate::prime_agent_slots`] for the two `Propose` paths.
//!
//! ## Why this exists
//!
//! `PluginInstallation` and `PermissionChange` turns still derive their subject with
//! raw string slicing in [`crate::prime`]: `derive_plugin_id` takes the first
//! whitespace token that starts with `relux-`, and the permission path literally does
//! `if message.to_lowercase().contains("agent") { derive_agent_name(message) }` with a
//! two-keyword permission map (`derive_permission_label`). So "let the research
//! operative use the GitHub tool" yields the placeholder `(unspecified subject)`. The
//! master plan asks Prime to *understand* the request (`docs/RELUX_MASTER_PLAN.md`
//! §10.1, §17.1); a brain can name a clean subject, keyword slicing cannot.
//!
//! ## The safety shape (binding)
//!
//! Both actions are ALWAYS gated behind a human approval (`PrimePlan::Propose`): the
//! kernel logs an approval and does NOTHING until a human accepts it. So a brain slot
//! here can never *execute* a plugin install or a permission grant by itself — it only
//! sharpens the subject the human reviews. This is exactly the master plan's native
//! governance (`docs/prime-processing-audit.md` "Governance is native") and the
//! reference-driven safety rule (`docs/reference-driven-development.md`: work/
//! control-plane capabilities are one explicit, gated capability — never inferred).
//!
//! ## Reference-driven design
//!
//! - **Paperclip/openclaw** `src/acp/approval-classifier.ts` — `classifyAcpToolApproval`
//!   resolves the approval *subject* (the tool name) from multiple sources
//!   (`resolveToolNameForPermission`, L73-103), normalizes it (`normalizeToolName`,
//!   L57-63: lowercase, length-bound, strict `^[a-z0-9._-]+$` shape — else
//!   `undefined`), and cross-checks the candidates before assigning an approval class;
//!   `EXEC_CAPABLE_TOOL_IDS` / `CONTROL_PLANE_TOOL_IDS` (L15-23) are explicit allowlists
//!   that force a NON-auto-approve class. We mirror that: a permission subject is
//!   normalized to an id shape and a `subject_kind` is checked against
//!   [`SUBJECT_KINDS`]; an off-allowlist kind fails the proposal closed; and the
//!   subject is honored ONLY when it names an EXISTING agent (validated against the
//!   live roster, like a task assignee).
//! - **Paperclip/openclaw** `src/agents/tools/common.ts` (`readStringParam`,
//!   `ToolInputError`) + `sessions-spawn-tool.ts` (`UNSUPPORTED_*_PARAM_KEYS`): the
//!   field discipline — reject unsupported keys, require/trim strings, default the
//!   rest. We reuse it field-by-field in [`parse_plugin_ref`] / [`parse_permission_slots`].
//! - **openclaw** `src/shared/balanced-json.ts` (`extractBalancedJsonPrefix`): the JSON
//!   is lifted from a noisy reply with a balanced-brace scan — reused via
//!   [`crate::prime_intent::extract_json_object`].

use relux_core::StateSummary;

use crate::prime_intent::extract_json_object;

const CONFIDENCE_FLOOR: f32 = 0.6;
const MAX_PLUGIN_CHARS: usize = 96;
const MAX_SUBJECT_CHARS: usize = 64;
const MAX_PERMISSION_CHARS: usize = 128;
const MAX_RATIONALE_CHARS: usize = 240;

/// The only subject kinds a permission grant may target today. An off-allowlist kind
/// fails the proposal closed (only agents are grantable subjects in the kernel).
const SUBJECT_KINDS: &[&str] = &["agent"];

const PLUGIN_ALLOWED_KEYS: &[&str] = &["plugin_id", "confidence", "rationale"];
const PERMISSION_ALLOWED_KEYS: &[&str] = &[
    "subject_kind",
    "subject_id",
    "permission",
    "confidence",
    "rationale",
];

// ---------------------------------------------------------------------------
// Plugin install reference (advisory; the install stays approval-gated)
// ---------------------------------------------------------------------------

/// A validated plugin reference a brain *proposes* for a `PluginInstallation` turn.
#[derive(Debug, Clone, PartialEq)]
pub struct BrainPluginRef {
    pub plugin_id: String,
    pub confidence: f32,
    pub rationale: String,
}

/// The strict prompt for extracting the plugin a user asked Prime to install. The
/// install is approval-gated; this only names the subject the human will review.
pub fn build_plugin_ref_prompt(message: &str) -> String {
    format!(
        "You are extracting the id of a SINGLE plugin the user has asked Prime to install on a \
local Relux control plane. Installing is gated behind a human approval; you perform no action.\n\n\
Respond with JSON ONLY (no prose, no code fences) in EXACTLY this shape:\n\
{{\"plugin_id\":\"<the plugin id, e.g. relux-tools-github>\",\"confidence\":<0.0-1.0>}}\n\n\
Rules:\n\
- plugin_id: the plugin's id (typically prefixed `relux-`). REQUIRED.\n\
- Do NOT add any field other than these. Do NOT claim the plugin was installed.\n\n\
User message:\n{message}"
    )
}

/// Parse a brain reply into a validated [`BrainPluginRef`], or `Err`. Allowlist gate.
pub fn parse_plugin_ref(raw: &str) -> Result<BrainPluginRef, String> {
    let obj = parse_object(raw, PLUGIN_ALLOWED_KEYS)?;

    let plugin_id = sanitize_plugin_id(
        obj.get("plugin_id").and_then(|v| v.as_str()).unwrap_or(""),
    );
    if plugin_id.is_empty() {
        return Err("empty or missing plugin_id".to_string());
    }

    Ok(BrainPluginRef {
        plugin_id,
        confidence: confidence_of(&obj),
        rationale: rationale_of(&obj),
    })
}

/// Reconcile a brain plugin reference against the deterministically-derived plugin id.
/// Returns the normalized plugin id to propose (still approval-gated), or `None` to
/// keep the deterministic one. Low confidence, or a proposal that merely echoes the
/// deterministic id, yields `None`.
pub fn reconcile_plugin_ref(deterministic_plugin: &str, proposal: &BrainPluginRef) -> Option<String> {
    if proposal.confidence < CONFIDENCE_FLOOR {
        return None;
    }
    if proposal.plugin_id.eq_ignore_ascii_case(deterministic_plugin.trim()) {
        return None;
    }
    Some(proposal.plugin_id.clone())
}

// ---------------------------------------------------------------------------
// Permission grant subject (advisory; the grant stays approval-gated)
// ---------------------------------------------------------------------------

/// A validated permission-grant subject a brain *proposes* for a `PermissionChange`
/// turn. `subject_kind` (when present) is already checked against [`SUBJECT_KINDS`].
#[derive(Debug, Clone, PartialEq)]
pub struct BrainPermissionSlots {
    pub subject_kind: Option<String>,
    /// The raw (normalized but NOT yet roster-validated) subject id.
    pub subject_id: Option<String>,
    pub permission: Option<String>,
    pub confidence: f32,
    pub rationale: String,
}

/// The reconciled permission subject the kernel will propose (behind approval).
/// `subject_id` here always names an EXISTING agent.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedPermissionSlots {
    pub subject_kind: String,
    pub subject_id: String,
    pub permission: Option<String>,
}

/// The strict prompt for extracting the subject + permission of a grant request. The
/// grant is approval-gated; this only names the subject the human will review.
pub fn build_permission_slots_prompt(message: &str) -> String {
    format!(
        "You are extracting the subject of a SINGLE permission grant the user has asked Prime to \
make on a local Relux control plane. Granting is gated behind a human approval; you perform no \
action.\n\n\
Respond with JSON ONLY (no prose, no code fences) in EXACTLY this shape:\n\
{{\"subject_kind\":\"agent\",\"subject_id\":\"<the existing agent id>\",\
\"permission\":\"<optional permission label, or omit>\",\"confidence\":<0.0-1.0>}}\n\n\
Rules:\n\
- subject_kind: today only \"agent\" is supported. Omit if unsure.\n\
- subject_id: the id of an agent that ALREADY exists. NEVER invent an agent, tool, or plugin name.\n\
- permission: the permission label being granted (e.g. tool:relux-tools-github:access). Omit if unsure.\n\
- Do NOT add any field other than these. Do NOT claim the permission was granted.\n\n\
User message:\n{message}"
    )
}

/// Parse a brain reply into validated [`BrainPermissionSlots`], or `Err`. Allowlist
/// gate: an unsupported field, or a `subject_kind` outside [`SUBJECT_KINDS`], fails
/// the whole proposal closed.
pub fn parse_permission_slots(raw: &str) -> Result<BrainPermissionSlots, String> {
    let obj = parse_object(raw, PERMISSION_ALLOWED_KEYS)?;

    let subject_kind = match obj.get("subject_kind").and_then(|v| v.as_str()) {
        Some(raw_kind) => {
            let kind = raw_kind.trim().to_lowercase();
            if kind.is_empty() {
                None
            } else if SUBJECT_KINDS.contains(&kind.as_str()) {
                Some(kind)
            } else {
                return Err(format!("unsupported subject_kind '{kind}'"));
            }
        }
        None => None,
    };

    let subject_id = obj
        .get("subject_id")
        .and_then(|v| v.as_str())
        .map(sanitize_id)
        .filter(|s| !s.is_empty());

    let permission = obj
        .get("permission")
        .and_then(|v| v.as_str())
        .map(sanitize_permission)
        .filter(|s| !s.is_empty());

    Ok(BrainPermissionSlots {
        subject_kind,
        subject_id,
        permission,
        confidence: confidence_of(&obj),
        rationale: rationale_of(&obj),
    })
}

/// Reconcile a brain permission subject against the live agent roster. The subject is
/// honored ONLY when it names an EXISTING agent (case-insensitive match against
/// `summary.all_agent_ids`); an unknown/absent subject yields `None` and the
/// deterministic subject stands. Low confidence also yields `None`.
pub fn reconcile_permission_slots(
    proposal: &BrainPermissionSlots,
    summary: &StateSummary,
) -> Option<ResolvedPermissionSlots> {
    if proposal.confidence < CONFIDENCE_FLOOR {
        return None;
    }
    let raw_subject = proposal.subject_id.as_ref()?;
    let subject_id = summary
        .all_agent_ids
        .iter()
        .find(|id| id.eq_ignore_ascii_case(raw_subject))
        .cloned()?;

    Some(ResolvedPermissionSlots {
        // Only "agent" subjects are grantable, and the subject validated against the
        // agent roster, so the kind is "agent" regardless of what the brain labeled.
        subject_kind: "agent".to_string(),
        subject_id,
        permission: proposal.permission.clone(),
    })
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn parse_object(
    raw: &str,
    allowed: &[&str],
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let json = extract_json_object(raw).ok_or_else(|| "no JSON object in reply".to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&json).map_err(|_| "reply was not valid JSON".to_string())?;
    let obj = value
        .as_object()
        .ok_or_else(|| "reply was not a JSON object".to_string())?;
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("unsupported field '{key}'"));
        }
    }
    Ok(obj.clone())
}

fn confidence_of(obj: &serde_json::Map<String, serde_json::Value>) -> f32 {
    obj.get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.5)
        .clamp(0.0, 1.0) as f32
}

fn rationale_of(obj: &serde_json::Map<String, serde_json::Value>) -> String {
    obj.get("rationale")
        .and_then(|v| v.as_str())
        .map(|s| sanitize_keep(s, MAX_RATIONALE_CHARS, |c| !c.is_control()))
        .unwrap_or_default()
}

/// Normalize a plugin id: lowercase, keep `[a-z0-9-]` (`.`/`_`/space → hyphen),
/// collapse and trim hyphens, clamp.
fn sanitize_plugin_id(s: &str) -> String {
    normalize_handle(s, MAX_PLUGIN_CHARS, &['.'])
}

/// Normalize a subject (agent) id: lowercase, keep `[a-z0-9-]`, collapse/trim hyphens.
fn sanitize_id(s: &str) -> String {
    normalize_handle(s, MAX_SUBJECT_CHARS, &[])
}

/// Lowercase, keep `[a-z0-9-]` plus any `extra` separators (mapped to hyphen along
/// with `_`/whitespace), collapse repeats, trim hyphens, clamp.
fn normalize_handle(s: &str, max: usize, extra_seps: &[char]) -> String {
    let lowered = s.trim().to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut last_hyphen = false;
    for c in lowered.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_hyphen = false;
        } else if c == '-' || c == '_' || c.is_whitespace() || extra_seps.contains(&c) {
            if last_hyphen {
                continue;
            }
            last_hyphen = true;
            out.push('-');
        }
        if out.chars().count() >= max {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

/// Sanitize a permission label: lowercase, keep only `[a-z0-9:_-]` (the permission
/// grammar), trim, clamp. Strips any prose/punctuation an injected label might carry.
fn sanitize_permission(s: &str) -> String {
    sanitize_keep(&s.trim().to_lowercase(), MAX_PERMISSION_CHARS, |c| {
        c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-')
    })
}

/// Keep only chars passing `keep`, collapsing whitespace handled by the caller's
/// predicate; clamp to `max`.
fn sanitize_keep(s: &str, max: usize, keep: impl Fn(char) -> bool) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let mapped = if keep(c) {
            c
        } else if c.is_whitespace() {
            ' '
        } else {
            continue;
        };
        out.push(mapped);
        if out.chars().count() >= max {
            break;
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
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
            all_task_ids: Vec::new(),
            queued: Vec::new(),
            recent: Vec::new(),
        }
    }

    // --- plugin ref ----------------------------------------------------------

    #[test]
    fn parses_and_normalizes_a_plugin_ref() {
        let p = parse_plugin_ref(r#"{"plugin_id":"Relux-Tools-GitHub","confidence":0.9}"#).unwrap();
        assert_eq!(p.plugin_id, "relux-tools-github");
    }

    #[test]
    fn plugin_ref_rejects_unsupported_field_and_empty_id() {
        assert!(parse_plugin_ref(r#"{"plugin_id":"x","install":true,"confidence":0.9}"#).is_err());
        assert!(parse_plugin_ref(r#"{"plugin_id":"","confidence":0.9}"#).is_err());
        assert!(parse_plugin_ref("not json").is_err());
    }

    #[test]
    fn plugin_ref_reconcile_sharpens_only_a_confident_distinct_id() {
        let p = parse_plugin_ref(r#"{"plugin_id":"relux-tools-github","confidence":0.9}"#).unwrap();
        assert_eq!(
            reconcile_plugin_ref("(unspecified-plugin)", &p).as_deref(),
            Some("relux-tools-github")
        );
        // Low confidence keeps the deterministic id.
        let low = parse_plugin_ref(r#"{"plugin_id":"relux-tools-github","confidence":0.3}"#).unwrap();
        assert!(reconcile_plugin_ref("(unspecified-plugin)", &low).is_none());
        // An echo of the deterministic id is a no-op.
        let echo = parse_plugin_ref(r#"{"plugin_id":"relux-tools-github","confidence":0.9}"#).unwrap();
        assert!(reconcile_plugin_ref("relux-tools-github", &echo).is_none());
    }

    // --- permission slots ----------------------------------------------------

    #[test]
    fn parses_a_clean_permission_object() {
        let p = parse_permission_slots(
            r#"{"subject_kind":"agent","subject_id":"code-agent","permission":"tool:relux-tools-github:access","confidence":0.9}"#,
        )
        .unwrap();
        assert_eq!(p.subject_kind.as_deref(), Some("agent"));
        assert_eq!(p.subject_id.as_deref(), Some("code-agent"));
        assert_eq!(p.permission.as_deref(), Some("tool:relux-tools-github:access"));
    }

    #[test]
    fn permission_rejects_unsupported_field_and_kind_fail_closed() {
        assert!(parse_permission_slots(
            r#"{"subject_id":"code-agent","grant":true,"confidence":0.9}"#
        )
        .is_err());
        // An unsupported subject_kind fails the whole proposal closed.
        let err = parse_permission_slots(
            r#"{"subject_kind":"plugin","subject_id":"relux-tools-github","confidence":0.9}"#,
        )
        .unwrap_err();
        assert!(err.contains("unsupported subject_kind"), "got: {err}");
    }

    #[test]
    fn permission_sanitizes_the_label() {
        // Prose/punctuation around the permission grammar is stripped.
        let p = parse_permission_slots(
            r#"{"subject_id":"code-agent","permission":"TOOL:relux-tools-github:access!!","confidence":0.9}"#,
        )
        .unwrap();
        assert_eq!(p.permission.as_deref(), Some("tool:relux-tools-github:access"));
    }

    #[test]
    fn permission_reconcile_honors_only_an_existing_subject() {
        let summary = summary_with_agents(&["code-agent"]);

        let known = parse_permission_slots(
            r#"{"subject_id":"code-agent","permission":"tool:relux-tools-github:access","confidence":0.9}"#,
        )
        .unwrap();
        let r = reconcile_permission_slots(&known, &summary).unwrap();
        assert_eq!(r.subject_id, "code-agent");
        assert_eq!(r.subject_kind, "agent");

        // An unknown subject yields None (the deterministic subject stands).
        let unknown = parse_permission_slots(
            r#"{"subject_id":"ghost-agent","confidence":0.9}"#,
        )
        .unwrap();
        assert!(reconcile_permission_slots(&unknown, &summary).is_none());

        // Low confidence yields None.
        let low = parse_permission_slots(
            r#"{"subject_id":"code-agent","confidence":0.3}"#,
        )
        .unwrap();
        assert!(reconcile_permission_slots(&low, &summary).is_none());
    }
}
