// Pure helpers for the Crew org-lattice (chain-of-command) UI. Kept React-free (a plain
// .ts module) so it is directly unit-testable under `node --strip-types` — see
// apps/dashboard/test/hierarchy.test.ts and the dashboard-test-tsx-vs-ts split note.
//
// These MIRROR the backend model in crates/relux-core/src/hierarchy.rs + the
// agent_config / kernel validation: a Lead (`reports_to`) is an existing crew member, an
// operative cannot report to itself, and the graph stays acyclic. The backend is the
// authority and re-validates everything; this is only for a tidy picker + honest display
// (it deliberately keeps obvious cycles out of the dropdown rather than relying on a 400).

// The minimal agent shape these helpers need (a subset of ReluxAgent).
export interface HierAgent {
  id: string;
  name: string;
  reports_to?: string;
}

// Walk DOWN from `rootId`, collecting every operative in its Branch (subtree) — i.e.
// every agent that (transitively) reports to it. `rootId` itself is NOT included. Bounded
// by the roster size (each agent is visited at most once), so it is total even if the
// roster ever carried a stray cycle.
export function descendantIds(agents: HierAgent[], rootId: string): Set<string> {
  const childrenOf = new Map<string, string[]>();
  for (const a of agents) {
    if (a.reports_to) {
      const list = childrenOf.get(a.reports_to) ?? [];
      list.push(a.id);
      childrenOf.set(a.reports_to, list);
    }
  }
  const out = new Set<string>();
  const stack = [...(childrenOf.get(rootId) ?? [])];
  while (stack.length > 0) {
    const id = stack.pop() as string;
    if (out.has(id)) continue; // cycle guard
    out.add(id);
    for (const child of childrenOf.get(id) ?? []) stack.push(child);
  }
  return out;
}

// The agents eligible to be `selfId`'s Lead: everyone except `selfId` itself and its own
// Branch (its descendants) — picking one of those would be an obvious cycle the backend
// would reject anyway. When `selfId` is undefined (the create form, before the operative
// exists) every agent is eligible. Order is preserved from the input roster.
export function managerOptions(agents: HierAgent[], selfId?: string): HierAgent[] {
  if (!selfId) return agents;
  const blocked = descendantIds(agents, selfId);
  blocked.add(selfId);
  return agents.filter((a) => !blocked.has(a.id));
}

// A short, honest label for an operative's Lead, preferring the resolved display name and
// falling back to the raw id; "none" when top-level. `name` is the optional resolved
// Lead name (ReluxAgent.reports_to_name from the list endpoint).
export function leadLabel(reportsTo: string | undefined, name?: string): string {
  if (!reportsTo) return "none";
  return name && name.trim() ? name : reportsTo;
}

// A compact summary of an operative's direct reports for the card: "none", "1 report", or
// "N reports". Counts the first level of the Branch only (no deep org chart).
export function directReportsSummary(reports: string[] | undefined): string {
  const n = reports?.length ?? 0;
  if (n === 0) return "none";
  return n === 1 ? "1 report" : `${n} reports`;
}
