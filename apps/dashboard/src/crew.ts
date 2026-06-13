// Pure, dependency-free derivations for the Crew page. Kept out of Crew.tsx so the
// roster-state logic is unit-testable without rendering React (the page's loading /
// error / empty / populated states are otherwise only reachable through useAsync,
// which never fetches under renderToStaticMarkup).

import type { ReluxAgent } from "./api";

// The seeded control-plane operative. The kernel always seeds Prime, so the roster
// is never literally empty in normal operation; "only Prime" is the real "no crew
// built yet" signal. Mirrors the kernel id (crates/relux-kernel/src/store.rs).
export const PRIME_AGENT_ID = "prime";

// True when the roster carries no operatives beyond Prime. This — not a zero-length
// list — is the honest "create your first crew member" signal, because Prime is
// always present. Drives the actionable empty state on the Crew page (RELUX_MASTER_PLAN
// §6, §8.1). A truly empty roster (no Prime either, e.g. a control plane that could
// not be reached) is handled separately as the loading/error path.
export function isPrimeOnlyRoster(agents: ReluxAgent[]): boolean {
  return agents.length > 0 && agents.every((a) => a.id === PRIME_AGENT_ID);
}
