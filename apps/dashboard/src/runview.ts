// Pure, framework-free derivations for the Work page's run-depth view.
//
// These mirror the backend's run-depth fields (master plan section 11.3 Active
// Runs) and are unit-tested in `test/runview.test.ts`. Keeping them here (out of
// the React component) means the honest-rendering rules are pinned by tests and
// reused without a DOM. Nothing here invents data: every helper only formats or
// classifies what the API already recorded.

import type { ReluxRun, ReluxRunArtifact, ReluxRunDetail, ReluxRunEvent } from "./api";

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

// An honest one-line tool-call summary derived from the run transcript (master
// plan section 11.3 Active Runs lists "tool calls"). We only count the kernel's
// real tool events — `tool_call` (a permitted, attempted call), `tool_call_denied`
// (blocked by permissions), and `tool_call_failed` (errored) — and never invent a
// call from an `adapter_output`. Returns null when the transcript recorded no tool
// activity, so the UI can omit the row rather than show a misleading "0".
export function toolCallSummary(events: ReluxRunEvent[] | undefined | null): string | null {
  if (!events || events.length === 0) return null;
  let calls = 0;
  let denied = 0;
  let failed = 0;
  for (const e of events) {
    if (e.kind === "tool_call") calls += 1;
    else if (e.kind === "tool_call_denied") denied += 1;
    else if (e.kind === "tool_call_failed") failed += 1;
  }
  if (calls === 0 && denied === 0 && failed === 0) return null;
  const parts: string[] = [`${calls} tool call${calls === 1 ? "" : "s"}`];
  if (denied > 0) parts.push(`${denied} denied`);
  if (failed > 0) parts.push(`${failed} failed`);
  return parts.join(" · ");
}

// The set of known artifact-kind strings the backend emits; anything else is
// normalized to "other" so a future/unknown type still renders honestly.
const KNOWN_ARTIFACT_TYPES = new Set([
  "file",
  "diff",
  "patch",
  "log",
  "url",
  "note",
  "other",
]);

// The read-only artifact references a run captured from its adapter result
// envelope (master plan §9.6 / §15). Defensive: only accepts well-formed entries
// (object with a string `name` + `type`), so a malformed payload renders nothing
// rather than throwing. These are references — name/type/summary/source — NOT a
// diff or an apply plan; the UI lists them read-only.
export function runArtifacts(
  run: ReluxRun | ReluxRunDetail | null | undefined,
): ReluxRunArtifact[] {
  const raw = (run as unknown as { artifacts?: unknown } | null | undefined)?.artifacts;
  if (!Array.isArray(raw)) return [];
  const out: ReluxRunArtifact[] = [];
  for (const a of raw) {
    if (!a || typeof a !== "object") continue;
    const rec = a as Record<string, unknown>;
    if (typeof rec.name !== "string" || typeof rec.source !== "string") continue;
    const type =
      typeof rec.type === "string" && KNOWN_ARTIFACT_TYPES.has(rec.type)
        ? (rec.type as string)
        : "other";
    out.push({
      name: rec.name,
      type,
      source: rec.source,
      summary: typeof rec.summary === "string" ? rec.summary : undefined,
      path: typeof rec.path === "string" ? rec.path : undefined,
      bytes: typeof rec.bytes === "number" ? rec.bytes : undefined,
      truncated: rec.truncated === true ? true : undefined,
    });
  }
  return out;
}

// A short, human label for an artifact kind.
export function artifactTypeLabel(type: string): string {
  const map: Record<string, string> = {
    file: "File",
    diff: "Diff",
    patch: "Patch",
    log: "Log",
    url: "URL",
    note: "Note",
    other: "Other",
  };
  return map[type] ?? "Other";
}

// The honest reason that apply/diff/accept-reject review is unavailable on a
// Relux run with NO captured artifact references. A Relux run record carries a
// transcript, output excerpt, metrics, and (when the adapter declared them)
// read-only artifact references — but NO workspace diff plan or review verdict.
// Diff/apply/review live on the legacy Runs surface, backed by a separate run
// store whose ids are NOT Relux run ids — so we never link this run there or fake
// the controls.
export const REVIEW_APPLY_UNAVAILABLE_REASON =
  "This Relux run is a read-only execution record — a transcript, output excerpt, and metrics, " +
  "with no artifact references. Diff/apply and accept/reject review are not part of the Relux run model; " +
  "those affordances live on the legacy Runs surface, which uses a separate run store (its run ids are not Relux run ids).";

// The honest reason that apply is STILL unavailable even when a run DID capture
// artifact references: those references are read-only (name/type/summary/source),
// not a diff or an apply plan. A safe diff/apply model for Relux runs does not
// exist yet, so we list the references but never offer apply (and never fake it).
export const APPLY_PENDING_DIFF_MODEL_REASON =
  "These are read-only artifact references captured from the adapter's result envelope. " +
  "Diff preview, apply, and accept/reject review require a Relux diff/apply model that does not exist yet — " +
  "apply is unavailable until then. (The legacy Runs surface has apply, but its run ids are not Relux run ids.)";

// Whether to offer artifact/diff/apply + accept/reject review for THIS run. Apply
// is ALWAYS unavailable today: the Relux run model has no diff/apply, so even a
// run that captured artifact references cannot be applied honestly. The reason
// adapts so the UI explains the right thing (references-are-read-only vs.
// no-data-at-all) rather than hiding it or wiring dead controls. Never returns
// available: true until a real diff/apply model exists.
export function reviewApplyAvailability(
  run: ReluxRunDetail,
): { available: boolean; reason: string } {
  if (runArtifacts(run).length > 0) {
    return { available: false, reason: APPLY_PENDING_DIFF_MODEL_REASON };
  }
  return { available: false, reason: REVIEW_APPLY_UNAVAILABLE_REASON };
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
