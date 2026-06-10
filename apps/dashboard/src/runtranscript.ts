// Pure, framework-free helpers for the legacy run-transcript live-tail
// (relix-dashboard-design §8). These back <RunTranscript>'s efficient
// incremental polling: a `since` cursor over `/v1/runs/:id/events?since=`
// fetches only the new tail, which we merge onto what we already have, and an
// honest in-flight summary (event count / current phase / last event) shown
// while a Shift runs. Kept out of the React component so the merge + summary
// rules are pinned by tests and never invent data — they only count, order,
// and humanize events the backend already recorded.

import type { RunEvent } from "./api";

// Humanize a relix/codex lifecycle kind for the "nice" view + the progress
// summary. Mirrors the durable `run_events.kind` vocabulary.
export const KIND_LABEL: Record<string, string> = {
  accepted: "accepted",
  workspace_prepared: "workspace ready",
  process_started: "process started",
  process_exited: "process exited",
  "artifacts.scan_started": "scanning changes",
  "artifacts.detected": "changes detected",
  "artifacts.scan_failed": "change-scan failed",
  result: "result",
  failed: "failed",
  continued: "continued",
  cancelled: "cancelled",
  cancel_requested: "cancel requested",
  thread_started: "thread started",
  turn_started: "turn started",
  turn_completed: "turn completed",
  "apply.plan": "apply plan",
  "apply.started": "apply started",
  "apply.applied": "applied",
  "apply.conflicted": "apply conflicted",
  "apply.failed": "apply failed",
  review: "review",
  // Transcript-body kinds (the model working), so the progress chip reads
  // naturally mid-run instead of echoing the raw kind.
  assistant_message: "thinking",
  tool_use: "tool call",
  command: "command",
  file_change: "editing files",
  permission_denied: "permission denied",
  usage: "usage",
  stderr: "stderr",
  error: "error",
};

export function kindLabel(kind?: string): string {
  return KIND_LABEL[kind ?? ""] ?? (kind ?? "event");
}

// The highest `event_id` in a transcript — the exclusive cursor for the next
// incremental fetch. Events without an id (shouldn't happen for durable rows)
// don't advance the cursor. Returns 0 for an empty transcript (⇒ full fetch).
export function latestEventId(events: RunEvent[]): number {
  let max = 0;
  for (const e of events) {
    if (typeof e.event_id === "number" && e.event_id > max) max = e.event_id;
  }
  return max;
}

// Merge a freshly fetched tail onto the events we already have, de-duplicating
// by `event_id` (a poll + an SSE-triggered fetch can overlap) and keeping the
// result in `event_id` order. Events lacking an id are appended verbatim (we
// can't dedupe them, but we never drop a real event). This is append-only by
// construction — the tail only ever carries ids greater than our cursor.
export function mergeRunEvents(existing: RunEvent[], incoming: RunEvent[]): RunEvent[] {
  if (incoming.length === 0) return existing;
  const seen = new Set<number>();
  for (const e of existing) {
    if (typeof e.event_id === "number") seen.add(e.event_id);
  }
  const merged = existing.slice();
  for (const e of incoming) {
    if (typeof e.event_id === "number") {
      if (seen.has(e.event_id)) continue;
      seen.add(e.event_id);
    }
    merged.push(e);
  }
  // Stable chronological order by id; id-less events keep their relative spot.
  merged.sort((a, b) => (a.event_id ?? Number.MAX_SAFE_INTEGER) - (b.event_id ?? Number.MAX_SAFE_INTEGER));
  return merged;
}

// An honest at-a-glance summary of an in-flight transcript: how many events
// have landed, the current phase (the latest event, humanized), and when the
// last event arrived. No progress percentage — the backend never reports one,
// so we never fabricate one.
export interface TranscriptProgress {
  count: number;
  phase: string | null;
  lastTs: number | null;
}

export function runTranscriptProgress(events: RunEvent[]): TranscriptProgress {
  if (events.length === 0) {
    return { count: 0, phase: null, lastTs: null };
  }
  const last = events[events.length - 1];
  return {
    count: events.length,
    phase: kindLabel(last.kind),
    lastTs: typeof last.ts === "number" ? last.ts : null,
  };
}

// "just now" / "12s ago" / "3m ago" for the last-event clock on the progress
// chip. `nowSecs` is injected so the helper stays pure + testable. Returns null
// when there's no timestamp to age.
export function lastEventAgo(lastTs: number | null, nowSecs: number): string | null {
  if (lastTs == null) return null;
  const delta = Math.max(0, Math.floor(nowSecs - lastTs));
  if (delta < 2) return "just now";
  if (delta < 60) return `${delta}s ago`;
  const mins = Math.floor(delta / 60);
  return `${mins}m ago`;
}
