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

// Run-detail deep links stay INSIDE the Relux shell.
//
// IA decision: a run's detail is part of the Relux Work surface, not the legacy
// `/runs` console. So a deep link to a run is `/work?run=<id>` — the same
// query-param style the Work page already uses for `?agentId`/`?status`, and the
// Work page reads it to open that run's detail panel. This keeps an operator on
// the Relux shell (and out of the bridge-gated legacy console) when they follow a
// run from anywhere — e.g. an orchestration step's `run_id`. The param is the
// single source of truth, so browser back/forward/refresh restore the same view.

/// Build the in-shell href that opens a run's detail on the Work page.
export function workRunHref(runId: string): string {
  return `/work?run=${encodeURIComponent(runId)}`;
}

/// Read the run id a `/work` URL is pointing at, or null when none is present.
/// Accepts the raw `location.search` (with or without the leading `?`).
export function runIdFromSearch(search: string): string | null {
  const id = new URLSearchParams(search).get("run");
  return id && id.length > 0 ? id : null;
}
