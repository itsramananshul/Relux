// Pure, framework-free helpers for the Relux Work Run Detail live-tail.
//
// These back <RunDetailPanel>'s efficient incremental polling for the Relux run
// model (`/v1/relux/runs/:id/events?since=`), the analogue of the legacy
// bridge's `runtranscript.ts`. The Relux transcript event shape differs from the
// legacy `RunEvent`: its `id` is a string (`revent_NNNN`) and its `ts` is a
// LOGICAL-clock string (ordering, not wall time), so the cursor + merge work on
// the string id and the stalled-run signal is driven by real wall-clock elapsed
// time tracked in the component — never derived from `ts`.
//
// Nothing here invents data: the helpers only order, de-duplicate, and age the
// real events the kernel already recorded. No fabricated progress.

import type { ReluxRunEvent } from "./api";
import { RUN_STALL_SECS, noActivityLabel } from "./runstall.ts";

// The stalled-run signal is event-model-agnostic (pure time math), so it lives
// in the shared `./runstall` module and is used identically by the legacy
// `<RunTranscript>` surface. Re-exported here so existing Relux imports/tests
// keep their co-located names; `RELUX_RUN_STALL_SECS` is the historical alias.
export { noActivityLabel };
export const RELUX_RUN_STALL_SECS = RUN_STALL_SECS;

// Parse the numeric sequence from a `revent_NNNN` event id. The kernel mints ids
// off a monotonic counter, so the numeric suffix orders events even past the
// 4-digit zero-pad width (where a lexicographic compare on the raw id would
// break). Returns null for an id with no parseable suffix.
export function reluxEventSeq(id: string | undefined): number | null {
  if (typeof id !== "string") return null;
  const suffix = id.slice(id.lastIndexOf("_") + 1);
  if (suffix === "") return null;
  const n = Number.parseInt(suffix, 10);
  return Number.isFinite(n) ? n : null;
}

// The highest event id in a transcript — the exclusive cursor for the next
// incremental fetch. Returns null for an empty transcript (⇒ a full fetch). We
// return the real id STRING (not the parsed number) so the value handed back to
// the API is a genuine event id the backend can re-parse.
export function latestReluxEventId(events: ReluxRunEvent[]): string | null {
  let bestId: string | null = null;
  let bestSeq = -Infinity;
  for (const e of events) {
    const seq = reluxEventSeq(e.id);
    if (seq != null && seq > bestSeq) {
      bestSeq = seq;
      bestId = e.id;
    }
  }
  return bestId;
}

// Merge a freshly fetched tail onto the events we already have, de-duplicating
// by `id` (a poll and the initial load can overlap) and keeping the result in
// sequence order. Append-only by construction — the tail only ever carries ids
// past our cursor — but we still dedupe + sort defensively. Events with an
// unparseable id keep their relative spot at the end rather than being dropped.
export function mergeReluxRunEvents(
  existing: ReluxRunEvent[],
  incoming: ReluxRunEvent[],
): ReluxRunEvent[] {
  if (incoming.length === 0) return existing;
  const seen = new Set<string>();
  for (const e of existing) seen.add(e.id);
  const merged = existing.slice();
  for (const e of incoming) {
    if (seen.has(e.id)) continue;
    seen.add(e.id);
    merged.push(e);
  }
  merged.sort((a, b) => {
    const sa = reluxEventSeq(a.id);
    const sb = reluxEventSeq(b.id);
    return (sa ?? Number.MAX_SAFE_INTEGER) - (sb ?? Number.MAX_SAFE_INTEGER);
  });
  return merged;
}
