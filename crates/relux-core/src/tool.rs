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
    /// The tool declares an approval requirement that is not satisfied for a
    /// direct invocation (e.g. an operator-configured non-low-risk tool whose
    /// `approval` is `Required`). The tool stays discoverable but the kernel
    /// refuses to run it through the direct call/invoke path until the
    /// requirement is removed (or, in future, a per-call approval is granted).
    /// This is what keeps a freshly-configured risky tool from being runnable
    /// just because a loopback runtime is enabled.
    NeedsApproval,
}

/// Whether a tool's declared [`ApprovalRequirement`] blocks a direct
/// (no-approval-flow) invocation given its [`RiskLevel`].
///
/// The kernel does not yet wire a per-tool-call approval flow, so a tool that
/// requires approval cannot be run through the direct call/invoke path. This is
/// the single, fail-closed predicate behind both the [`ToolExecutability::NeedsApproval`]
/// discovery status and the runtime refusal in `call_tool`/`invoke_tool`:
///
/// - `Never` → never blocked.
/// - `Required` → always blocked.
/// - `RequiredWhenRisk(threshold)` → blocked when the tool's risk is at or above
///   the threshold.
///
/// All bundled fixtures declare `Never`, so this never changes their behavior.
pub fn approval_blocks_direct_invocation(
    approval: &crate::permission::ApprovalRequirement,
    risk: &RiskLevel,
) -> bool {
    use crate::permission::ApprovalRequirement;
    match approval {
        ApprovalRequirement::Never => false,
        ApprovalRequirement::Required => true,
        ApprovalRequirement::RequiredWhenRisk(threshold) => {
            risk_rank(risk) >= risk_rank(threshold)
        }
    }
}

/// Ordinal rank for a [`RiskLevel`] so thresholds can be compared.
fn risk_rank(risk: &RiskLevel) -> u8 {
    match risk {
        RiskLevel::Low => 0,
        RiskLevel::Medium => 1,
        RiskLevel::High => 2,
        RiskLevel::Critical => 3,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::{ApprovalRequirement, RiskLevel};

    #[test]
    fn never_is_never_blocked() {
        for risk in [RiskLevel::Low, RiskLevel::Critical] {
            assert!(!approval_blocks_direct_invocation(
                &ApprovalRequirement::Never,
                &risk
            ));
        }
    }

    #[test]
    fn required_always_blocks() {
        for risk in [RiskLevel::Low, RiskLevel::Medium, RiskLevel::High] {
            assert!(approval_blocks_direct_invocation(
                &ApprovalRequirement::Required,
                &risk
            ));
        }
    }

    #[test]
    fn required_when_risk_compares_against_threshold() {
        let gate = ApprovalRequirement::RequiredWhenRisk(RiskLevel::High);
        assert!(!approval_blocks_direct_invocation(&gate, &RiskLevel::Low));
        assert!(!approval_blocks_direct_invocation(&gate, &RiskLevel::Medium));
        assert!(approval_blocks_direct_invocation(&gate, &RiskLevel::High));
        assert!(approval_blocks_direct_invocation(&gate, &RiskLevel::Critical));
    }
}
