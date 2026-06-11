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

/** Whether a permission is the one accepted scoped wildcard (`tool:<plugin-id>:*`). */
export function isScopedWildcard(permission: string): boolean {
  return TOOL_WILDCARD_RE.test(permission.trim());
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
