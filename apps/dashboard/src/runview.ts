// Pure, framework-free derivations for the Work page's run-depth view.
//
// These mirror the backend's run-depth fields (master plan section 11.3 Active
// Runs) and are unit-tested in `test/runview.test.ts`. Keeping them here (out of
// the React component) means the honest-rendering rules are pinned by tests and
// reused without a DOM. Nothing here invents data: every helper only formats or
// classifies what the API already recorded.

import type { ReluxRun, ReluxRunDetail, ReluxRunEvent } from "./api";

// The badge tone for a run status. Unknown/!terminal statuses fall back to the
// neutral "backlog" tone rather than guessing success.
export function runStatusTone(status: string | undefined): "done" | "running" | "backlog" {
  if (status === "completed") return "done";
  if (status === "running") return "running";
  return "backlog";
}

// Format a real measured duration (milliseconds) for display, or null when the
// run has no measured duration (e.g. the deterministic local echo path). We only
// ever format a value the backend actually measured.
export function formatRunDuration(durationMs: number | undefined | null): string | null {
  if (durationMs == null || !Number.isFinite(durationMs) || durationMs < 0) return null;
  if (durationMs < 1000) return `${Math.round(durationMs)} ms`;
  const secs = durationMs / 1000;
  if (secs < 60) return `${secs.toFixed(secs < 10 ? 1 : 0)} s`;
  const mins = Math.floor(secs / 60);
  const rem = Math.round(secs % 60);
  return `${mins}m ${rem}s`;
}

// Whether the UI should offer a Retry action for a run. Prefer the backend's
// derived `retryable` flag (run detail); fall back to the honest rule (a failed
// run) for the list view, which only carries the base run shape.
export function canRetryRun(run: ReluxRun | ReluxRunDetail): boolean {
  const detail = run as ReluxRunDetail;
  if (typeof detail.retryable === "boolean") return detail.retryable;
  return run.status === "failed";
}

// A one-line, honest metrics summary (cost / turns) for the run header, or null
// when the adapter reported no structured metrics. Never fabricates numbers.
export function runMetricsLine(run: ReluxRunDetail): string | null {
  const parts: string[] = [];
  if (typeof run.cost === "number") parts.push(`$${run.cost.toFixed(4)}`);
  const turns = run.usage?.["num_turns"];
  if (typeof turns === "number") parts.push(`${turns} turns`);
  const out = run.usage?.["output_tokens"];
  if (typeof out === "number") parts.push(`${out} output tokens`);
  return parts.length ? parts.join(" · ") : null;
}

// A human label for the current/last phase of a run, derived from the latest
// transcript event kind. Falls back to the run status when there are no events.
export function phaseLabel(
  phase: string | undefined,
  status: string | undefined,
): string {
  const map: Record<string, string> = {
    run_started: "Started",
    adapter_spawn: "Spawning adapter",
    adapter_output: "Adapter output",
    run_completed: "Completed",
    run_failed: "Failed",
    run_retried_from: "Retry started",
    tool_call: "Tool call",
  };
  if (phase && map[phase]) return map[phase];
  if (phase) return phase;
  return status ? status : "—";
}

// Whether a run should keep being polled (it is still in flight). Synchronous
// execution means a run is usually already terminal by the time the panel opens,
// but a panel left open during a long CLI run benefits from polling.
export function isRunInFlight(status: string | undefined): boolean {
  return status === "running" || status === "pending" || status === "waiting_for_approval";
}

// Pretty-print an event's payload object for the transcript detail, dropping the
// bulky/duplicated stdout/stderr (already shown as the excerpt) so the row stays
// legible. Returns null when there is nothing useful to show.
export function eventPayloadPreview(
  payload: ReluxRunEvent["payload"],
): string | null {
  if (!payload || typeof payload !== "object") return null;
  const trimmed: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(payload)) {
    if (k === "stdout" || k === "stderr") continue;
    if (v == null) continue;
    trimmed[k] = v;
  }
  const keys = Object.keys(trimmed);
  if (!keys.length) return null;
  return JSON.stringify(trimmed, null, 2);
}
