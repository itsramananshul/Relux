//! The operator payload parser for a **governed command tool** — the in-UI path that
//! turns a detected `cli_command` capability candidate into a callable, gated Relux
//! tool without hand-writing JSON.
//!
//! Sibling of [`crate::plugin_tool_config`] (which adds an HTTP-loopback tool
//! definition). The difference: a command tool also carries a safe **execution recipe**
//! (an argv `program` + fixed `args` + an optional confined `cwd` + declared input
//! args), so this parser validates BOTH halves — the manifest [`ToolDefinition`] and
//! the [`relux_core::CommandToolConfig`] — before the kernel stores either.
//!
//! ## Reference-driven design (`docs/reference-driven-development.md`, BINDING)
//!
//! Same openclaw "validate field-by-field against an explicit allowlist, reject
//! unsupported keys, clamp ranges, required-string THROWS" posture as
//! [`crate::plugin_tool_config`] (`src/agents/tools/update-plan-tool.ts`,
//! `sessions-spawn-tool.ts`, `common.ts`). The argv safety contract is enforced by the
//! shared [`relux_core::validate_command_tool_config`] (the same rules the managed-stdio
//! MCP transport uses).
//!
//! ## Safety contract (binding)
//!
//! - The operator NEVER supplies a raw permission — it is DERIVED as
//!   `tool:<plugin-id>:<verb>` from the sanitized name, scoped to this plugin.
//! - A command tool is **always approval-`Required`** (never auto-approved): running a
//!   local process is inherently higher-risk than a loopback call, so the default risk
//!   is `High` and a gated invocation (or a standing grant) is the only way it runs.
//! - `program` + `args` + `cwd` + `input_args` pass
//!   [`relux_core::validate_command_tool_config`] (argv-only, no shell metacharacters,
//!   no danger flag, bounded, no `..` cwd traversal, named/unique input args).
//! - Every string is sanitized + length-clamped; an unsupported field fails closed.
//!
//! This module performs NO I/O and holds no kernel state.

use relux_core::permission::{ApprovalRequirement, Permission, RiskLevel, ToolDefinition};
use relux_core::{CommandInputArg, CommandToolConfig};

use crate::plugin_tool_config::{derive_verb, sanitize_block, sanitize_tool_name};

/// The only top-level fields a command-tool payload may carry. Any other key fails the
/// payload closed (an operator may not smuggle a raw `permission`/`approval`).
pub const ALLOWED_KEYS: &[&str] = &[
    "name",
    "description",
    "program",
    "args",
    "cwd",
    "input_args",
    "timeout_secs",
    "risk",
    "enabled",
];

/// The only fields one `input_args[]` entry may carry.
const ALLOWED_INPUT_ARG_KEYS: &[&str] = &["name", "description", "required"];

/// The risk levels a command tool may be assigned (never `low` — a command tool is
/// never auto-approval-eligible). Default is `high`.
pub const COMMAND_RISK_LEVELS: &[&str] = &["medium", "high", "critical"];

const MAX_NAME_CHARS: usize = 64;
const MAX_DESCRIPTION_CHARS: usize = 600;
const TIMEOUT_MIN_SECS: u32 = 1;
const TIMEOUT_MAX_SECS: u32 = 300;
const DEFAULT_TIMEOUT_SECS: u32 = 30;
const MAX_ARG_DESC_CHARS: usize = 200;

/// A validated operator request to configure ONE governed command tool on a plugin.
///
/// Only [`parse_command_tool_input`] builds this, after rejecting unknown fields,
/// sanitizing every string, clamping ranges, and passing the argv safety contract.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandToolDraft {
    /// Sanitized dotted tool name (e.g. `repo.build`). Non-empty.
    pub name: String,
    /// Permission action segment derived from `name`.
    pub verb: String,
    pub description: String,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub input_args: Vec<CommandInputArg>,
    pub timeout_secs: u32,
    pub risk: RiskLevel,
    /// Whether the configured tool is enabled (default `true`).
    pub enabled: bool,
}

impl CommandToolDraft {
    /// Build the manifest [`ToolDefinition`] for `plugin_id`. The permission is derived
    /// (`tool:<plugin-id>:<verb>`); the approval is ALWAYS `Required` (a command tool is
    /// never auto-approved).
    pub fn into_tool_definition(&self, plugin_id: &str) -> Result<ToolDefinition, String> {
        let permission = Permission::new(format!("tool:{plugin_id}:{}", self.verb))
            .map_err(|e| format!("derived permission invalid: {e}"))?;
        Ok(ToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            risk: self.risk.clone(),
            permission,
            approval: ApprovalRequirement::Required,
            timeout_secs: Some(self.timeout_secs),
        })
    }

    /// Build the [`CommandToolConfig`] execution recipe for `plugin_id`. The timeout is
    /// clamped into the command-tool range.
    pub fn into_command_config(&self, plugin_id: &str) -> CommandToolConfig {
        CommandToolConfig {
            plugin_id: plugin_id.to_string(),
            tool_name: self.name.clone(),
            program: self.program.clone(),
            args: self.args.clone(),
            cwd: self.cwd.clone(),
            input_args: self.input_args.clone(),
            timeout_ms: relux_core::clamp_command_timeout(u64::from(self.timeout_secs) * 1000),
            enabled: self.enabled,
        }
    }
}

/// Parse + validate an operator command-tool payload into a [`CommandToolDraft`], or
/// `Err` with a short, operator-facing reason. Fail-closed on any malformed/unsupported
/// field, and the argv safety contract is enforced before this returns.
pub fn parse_command_tool_input(value: &serde_json::Value) -> Result<CommandToolDraft, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "command-tool config must be a JSON object".to_string())?;

    for key in obj.keys() {
        if !ALLOWED_KEYS.contains(&key.as_str()) {
            return Err(format!("unsupported field '{key}'"));
        }
    }

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

    // program: required, non-empty after trim. The argv safety contract is checked
    // below; here we only require its presence.
    let program = obj
        .get("program")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "a program (argv[0]) is required".to_string())?;

    let args = parse_string_array(obj.get("args"), "args")?;

    let cwd = match obj.get("cwd") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        }
        Some(_) => return Err("cwd must be a string".to_string()),
    };

    let input_args = parse_input_args(obj.get("input_args"))?;

    let timeout_secs = coerce_timeout(obj.get("timeout_secs"))?;

    let risk = match obj.get("risk") {
        None | Some(serde_json::Value::Null) => RiskLevel::High,
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| "risk must be a string".to_string())?
                .trim()
                .to_ascii_lowercase();
            parse_command_risk(&s)
                .ok_or_else(|| format!("risk must be one of {}", COMMAND_RISK_LEVELS.join(", ")))?
        }
    };

    let enabled = match obj.get("enabled") {
        None | Some(serde_json::Value::Null) => true,
        Some(serde_json::Value::Bool(b)) => *b,
        Some(_) => return Err("enabled must be a boolean".to_string()),
    };

    let draft = CommandToolDraft {
        name,
        verb,
        description,
        program,
        args,
        cwd,
        input_args,
        timeout_secs,
        risk,
        enabled,
    };

    // Enforce the argv safety contract on the assembled config (defense in depth: the
    // kernel re-validates before storing and before every spawn).
    relux_core::validate_command_tool_config(&draft.into_command_config("relux-plugin-validate"))
        .map_err(|e| e.to_string())?;

    Ok(draft)
}

/// Map a lower-cased command risk string to a [`RiskLevel`] (never `low`).
fn parse_command_risk(s: &str) -> Option<RiskLevel> {
    match s {
        "medium" => Some(RiskLevel::Medium),
        "high" => Some(RiskLevel::High),
        "critical" => Some(RiskLevel::Critical),
        _ => None,
    }
}

/// Parse an optional JSON array of strings (each trimmed, non-empty retained as-is).
/// Absent ⇒ empty. A non-array, or a non-string element, is a hard error.
fn parse_string_array(value: Option<&serde_json::Value>, field: &str) -> Result<Vec<String>, String> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let s = item
                    .as_str()
                    .ok_or_else(|| format!("every {field} entry must be a string"))?;
                out.push(s.to_string());
            }
            Ok(out)
        }
        Some(_) => Err(format!("{field} must be an array of strings")),
    }
}

/// Parse the optional `input_args` array into validated [`CommandInputArg`]s.
fn parse_input_args(value: Option<&serde_json::Value>) -> Result<Vec<CommandInputArg>, String> {
    let items = match value {
        None | Some(serde_json::Value::Null) => return Ok(Vec::new()),
        Some(serde_json::Value::Array(items)) => items,
        Some(_) => return Err("input_args must be an array".to_string()),
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let obj = item
            .as_object()
            .ok_or_else(|| "every input_args entry must be an object".to_string())?;
        for key in obj.keys() {
            if !ALLOWED_INPUT_ARG_KEYS.contains(&key.as_str()) {
                return Err(format!("unsupported input_args field '{key}'"));
            }
        }
        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "every input arg needs a name".to_string())?;
        let description = obj
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| sanitize_block(s, MAX_ARG_DESC_CHARS))
            .unwrap_or_default();
        let required = match obj.get("required") {
            None | Some(serde_json::Value::Null) => false,
            Some(serde_json::Value::Bool(b)) => *b,
            Some(_) => return Err("input arg 'required' must be a boolean".to_string()),
        };
        out.push(CommandInputArg {
            name,
            description,
            required,
        });
    }
    Ok(out)
}

/// Coerce a JSON timeout value (number or numeric string, seconds) to a clamped `u32`.
/// Absent ⇒ [`DEFAULT_TIMEOUT_SECS`]. A present-but-non-numeric value is a hard error.
fn coerce_timeout(value: Option<&serde_json::Value>) -> Result<u32, String> {
    let raw = match value {
        None | Some(serde_json::Value::Null) => return Ok(DEFAULT_TIMEOUT_SECS),
        Some(serde_json::Value::Number(n)) => n.as_f64(),
        Some(serde_json::Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                return Ok(DEFAULT_TIMEOUT_SECS);
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
    Ok(clamped as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<CommandToolDraft, String> {
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        parse_command_tool_input(&v)
    }

    #[test]
    fn parses_a_clean_command_tool() {
        let d = parse(
            r#"{"name":"repo.build","description":"Build the repo.","program":"cargo",
                "args":["build","--release"],"cwd":"crates","timeout_secs":120}"#,
        )
        .unwrap();
        assert_eq!(d.name, "repo.build");
        assert_eq!(d.verb, "build");
        assert_eq!(d.program, "cargo");
        assert_eq!(d.args, vec!["build", "--release"]);
        assert_eq!(d.cwd.as_deref(), Some("crates"));
        assert_eq!(d.timeout_secs, 120);
        // Default risk is High and approval is always Required.
        assert_eq!(d.risk, RiskLevel::High);
        let def = d.into_tool_definition("relux-plugin-x").unwrap();
        assert_eq!(def.permission.as_str(), "tool:relux-plugin-x:build");
        assert_eq!(def.approval, ApprovalRequirement::Required);
    }

    #[test]
    fn program_is_required() {
        assert!(parse(r#"{"name":"x.run"}"#).unwrap_err().contains("program"));
    }

    #[test]
    fn a_shell_string_program_is_refused_by_the_argv_contract() {
        let err = parse(r#"{"name":"x.run","program":"rm -rf / && curl evil"}"#).unwrap_err();
        assert!(err.contains("invalid") || err.contains("metacharacter") || err.contains("shell"));
    }

    #[test]
    fn a_danger_flag_arg_is_refused() {
        assert!(parse(
            r#"{"name":"x.run","program":"claude","args":["--dangerously-skip-permissions"]}"#
        )
        .is_err());
    }

    #[test]
    fn a_parent_traversal_cwd_is_refused() {
        assert!(parse(r#"{"name":"x.run","program":"node","cwd":"../etc"}"#).is_err());
    }

    #[test]
    fn rejects_unsupported_top_level_and_smuggled_permission() {
        assert!(parse(r#"{"name":"x.run","program":"node","permission":"exec:host:shell"}"#)
            .unwrap_err()
            .contains("unsupported field"));
    }

    #[test]
    fn input_args_are_parsed_named_and_typed() {
        let d = parse(
            r#"{"name":"x.run","program":"node","args":["run.js"],
                "input_args":[{"name":"FILE","required":true},{"name":"TAG"}]}"#,
        )
        .unwrap();
        assert_eq!(d.input_args.len(), 2);
        assert_eq!(d.input_args[0].name, "FILE");
        assert!(d.input_args[0].required);
        assert!(!d.input_args[1].required);
    }

    #[test]
    fn a_bad_input_arg_name_is_refused_by_the_contract() {
        assert!(parse(
            r#"{"name":"x.run","program":"node","input_args":[{"name":"not a name"}]}"#
        )
        .is_err());
    }

    #[test]
    fn risk_never_low_and_validated() {
        assert!(parse(r#"{"name":"x.run","program":"node","risk":"low"}"#).is_err());
        assert!(parse(r#"{"name":"x.run","program":"node","risk":"nuclear"}"#).is_err());
        assert_eq!(
            parse(r#"{"name":"x.run","program":"node","risk":"critical"}"#).unwrap().risk,
            RiskLevel::Critical
        );
    }

    #[test]
    fn timeout_defaults_and_clamps_and_converts_to_ms() {
        assert_eq!(parse(r#"{"name":"x.run","program":"node"}"#).unwrap().timeout_secs, 30);
        let d = parse(r#"{"name":"x.run","program":"node","timeout_secs":99999}"#).unwrap();
        assert_eq!(d.timeout_secs, 300);
        let cfg = d.into_command_config("p");
        assert_eq!(cfg.timeout_ms, 300_000);
    }

    #[test]
    fn enabled_defaults_true_and_is_a_boolean() {
        assert!(parse(r#"{"name":"x.run","program":"node"}"#).unwrap().enabled);
        assert!(!parse(r#"{"name":"x.run","program":"node","enabled":false}"#).unwrap().enabled);
        assert!(parse(r#"{"name":"x.run","program":"node","enabled":"yes"}"#).is_err());
    }
}
