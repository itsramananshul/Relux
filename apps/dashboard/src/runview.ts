// Pure, framework-free derivations for the Work page's run-depth view.
//
// These mirror the backend's run-depth fields (master plan section 11.3 Active
// Runs) and are unit-tested in `test/runview.test.ts`. Keeping them here (out of
// the React component) means the honest-rendering rules are pinned by tests and
// reused without a DOM. Nothing here invents data: every helper only formats or
// classifies what the API already recorded.

import type {
  ReluxProposedChange,
  ReluxRun,
  ReluxRunArtifact,
  ReluxRunDetail,
  ReluxRunEvent,
  ReluxRunSession,
} from "./api";

// The badge tone for a run status. `failed` maps to the shared error-red chip
// tone (`.badge.blocked` — the same muted `var(--err)` chip the transcript uses
// for "permission denied"), so a failed run's status badge carries the "error"
// meaning from the design-system status vocabulary (relix-dashboard-design §12:
// "blocked, live/running, done/healthy, error" rendered as restrained badges;
// "No silent failures: every failed run is visible") and reads consistently with
// the red failure-reason text beneath it — never the neutral tone. `cancelled`
// is a terminal non-error, so it stays neutral. Unknown/pre-terminal statuses
// (pending, waiting_for_approval, …) fall back to "backlog" rather than guessing.
export function runStatusTone(status: string | undefined): "done" | "running" | "backlog" | "blocked" {
  if (status === "completed") return "done";
  if (status === "running") return "running";
  if (status === "failed") return "blocked";
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

// The durable session-identity / handoff metadata a run captured from its
// adapter result envelope (HERMES_OPENCLAW_DEEP_AUDIT §3). Defensive: only accepts
// a well-formed object (string `adapter_session_id` + `source`, boolean
// `resume_supported`), so a malformed payload renders nothing rather than
// throwing. Never invents a session — returns null when none was captured.
export function runSession(
  run: ReluxRun | ReluxRunDetail | null | undefined,
): ReluxRunSession | null {
  const raw = (run as unknown as { session?: unknown } | null | undefined)?.session;
  if (!raw || typeof raw !== "object") return null;
  const rec = raw as Record<string, unknown>;
  if (typeof rec.adapter_session_id !== "string" || typeof rec.source !== "string") return null;
  return {
    adapter_session_id: rec.adapter_session_id,
    source: rec.source,
    resume_supported: rec.resume_supported === true,
  };
}

// Whether the UI should offer a Resume action for a run. Prefer the backend's
// derived `resumable` flag (run detail); fall back to the honest local rule (a
// terminal run that captured a resumable session) for the list view, which only
// carries the base run shape. Distinct from `canRetryRun` — resume continues the
// captured provider session; retry starts cold.
export function canResumeRun(run: ReluxRun | ReluxRunDetail): boolean {
  const detail = run as ReluxRunDetail;
  if (typeof detail.resumable === "boolean") return detail.resumable;
  const session = runSession(run);
  const terminal = run.status === "completed" || run.status === "failed";
  return terminal && !!session?.resume_supported;
}

// Whether the UI should offer a Cancel action for a run. A cancel only reaches an
// in-flight, process-backed run, so we offer it for a RUNNING run; the backend is
// the honest authority (a run that turns out not to be a cancellable off-lock
// process run returns `not_running` and the UI surfaces that message rather than
// silently doing nothing). HERMES_OPENCLAW_DEEP_AUDIT §8/§26.
export function canCancelRun(run: ReluxRun | ReluxRunDetail): boolean {
  return run.status === "running";
}

// An honest one-line label for a run's session handoff: the source + whether
// Relux can resume it. When a session was captured but is NOT resumable, the id is
// still kept for handoff/audit/manual continuation — we say so rather than imply a
// resume that would be refused. Returns null when no session was captured.
export function sessionHandoffLabel(
  run: ReluxRun | ReluxRunDetail | null | undefined,
): string | null {
  const session = runSession(run);
  if (!session) return null;
  if (session.resume_supported) {
    return `${session.source} session — resume supported (continues this session)`;
  }
  return `${session.source} session — resume not supported here (kept for handoff/audit; use Re-run for a fresh attempt)`;
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
    run_resumed_from: "Resume started",
    tool_call: "Tool call",
    // MCP tool calls made inside a run get distinct, bounded transcript events
    // (`docs/mcp.md` "Run transcript visibility").
    mcp_tool_call: "MCP tool call",
    mcp_tool_call_denied: "MCP tool call denied",
    mcp_tool_call_failed: "MCP tool call failed",
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
// call from an `adapter_output`. MCP tool calls in a run carry distinct
// `mcp_tool_call*` kinds (`docs/mcp.md` "Run transcript visibility"); an MCP call
// IS a tool call, so it folds into the same totals. Returns null when the
// transcript recorded no tool activity, so the UI can omit the row rather than
// show a misleading "0".
export function toolCallSummary(events: ReluxRunEvent[] | undefined | null): string | null {
  if (!events || events.length === 0) return null;
  let calls = 0;
  let denied = 0;
  let failed = 0;
  for (const e of events) {
    if (e.kind === "tool_call" || e.kind === "mcp_tool_call") calls += 1;
    else if (e.kind === "tool_call_denied" || e.kind === "mcp_tool_call_denied") denied += 1;
    else if (e.kind === "tool_call_failed" || e.kind === "mcp_tool_call_failed") failed += 1;
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

// The set of known proposed-change lifecycle states; anything else normalizes to
// "proposed" so a future/unknown status still renders honestly.
const KNOWN_CHANGE_STATUSES = new Set(["proposed", "approved", "rejected", "applied"]);

// The set of known proposed-change actions; anything else (or absent) normalizes
// to "replace" so older records and unknown future actions still render honestly.
const KNOWN_CHANGE_ACTIONS = new Set(["replace", "create", "rename", "delete"]);

// The reviewable proposed file changes a run captured from its adapter result
// envelope (master plan §15 / §9.6). Defensive: only accepts well-formed entries
// (object with string `path` + `new_content` + `status`), so a malformed payload
// renders nothing rather than throwing. Unlike `runArtifacts`, these carry the
// full proposed content and can be approved + applied.
export function runProposedChanges(
  run: ReluxRun | ReluxRunDetail | null | undefined,
): ReluxProposedChange[] {
  const raw = (run as unknown as { proposed_changes?: unknown } | null | undefined)
    ?.proposed_changes;
  if (!Array.isArray(raw)) return [];
  const out: ReluxProposedChange[] = [];
  for (const c of raw) {
    if (!c || typeof c !== "object") continue;
    const rec = c as Record<string, unknown>;
    if (
      typeof rec.path !== "string" ||
      typeof rec.new_content !== "string" ||
      typeof rec.source !== "string"
    )
      continue;
    const status =
      typeof rec.status === "string" && KNOWN_CHANGE_STATUSES.has(rec.status)
        ? (rec.status as string)
        : "proposed";
    const action =
      typeof rec.action === "string" && KNOWN_CHANGE_ACTIONS.has(rec.action)
        ? (rec.action as string)
        : "replace";
    out.push({
      path: rec.path,
      action,
      dest_path: typeof rec.dest_path === "string" ? rec.dest_path : undefined,
      new_content: rec.new_content,
      new_sha256: typeof rec.new_sha256 === "string" ? rec.new_sha256 : "",
      bytes: typeof rec.bytes === "number" ? rec.bytes : rec.new_content.length,
      source: rec.source,
      status,
      baseline_sha256: typeof rec.baseline_sha256 === "string" ? rec.baseline_sha256 : undefined,
      review_note: typeof rec.review_note === "string" ? rec.review_note : undefined,
      refused_reason: typeof rec.refused_reason === "string" ? rec.refused_reason : undefined,
      applied_at: typeof rec.applied_at === "string" ? rec.applied_at : undefined,
    });
  }
  return out;
}

// A short, human label for a proposed-change action ("replace" / "create" /
// "rename" / "delete"). Anything unrecognized reads as "Replace" (the default).
export function proposedChangeActionLabel(action: string | undefined): string {
  if (action === "create") return "Create";
  if (action === "rename") return "Rename";
  if (action === "delete") return "Delete";
  return "Replace";
}

// Whether this change creates a brand-new file (vs replacing an existing one).
// A create needs no baseline; a replace does. Treats a missing action as replace.
export function isCreateProposedChange(change: ReluxProposedChange): boolean {
  return change.action === "create";
}

// Whether this change moves a file (a rename). A rename has a destination path
// and, like a replace, needs the source baseline to apply.
export function isRenameProposedChange(change: ReluxProposedChange): boolean {
  return change.action === "rename";
}

// Whether this change removes a file (a delete). A delete carries no new content
// and, like a replace, needs the source baseline to apply.
export function isDeleteProposedChange(change: ReluxProposedChange): boolean {
  return change.action === "delete";
}

// A "source → destination" label for a rename, or just the path for a
// replace/create. Used by the run view to show what a change touches at a glance.
export function proposedChangePathLabel(change: ReluxProposedChange): string {
  if (isRenameProposedChange(change) && change.dest_path) {
    return `${change.path} → ${change.dest_path}`;
  }
  return change.path;
}

// A short, human label for a proposed-change status.
export function proposedChangeStatusLabel(status: string): string {
  const map: Record<string, string> = {
    proposed: "Proposed",
    approved: "Approved",
    rejected: "Rejected",
    applied: "Applied",
  };
  return map[status] ?? "Proposed";
}

// The badge tone for a proposed-change status. Applied/approved read as success;
// rejected as neutral; proposed as the in-flight tone.
export function proposedChangeStatusTone(
  status: string,
): "done" | "running" | "backlog" {
  if (status === "applied" || status === "approved") return "done";
  if (status === "proposed") return "running";
  return "backlog";
}

// Whether an operator can review (approve/reject) this change: only while it is
// still `proposed`. An applied change is terminal; a decided one can be re-decided
// only by the backend rule (review after applied is refused there).
export function canReviewProposedChange(change: ReluxProposedChange): boolean {
  return change.status === "proposed";
}

// Whether an operator can apply this change from the UI: it must be `approved`,
// and a `replace`, `rename`, or `delete` additionally needs a baseline hash (apply
// refuses without one in v1). A `create` needs no baseline (there is no prior
// file). The backend re-checks everything; this just avoids offering a dead button.
export function canApplyProposedChange(change: ReluxProposedChange): boolean {
  if (change.status !== "approved") return false;
  if (isCreateProposedChange(change)) return true;
  return typeof change.baseline_sha256 === "string";
}

// The indices of the changes that are still reviewable (status "proposed"), in
// list order. Drives the batch "Approve all" affordance.
export function reviewableProposedChangeIndices(
  changes: ReluxProposedChange[],
): number[] {
  const out: number[] = [];
  changes.forEach((c, i) => {
    if (canReviewProposedChange(c)) out.push(i);
  });
  return out;
}

// The indices of the changes eligible for a transactional apply (approved + a
// baseline hash), in list order. This is exactly the selection the dashboard
// sends to the multi-file apply endpoint; the backend re-validates everything.
export function applyEligibleProposedChangeIndices(
  changes: ReluxProposedChange[],
): number[] {
  const out: number[] = [];
  changes.forEach((c, i) => {
    if (canApplyProposedChange(c)) out.push(i);
  });
  return out;
}

// Whether to offer the batch (multi-file) review/apply toolbar: only when a run
// has MORE THAN ONE proposed change. With a single change the existing per-change
// flow is sufficient (master plan §15: "If only one, existing flow remains fine").
export function showBatchProposedChangeControls(
  changes: ReluxProposedChange[],
): boolean {
  return changes.length > 1;
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

// The reason apply IS available: this run captured reviewable proposed changes
// (full-content replacements with a baseline hash), which ARE the Relux diff/apply
// model. The Proposed Changes section drives the real review + apply controls.
export const APPLY_AVAILABLE_REASON =
  "This run proposed reviewable file changes. Approve a change to enable apply; applying writes the " +
  "new content into the run's controlled workspace root only after a baseline-conflict check (apply is " +
  "refused without a baseline hash or a configured workspace, and never overwrites a file that changed).";

// Whether to offer the proposed-change review + apply controls for THIS run.
// Apply is available ONLY when the run captured proposed changes — those carry
// content + a baseline hash and ARE the Relux diff/apply model. A run that only
// captured read-only artifact references (or nothing) still has no apply, so the
// reason adapts honestly rather than hiding it or wiring dead controls.
export function reviewApplyAvailability(
  run: ReluxRunDetail,
): { available: boolean; reason: string } {
  if (runProposedChanges(run).length > 0) {
    return { available: true, reason: APPLY_AVAILABLE_REASON };
  }
  if (runArtifacts(run).length > 0) {
    return { available: false, reason: APPLY_PENDING_DIFF_MODEL_REASON };
  }
  return { available: false, reason: REVIEW_APPLY_UNAVAILABLE_REASON };
}

// A short human label for a structured run-failure class (kernel
// `relux_core::run_failure::RunFailureClass`). An unknown/absent class falls back
// to a neutral label so a new server class never renders blank.
export function failureClassLabel(failureClass: string | undefined): string | null {
  if (!failureClass) return null;
  switch (failureClass) {
    case "transient_provider":
      return "Transient provider error";
    case "auth_required":
      return "Authentication required";
    case "adapter_missing":
      return "Adapter not available";
    case "permission_denied":
      return "Permission denied";
    case "invalid_prompt":
      return "Invalid request";
    case "timeout":
      return "Timed out";
    case "cancelled":
      return "Cancelled";
    case "output_validation":
      return "Output validation failed";
    case "unknown":
      return "Unknown failure";
    default:
      return failureClass;
  }
}

// The B&W status tone for a failure class: a transient that will retry is the
// "running/in-progress" tone (auto-recovering), an operator-action failure is the
// "blocked" tone, an intentional cancel is neutral backlog.
export function failureClassTone(
  failureClass: string | undefined,
): "running" | "blocked" | "backlog" {
  switch (failureClass) {
    case "transient_provider":
    case "timeout":
      return "running";
    case "cancelled":
      return "backlog";
    default:
      return "blocked";
  }
}

// The honest one-line recovery status for a failed run: whether a bounded
// transient retry is scheduled / exhausted, or the operator must act. Returns
// null when the run did not fail or carries no class. Pure (no clock): the caller
// passes `nowSecs` so the "eligible now vs. waiting" read is testable.
export function recoveryStatusLine(
  run: ReluxRun | ReluxRunDetail,
  nowSecs: number,
): string | null {
  const failureClass = (run as ReluxRun).failure_class;
  if (!failureClass) return null;
  const retry = (run as ReluxRun).retry;
  if (retry) {
    if (retry.exhausted) {
      return `Automatic retries exhausted (${retry.max_attempts} attempts). Retry manually if still wanted.`;
    }
    const nb = retry.not_before_secs;
    if (typeof nb === "number" && nb > nowSecs) {
      return `Retry ${retry.attempt + 1}/${retry.max_attempts} scheduled in ~${formatWait(nb - nowSecs)} (transient — auto-recovering).`;
    }
    return `Retry ${retry.attempt + 1}/${retry.max_attempts} is due (transient — re-runs on the next tick or a manual retry).`;
  }
  if (failureClass === "cancelled") {
    return "Cancelled. Start a fresh run if it is still wanted.";
  }
  return "Needs operator action before it can succeed.";
}

// Compact, approximate wait formatting for the recovery line (seconds/minutes/hours).
function formatWait(secs: number): string {
  if (secs < 90) return `${Math.max(1, Math.round(secs))}s`;
  if (secs < 90 * 60) return `${Math.round(secs / 60)}m`;
  return `${Math.round(secs / 3600)}h`;
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
