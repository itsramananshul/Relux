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

// The default quiet-period threshold (seconds) before an in-flight run is
// flagged as showing no activity. Short enough to be honest about a stall, long
// enough to not flicker between two normal 1.5s polls.
export const RELUX_RUN_STALL_SECS = 10;

// An honest "no activity" signal for an in-flight run: when the run is still
// running but no new transcript event (and no phase change) has arrived for at
// least `thresholdSecs`, return human text like `No activity for 14s`. Returns
// null while activity is recent (or unknown), so the UI shows the normal live
// indicator instead. `lastActivityAtMs` / `nowMs` are real wall-clock millis
// (injected so the helper stays pure + testable). This is NOT a progress bar —
// it only reports elapsed silence, never fabricated forward motion.
export function noActivityLabel(
  lastActivityAtMs: number | null,
  nowMs: number,
  thresholdSecs: number = RELUX_RUN_STALL_SECS,
): string | null {
  if (lastActivityAtMs == null) return null;
  const elapsed = Math.floor((nowMs - lastActivityAtMs) / 1000);
  if (elapsed < thresholdSecs) return null;
  if (elapsed < 60) return `No activity for ${elapsed}s`;
  const mins = Math.floor(elapsed / 60);
  const secs = elapsed % 60;
  return `No activity for ${mins}m ${secs}s`;
}
