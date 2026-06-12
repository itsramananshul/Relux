// Pure, framework-free helpers for SAFE TASK REPARENTING on the board
// (docs/relix-dashboard-design.md §6.6 "reparent / reorder a subtask"). Given the flat
// task list, these compute the SAFE set of candidate parents for a task being moved —
// the exact same safety the kernel enforces (`relux_core::would_create_task_cycle`),
// mirrored client-side so the UI never even OFFERS a parent the backend would reject.
//
// THE EDGE IS `parent_task` (the real kernel edge — see adhocsubtrees.ts). A task may
// be moved under any OTHER task that (a) is not itself, (b) is not one of its own
// descendants (else the move would close a cycle), (c) shares its namespace (a subtask
// lives in its parent's scope), and (d) is not already its parent (that move is a
// no-op). When no task satisfies all four, there is honestly NO valid parent — the UI
// says so rather than presenting an empty/misleading control.
//
// Kept dependency-free (no React/DOM) so it runs under `node --strip-types` (see the
// docs note dashboard-test-tsx-vs-ts-split). The Work page's reparent control renders
// these.

import type { ReluxTask } from "./api";

// The child→parent edge map of the live task list (one entry per task that carries a
// `parent_task`). Mirrors the kernel's `task_parent_map` — the graph the cycle check
// walks. A standalone task simply has no entry.
function parentOf(tasks: ReluxTask[]): Map<string, string> {
  const m = new Map<string, string>();
  for (const t of tasks) {
    if (t.parent_task) m.set(t.id, t.parent_task);
  }
  return m;
}

// Hard cap on how deep the ancestor walk goes — defence in depth against a malformed/
// cyclic persisted map, so the walk is always total. Mirrors the kernel's
// `relux_core::MAX_TASK_DEPTH` (64).
const MAX_TASK_DEPTH = 64;

// True iff `taskId` is a (transitive) ancestor of `candidate` — i.e. `candidate` sits
// in `taskId`'s subtree, so parenting `taskId` UNDER `candidate` would close a loop.
// Proper-descendant semantics + a bounded, cycle-guarded walk (the same shape as the
// kernel's `is_in_task_subtree` / `task_ancestors`). Total even on a cyclic map.
function isDescendantOf(taskId: string, candidate: string, edges: Map<string, string>): boolean {
  if (taskId === candidate) return false;
  const seen = new Set<string>([candidate]);
  let current = candidate;
  for (let i = 0; i < MAX_TASK_DEPTH; i++) {
    const next = edges.get(current);
    if (next === undefined || seen.has(next)) break;
    if (next === taskId) return true;
    seen.add(next);
    current = next;
  }
  return false;
}

// The full set of `taskId`'s descendants (every task whose ancestor chain passes
// through `taskId`). Used both to filter candidate parents and by render/tests to
// reason about a subtree. Excludes `taskId` itself.
export function taskDescendants(tasks: ReluxTask[], taskId: string): Set<string> {
  const edges = parentOf(tasks);
  const out = new Set<string>();
  for (const t of tasks) {
    if (t.id !== taskId && isDescendantOf(taskId, t.id, edges)) out.add(t.id);
  }
  return out;
}

// The SAFE candidate parents for `taskId`, in stable id order: every task that is not
// itself, not one of its descendants (no cycle), not its current parent (no-op), and in
// the same namespace. Returns [] when `taskId` is unknown or no task qualifies — the
// honest "nowhere safe to move this" state the UI surfaces verbatim.
export function candidateParents(tasks: ReluxTask[], taskId: string): ReluxTask[] {
  const self = tasks.find((t) => t.id === taskId);
  if (!self) return [];
  const descendants = taskDescendants(tasks, taskId);
  return tasks
    .filter(
      (t) =>
        t.id !== taskId &&
        !descendants.has(t.id) &&
        t.namespace_id === self.namespace_id &&
        t.id !== (self.parent_task ?? undefined),
    )
    .sort((a, b) => a.id.localeCompare(b.id));
}
