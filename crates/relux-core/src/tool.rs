//! Tool invocation surface types - capability discovery + invocation results.
//!
//! Spec ref: `docs/RELUX_MASTER_PLAN.md` section 7.4 (Plugin Kernel Layer:
//! plugin routing/permissions/audit), section 8.2 (ToolSet Plugins), section 9.8
//! (Tool Call entity) and `docs/Relux spec.md` section 10.2 (ToolSet Plugin),
//! section 13.6 (Tool Call Flow).
//!
//! These are pure types. The kernel resolves an installed plugin tool, marks
//! whether the local runtime can actually execute it, and (when invoked) returns
//! a structured result. The honest rule the kernel enforces is: only built-in
//! deterministic handlers run; an installed-but-unimplemented tool is reported as
//! [`ToolExecutability::NotImplemented`] rather than faked.

use serde::{Deserialize, Serialize};

use crate::permission::RiskLevel;

/// Whether the local kernel runtime can actually execute a given installed
/// plugin tool right now.
///
/// This is the heart of the "honest tool surface": discovery never claims a tool
/// is runnable when no deterministic handler backs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutability {
    /// Either a built-in deterministic handler exists, or the backing plugin has
    /// an enabled HTTP loopback runtime configured. If an agent context was
    /// supplied, that agent also holds the tool's permission. The tool can be
    /// invoked.
    Ready,
    /// The backing plugin has no built-in handler and no runtime configured yet.
    /// The operator can make it executable by configuring an HTTP loopback
    /// endpoint for the plugin. Listed honestly as not (yet) executable.
    RuntimeNotConfigured,
    /// The backing plugin has an HTTP loopback runtime configured but it is
    /// disabled. The tool stays discoverable but refuses invocation until the
    /// runtime is re-enabled.
    RuntimeDisabled,
    /// No supported runtime exists for this tool at all (reserved for plugin
    /// kinds the loopback runtime does not cover). Listed honestly instead of
    /// pretending it works.
    NotImplemented,
    /// A specific agent context was supplied and that agent lacks the tool's
    /// required permission. Only reported when discovery is scoped to an agent.
    MissingPermission,
}

/// One installed plugin tool discovered through the kernel, with its current
/// executable status. Returned by capability discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub plugin_id: String,
    pub tool_name: String,
    pub description: String,
    /// The permission string the kernel checks before invoking the tool.
    pub permission: String,
    pub risk: RiskLevel,
    /// How the backing plugin was installed (`Bundled`, `LocalDir`, `Zip`,
    /// `Github`) - rendered from `PluginSourceKind`.
    pub source_kind: String,
    /// Always true for a discovered tool (it comes from an installed plugin);
    /// present so a UI can render an explicit installed marker.
    pub installed: bool,
    /// Whether the backing installed plugin is enabled.
    pub enabled: bool,
    /// Whether the backing plugin is a protected (bundled) fixture.
    pub protected: bool,
    /// Whether the kernel can execute this tool, and why not when it cannot.
    pub executable: ToolExecutability,
}

/// The structured result of invoking a tool through the kernel.
///
/// Only produced on a successful, permission-checked, deterministically-executed
/// invocation; failures surface as `KernelError` (denied / not implemented /
/// unknown) and never fabricate an `output`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocationResult {
    pub plugin_id: String,
    pub tool_name: String,
    /// The agent the kernel attributed the call to (the permission subject).
    pub agent_id: String,
    /// The permission that was checked and satisfied.
    pub permission: String,
    /// The deterministic tool output.
    pub output: serde_json::Value,
}
