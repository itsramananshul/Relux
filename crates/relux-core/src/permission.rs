use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::agent::AgentId;

/// Valid permission prefixes from the Relux permission model.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 7.5 (Permission And Approval Layer)
/// and `docs/Relux spec.md` section 12.1 (Permission Philosophy).
///
/// Format: `<prefix>:<resource>:<action>`
pub const VALID_PREFIXES: &[&str] = &[
    "tool:",
    "adapter:",
    "provider:",
    "exec:",
    "plugin:",
    "agent:",
    "task:",
    "approval:",
    "audit:",
];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PermissionError {
    #[error("permission string is empty")]
    Empty,
    #[error("permission '{0}' does not start with a canonical prefix (tool:, adapter:, provider:, exec:, plugin:, agent:, task:, approval:, audit:)")]
    InvalidPrefix(String),
    /// A wildcard / scope token in an unsupported shape, or path-like / injection
    /// characters. The only scoped form Relux accepts is a single tool-plugin wildcard
    /// `tool:<plugin-id>:*`; everything broader (`*`, `tool:*`, `tool:*:*`,
    /// `agent:<id>:*`, partial globs like `tool:p:re*`) is rejected fail-closed.
    #[error("permission '{0}' is a malformed or over-broad scope (only `tool:<plugin-id>:*` is allowed; no global/partial wildcards or path-like characters)")]
    MalformedScope(String),
}

/// True if `s` carries path-like or injection characters that must never appear in a
/// capability string (whitespace, control chars, slashes, or a `..` traversal). Keeps
/// the grammar a flat `prefix:resource:action` and blocks resource injection.
fn has_injection_chars(s: &str) -> bool {
    s.contains("..")
        || s.chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '/' || c == '\\')
}

/// A single permission segment (a plugin id or action) is `[A-Za-z0-9][A-Za-z0-9_-]*`:
/// non-empty, no colon, no `*`, no path characters. Used to validate both the plugin in
/// a scoped wildcard and the concrete tool name it would authorize.
fn is_valid_segment(seg: &str) -> bool {
    !seg.is_empty()
        && seg
            .chars()
            .next()
            .map(|c| c.is_ascii_alphanumeric())
            .unwrap_or(false)
        && seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// If `s` is exactly a tool-plugin scoped wildcard `tool:<plugin-id>:*` with a
/// well-formed plugin id, return the plugin id. This is the ONLY wildcard shape Relux
/// recognizes — no global `*`, no `tool:*`, no partial globs.
fn parse_tool_wildcard(s: &str) -> Option<&str> {
    let plugin = s.strip_prefix("tool:")?.strip_suffix(":*")?;
    if is_valid_segment(plugin) {
        Some(plugin)
    } else {
        None
    }
}

/// Reserved keyword that marks the manager-subtree scoped grant
/// (`agent:<manager-id>:subtree:<action>`). It is reserved inside the `agent:` namespace:
/// a string that uses it anywhere but the exact structured position is rejected
/// fail-closed rather than stored as an opaque `agent:` capability.
const SUBTREE_KEYWORD: &str = "subtree";

/// If `s` is exactly the manager-subtree scoped grant `agent:<manager-id>:subtree:<action>`
/// with well-formed `manager-id` and `action` segments, return `(manager-id, action)`.
///
/// This is the ONLY agent-control scope Relux recognizes. Unlike the tool-plugin wildcard
/// it carries no `*` — it is a flat, exact, individually-revocable capability string whose
/// *authority* only widens at enforcement time, and only over the holder's own Branch
/// (Paperclip `principal_permission_grants` scope = `managerAgentId-subtree`, resolved by
/// `scopeAllows` + `agentIsInSubtree`). No global form (`agent:*:subtree:*`) exists: the
/// manager id is always concrete, so the scope can never name "every manager's subtree".
fn parse_agent_subtree(s: &str) -> Option<(&str, &str)> {
    let mut parts = s.split(':');
    let kind = parts.next()?;
    let manager = parts.next()?;
    let keyword = parts.next()?;
    let action = parts.next()?;
    // Exactly four segments — anything longer is malformed.
    if parts.next().is_some() {
        return None;
    }
    if kind == "agent"
        && keyword == SUBTREE_KEYWORD
        && is_valid_segment(manager)
        && is_valid_segment(action)
    {
        Some((manager, action))
    } else {
        None
    }
}

/// True iff `s` is *attempting* the manager-subtree form (an `agent:` string that uses the
/// reserved [`SUBTREE_KEYWORD`] as a segment) — used to force the strict
/// `agent:<manager-id>:subtree:<action>` shape so a malformed variant (empty manager/action,
/// wrong segment count, keyword in the wrong slot) is rejected instead of silently stored.
fn looks_like_agent_subtree(s: &str) -> bool {
    s.starts_with("agent:") && s.split(':').skip(1).any(|seg| seg == SUBTREE_KEYWORD)
}

/// A capability string that controls what an actor may do.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 7.5 and section 12.1.
/// Format: `<category>:<resource>:<action>` e.g. `tool:relux-tools-github:create_pr`
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Permission(String);

impl Permission {
    /// Construct a `Permission`, validating the canonical prefix and (if a `*` is
    /// present) the strict scoped-wildcard grammar. Exact capability strings keep
    /// working unchanged; the only new accepted shape is `tool:<plugin-id>:*`.
    pub fn new(s: impl Into<String>) -> Result<Self, PermissionError> {
        let s = s.into();
        if s.is_empty() {
            return Err(PermissionError::Empty);
        }
        // Block path-like / injection characters before anything else (defence in depth
        // for both exact and scoped strings).
        if has_injection_chars(&s) {
            return Err(PermissionError::MalformedScope(s));
        }
        if !VALID_PREFIXES.iter().any(|p| s.starts_with(p)) {
            return Err(PermissionError::InvalidPrefix(s));
        }
        // A `*` is only ever legal as a tool-plugin scope. Reject every broader or
        // partial wildcard fail-closed.
        if s.contains('*') && parse_tool_wildcard(&s).is_none() {
            return Err(PermissionError::MalformedScope(s));
        }
        // The reserved `subtree` keyword is only legal as the strict
        // `agent:<manager-id>:subtree:<action>` grant. Reject every malformed variant
        // fail-closed so it is never stored as an opaque `agent:` capability.
        if looks_like_agent_subtree(&s) && parse_agent_subtree(&s).is_none() {
            return Err(PermissionError::MalformedScope(s));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Exact-match comparison. Used by the GRANT-side (dedup) and REVOKE-side paths so a
    /// revoke removes exactly the stored grant and never pattern-expands.
    pub fn matches_exact(&self, other: &Permission) -> bool {
        self.0 == other.0
    }

    /// Whether this permission is a scoped tool-plugin wildcard (`tool:<plugin-id>:*`).
    pub fn is_scoped_wildcard(&self) -> bool {
        parse_tool_wildcard(&self.0).is_some()
    }

    /// Whether this permission is a manager-subtree scoped grant
    /// (`agent:<manager-id>:subtree:<action>`).
    pub fn is_manager_subtree(&self) -> bool {
        parse_agent_subtree(&self.0).is_some()
    }

    /// If this is a manager-subtree grant, its `(manager-id, action)`; else `None`. The
    /// `manager-id` is the operative whose Branch the grant covers — authority is only
    /// ever granted over the holder's OWN subtree, so enforcement additionally requires
    /// `manager-id == holder` (see [`manager_subtree_authorizes`]).
    pub fn agent_subtree_parts(&self) -> Option<(&str, &str)> {
        parse_agent_subtree(&self.0)
    }

    /// Whether holding `self` (a GRANT an agent holds) authorizes `required` (the
    /// concrete capability a tool/task demands). This is the ENFORCEMENT comparison —
    /// distinct from [`matches_exact`], which is the grant/revoke bookkeeping comparison.
    ///
    /// Authorization holds when either:
    /// - `self` equals `required` exactly (the original exact-match contract), or
    /// - `self` is a tool-plugin wildcard `tool:<plugin>:*` and `required` is a concrete
    ///   `tool:<plugin>:<tool>` in the SAME plugin.
    ///
    /// A wildcard never authorizes another wildcard, never crosses plugins, and never
    /// matches a non-`tool:` capability. The `required` side is treated as concrete: a
    /// wildcard on the required side is only honoured by an exact-equal grant.
    pub fn authorizes(&self, required: &Permission) -> bool {
        if self.0 == required.0 {
            return true;
        }
        if let Some(plugin) = parse_tool_wildcard(&self.0) {
            if let Some(rest) = required.0.strip_prefix("tool:") {
                if let Some((req_plugin, req_tool)) = rest.split_once(':') {
                    return req_plugin == plugin
                        && !req_tool.is_empty()
                        && !req_tool.contains('*');
                }
            }
        }
        false
    }
}

impl std::fmt::Display for Permission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Whether `grant` (a capability the `holder` agent holds) authorizes `action` on `target`
/// under the **manager-subtree** rule — the agent-control analogue of [`Permission::authorizes`],
/// but one that needs a target-agent context and the org lattice and so lives outside the
/// context-free `authorizes` path.
///
/// Returns true iff ALL of:
/// - `grant` is a well-formed `agent:<manager-id>:subtree:<action>`;
/// - the grant's `manager-id` equals `holder` — a manager only ever wields authority over
///   its OWN Branch, so a subtree grant naming someone else's id authorizes nothing (no
///   borrowing another manager's subtree);
/// - the grant's action equals the requested `action` (exact, no action globbing); and
/// - `target` is a proper descendant of `holder` in `reports_to` (the bounded
///   [`crate::hierarchy::is_in_subtree`] walk) — **self, siblings, ancestors, and unrelated
///   operatives all fail**, and the walk is total even on a malformed/cyclic map.
///
/// This is the pure half of Paperclip's `scopeAllows` + `agentIsInSubtree`. It performs NO
/// status / enablement check (status is orthogonal to the lattice); a caller that wants a
/// fail-closed "disabled manager wields no authority" rule layers that on top — the kernel
/// chokepoint does exactly that.
pub fn manager_subtree_authorizes(
    grant: &Permission,
    holder: &AgentId,
    action: &str,
    target: &AgentId,
    reports_to: &crate::hierarchy::ReportsToMap,
) -> bool {
    let Some((manager, granted_action)) = grant.agent_subtree_parts() else {
        return false;
    };
    manager == holder.as_str()
        && granted_action == action
        && crate::hierarchy::is_in_subtree(holder, target, reports_to)
}

/// How dangerous a tool call or action is considered to be.
///
/// Spec ref: `docs/Relux spec.md` section 12.4 (Risk Levels).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

/// Whether an action requires a human approval gate before execution.
///
/// Spec ref: `docs/Relux spec.md` section 12.5 (Approval Rules).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRequirement {
    Never,
    Required,
    RequiredWhenRisk(RiskLevel),
}

/// A single callable tool exposed by a ToolSet plugin.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 8.2 (ToolSet Plugins) and
/// `docs/Relux spec.md` section 10.2 (ToolSet Plugin).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub risk: RiskLevel,
    pub permission: Permission,
    pub approval: ApprovalRequirement,
    pub timeout_secs: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_permissions_are_accepted() {
        for prefix in VALID_PREFIXES {
            let s = format!("{}resource:action", prefix);
            assert!(Permission::new(&s).is_ok(), "expected ok for {s}");
        }
    }

    #[test]
    fn empty_permission_is_rejected() {
        assert_eq!(Permission::new(""), Err(PermissionError::Empty));
    }

    #[test]
    fn invalid_prefix_is_rejected() {
        let err = Permission::new("fs:some:action").unwrap_err();
        assert!(matches!(err, PermissionError::InvalidPrefix(_)));
    }

    #[test]
    fn exact_match_works() {
        let a = Permission::new("tool:relux-tools-github:create_pr").unwrap();
        let b = Permission::new("tool:relux-tools-github:create_pr").unwrap();
        let c = Permission::new("tool:relux-tools-github:merge_pr").unwrap();
        assert!(a.matches_exact(&b));
        assert!(!a.matches_exact(&c));
    }

    // --- scoped wildcard grammar -------------------------------------------------

    #[test]
    fn tool_plugin_wildcard_is_accepted_and_flagged() {
        let p = Permission::new("tool:relux-tools-github:*").unwrap();
        assert!(p.is_scoped_wildcard());
        // An exact grant is NOT a wildcard.
        assert!(!Permission::new("tool:relux-tools-github:create_pr")
            .unwrap()
            .is_scoped_wildcard());
    }

    #[test]
    fn broad_and_partial_wildcards_are_rejected() {
        for bad in [
            "*",                              // bare global
            "tool:*",                         // whole-namespace
            "tool:*:*",                       // plugin + action glob
            "tool:relux-tools-github:cre*",   // partial action glob
            "tool:relux*tools:read",          // glob inside plugin id
            "agent:research-bot:*",           // non-tool wildcard
            "adapter:relux-adapter-claude:*", // non-tool wildcard
            "tool::*",                        // empty plugin
        ] {
            let err = Permission::new(bad).unwrap_err();
            assert!(
                matches!(err, PermissionError::MalformedScope(_) | PermissionError::InvalidPrefix(_)),
                "expected {bad} rejected, got {err:?}"
            );
        }
        // Bare `*` has no canonical prefix.
        assert!(matches!(
            Permission::new("*").unwrap_err(),
            PermissionError::InvalidPrefix(_)
        ));
    }

    #[test]
    fn path_like_and_injection_strings_are_rejected() {
        for bad in [
            "tool:relux-tools-github:../etc",
            "tool:relux-tools-github:read write",
            "tool:relux/tools:read",
            "tool:relux\\tools:read",
        ] {
            assert!(
                matches!(
                    Permission::new(bad).unwrap_err(),
                    PermissionError::MalformedScope(_)
                ),
                "expected {bad} rejected as malformed scope"
            );
        }
    }

    // --- authorization (enforcement) ---------------------------------------------

    #[test]
    fn exact_grant_authorizes_only_itself() {
        let grant = Permission::new("tool:relux-tools-github:create_pr").unwrap();
        assert!(grant.authorizes(&Permission::new("tool:relux-tools-github:create_pr").unwrap()));
        assert!(!grant.authorizes(&Permission::new("tool:relux-tools-github:merge_pr").unwrap()));
    }

    #[test]
    fn tool_plugin_wildcard_authorizes_every_tool_in_that_plugin() {
        let grant = Permission::new("tool:relux-tools-github:*").unwrap();
        assert!(grant.authorizes(&Permission::new("tool:relux-tools-github:create_pr").unwrap()));
        assert!(grant.authorizes(&Permission::new("tool:relux-tools-github:merge_pr").unwrap()));
    }

    #[test]
    fn tool_plugin_wildcard_does_not_overmatch_other_plugins_or_kinds() {
        let grant = Permission::new("tool:relux-tools-github:*").unwrap();
        // Different plugin.
        assert!(!grant.authorizes(&Permission::new("tool:relux-tools-gitlab:create_pr").unwrap()));
        // A plugin whose id is a prefix of the granted one must not match.
        assert!(!grant.authorizes(&Permission::new("tool:relux-tools-git:create_pr").unwrap()));
        // Non-tool capability.
        assert!(!grant.authorizes(&Permission::new("adapter:relux-tools-github:run").unwrap()));
        // A wildcard never authorizes another wildcard (only exact-equal would).
        assert!(!grant.authorizes(&Permission::new("tool:relux-tools-github:*").unwrap())
            || grant.matches_exact(&Permission::new("tool:relux-tools-github:*").unwrap()));
        // Same-plugin wildcard required is only honoured by exact equality.
        let other_plugin_wild = Permission::new("tool:relux-tools-gitlab:*").unwrap();
        assert!(!grant.authorizes(&other_plugin_wild));
    }

    // --- manager-subtree scoped grant grammar ------------------------------------

    #[test]
    fn manager_subtree_grant_is_accepted_and_flagged() {
        let p = Permission::new("agent:lead-1:subtree:grant_permission").unwrap();
        assert!(p.is_manager_subtree());
        assert_eq!(p.agent_subtree_parts(), Some(("lead-1", "grant_permission")));
        // It is NOT a tool-plugin wildcard.
        assert!(!p.is_scoped_wildcard());
        // A plain exact `agent:` capability is not a subtree grant.
        let plain = Permission::new("agent:lead-1:configure").unwrap();
        assert!(!plain.is_manager_subtree());
        assert_eq!(plain.agent_subtree_parts(), None);
    }

    #[test]
    fn malformed_subtree_scopes_are_rejected_fail_closed() {
        for bad in [
            "agent:lead-1:subtree",     // no action segment
            "agent:lead-1:subtree:",    // empty action
            "agent::subtree:grant",     // empty manager id
            "agent:lead-1:subtree:a:b", // too many segments
            "agent:subtree:grant",      // reserved keyword in the manager slot
            "agent:*:subtree:grant",    // global manager (also a wildcard)
            "agent:lead-1:subtree:*",   // action glob
        ] {
            let err = Permission::new(bad).unwrap_err();
            assert!(
                matches!(err, PermissionError::MalformedScope(_)),
                "expected {bad} rejected as malformed scope, got {err:?}"
            );
        }
        // The keyword is case-sensitive: `Subtree` is not reserved, so this stays a valid
        // opaque 4-segment `agent:` capability (no false strictness).
        assert!(Permission::new("agent:lead-1:Subtree:grant").is_ok());
    }

    fn aid(s: &str) -> AgentId {
        AgentId::new(s)
    }

    fn rmap(edges: &[(&str, &str)]) -> crate::hierarchy::ReportsToMap {
        edges.iter().map(|(c, m)| (aid(c), aid(m))).collect()
    }

    #[test]
    fn subtree_grant_authorizes_only_descendants_of_its_own_manager() {
        // ic -> lead -> director ; peer -> director
        let m = rmap(&[("ic", "lead"), ("lead", "director"), ("peer", "director")]);
        let grant = Permission::new("agent:lead:subtree:grant_permission").unwrap();

        // Subordinate (direct child) — allowed.
        assert!(manager_subtree_authorizes(&grant, &aid("lead"), "grant_permission", &aid("ic"), &m));

        // Self — NOT allowed (proper-descendant: a node is not in its own subtree).
        assert!(!manager_subtree_authorizes(&grant, &aid("lead"), "grant_permission", &aid("lead"), &m));
        // Manager (ancestor) — NOT allowed.
        assert!(!manager_subtree_authorizes(&grant, &aid("lead"), "grant_permission", &aid("director"), &m));
        // Sibling subtree — NOT allowed.
        assert!(!manager_subtree_authorizes(&grant, &aid("lead"), "grant_permission", &aid("peer"), &m));
        // Wrong action — NOT allowed (no action globbing).
        assert!(!manager_subtree_authorizes(&grant, &aid("lead"), "assign_task", &aid("ic"), &m));
    }

    #[test]
    fn subtree_grant_action_is_exact_and_generic_over_the_action_name() {
        // The subtree grammar accepts ANY well-formed action segment, so a second action
        // (`assign_task`) is a valid, distinct scope — and only ever authorizes its own
        // action over a real subordinate (no cross-action bleed with `grant_permission`).
        let m = rmap(&[("ic", "lead"), ("lead", "director")]);
        let assign = Permission::new("agent:lead:subtree:assign_task").unwrap();
        assert!(assign.is_manager_subtree());
        assert_eq!(assign.agent_subtree_parts(), Some(("lead", "assign_task")));

        // Authorizes `assign_task` on a subordinate…
        assert!(manager_subtree_authorizes(&assign, &aid("lead"), "assign_task", &aid("ic"), &m));
        // …but NOT a different action (an assign_task scope is not a grant_permission scope).
        assert!(!manager_subtree_authorizes(&assign, &aid("lead"), "grant_permission", &aid("ic"), &m));
        // …and a grant_permission scope is likewise not an assign_task scope.
        let grant = Permission::new("agent:lead:subtree:grant_permission").unwrap();
        assert!(!manager_subtree_authorizes(&grant, &aid("lead"), "assign_task", &aid("ic"), &m));
        // Self is still never an assignment target via the subtree path.
        assert!(!manager_subtree_authorizes(&assign, &aid("lead"), "assign_task", &aid("lead"), &m));
    }

    #[test]
    fn subtree_grant_cannot_borrow_another_managers_branch() {
        // A grant naming `director`'s subtree, but HELD by `lead`, authorizes nothing —
        // even over a node that IS in director's subtree. Authority is over the holder's
        // own Branch only.
        let m = rmap(&[("ic", "lead"), ("lead", "director"), ("peer", "director")]);
        let foreign = Permission::new("agent:director:subtree:grant_permission").unwrap();
        // `lead` holding a `director`-subtree grant: denied (manager-id != holder).
        assert!(!manager_subtree_authorizes(&foreign, &aid("lead"), "grant_permission", &aid("peer"), &m));
        // But `director` holding it over `peer` (a real subordinate): allowed.
        assert!(manager_subtree_authorizes(&foreign, &aid("director"), "grant_permission", &aid("peer"), &m));
        // A non-subtree grant never authorizes via this path.
        let exact = Permission::new("agent:lead:grant_permission").unwrap();
        assert!(!manager_subtree_authorizes(&exact, &aid("lead"), "grant_permission", &aid("ic"), &m));
    }

    #[test]
    fn subtree_authorization_is_total_under_a_cyclic_map() {
        // a -> b -> a (a cycle that should never persist, but must not hang the matcher).
        let m = rmap(&[("a", "b"), ("b", "a")]);
        let grant = Permission::new("agent:a:subtree:grant_permission").unwrap();
        // The bounded walk terminates; b is a descendant of a in this (degenerate) map.
        let _ = manager_subtree_authorizes(&grant, &aid("a"), "grant_permission", &aid("b"), &m);
        // Self is still never authorized, even under a cycle.
        assert!(!manager_subtree_authorizes(&grant, &aid("a"), "grant_permission", &aid("a"), &m));
    }
}
