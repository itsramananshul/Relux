//! Pure org-lattice (chain-of-command) helpers over the `reports_to` (Lead) pointer.
//!
//! Section: `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §3 (Agent / subagent model — Paperclip
//! `reportsTo` org tree + `agentIsInSubtree` bounded walk) and §5 (manager-subtree
//! authority, **still future**: these helpers exist for a later scoped-grant slice and
//! are NOT wired into any permission check today).
//!
//! Reference-driven (`docs/reference-driven-development.md`):
//! - **Paperclip** `authorization.ts` `agentIsInSubtree` (a 50-depth walk up the
//!   `reportsTo` chain) and `packages/db/src/schema/agents.ts` (`reportsTo` indexed
//!   `(companyId, reportsTo)`) — summarized in the audit; the source is **not vendored**
//!   under `reference/`, so only the bounded-walk *shape* is taken, never scope
//!   enforcement.
//! - **OpenClaw** `src/acp/session-lineage-meta.ts` — a `parentSessionId`
//!   (`parentSessionKey ?? spawnedBy`), a bounded non-negative `spawnDepth`, and
//!   `subagentControlScope: "children" | "none"` (a node's authority is its children
//!   subtree, or nothing; default narrow). **Hermes** `tools/delegate_tool.py`
//!   (`MAX_DEPTH`, per-record `parent_id`/`depth`). A parent pointer walked under a hard
//!   depth bound, fail-narrow by default.
//!
//! Every walk here is **bounded** by [`MAX_HIERARCHY_DEPTH`] and guards against repeats,
//! so a malformed/cyclic map can never loop forever (defence in depth — the config
//! boundary already rejects cycles before persisting an edge).

use std::collections::{HashMap, HashSet};

use crate::agent::AgentId;

/// Child id → manager (Lead) id. One entry per operative that has a Lead; a top-level
/// operative simply has no entry. Built from the live roster at the call site. A hash map
/// (not a btree) because every consumer here walks the graph by following pointers — the
/// outputs depend on the edges, never on map iteration order, so they stay deterministic.
pub type ReportsToMap = HashMap<AgentId, AgentId>;

/// Hard cap on how far a chain-of-command / subtree walk follows the `reports_to`
/// pointer. Mirrors Paperclip's 50-depth `agentIsInSubtree` walk: deep enough for any
/// real org, bounded so a stray cycle is still total.
pub const MAX_HIERARCHY_DEPTH: usize = 50;

/// The ordered chain of command above `agent` (**the Line**): its Lead, then its Lead's
/// Lead, …, nearest manager first. The walk stops at a top-level operative (no entry), a
/// dangling id (manager not in the map as a child of anyone — still returned, the walk
/// just can't continue), a repeat (cycle guard), or [`MAX_HIERARCHY_DEPTH`]. `agent`
/// itself is never included.
pub fn chain_of_command(agent: &AgentId, reports_to: &ReportsToMap) -> Vec<AgentId> {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    seen.insert(agent.clone());
    let mut current = agent.clone();
    for _ in 0..MAX_HIERARCHY_DEPTH {
        match reports_to.get(&current) {
            // Stop the moment we'd revisit a node — the cycle guard keeps the walk total
            // even if a cyclic map ever reached this code despite config-boundary checks.
            Some(manager) if !seen.contains(manager) => {
                chain.push(manager.clone());
                seen.insert(manager.clone());
                current = manager.clone();
            }
            _ => break,
        }
    }
    chain
}

/// True iff `manager` is a (transitive) Lead of `child` — i.e. `child` sits somewhere in
/// `manager`'s **Branch** (subtree). Proper-descendant semantics: a node is NOT in its
/// own subtree, so `is_in_subtree(x, x)` is `false`. Bounded by [`MAX_HIERARCHY_DEPTH`].
///
/// This is the pure helper reserved for a future manager-subtree scoped permission
/// (Paperclip `scopeAllows` + `agentIsInSubtree`). It is intentionally NOT consulted by
/// any permission check yet — enforcement stays exactly as it was.
pub fn is_in_subtree(manager: &AgentId, child: &AgentId, reports_to: &ReportsToMap) -> bool {
    if manager == child {
        return false;
    }
    chain_of_command(child, reports_to)
        .iter()
        .any(|ancestor| ancestor == manager)
}

/// True iff pointing `child` → `new_manager` would create a cycle in the lattice: either
/// a self-report (`child == new_manager`) or `new_manager` already sits in `child`'s
/// Branch (so the new edge would close a loop). Pure; used by the config boundary
/// (`relux-kernel` kernel `create`/`update`) before persisting a `reports_to` edge.
///
/// The map passed in is the CURRENT graph (it does not yet contain the proposed
/// `child → new_manager` edge); the check walks up from `new_manager` and asks whether it
/// can already reach `child`, which is exactly the loop the new edge would close.
pub fn would_create_cycle(
    child: &AgentId,
    new_manager: &AgentId,
    reports_to: &ReportsToMap,
) -> bool {
    child == new_manager || is_in_subtree(child, new_manager, reports_to)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> AgentId {
        AgentId::new(s)
    }

    /// Build a child→manager map from `(child, manager)` pairs.
    fn map(edges: &[(&str, &str)]) -> ReportsToMap {
        edges.iter().map(|(c, m)| (id(c), id(m))).collect()
    }

    #[test]
    fn chain_of_command_walks_nearest_first_and_stops_at_top() {
        // ic -> lead -> director ; director is top-level.
        let m = map(&[("ic", "lead"), ("lead", "director")]);
        assert_eq!(chain_of_command(&id("ic"), &m), vec![id("lead"), id("director")]);
        assert_eq!(chain_of_command(&id("lead"), &m), vec![id("director")]);
        // A top-level operative has an empty Line.
        assert!(chain_of_command(&id("director"), &m).is_empty());
        // An operative not in the map at all is treated as top-level.
        assert!(chain_of_command(&id("ghost"), &m).is_empty());
    }

    #[test]
    fn is_in_subtree_true_for_descendants_false_for_self_and_unrelated() {
        let m = map(&[("ic", "lead"), ("lead", "director"), ("peer", "director")]);
        // director's Branch contains lead, ic, and peer (transitively).
        assert!(is_in_subtree(&id("director"), &id("lead"), &m));
        assert!(is_in_subtree(&id("director"), &id("ic"), &m));
        assert!(is_in_subtree(&id("director"), &id("peer"), &m));
        assert!(is_in_subtree(&id("lead"), &id("ic"), &m));
        // Not a descendant: self, upward, and sideways.
        assert!(!is_in_subtree(&id("director"), &id("director"), &m), "self not in own subtree");
        assert!(!is_in_subtree(&id("ic"), &id("director"), &m), "child is not above its lead");
        assert!(!is_in_subtree(&id("lead"), &id("peer"), &m), "siblings' subtrees don't overlap");
    }

    #[test]
    fn would_create_cycle_rejects_self_direct_and_transitive_loops() {
        let m = map(&[("ic", "lead"), ("lead", "director")]);
        // Self-report.
        assert!(would_create_cycle(&id("lead"), &id("lead"), &m));
        // director -> ic would close director -> ic -> lead -> director.
        assert!(would_create_cycle(&id("director"), &id("ic"), &m));
        // director -> lead would close director -> lead -> director.
        assert!(would_create_cycle(&id("director"), &id("lead"), &m));
        // A safe edge (peer reports to lead) is fine; so is re-setting an existing edge.
        assert!(!would_create_cycle(&id("peer"), &id("lead"), &m));
        assert!(!would_create_cycle(&id("ic"), &id("lead"), &m), "idempotent re-set is allowed");
        // ic -> director (skip-level) is acyclic and allowed.
        assert!(!would_create_cycle(&id("ic"), &id("director"), &m));
    }

    #[test]
    fn walks_stay_total_under_a_cyclic_map() {
        // a -> b -> a (a cycle that should never be persisted, but must not hang a walk).
        let m = map(&[("a", "b"), ("b", "a")]);
        let chain = chain_of_command(&id("a"), &m);
        // The cycle guard stops after the first revisit, bounded length.
        assert!(chain.len() <= MAX_HIERARCHY_DEPTH);
        assert_eq!(chain, vec![id("b")]);
    }

    #[test]
    fn deep_chain_is_capped_at_max_depth() {
        // A chain longer than the cap: n0 -> n1 -> ... -> n(MAX+5).
        let edges: Vec<(String, String)> = (0..MAX_HIERARCHY_DEPTH + 5)
            .map(|i| (format!("n{i}"), format!("n{}", i + 1)))
            .collect();
        let m: ReportsToMap = edges.iter().map(|(c, p)| (id(c), id(p))).collect();
        let chain = chain_of_command(&id("n0"), &m);
        assert_eq!(chain.len(), MAX_HIERARCHY_DEPTH, "walk is bounded by the depth cap");
    }
}
