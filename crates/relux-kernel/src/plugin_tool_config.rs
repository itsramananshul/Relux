//! Operator-supplied, VALIDATED tool definitions for a metadata-only plugin
//! wrapper — the first safe in-UI path to make an installed wrapper useful.
//!
//! ## Why this exists
//!
//! A source installed without a `relux-plugin.json` is scaffolded as a
//! metadata-only wrapper that declares ZERO tools
//! ([`crate::plugin_install::scaffold_manifest`]). That is safe, but a wrapper
//! cannot do anything: with no tool definitions, even an enabled HTTP loopback
//! runtime surfaces nothing (`crate::server` pins this with
//! `enabling_a_runtime_on_a_wrapper_surfaces_no_tools`). The only prior path to
//! add tools was to hand-edit the on-disk manifest and re-install. This module is
//! the kernel half of an in-UI "add a tool" form: the operator describes ONE tool
//! and the kernel validates it hard before it enters the manifest
//! (`docs/RELUX_MASTER_PLAN.md` §7.4 Plugin Kernel Layer, §8.2 ToolSet Plugins).
//!
//! ## Reference-driven design (see `docs/reference-driven-development.md`)
//!
//! This is the openclaw "validate a structured payload field-by-field against an
//! explicit schema + status allowlist, reject unsupported keys, require/trim the
//! mandatory string, clamp the rest" pattern, read first:
//!
//! - **openclaw** `src/agents/tools/update-plan-tool.ts` (`readPlanSteps`, the
//!   `PLAN_STEP_STATUSES` allowlist, L9/L39-74): validate each field, check an
//!   enum against an ALLOWLIST, fail closed on a bad value. We mirror it:
//!   [`parse_plugin_tool_input`] validates `risk` against [`RISK_LEVELS`].
//! - **openclaw** `src/agents/tools/sessions-spawn-tool.ts`
//!   (`UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS`, L46-55, rejected before any param is
//!   read; the `Math.max(0, Math.floor(...))` clamps): reject unsupported keys up
//!   front and clamp ranges. We mirror it: any key outside [`ALLOWED_KEYS`] fails
//!   the whole payload closed; the timeout is clamped to `[1, 300]`.
//! - **openclaw** `src/agents/tools/common.ts` (`readStringParam` required-throws,
//!   `ToolInputError`, L57-122): a required string THROWS on bad input rather than
//!   coercing silently. We mirror it: an empty/missing `name` is a hard error.
//!
//! ## The safety contract (binding)
//!
//! - The operator NEVER supplies a raw permission. The kernel DERIVES it as
//!   `tool:<plugin-id>:<verb>` so a configured tool can only ever gate on this
//!   plugin's own `tool:` namespace — no smuggling a different prefix/resource.
//! - `risk` drives the approval requirement, and that requirement is now
//!   LOAD-BEARING (`crate::state` refuses a tool whose approval blocks a direct
//!   invocation). A non-low-risk tool is `Required` → not runnable through the
//!   direct call/invoke path; a low-risk tool is auto-approved ONLY when the
//!   operator opts in (default for low risk), so a freshly-added risky tool can
//!   never be runnable just because a loopback runtime is enabled.
//! - Every string is sanitized (control chars stripped) and length-clamped; an
//!   unsupported field fails the whole payload closed.
//!
//! This module performs NO I/O and holds no kernel state; it is the pure parser
//! the kernel calls before mutating a manifest.

use relux_core::permission::{ApprovalRequirement, Permission, RiskLevel, ToolDefinition};

/// The only fields a tool-config payload may carry. Any other key fails the
/// payload closed (openclaw's `UNSUPPORTED_*_PARAM_KEYS` rejection) — an operator
/// (or a forged request) may not smuggle a raw `permission`/`approval` field in to
/// bypass the derived-permission / risk-driven-approval rules.
pub const ALLOWED_KEYS: &[&str] = &[
    "name",
    "description",
    "risk",
    "auto_approve",
    "timeout_secs",
];

/// The accepted risk levels (the `RiskLevel` allowlist), lower-cased on the wire.
pub const RISK_LEVELS: &[&str] = &["low", "medium", "high", "critical"];

/// Max characters kept for a tool name before it is sanitized into a dotted id.
const MAX_NAME_CHARS: usize = 64;
/// Max characters kept for a tool description.
const MAX_DESCRIPTION_CHARS: usize = 600;
/// Inclusive per-call timeout range (seconds). Matches the plugin manifest's
/// `invoke_timeout_secs` bound in `docs/plugins.md`.
const TIMEOUT_MIN_SECS: u32 = 1;
const TIMEOUT_MAX_SECS: u32 = 300;

/// A validated operator request to add/replace ONE tool on a plugin manifest.
///
/// Only [`parse_plugin_tool_input`] builds this, and only after rejecting unknown
/// fields, sanitizing every string, and clamping ranges. `name` is guaranteed a
/// non-empty, safe dotted id; `verb` is the permission action derived from it.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginToolInput {
    /// The sanitized, dotted tool name (e.g. `report.fetch`). Non-empty.
    pub name: String,
    /// The permission action segment derived from `name` (e.g. `fetch`).
    pub verb: String,
    pub description: String,
    pub risk: RiskLevel,
    /// Whether the operator opted into auto-approval. Only honored for low risk
    /// (a non-low-risk tool is always approval-`Required`). `None` ⇒ use the
    /// default (auto for low risk).
    pub auto_approve: Option<bool>,
    pub timeout_secs: Option<u32>,
}

impl PluginToolInput {
    /// The [`ApprovalRequirement`] this input maps to: a low-risk tool is
    /// auto-approved unless the operator explicitly turned that off; any non-low
    /// risk is always `Required` (the operator cannot auto-approve a risky tool).
    /// This is the rule behind "a newly configured tool remains disabled / requires
    /// explicit enable if risk is not low."
    pub fn approval(&self) -> ApprovalRequirement {
        let auto = self.risk == RiskLevel::Low && self.auto_approve != Some(false);
        if auto {
            ApprovalRequirement::Never
        } else {
            ApprovalRequirement::Required
        }
    }

    /// Build the [`ToolDefinition`] for `plugin_id`, deriving the permission as
    /// `tool:<plugin-id>:<verb>` (never operator-supplied). Returns an error only
    /// if the derived permission is malformed, which cannot happen for a safe
    /// plugin id + sanitized verb but is checked rather than `unwrap`ped.
    pub fn into_tool_definition(self, plugin_id: &str) -> Result<ToolDefinition, String> {
        let permission = Permission::new(format!("tool:{plugin_id}:{}", self.verb))
            .map_err(|e| format!("derived permission invalid: {e}"))?;
        let approval = self.approval();
        Ok(ToolDefinition {
            name: self.name,
            description: self.description,
            risk: self.risk,
            permission,
            approval,
            timeout_secs: self.timeout_secs,
        })
    }
}

/// Parse + validate an operator tool-config payload into a [`PluginToolInput`], or
/// `Err` with a short, operator-facing reason on anything malformed/unsupported.
///
/// The payload must be a JSON object, every key must be in [`ALLOWED_KEYS`] (an
/// unsupported field fails closed), `name` must sanitize to a non-empty dotted id,
/// `risk` (when present) must be in [`RISK_LEVELS`], and every value is sanitized
/// and clamped.
pub fn parse_plugin_tool_input(value: &serde_json::Value) -> Result<PluginToolInput, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "tool config must be a JSON object".to_string())?;

    // Reject unknown / unsupported fields outright (fail closed).
    for key in obj.keys() {
        if !ALLOWED_KEYS.contains(&key.as_str()) {
            return Err(format!("unsupported field '{key}'"));
        }
    }

    // name is required and must sanitize to a non-empty dotted id.
    let raw_name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "tool name is required".to_string())?;
    let name = sanitize_tool_name(raw_name, MAX_NAME_CHARS);
    if name.is_empty() {
        return Err("tool name is empty after sanitizing; use letters, digits, '.', '-' or '_'".to_string());
    }
    let verb = derive_verb(&name);
    if verb.is_empty() {
        return Err("tool name has no usable verb segment for a permission".to_string());
    }

    let description = obj
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| sanitize_block(s, MAX_DESCRIPTION_CHARS))
        .unwrap_or_default();

    let risk = match obj.get("risk") {
        None | Some(serde_json::Value::Null) => RiskLevel::Low,
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| "risk must be a string".to_string())?
                .trim()
                .to_ascii_lowercase();
            parse_risk(&s)
                .ok_or_else(|| format!("risk must be one of {}", RISK_LEVELS.join(", ")))?
        }
    };

    let auto_approve = match obj.get("auto_approve") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Bool(b)) => Some(*b),
        Some(_) => return Err("auto_approve must be a boolean".to_string()),
    };

    let timeout_secs = coerce_timeout(obj.get("timeout_secs"))?;

    Ok(PluginToolInput {
        name,
        verb,
        description,
        risk,
        auto_approve,
        timeout_secs,
    })
}

/// Map a lower-cased risk string to a [`RiskLevel`], or `None` if off the allowlist.
fn parse_risk(s: &str) -> Option<RiskLevel> {
    match s {
        "low" => Some(RiskLevel::Low),
        "medium" => Some(RiskLevel::Medium),
        "high" => Some(RiskLevel::High),
        "critical" => Some(RiskLevel::Critical),
        _ => None,
    }
}

/// Coerce a JSON timeout value (number or numeric string, seconds) to a clamped
/// `u32`, or `None` when absent. A present-but-non-numeric value is a hard error
/// (the operator typed something that is not a number), while absence is fine.
fn coerce_timeout(value: Option<&serde_json::Value>) -> Result<Option<u32>, String> {
    let raw = match value {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(serde_json::Value::Number(n)) => n.as_f64(),
        Some(serde_json::Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                return Ok(None);
            }
            t.parse::<f64>().ok()
        }
        Some(_) => return Err("timeout_secs must be a number of seconds".to_string()),
    };
    let raw = raw.ok_or_else(|| "timeout_secs must be a number of seconds".to_string())?;
    if !raw.is_finite() || raw < 0.0 {
        return Err("timeout_secs must be a positive number of seconds".to_string());
    }
    let clamped = raw
        .round()
        .clamp(TIMEOUT_MIN_SECS as f64, TIMEOUT_MAX_SECS as f64);
    Ok(Some(clamped as u32))
}

/// Sanitize a tool name into a safe, dotted lowercase id: keep `[a-z0-9._-]`
/// (everything else, including whitespace, becomes `-`), collapse repeated
/// separators, trim leading/trailing separators, and clamp to `max` characters.
/// The result can only ever contain characters that are safe in a permission
/// string and a manifest, so a sanitized name can never carry an injection.
pub(crate) fn sanitize_tool_name(s: &str, max: usize) -> String {
    let lowered = s.trim().to_ascii_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut last_sep = false;
    for c in lowered.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
            last_sep = false;
        } else if c == '.' {
            // Preserve dots (the `namespace.verb` shape) but never repeat them.
            if !last_sep && !out.is_empty() {
                out.push('.');
                last_sep = true;
            }
        } else {
            // Any other char (including whitespace and '-') collapses to a hyphen.
            if !last_sep && !out.is_empty() {
                out.push('-');
                last_sep = true;
            }
        }
        if out.chars().count() >= max {
            break;
        }
    }
    out.trim_matches(|c| c == '.' || c == '-').to_string()
}

/// Derive the permission action ("verb") from a sanitized dotted name: the segment
/// after the last `.`, reduced to `[a-z0-9_]` (hyphens become underscores so the
/// verb is a clean identifier). Falls back to the whole flattened name when there
/// is no dotted segment.
pub(crate) fn derive_verb(name: &str) -> String {
    let tail = name.rsplit('.').next().unwrap_or(name);
    let candidate = if tail.is_empty() { name } else { tail };
    let verb: String = candidate
        .chars()
        .map(|c| if c == '-' { '_' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    verb.trim_matches('_').to_string()
}

/// Sanitize a multi-line description: drop control chars except `\n`, collapse
/// intra-line whitespace, drop blank lines, trim, and clamp to `max` characters.
pub(crate) fn sanitize_block(s: &str, max: usize) -> String {
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

    fn parse(json: &str) -> Result<PluginToolInput, String> {
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        parse_plugin_tool_input(&v)
    }

    #[test]
    fn parses_a_clean_low_risk_tool() {
        let p = parse(r#"{"name":"report.fetch","description":"Fetch a report.","risk":"low"}"#)
            .unwrap();
        assert_eq!(p.name, "report.fetch");
        assert_eq!(p.verb, "fetch");
        assert_eq!(p.description, "Fetch a report.");
        assert_eq!(p.risk, RiskLevel::Low);
        // Low risk defaults to auto-approve (Never).
        assert_eq!(p.approval(), ApprovalRequirement::Never);
    }

    #[test]
    fn name_is_required_and_sanitized() {
        assert!(parse(r#"{"description":"x"}"#).is_err());
        assert!(parse(r#"{"name":"   "}"#).is_err());
        assert!(parse(r#"{"name":"!!!"}"#).is_err());
        // Whitespace / punctuation collapse to a safe dotted id.
        let p = parse(r#"{"name":"My Cool  Tool"}"#).unwrap();
        assert_eq!(p.name, "my-cool-tool");
        assert_eq!(p.verb, "my_cool_tool");
    }

    #[test]
    fn rejects_unsupported_fields_fail_closed() {
        // A smuggled raw permission/approval (or any unknown key) fails the whole
        // payload closed — the operator cannot bypass the derived-permission rule.
        assert!(parse(r#"{"name":"x.run","permission":"exec:host:shell"}"#)
            .unwrap_err()
            .contains("unsupported field"));
        assert!(parse(r#"{"name":"x.run","approval":"never"}"#).is_err());
    }

    #[test]
    fn risk_is_validated_against_the_allowlist() {
        assert!(parse(r#"{"name":"x.run","risk":"medium"}"#).is_ok());
        assert!(parse(r#"{"name":"x.run","risk":"nuclear"}"#).is_err());
        // Default risk is low when omitted.
        assert_eq!(parse(r#"{"name":"x.run"}"#).unwrap().risk, RiskLevel::Low);
    }

    #[test]
    fn non_low_risk_is_always_required_even_with_auto_approve() {
        let p = parse(r#"{"name":"x.run","risk":"high","auto_approve":true}"#).unwrap();
        // The operator cannot auto-approve a risky tool.
        assert_eq!(p.approval(), ApprovalRequirement::Required);
    }

    #[test]
    fn low_risk_auto_approve_can_be_turned_off() {
        let p = parse(r#"{"name":"x.run","risk":"low","auto_approve":false}"#).unwrap();
        assert_eq!(p.approval(), ApprovalRequirement::Required);
    }

    #[test]
    fn timeout_is_coerced_and_clamped() {
        assert_eq!(parse(r#"{"name":"x.run","timeout_secs":30}"#).unwrap().timeout_secs, Some(30));
        assert_eq!(parse(r#"{"name":"x.run","timeout_secs":"45"}"#).unwrap().timeout_secs, Some(45));
        assert_eq!(parse(r#"{"name":"x.run","timeout_secs":99999}"#).unwrap().timeout_secs, Some(300));
        assert_eq!(parse(r#"{"name":"x.run","timeout_secs":0}"#).unwrap().timeout_secs, Some(1));
        assert_eq!(parse(r#"{"name":"x.run"}"#).unwrap().timeout_secs, None);
        assert!(parse(r#"{"name":"x.run","timeout_secs":"soon"}"#).is_err());
    }

    #[test]
    fn derives_a_permission_scoped_to_the_plugin() {
        let p = parse(r#"{"name":"report.fetch","risk":"low"}"#).unwrap();
        let def = p.into_tool_definition("relux-plugin-my-repo").unwrap();
        assert_eq!(def.permission.as_str(), "tool:relux-plugin-my-repo:fetch");
        assert_eq!(def.approval, ApprovalRequirement::Never);
    }

    #[test]
    fn description_is_clamped_and_control_chars_stripped() {
        let long = "a".repeat(1000);
        let json = format!(r#"{{"name":"x.run","description":"{long}"}}"#);
        let p = parse(&json).unwrap();
        assert_eq!(p.description.chars().count(), MAX_DESCRIPTION_CHARS);
    }
}
