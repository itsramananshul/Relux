// Pure unit tests for the safe-reparent helpers (design §6.6). Framework-free, so they
// run under `node --strip-types` without the esbuild render harness (see the docs note
// dashboard-test-tsx-vs-ts-split).
//
// Run: `npm test` (auto-discovered) or `node --test test/reparent.test.ts`.

import { test } from "node:test";
import assert from "node:assert/strict";
import { candidateParents, taskDescendants } from "../src/reparent.ts";

function task(id, parent, ns = "default") {
  return {
    id,
    title: `Task ${id}`,
    input: {},
    status: "created",
    priority: 5,
    created_by: "op",
    namespace_id: ns,
    parent_task: parent,
    created_at: "1",
    updated_at: "2",
    assigned_agent: "a1",
  };
}

// A chain task_1 -> task_2 -> task_3 (task_3 is the deepest leaf), a sibling task_4
// under task_1, a standalone task_5, and task_6 in a DIFFERENT namespace.
const TASKS = [
  task("task_1", null),
  task("task_2", "task_1"),
  task("task_3", "task_2"),
  task("task_4", "task_1"),
  task("task_5", null),
  task("task_6", null, "other"),
];

test("taskDescendants returns the whole subtree (transitive), excluding self", () => {
  // task_1's subtree: 2, 3 (under 2), and 4.
  assert.deepEqual([...taskDescendants(TASKS, "task_1")].sort(), ["task_2", "task_3", "task_4"]);
  // task_2's subtree: just 3.
  assert.deepEqual([...taskDescendants(TASKS, "task_2")], ["task_3"]);
  // A leaf / standalone has no descendants.
  assert.deepEqual([...taskDescendants(TASKS, "task_3")], []);
  assert.deepEqual([...taskDescendants(TASKS, "task_5")], []);
});

test("candidateParents excludes self, descendants, the current parent, and cross-namespace", () => {
  // For task_1 (root of a subtree): self (1) and its descendants (2, 3, 4) are out;
  // task_1 is already top-level so no current-parent exclusion; task_6 is cross-ns.
  // Only task_5 (same-ns standalone) remains.
  assert.deepEqual(candidateParents(TASKS, "task_1").map((t) => t.id), ["task_5"]);
});

test("candidateParents excludes the current parent (a no-op move) but keeps other ancestors? no — only proper, safe targets", () => {
  // task_3's current parent is task_2 (excluded as a no-op). task_3 has no descendants,
  // so the only cycle exclusion is none. Same-ns, non-self, non-current-parent: 1, 4, 5.
  // (task_2 is the current parent → excluded; task_6 cross-ns → excluded.)
  assert.deepEqual(candidateParents(TASKS, "task_3").map((t) => t.id), ["task_1", "task_4", "task_5"]);
});

test("candidateParents never offers a descendant (would close a cycle)", () => {
  const ids = candidateParents(TASKS, "task_1").map((t) => t.id);
  assert.equal(ids.includes("task_2"), false);
  assert.equal(ids.includes("task_3"), false); // transitive descendant
  assert.equal(ids.includes("task_4"), false);
  assert.equal(ids.includes("task_1"), false); // self
});

test("candidateParents is empty when nothing safe exists (honest no-valid-parent state)", () => {
  // A lone task in its namespace: no other same-ns task to be its parent.
  const lone = [task("task_1", null), task("task_2", null, "other")];
  assert.deepEqual(candidateParents(lone, "task_1"), []);
  // An unknown task has no candidates.
  assert.deepEqual(candidateParents(TASKS, "task_does_not_exist"), []);
});

test("results are in stable id order", () => {
  const ids = candidateParents(TASKS, "task_5").map((t) => t.id);
  assert.deepEqual(ids, [...ids].sort((a, b) => a.localeCompare(b)));
});
