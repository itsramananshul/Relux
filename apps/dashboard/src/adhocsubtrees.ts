// Pure, framework-free helpers for AD-HOC task subtrees on the board
// (docs/relix-dashboard-design.md §6.2 "ad-hoc (non-orchestration) task subtrees").
//
// THE SECOND REAL PARENT→CHILD LINK. Until now the multi-agent ORCHESTRATION was the
// only parent→child relationship the kernel recorded (see workhierarchy.ts). The
// kernel now also populates `relux_core::Task.parent_task` for tasks an operator
// breaks down by hand, so a task can be a subtask of another OUTSIDE any
// orchestration. This module joins the FLAT task list to itself by that edge —
// `child.parent_task === parent.id` — into the same progress-strip + numbered-list
// shape the orchestration groups use, so the two hierarchies render identically.
//
// Kept dependency-free (no React/DOM) so it runs under `node --strip-types` (see the
// docs note dashboard-test-tsx-vs-ts-split). The Work page renders these.

import type { ReluxTask } from "./api";
import { taskBucket, type WorkBucket } from "./oversight.ts";
import { type GroupProgress } from "./workhierarchy.ts";

// One ad-hoc subtask of a parent task: the child task joined to its live board
// status. Shaped to mirror workhierarchy's HierarchyChild so the same renderers
// (numbered checklist, progress strip) work for both hierarchies.
export interface AdhocChild {
  // 0-based position among siblings (sorted by task id) — drives the 1, 2, 3 … plan.
  index: number;
  taskId: string;
  title: string;
  // The LIVE board status (snake_case TaskStatus) — always present here, since an
  // ad-hoc child is a real task in the same flat list (unlike an orchestration step,
  // which can be off-board). Drives the bucket directly.
  status: string;
  bucket: WorkBucket;
  assignedAgent: string | null;
}

// The direct children of `parentId` (one level), in stable task-id order. A task is a
// child iff its `parent_task` names the parent — the genuine kernel edge, never a
// fabricated grouping. Returns [] for a standalone task (the honest empty subtree).
export function childrenOfTask(tasks: ReluxTask[], parentId: string): AdhocChild[] {
  return tasks
    .filter((t) => t.parent_task === parentId)
    .sort((a, b) => a.id.localeCompare(b.id))
    .map((t, index) => ({
      index,
      taskId: t.id,
      title: t.title,
      status: t.status,
      bucket: taskBucket(t.status),
      assignedAgent: t.assigned_agent ?? null,
    }));
}

// Tally an ad-hoc child list into the four board buckets (the same GroupProgress
// shape + buckets the orchestration progress strip uses, so the two can't disagree).
export function adhocSubtaskProgress(children: AdhocChild[]): GroupProgress {
  const p: GroupProgress = { total: children.length, done: 0, running: 0, blocked: 0, open: 0 };
  for (const c of children) p[c.bucket] += 1;
  return p;
}

// The set of task ids that are themselves a parent in the ad-hoc tree (some task
// names them as its `parent_task`). Used to mark a board card as having sub-work
// without re-scanning the whole list per card.
export function parentTaskIds(tasks: ReluxTask[]): Set<string> {
  const ids = new Set<string>();
  for (const t of tasks) {
    if (t.parent_task) ids.add(t.parent_task);
  }
  return ids;
}

// The direct-child count for every parent in the ad-hoc tree, keyed by parent id.
// (A board card shows "↳ N" from this without filtering the list itself.)
export function subtaskCounts(tasks: ReluxTask[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const t of tasks) {
    if (!t.parent_task) continue;
    counts.set(t.parent_task, (counts.get(t.parent_task) ?? 0) + 1);
  }
  return counts;
}
