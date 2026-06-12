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
