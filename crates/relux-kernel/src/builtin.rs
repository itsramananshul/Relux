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
}
