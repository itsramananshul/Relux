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

/// True when dev/test fixtures (the echo ToolSet) should be REVEALED on the normal
/// product + Prime surfaces in this process. Off by default; an operator opts in
/// explicitly by exporting `RELUX_DEV_FIXTURES=1` (or `=true`). This is the single
/// master switch the user asked for: with it unset, a loop-prover is never shown
/// as a real capability anywhere; with it set, dev/test runs see the fixtures.
pub fn dev_fixtures_enabled() -> bool {
    std::env::var("RELUX_DEV_FIXTURES")
        .map(|v| {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

/// Pure composition of [`is_internal_plugin`] and the dev-fixtures switch, factored
/// out so it can be unit-tested without touching process env. A plugin is a HIDDEN
/// fixture when it is an internal fixture AND dev fixtures are not enabled.
pub fn is_hidden_fixture_when(plugin_id: &str, dev_fixtures_enabled: bool) -> bool {
    is_internal_plugin(plugin_id) && !dev_fixtures_enabled
}

/// True when this plugin must be hidden from user-facing AND Prime-facing surfaces
/// in the current process: an internal dev/test fixture with `RELUX_DEV_FIXTURES`
/// not set. Use this (not the bare [`is_internal_plugin`]) at every surface that
/// offers a capability to an operator or to Prime's brain, so the master switch is
/// honored consistently. The governed EXECUTION path is intentionally left alone:
/// an explicitly named fixture still resolves + runs through the unchanged gate, so
/// the internal smoke/test harness exercises the real tool/run path.
pub fn is_hidden_fixture(plugin_id: &str) -> bool {
    is_hidden_fixture_when(plugin_id, dev_fixtures_enabled())
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

    #[test]
    fn hidden_fixture_respects_the_dev_switch() {
        // Default (dev fixtures OFF): the echo fixture is hidden everywhere the
        // helper gates, so Prime/operators never see it as a real ability.
        assert!(is_hidden_fixture_when("relux-tools-echo", false));
        // Dev fixtures ON: the fixture is revealed for dev/test runs.
        assert!(!is_hidden_fixture_when("relux-tools-echo", true));
        // A genuine capability is never hidden, regardless of the switch.
        assert!(!is_hidden_fixture_when("relux-tools-status", false));
        assert!(!is_hidden_fixture_when("relux-tools-status", true));
    }
}
