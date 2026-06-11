// Pure governance helpers for the Crew permission surface (no React, unit-testable).
//
// Section: docs/relix-dashboard-design.md §9 / §9.1 (the per-agent Permissions panel).
//
// Relux permissions are explicit capability strings of the shape
// `<prefix>:<resource>:<action>` (e.g. `tool:relux-tools-github:create_pr`). The
// backend (`crates/relux-core/src/permission.rs`) validates the prefix; we mirror that
// allowlist here so the form can reject a malformed string BEFORE calling the API, and
// flag the control-plane prefixes the operator should confirm deliberately.

/// Canonical permission prefixes, mirrored from `relux-core` `VALID_PREFIXES`. A
/// permission must start with one of these (with the trailing colon).
export const VALID_PERMISSION_PREFIXES = [
  "tool:",
  "adapter:",
  "provider:",
  "exec:",
  "plugin:",
  "agent:",
  "task:",
  "approval:",
  "audit:",
] as const;

// Control-plane / capability-granting / execution prefixes. Granting one of these is
// not a routine tool grant — it lets the agent reach a runtime, a provider, another
// agent's config, or the approval gate itself — so the UI requires an explicit,
// warned confirmation. (This is a UI-side caution, not an enforcement boundary: the
// kernel still audits every grant/revoke and enforces least privilege.)
const ELEVATED_PREFIXES = new Set([
  "adapter:",
  "provider:",
  "exec:",
  "plugin:",
  "agent:",
  "approval:",
]);

export type PermissionRisk = "elevated" | "standard";

/** The prefix (including the trailing colon) of a permission string, or "" if none. */
export function permissionPrefix(permission: string): string {
  const i = permission.indexOf(":");
  return i >= 0 ? permission.slice(0, i + 1) : "";
}

// A permission segment (plugin id / action) the backend accepts: `[A-Za-z0-9][A-Za-z0-9_-]*`.
// Mirrors `relux_core::permission::is_valid_segment` so the form rejects the same shapes.
const SEGMENT_RE = /^[A-Za-z0-9][A-Za-z0-9_-]*$/;
// The ONLY scoped wildcard the backend recognizes: `tool:<plugin-id>:*`.
const TOOL_WILDCARD_RE = /^tool:[A-Za-z0-9][A-Za-z0-9_-]*:\*$/;
// The manager-subtree scoped grant (advanced / manager scope):
// `agent:<manager-id>:subtree:<action>`. It authorizes the manager to perform <action> on
// operatives inside its OWN Branch (the `reports_to` subtree) — never siblings, ancestors,
// or itself, and only while the manager is live. Mirrors
// `relux_core::permission::parse_agent_subtree`.
const AGENT_SUBTREE_RE =
  /^agent:[A-Za-z0-9][A-Za-z0-9_-]*:subtree:[A-Za-z0-9][A-Za-z0-9_-]*$/;

/** Whether a permission is the one accepted scoped wildcard (`tool:<plugin-id>:*`). */
export function isScopedWildcard(permission: string): boolean {
  return TOOL_WILDCARD_RE.test(permission.trim());
}

/** Whether a permission is the manager-subtree scoped grant (`agent:<manager-id>:subtree:<action>`). */
export function isManagerSubtree(permission: string): boolean {
  return AGENT_SUBTREE_RE.test(permission.trim());
}

/** Build the manager-subtree grant scoping `action` to `managerId`'s Branch (or null if malformed). */
export function managerSubtreePermission(
  managerId: string,
  action: string,
): string | null {
  const id = managerId.trim();
  const act = action.trim();
  return SEGMENT_RE.test(id) && SEGMENT_RE.test(act)
    ? `agent:${id}:subtree:${act}`
    : null;
}

// True if `s` is *attempting* the manager-subtree form (an `agent:` string that uses the
// reserved `subtree` keyword as a segment). Mirrors the backend's `looks_like_agent_subtree`
// so a malformed subtree string is rejected with a scope-specific reason, not stored opaque.
function looksLikeAgentSubtree(s: string): boolean {
  return s.startsWith("agent:") && s.split(":").slice(1).includes("subtree");
}

/** Build the scoped grant that authorizes every tool in `pluginId` (or null if the id is malformed). */
export function pluginWildcardPermission(pluginId: string): string | null {
  const id = pluginId.trim();
  return SEGMENT_RE.test(id) ? `tool:${id}:*` : null;
}

/** Whether a permission string is well-formed (non-empty + a canonical prefix + no bad wildcard/injection). */
export function isValidPermission(permission: string): boolean {
  return permissionInvalidReason(permission) === null;
}

/**
 * Honest reason a permission string is rejected, or null if it is valid. Used to
 * disable the Add button and explain why, rather than letting the API 400. Mirrors the
 * backend grammar in `relux_core::permission` (prefix allowlist + the single
 * `tool:<plugin-id>:*` scoped wildcard; everything broader/partial is rejected).
 */
export function permissionInvalidReason(permission: string): string | null {
  const s = permission.trim();
  if (!s) return "Enter a permission string.";
  // Path-like / injection characters are never part of a capability string.
  if (/[\s/\\]/.test(s) || s.includes("..")) {
    return "Remove spaces, slashes, or `..` — a permission is a flat prefix:resource:action.";
  }
  if (!VALID_PERMISSION_PREFIXES.some((p) => s.startsWith(p))) {
    return `Must start with one of: ${VALID_PERMISSION_PREFIXES.join(" ")}`;
  }
  // A `*` is only legal as a tool-plugin scope; reject `*`, `tool:*`, `tool:*:*`,
  // `agent:x:*`, partial globs like `tool:p:re*`, etc.
  if (s.includes("*") && !isScopedWildcard(s)) {
    return "Only `tool:<plugin-id>:*` is allowed as a scope — no global or partial wildcards.";
  }
  // The reserved `subtree` keyword is only legal as the strict manager-subtree grant.
  if (looksLikeAgentSubtree(s) && !isManagerSubtree(s)) {
    return "A manager-subtree scope must be exactly `agent:<manager-id>:subtree:<action>` (e.g. `agent:lead-1:subtree:grant_permission`).";
  }
  return null;
}

/**
 * Classify a permission's risk for the UI. Control-plane prefixes are "elevated"
 * (warn + confirm before granting); everything else is "standard".
 */
export function permissionRisk(permission: string): PermissionRisk {
  return ELEVATED_PREFIXES.has(permissionPrefix(permission.trim()))
    ? "elevated"
    : "standard";
}

/** Whether granting/holding this permission is an elevated (control-plane) capability. */
export function isElevatedPermission(permission: string): boolean {
  return permissionRisk(permission) === "elevated";
}

// --- Operator-assisted manager-subtree grant (Crew Governance) ---------------
//
// HONEST trust boundary: Relux has no per-agent auth identity yet, so a manager agent
// cannot authenticate its own request. The Crew Governance panel lets an authenticated
// OPERATOR authorize "grant as this manager" — the backend still enforces the real
// own-Branch + Active + scope rule (`POST /v1/relux/agents/:id/manager-grant`), so the
// operator cannot widen anything the manager itself could not do. These helpers decide
// when to OFFER that affordance (and explain, honestly, why it is unavailable) — they are
// a UI gate only; the kernel is the authority and re-checks everything.

/** A minimal agent shape the manager-grant availability check needs. */
export interface ManagerGrantAgent {
  id: string;
  status?: string;
  permissions?: string[];
  reports_to?: string;
}

/** The actions a manager-subtree grant may scope. Today only `grant_permission` has a
 * real enforcement path (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §19). */
export const MANAGER_SUBTREE_GRANT_ACTION = "grant_permission";

export interface ManagerGrantAvailability {
  /** Whether the "Grant as manager" affordance should be offered for this agent. */
  available: boolean;
  /** An honest one-line reason when unavailable (empty when available). */
  reason: string;
  /** The subordinate ids the manager may grant to (its own-Branch descendants). */
  targets: string[];
}

/**
 * The `<action>`s for which `manager` holds a live `agent:<manager>:subtree:<action>`
 * scope (own-id only — a grant naming another manager authorizes nothing here, mirroring
 * the backend's `holder == manager` rule).
 */
export function managerSubtreeActions(manager: ManagerGrantAgent): string[] {
  const out: string[] = [];
  for (const p of manager.permissions ?? []) {
    const parts = p.trim().split(":");
    if (
      parts.length === 4 &&
      parts[0] === "agent" &&
      parts[1] === manager.id &&
      parts[2] === "subtree" &&
      SEGMENT_RE.test(parts[3])
    ) {
      out.push(parts[3]);
    }
  }
  return out;
}

/**
 * Decide whether the operator may be offered the "Grant as manager" affordance for
 * `manager`, and over which subordinate targets. Mirrors the backend authority gate so
 * the affordance never appears when the kernel would only 403:
 *   - the manager must be Active (a paused/disabled manager wields no subtree authority),
 *   - it must hold a `agent:<manager>:subtree:grant_permission` scope over its OWN Branch,
 *   - at least one operative must actually sit in that Branch (a proper descendant).
 * The reason string is the first failing rule, so the UI can explain the unavailability.
 */
export function managerGrantAvailability(
  manager: ManagerGrantAgent,
  roster: ManagerGrantAgent[],
): ManagerGrantAvailability {
  const none = (reason: string): ManagerGrantAvailability => ({
    available: false,
    reason,
    targets: [],
  });
  const status = (manager.status ?? "").toLowerCase();
  if (status && status !== "active") {
    return none("Only a live (Active) manager can grant to its Branch.");
  }
  if (!managerSubtreeActions(manager).includes(MANAGER_SUBTREE_GRANT_ACTION)) {
    return none(
      `No manager-subtree grant scope (add agent:${manager.id}:subtree:grant_permission first).`,
    );
  }
  const targets = subordinateIds(manager.id, roster);
  if (targets.length === 0) {
    return none("No operatives in this manager's Branch yet.");
  }
  return { available: true, reason: "", targets };
}

// --- Token-authenticated manager actions (Crew Access-tokens panel) ----------
//
// A per-agent access token authenticates a request AS its subject on the tiny agent-self
// route subset (`/v1/relux/agents/me/*`) and NOTHING else — it never touches the operator
// console. Two manager-subtree actions are reachable today, each requiring the matching
// `agent:<manager-id>:subtree:<action>` scope on the acting manager (own-Branch + Active):
//   - `manager-grant`  → `POST /v1/relux/agents/me/manager-grant`  (grant_permission)
//   - `assign-task`    → `POST /v1/relux/agents/me/assign-task`    (assign_task)
// These helpers build copy-paste snippets and validate the local test form. The raw token
// is NEVER inlined into a snippet (it is referenced as a shell variable) and is never
// stored — only the operator who just minted it (copy-once) can paste it.
// docs/HERMES_OPENCLAW_DEEP_AUDIT.md §20 / §21.

/** The agent-self manager-action routes a per-agent token unlocks (the ONLY routes it reaches). */
export const AGENT_SELF_MANAGER_GRANT_ROUTE = "/v1/relux/agents/me/manager-grant";
export const AGENT_SELF_ASSIGN_TASK_ROUTE = "/v1/relux/agents/me/assign-task";

// The raw per-agent token shape (`relux_agt_<hex>`), mirrored from
// `crates/relux-kernel/src/agent_auth.rs`. Used only to reject an obviously-wrong paste
// BEFORE the request — the kernel is the real authority and re-validates every token.
const AGENT_TOKEN_RE = /^relux_agt_[A-Za-z0-9]+$/;

/** Whether a pasted string has the per-agent raw-token shape (`relux_agt_…`). */
export function agentTokenLooksValid(token: string): boolean {
  return AGENT_TOKEN_RE.test(token.trim());
}

/**
 * Honest reason the token-authenticated assign-task test form is not ready to submit, or
 * null when every field is present and well-shaped. A UI gate only (the kernel re-checks
 * authority, Branch membership, and task assignability) — it never widens anything.
 */
export function assignTaskFormReason(
  token: string,
  taskId: string,
  targetAgentId: string,
): string | null {
  if (!token.trim()) return "Paste the agent's raw token (shown once at mint).";
  if (!agentTokenLooksValid(token)) {
    return "That does not look like a per-agent token (expected `relux_agt_…`).";
  }
  if (!taskId.trim()) return "Enter the task id to assign.";
  if (!targetAgentId.trim()) return "Enter the target subordinate's id.";
  return null;
}

/**
 * A copy-paste curl snippet for the token-authenticated assign-task call. The token is
 * referenced as the `$RELUX_AGENT_TOKEN` shell variable and is NEVER embedded, so the
 * snippet carries no secret and is safe to display/copy. Blank ids fall back to angle-
 * bracket placeholders so the shape is always clear.
 */
export function assignTaskCurlSnippet(taskId: string, targetAgentId: string): string {
  const t = taskId.trim() || "<task_id>";
  const a = targetAgentId.trim() || "<target_agent_id>";
  return [
    `curl -sS -X POST http://127.0.0.1:19891${AGENT_SELF_ASSIGN_TASK_ROUTE} \\`,
    `  -H "Authorization: Bearer $RELUX_AGENT_TOKEN" \\`,
    `  -H "content-type: application/json" \\`,
    `  -d '{"task_id":"${t}","target_agent_id":"${a}"}'`,
  ].join("\n");
}

/**
 * A copy-paste curl snippet for the token-authenticated manager-grant call (the sibling
 * action). Same no-secret discipline: the token is the `$RELUX_AGENT_TOKEN` variable.
 */
export function managerGrantCurlSnippet(targetAgentId: string, permission: string): string {
  const a = targetAgentId.trim() || "<target_agent_id>";
  const p = permission.trim() || "<permission>";
  return [
    `curl -sS -X POST http://127.0.0.1:19891${AGENT_SELF_MANAGER_GRANT_ROUTE} \\`,
    `  -H "Authorization: Bearer $RELUX_AGENT_TOKEN" \\`,
    `  -H "content-type: application/json" \\`,
    `  -d '{"target_id":"${a}","permission":"${p}"}'`,
  ].join("\n");
}

/**
 * Parse the optional "lifetime (days)" field on the agent-token mint form into a TTL in
 * seconds, or `undefined` when blank/invalid (the backend then applies its default and
 * always clamps to its own bounded window — this is only a convenience conversion, not a
 * security boundary). A non-positive or non-finite value is treated as "unspecified".
 */
export function parseTokenTtlSecs(input: string): number | undefined {
  const trimmed = input.trim();
  if (!trimmed) return undefined;
  const days = Number(trimmed);
  if (!Number.isFinite(days) || days <= 0) return undefined;
  return Math.round(days * 86400);
}

/**
 * The ids that (transitively) report to `rootId` — its Branch, excluding itself. A
 * bounded walk down the `reports_to` lattice (each id visited once, so it is total even
 * under a stray cycle). Mirrors `crates/relux-core/src/hierarchy.rs` `is_in_subtree` and
 * the dashboard `hierarchy.ts` `descendantIds`.
 */
function subordinateIds(rootId: string, roster: ManagerGrantAgent[]): string[] {
  const childrenOf = new Map<string, string[]>();
  for (const a of roster) {
    if (a.reports_to) {
      const list = childrenOf.get(a.reports_to) ?? [];
      list.push(a.id);
      childrenOf.set(a.reports_to, list);
    }
  }
  const out: string[] = [];
  const seen = new Set<string>();
  const stack = [...(childrenOf.get(rootId) ?? [])];
  while (stack.length > 0) {
    const id = stack.pop() as string;
    if (seen.has(id)) continue;
    seen.add(id);
    out.push(id);
    for (const child of childrenOf.get(id) ?? []) stack.push(child);
  }
  return out;
}
