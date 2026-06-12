// Pure unit tests for the ad-hoc task-subtree helpers (design §6.2). Framework-free,
// so they run under `node --strip-types` without the esbuild render harness (see the
// docs note dashboard-test-tsx-vs-ts-split).
//
// Run: `npm test` (auto-discovered) or `node --test test/adhocsubtrees.test.ts`.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  childrenOfTask,
  adhocSubtaskProgress,
  parentTaskIds,
  subtaskCounts,
} from "../src/adhocsubtrees.ts";

function task(id, parent, status) {
  return {
    id,
    title: `Task ${id}`,
    input: {},
    status,
    priority: 5,
    created_by: "op",
    namespace_id: "default",
    parent_task: parent,
    created_at: "1",
    updated_at: "2",
    assigned_agent: "a1",
  };
}

// parent task_1 with three hand-made children in mixed board states; task_5 is a
// standalone top-level task (no parent).
const TASKS = [
  task("task_1", null, "running"),
  task("task_2", "task_1", "completed"),
  task("task_3", "task_1", "running"),
  task("task_4", "task_1", "blocked"),
  task("task_5", null, "created"),
];

test("childrenOfTask returns only the direct children, in stable id order", () => {
  const kids = childrenOfTask(TASKS, "task_1");
  assert.deepEqual(kids.map((c) => c.taskId), ["task_2", "task_3", "task_4"]);
  // 0-based index drives the 1, 2, 3 numbering.
  assert.deepEqual(kids.map((c) => c.index), [0, 1, 2]);
  // Live status maps to the same board buckets the columns use.
  assert.deepEqual(kids.map((c) => c.bucket), ["done", "running", "blocked"]);
});

test("a standalone task has no children (honest empty subtree)", () => {
  assert.deepEqual(childrenOfTask(TASKS, "task_5"), []);
  assert.deepEqual(childrenOfTask(TASKS, "task_does_not_exist"), []);
});

test("adhocSubtaskProgress tallies the four board buckets", () => {
  const p = adhocSubtaskProgress(childrenOfTask(TASKS, "task_1"));
  assert.deepEqual(p, { total: 3, done: 1, running: 1, blocked: 1, open: 0 });
});

test("parentTaskIds is the set of tasks that are themselves a parent", () => {
  const ids = parentTaskIds(TASKS);
  assert.equal(ids.has("task_1"), true);
  assert.equal(ids.has("task_2"), false); // a leaf, not a parent
  assert.equal(ids.has("task_5"), false);
  assert.equal(ids.size, 1);
});

test("subtaskCounts gives the direct-child count per parent", () => {
  const counts = subtaskCounts(TASKS);
  assert.equal(counts.get("task_1"), 3);
  assert.equal(counts.get("task_5"), undefined);
});

test("an empty task list yields no subtrees", () => {
  assert.deepEqual(childrenOfTask([], "task_1"), []);
  assert.equal(parentTaskIds([]).size, 0);
  assert.equal(subtaskCounts([]).size, 0);
});
