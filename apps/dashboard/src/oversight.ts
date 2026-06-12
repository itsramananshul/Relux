// Pure, testable helpers for the Board Oversight v1 surface on the Work page
// (docs/relix-dashboard-design.md §5 Inbox / §6 board columns / §11 Active Runs).
//
// Kept dependency-free (no React, no DOM) so the bucketing + summary logic can be
// unit-tested under `node --strip-types` without the esbuild render harness (see
// docs note dashboard-test-tsx-vs-ts-split). The Work page renders these.

import type { ReluxTask, ReluxOversight } from "./api";

// The four board columns the Work kanban renders. Every task status maps to
// exactly one bucket, so no task is ever invisible (the reported gap: blocked /
// failed tasks were computed into an "other" bucket that was never rendered).
export type WorkBucket = "open" | "running" | "blocked" | "done";

// Map one task status (the snake_case wire form from relux-core TaskStatus) to its
// board column. Unknown/terminal statuses fall to "done" so a new status can never
// drop a card off the board entirely — it just lands in the terminal column.
export function taskBucket(status: string): WorkBucket {
  switch (status) {
    case "created":
    case "queued":
      return "open";
    case "leased":
    case "running":
    case "waiting_for_tool":
      return "running";
    case "waiting_for_approval":
    case "blocked":
    case "failed":
      // "Needs attention": work that is stuck (blocked / waiting on a human) or
      // ended in failure — the column an operator scans first.
      return "blocked";
    default:
      // completed / cancelled / expired and any future terminal status.
      return "done";
  }
}

// Partition a task list into the four board columns in one pass. Order within a
// column preserves the input order (the caller sorts upstream).
export function bucketTasks(tasks: ReluxTask[]): Record<WorkBucket, ReluxTask[]> {
  const out: Record<WorkBucket, ReluxTask[]> = {
    open: [],
    running: [],
    blocked: [],
    done: [],
  };
  for (const t of tasks) {
    out[taskBucket(t.status)].push(t);
  }
  return out;
}

// One dense oversight chip: a short label, its count, and a tone class that maps to
// the existing B&W badge vocabulary (never a loud color fill — design §12).
export interface OversightChip {
  label: string;
  value: number;
  // Maps to a `badge <tone>` class already in styles.css.
  tone: "running" | "blocked" | "failed" | "queued" | "done";
}

// Build the oversight count strip from the composed summary. Only the operationally
// meaningful counts are shown (the raw totals like plugin count live elsewhere); the
// order is "what is live → what is stuck → what needs me".
export function oversightCountChips(counts: ReluxOversight["counts"]): OversightChip[] {
  return [
    { label: "Active runs", value: counts.active_runs, tone: "running" },
    { label: "Open tasks", value: counts.open_tasks, tone: "queued" },
    { label: "Blocked", value: counts.blocked, tone: "blocked" },
    { label: "Failed", value: counts.failed, tone: "failed" },
    { label: "Waiting approval", value: counts.waiting_approval, tone: "blocked" },
    { label: "Pending approvals", value: counts.pending_approvals, tone: "blocked" },
  ];
}

// Whether the oversight strip has anything actionable to surface beyond raw counts:
// a pending approval, a run needing attention, or a resumable continuation. Used to
// decide if the "needs attention" region renders at all (honest empty state otherwise).
export function hasOversightAttention(o: ReluxOversight | null | undefined): boolean {
  if (!o) return false;
  return (
    (o.pending_approvals?.length ?? 0) > 0 ||
    (o.attention_runs?.length ?? 0) > 0 ||
    !!o.continuation
  );
}

// The honest one-line label for the resumable-continuation Continue affordance.
// Distinguishes a loop paused at a configured limit (resume proceeds) from one
// waiting on a tool approval (the operator must approve the tool first).
export function continuationActionLabel(
  c: NonNullable<ReluxOversight["continuation"]>,
): string {
  const obs = `${c.observation_count} observation${c.observation_count === 1 ? "" : "s"} gathered`;
  if (c.awaiting_approval) {
    return `Paused awaiting tool approval — approve the pending tool first (${obs}).`;
  }
  return `Paused: ${c.reason} — Continue resumes from where it stopped (${obs}).`;
}
