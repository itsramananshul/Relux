// Pure, framework-free helpers for Work hierarchy/progress v1 on the board
// (docs/relix-dashboard-design.md §6 "A progress strip on a parent: done/in-progress/
// blocked counts, a segmented bar … waiting on blockers" + §6.1, which lists
// "sub-issue nesting / workflow-checklist rendering, and per-subtree progress strips"
// as the still-pending §6/§7 targets).
//
// THE REAL HIERARCHY (no fake grouping). The only parent→child relationship the
// kernel records today is the multi-agent ORCHESTRATION: `reluxOrchestration.list()`
// returns each goal's `steps[]`, every step carries the real child `task_id`, the
// specialist `role`, the durable `outcome`, and `depends_on` (indices into the step
// array — the genuine dependency / blocked-by / blocking edges). `relux_core::Task`
// has a `parent_task` field but the kernel NEVER populates it, and there is no
// ad-hoc task→subtask link — so the orchestration IS the parent, and a task in no
// orchestration is genuinely standalone (rendered as a flat card, never given a
// fabricated parent). See docs/product-spine-implementation.md divergence ledger.
//
// This module composes TWO EXISTING reads — the orchestration list (the structure
// + dependency edges) and the live task list (the current per-child status on the
// board) — joining `step.task_id` → task so a group's progress reflects what the
// board columns actually show, never a stale single summary field. Kept dependency-
// free (no React/DOM) so it runs under `node --strip-types` (see the docs note
// dashboard-test-tsx-vs-ts-split). The Work page renders these.

import type {
  ReluxOrchestration,
  ReluxOrchestrationStatus,
  ReluxStepOutcome,
  ReluxTask,
} from "./api";
import { taskBucket, type WorkBucket } from "./oversight.ts";

// Per-group progress, keyed by the SAME four board buckets the columns use
// (oversight.ts::WorkBucket) so the strip and the columns can never disagree.
export interface GroupProgress {
  total: number;
  done: number;
  running: number;
  blocked: number; // blocked / failed / waiting-for-approval (the attention bucket)
  open: number; // created / queued (not yet started)
}

// One child of a group: the orchestration step joined to its live board task.
export interface HierarchyChild {
  // 0-based step position — drives the numbered-plan rendering (1, 2, 3 …).
  index: number;
  taskId: string;
  title: string;
  role: string;
  // The LIVE board status (snake_case TaskStatus) when the child task is present
  // in the task list; null when it is absent (filtered out, or not yet loaded) —
  // then the bucket falls back to the durable step outcome (always honest).
  status: string | null;
  bucket: WorkBucket;
  // Sibling task ids this child waits on (blocked-by) and the ids that wait on it
  // (blocking), resolved from the step's `depends_on` indices.
  blockedBy: string[];
  blocking: string[];
  // The agent the child is assigned to (live task assignment wins; else the step's
  // recorded agent_id).
  assignedAgent: string | null;
}

// One parent group on the board: an orchestration goal with its child steps,
// progress, and whether any child is currently visible on the board.
export interface WorkGroup {
  id: string;
  goal: string;
  status: ReluxOrchestrationStatus;
  children: HierarchyChild[];
  progress: GroupProgress;
  // True when at least one child task is present in the live task list, so the
  // progress reflects live board state (not only the durable orchestration record).
  hasLiveChildren: boolean;
}

// The board bucket for a child: prefer its LIVE task status (so the strip matches
// the columns); fall back to the durable step outcome when the task is absent.
function childBucket(status: string | null, outcome: ReluxStepOutcome): WorkBucket {
  if (status) return taskBucket(status);
  switch (outcome) {
    case "completed":
      return "done";
    case "failed":
    case "blocked":
      return "blocked";
    default:
      return "open"; // pending — not yet run
  }
}

// Tally a child list into the four board buckets in one pass.
export function groupProgress(children: HierarchyChild[]): GroupProgress {
  const p: GroupProgress = { total: children.length, done: 0, running: 0, blocked: 0, open: 0 };
  for (const c of children) p[c.bucket] += 1;
  return p;
}

// Join orchestrations to the live task list into board groups. Each group's
// children resolve their live status by `step.task_id`; dependency edges become
// blocked-by (this child's `depends_on`) and blocking (siblings whose `depends_on`
// names this child). Order preserves the orchestration list and step order.
export function buildWorkGroups(
  orchestrations: ReluxOrchestration[],
  tasks: ReluxTask[],
): WorkGroup[] {
  const byId = new Map(tasks.map((t) => [t.id, t]));
  return orchestrations.map((o) => {
    const steps = o.steps ?? [];
    const children: HierarchyChild[] = steps.map((s, i) => {
      const task = byId.get(s.task_id);
      const status = task ? task.status : null;
      const blockedBy = (s.depends_on ?? [])
        .map((j) => steps[j]?.task_id)
        .filter((id): id is string => typeof id === "string");
      const blocking = steps
        .filter((other) => (other.depends_on ?? []).includes(i))
        .map((other) => other.task_id);
      return {
        index: i,
        taskId: s.task_id,
        title: s.title,
        role: s.role,
        status,
        bucket: childBucket(status, s.outcome),
        blockedBy,
        blocking,
        assignedAgent: task?.assigned_agent ?? s.agent_id ?? null,
      };
    });
    return {
      id: o.id,
      goal: o.goal,
      status: o.status,
      children,
      progress: groupProgress(children),
      hasLiveChildren: children.some((c) => c.status !== null),
    };
  });
}

// Only the groups that actually have children — a planned orchestration with no
// committed steps yet is not surfaced as a parent (nothing to show).
export function nonEmptyGroups(groups: WorkGroup[]): WorkGroup[] {
  return groups.filter((g) => g.children.length > 0);
}

// One visible slice of the segmented progress bar: a bucket, its count, and its
// width percentage. Zero-count buckets are dropped so the bar shows only real
// segments. Order is done → running → blocked → open (finished work first).
export interface ProgressSegment {
  bucket: WorkBucket;
  count: number;
  pct: number; // 0..100
}

const SEGMENT_ORDER: WorkBucket[] = ["done", "running", "blocked", "open"];

export function progressSegments(p: GroupProgress): ProgressSegment[] {
  const total = p.total > 0 ? p.total : 1;
  return SEGMENT_ORDER.map((bucket) => ({
    bucket,
    count: p[bucket],
    pct: (p[bucket] / total) * 100,
  })).filter((s) => s.count > 0);
}

// A compact "2/5 done" line with an attention tail ("· 1 running · 1 blocked").
export function groupProgressLabel(p: GroupProgress): string {
  const bits = [`${p.done}/${p.total} done`];
  if (p.running > 0) bits.push(`${p.running} running`);
  if (p.blocked > 0) bits.push(`${p.blocked} blocked`);
  if (p.open > 0) bits.push(`${p.open} open`);
  return bits.join(" · ");
}

// The blocked-by summary for a child ("blocked by task_0002, task_0003"), or null
// when it has no upstream dependency.
export function blockedByLabel(c: HierarchyChild): string | null {
  if (!c.blockedBy.length) return null;
  return `blocked by ${c.blockedBy.join(", ")}`;
}

// The blocking summary for a child ("blocks task_0004"), or null when nothing
// downstream depends on it.
export function blockingLabel(c: HierarchyChild): string | null {
  if (!c.blocking.length) return null;
  return `blocks ${c.blocking.join(", ")}`;
}

// The group a given task belongs to (the parent context for the task detail
// panel), or null when the task is standalone (in no orchestration).
export function groupForTask(groups: WorkGroup[], taskId: string): WorkGroup | null {
  return groups.find((g) => g.children.some((c) => c.taskId === taskId)) ?? null;
}

// Badge tone for a board bucket, reusing the shared B&W badge vocabulary (color is
// semantic-only — design §12). Used by both the strip legend and the child rows.
export function bucketTone(bucket: WorkBucket): "done" | "in_progress" | "blocked" | "backlog" {
  switch (bucket) {
    case "done":
      return "done";
    case "running":
      return "in_progress";
    case "blocked":
      return "blocked";
    default:
      return "backlog"; // open
  }
}

// The CSS var that paints a bucket's segment in the segmented bar (semantic-only).
export function bucketColorVar(bucket: WorkBucket): string {
  switch (bucket) {
    case "done":
      return "var(--ok)";
    case "running":
      return "var(--warn)";
    case "blocked":
      return "var(--err)";
    default:
      return "var(--text-faint)"; // open
  }
}
