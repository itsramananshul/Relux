// Pure, framework-free helpers for the Relux Work Run Detail **logs / tail**
// section (`/v1/relux/runs/:id/logs?since=`). The analogue of
// `reluxruntranscript.ts` for the bounded, redacted run-log model
// (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§10).
//
// A run log is the adapter's stdout/stderr split into per-line entries
// (classified `stdout`/`stderr`) framed by kernel-authored `system` lines, each
// carrying a 1-based `seq`. For an in-flight off-lock (parallel) run the backend
// streams lines into a live tail as the process produces them; once the run
// finalizes the canonical persisted log is served. Either way the cursor + merge
// work on `seq` (a dense numeric sequence — no zero-pad parsing needed, unlike
// the `revent_NNNN` transcript ids). Nothing here invents data: the helpers only
// order, de-duplicate, and summarize the real lines the kernel already captured +
// redacted.

import type { ReluxRunLog, ReluxRunLogLine, ReluxRunLogSource } from "./api";

// A short, stable label for a log source — the badge text in the UI.
export function runLogSourceLabel(source: ReluxRunLogSource): string {
  switch (source) {
    case "stdout":
      return "stdout";
    case "stderr":
      return "stderr";
    case "system":
      return "system";
    default:
      // Defensive: an unknown source from a newer backend still renders
      // honestly rather than throwing.
      return String(source);
  }
}

// The highest line `seq` present — the exclusive cursor for the next incremental
// poll. Returns null for an empty log (⇒ a full fetch next time).
export function latestRunLogSeq(log: ReluxRunLog | null | undefined): number | null {
  if (!log || log.lines.length === 0) return null;
  let best = -Infinity;
  for (const l of log.lines) {
    if (typeof l.seq === "number" && l.seq > best) best = l.seq;
  }
  return Number.isFinite(best) ? best : null;
}

// Merge a freshly fetched tail onto the lines we already have, de-duplicating by
// `seq` (a poll and the initial load can overlap) and keeping the result in seq
// order. Append-only by construction — the tail only carries seqs past the
// cursor — but we dedupe + sort defensively. The run-level markers
// (`dropped_lines`, truncation flags) always come from the FRESHEST fetch, since
// the backend re-sends them on every incremental response.
export function mergeRunLog(existing: ReluxRunLog | null, incoming: ReluxRunLog): ReluxRunLog {
  if (!existing) return incoming;
  const seen = new Set<number>();
  for (const l of existing.lines) seen.add(l.seq);
  const lines: ReluxRunLogLine[] = existing.lines.slice();
  for (const l of incoming.lines) {
    if (seen.has(l.seq)) continue;
    seen.add(l.seq);
    lines.push(l);
  }
  lines.sort((a, b) => a.seq - b.seq);
  return {
    run_id: incoming.run_id || existing.run_id,
    lines,
    // Markers reflect the latest known truth from the freshest response.
    dropped_lines: incoming.dropped_lines ?? existing.dropped_lines,
    stdout_truncated: incoming.stdout_truncated ?? existing.stdout_truncated,
    stderr_truncated: incoming.stderr_truncated ?? existing.stderr_truncated,
  };
}

// Whether the log carries no lines (the "No logs" empty state). A null/undefined
// log is also empty.
export function runLogIsEmpty(log: ReluxRunLog | null | undefined): boolean {
  return !log || log.lines.length === 0;
}

// A single honest truncation/redaction notice for the section header, or null
// when nothing was dropped/clamped. Combines the run-level markers so the
// operator always knows the tail is a bounded, redacted excerpt — never a claim
// of completeness it can't back up.
export function runLogTruncationNote(log: ReluxRunLog | null | undefined): string | null {
  if (!log) return null;
  const parts: string[] = [];
  const dropped = log.dropped_lines ?? 0;
  if (dropped > 0) {
    parts.push(`${dropped} earlier line${dropped === 1 ? "" : "s"} dropped`);
  }
  const streams: string[] = [];
  if (log.stdout_truncated) streams.push("stdout");
  if (log.stderr_truncated) streams.push("stderr");
  if (streams.length > 0) {
    parts.push(`${streams.join(" + ")} byte-capped`);
  }
  if (parts.length === 0) return null;
  return parts.join("; ");
}
