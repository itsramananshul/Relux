// Single source of truth for top-level route ownership (App.tsx consumes this).
//
// The Relux standalone shell is the DEFAULT owner of every path; only these exact
// legacy paths fall through to the bridge-backed dashboard. Keeping this as a
// pure, dependency-free module lets a test assert the invariant that prevents the
// blank-page bug: a deep link or stray sub-path must resolve to the Relux shell
// (which has an in-shell not-found), never silently into the bridge-gated console.

// The exact paths served by the legacy bridge dashboard (behind auth).
// `/approvals` is intentionally absent — the Relux shell owns it.
export const LEGACY_PATHS: readonly string[] = [
  "/overview",
  "/mandates",
  "/briefs",
  "/agents",
  "/lattice",
  "/company",
  "/costs",
  "/assign",
  "/runs",
  "/chat",
  "/scheduled",
  "/settings",
];

// The exact top-level paths the Relux shell declares a real route for.
export const RELUX_PATHS: readonly string[] = [
  "/",
  "/prime",
  "/work",
  "/plugins",
  "/crew",
  "/approvals",
  "/health",
];

const LEGACY_SET = new Set(LEGACY_PATHS);

/// Whether a path is served by the legacy bridge dashboard. Everything else is
/// owned by the Relux shell (including unknown paths, which render an in-shell
/// not-found rather than a blank page).
export function isLegacyPath(pathname: string): boolean {
  return LEGACY_SET.has(pathname);
}
