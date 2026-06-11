//! Persistent "allow-always" grants — a standing, explicit, revocable approval that
//! lets a FUTURE matching tool invocation bypass the per-call approval *prompt*
//! (`docs/RELUX_MASTER_PLAN.md` §7.4/§7.5, `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §5
//! P2 "persistent allow-always approval").
//!
//! ## Reference-driven design (see `docs/reference-driven-development.md`)
//!
//! Read first — OpenClaw's exec-approval allow-always model:
//!
//! - **openclaw** `src/acp/permission-relay.ts`
//!   (`GatewayExecApprovalDecision = "allow-once" | "allow-always" | "deny"`,
//!   `buildAcpPermissionOptions`, `resolveGatewayDecisionFromPermissionOutcome`):
//!   an approval prompt offers three explicit options; "allow-always" is a distinct,
//!   named decision, never a default. We mirror the *decision*: an operator chooses
//!   "Allow always" on a pending tool-invocation approval (or creates a grant
//!   directly), never a blanket auto-trust.
//! - **openclaw** `src/agents/bash-tools.exec-host-gateway.ts` (L610-618): only the
//!   `allow-always` branch persists a durable record, and only `if
//!   (!requiresInlineEvalApproval)` — i.e. allow-always is offered/persisted ONLY
//!   for the safe-to-persist case. We mirror it: a grant is mintable only for a tool
//!   that genuinely gates (`approval_blocks_direct_invocation`); a directly-runnable
//!   low-risk tool is refused (a grant would be meaningless).
//! - **openclaw** `src/infra/exec-approvals.types.ts` (`ExecAllowlistEntry { id,
//!   pattern, source: "allow-always", argPattern, lastUsedAt }`) + `exec-approvals.ts`
//!   `hasDurableExecApproval` (a future call bypasses the prompt ONLY when a stored
//!   `source === "allow-always"` entry matches the EXACT command/segments; any
//!   non-matching segment fails closed) + `recordAllowlistUse` (stamp `lastUsedAt`):
//!   a persisted, individually-identified, per-subject record, matched EXACTLY, used
//!   is recorded. We mirror it: [`PersistentGrant`] is an individually-revocable row
//!   bound to an exact `(subject, plugin, tool, permission, risk)`, matched by
//!   [`PersistentGrant::authorizes_invocation`] (every field exact, fail closed), and
//!   stamps `last_used_at` on use.
//!
//! ## The safety contract (binding)
//!
//! - A grant is bound to ONE concrete `(subject agent, plugin id, tool name)` plus a
//!   snapshot of the tool's CURRENT required permission and risk class. There is no
//!   wildcard / blanket / global form here.
//! - Matching is EXACT on every field. A different subject, a different plugin/tool,
//!   a permission that changed, or a risk that ESCALATED (or otherwise differs) all
//!   fail closed → the per-call approval is required again.
//! - A grant bypasses ONLY the per-call approval prompt. It NEVER bypasses the
//!   subject's permission check or the runtime/loopback gate (enforced by the
//!   kernel chokepoint, defense in depth).
//! - Grants are explicit (operator-created or chosen via "Allow always"), revocable
//!   (a single removable row), and auditable (create / revoke / use are logged).

use serde::{Deserialize, Serialize};

use crate::agent::AgentId;
use crate::permission::RiskLevel;

/// A persisted allow-always grant. See the module docs for the safety contract.
///
/// Construction is the kernel's job (it mints the `id`/`created_at`, validates the
/// subject holds the permission, and refuses a non-gating tool); this type owns only
/// the data + the pure, fail-closed [`authorizes_invocation`](Self::authorizes_invocation)
/// matcher so the bypass decision is unit-testable away from any kernel state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistentGrant {
    /// Stable grant id (`grant_0001`), the revoke key.
    pub id: String,
    /// The operator/actor that created the grant (for audit).
    pub created_by: String,
    /// The permission subject the bypass applies to. A grant only ever helps the
    /// exact agent it names — never a different subject.
    pub subject_agent: AgentId,
    /// The plugin the tool belongs to.
    pub plugin_id: String,
    /// The concrete tool name. No wildcard: one grant covers one tool.
    pub tool_name: String,
    /// The tool's required permission at grant time (`tool:<plugin>:<verb>`). The
    /// subject must hold this; matching re-checks the tool's CURRENT permission
    /// against it (a changed permission fails closed).
    pub permission: String,
    /// The tool's risk class at grant time. Matching re-checks the tool's CURRENT
    /// risk against it, so a risk escalation invalidates the grant (re-approve).
    pub risk: RiskLevel,
    /// When the grant was created (logical-clock timestamp).
    pub created_at: String,
    /// When the grant last authorized an invocation, if ever (observability; mirrors
    /// openclaw's `lastUsedAt`). `None` until first use.
    #[serde(default)]
    pub last_used_at: Option<String>,
}

impl PersistentGrant {
    /// True iff this grant authorizes a direct invocation of `tool_name` on
    /// `plugin_id` as `subject`, whose CURRENT required `permission` and `risk`
    /// exactly match what was granted.
    ///
    /// Every field is compared exactly and ALL must match — a different subject,
    /// plugin, or tool; a permission string that changed; or a risk class that
    /// differs (e.g. escalated) all return `false` (fail closed → re-approve). This
    /// is the pure half of openclaw's `hasDurableExecApproval` exact-match rule.
    pub fn authorizes_invocation(
        &self,
        subject: &AgentId,
        plugin_id: &str,
        tool_name: &str,
        permission: &str,
        risk: &RiskLevel,
    ) -> bool {
        &self.subject_agent == subject
            && self.plugin_id == plugin_id
            && self.tool_name == tool_name
            && self.permission == permission
            && &self.risk == risk
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant() -> PersistentGrant {
        PersistentGrant {
            id: "grant_0001".to_string(),
            created_by: "operator".to_string(),
            subject_agent: AgentId::new("prime"),
            plugin_id: "relux-plugin-my-repo".to_string(),
            tool_name: "deploy.run".to_string(),
            permission: "tool:relux-plugin-my-repo:run".to_string(),
            risk: RiskLevel::High,
            created_at: "t0".to_string(),
            last_used_at: None,
        }
    }

    #[test]
    fn exact_match_authorizes() {
        let g = grant();
        assert!(g.authorizes_invocation(
            &AgentId::new("prime"),
            "relux-plugin-my-repo",
            "deploy.run",
            "tool:relux-plugin-my-repo:run",
            &RiskLevel::High,
        ));
    }

    #[test]
    fn any_field_mismatch_fails_closed() {
        let g = grant();
        // Different subject.
        assert!(!g.authorizes_invocation(
            &AgentId::new("other"),
            "relux-plugin-my-repo",
            "deploy.run",
            "tool:relux-plugin-my-repo:run",
            &RiskLevel::High,
        ));
        // Different plugin.
        assert!(!g.authorizes_invocation(
            &AgentId::new("prime"),
            "relux-plugin-other",
            "deploy.run",
            "tool:relux-plugin-my-repo:run",
            &RiskLevel::High,
        ));
        // Different tool.
        assert!(!g.authorizes_invocation(
            &AgentId::new("prime"),
            "relux-plugin-my-repo",
            "deploy.destroy",
            "tool:relux-plugin-my-repo:run",
            &RiskLevel::High,
        ));
        // Permission changed (e.g. tool's verb changed under the same name).
        assert!(!g.authorizes_invocation(
            &AgentId::new("prime"),
            "relux-plugin-my-repo",
            "deploy.run",
            "tool:relux-plugin-my-repo:launch",
            &RiskLevel::High,
        ));
        // Risk escalated.
        assert!(!g.authorizes_invocation(
            &AgentId::new("prime"),
            "relux-plugin-my-repo",
            "deploy.run",
            "tool:relux-plugin-my-repo:run",
            &RiskLevel::Critical,
        ));
        // Risk lowered (also a mismatch — conservative, re-approve).
        assert!(!g.authorizes_invocation(
            &AgentId::new("prime"),
            "relux-plugin-my-repo",
            "deploy.run",
            "tool:relux-plugin-my-repo:run",
            &RiskLevel::Medium,
        ));
    }
}
