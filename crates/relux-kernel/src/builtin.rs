//! The built-in deterministic tool runtime.
//!
//! Standalone Relux does NOT execute arbitrary downloaded plugin code yet
//! (`docs/RELUX_MASTER_PLAN.md` section 8.2, section 18: no shelling out, no
//! filesystem/network side effects from installed plugins). The first honest
//! tool-invocation surface executes only a small, fixed set of safe,
//! deterministic handlers that ship with the kernel. Every other installed
//! plugin tool is discoverable but reported as "runtime not implemented" rather
//! than faked.
//!
//! This module is the single source of truth for "which (plugin, tool) pairs can
//! the kernel actually run". The actual handler bodies live on
//! [`crate::KernelState`] (some need read access to control-plane state), but the
//! membership test here gates them and keeps discovery honest.

/// The `(plugin_id, tool_name)` pairs the kernel can execute itself.
///
/// Keep this in sync with `KernelState::builtin_tool_output`, which holds the
/// matching deterministic handler bodies. A pair listed here MUST have a handler
/// there, and vice versa.
pub const BUILTIN_TOOLS: &[(&str, &str)] = &[
    // The bundled echo tool: returns its input unchanged. Proves the loop with no
    // external effect.
    ("relux-tools-echo", "echo.say"),
    // The bundled status tool: returns a deterministic summary of control-plane
    // counts. Read-only, no network or filesystem access.
    ("relux-tools-status", "status.summary"),
];

/// True when the kernel has a built-in deterministic handler for this tool.
pub fn is_builtin_tool(plugin_id: &str, tool_name: &str) -> bool {
    BUILTIN_TOOLS
        .iter()
        .any(|(p, t)| *p == plugin_id && *t == tool_name)
}

/// Plugins that exist only as internal dev/test fixtures and must NEVER be shown
/// as a user-facing capability.
///
/// The echo ToolSet (`echo.say`) is a trivial loop-prover with no product value:
/// it returns its input unchanged. It stays installed so the internal test
/// harness and the dev smoke can exercise the tool/run path, but it is hidden
/// from every normal product surface (the Plugins list, the Tools list, and
/// Prime's "what tools can you use?" catalogue) so an operator never mistakes it
/// for a real ability. Dev/test callers opt back in explicitly (e.g. the
/// `?include_internal=true` API flag).
pub const INTERNAL_PLUGIN_IDS: &[&str] = &["relux-tools-echo"];

/// True when a plugin is an internal dev/test fixture that should be hidden from
/// user-facing product surfaces by default.
pub fn is_internal_plugin(plugin_id: &str) -> bool {
    INTERNAL_PLUGIN_IDS.contains(&plugin_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_and_status_are_builtin_others_are_not() {
        assert!(is_builtin_tool("relux-tools-echo", "echo.say"));
        assert!(is_builtin_tool("relux-tools-status", "status.summary"));
        assert!(!is_builtin_tool("relux-tools-github", "github.create_pr"));
        assert!(!is_builtin_tool("relux-tools-echo", "echo.shout"));
    }

    #[test]
    fn echo_is_internal_status_is_not() {
        // The echo fixture is hidden from user-facing surfaces; the read-only
        // status tool is a genuine capability and stays visible.
        assert!(is_internal_plugin("relux-tools-echo"));
        assert!(!is_internal_plugin("relux-tools-status"));
        assert!(!is_internal_plugin("relux-adapter-claude-cli"));
    }
}
