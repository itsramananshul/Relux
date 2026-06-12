// Unit tests for the pure Work hierarchy/progress helpers
// (apps/dashboard/src/workhierarchy.ts). Pure module → runs under
// `node --strip-types` (the .ts test path), no React/DOM.
//
// These pin the doc-specified semantics of Work hierarchy/progress v1
// (docs/relix-dashboard-design.md §6 progress strip / §6.1 nesting): progress is
// computed from the LIVE board task status (joined by step.task_id), the four
// buckets match the board columns, blocked-by/blocking resolve from depends_on,
// and a child absent from the board falls back to the durable step outcome.
//
// Run: `npm test` (auto-discovered) or
//   node --test --experimental-strip-types test/workhierarchy.test.ts

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  buildWorkGroups,
  groupProgress,
  nonEmptyGroups,
  progressSegments,
  groupProgressLabel,
  blockedByLabel,
  blockingLabel,
  groupForTask,
  bucketTone,
  bucketColorVar,
  type WorkGroup,
} from "../src/workhierarchy.ts";
import type {
  ReluxOrchestration,
  ReluxOrchestrationStep,
  ReluxStepOutcome,
  ReluxTask,
} from "../src/api.ts";

function step(
  taskId: string,
  outcome: ReluxStepOutcome,
  extra: Partial<ReluxOrchestrationStep> = {},
): ReluxOrchestrationStep {
  return {
    task_id: taskId,
    agent_id: "agent_1",
    role: "implementation",
    title: `Brief ${taskId}`,
    outcome,
    ...extra,
  };
}

function orch(steps: ReluxOrchestrationStep[], extra: Partial<ReluxOrchestration> = {}): ReluxOrchestration {
  return {
    id: "orch_0001",
    goal: "Ship the thing",
    created_by: "operator",
    namespace_id: "default",
    status: "running",
    steps,
    notes: [],
    created_at: "1",
    updated_at: "2",
    ...extra,
  };
}

function task(id: string, status: string, extra: Partial<ReluxTask> = {}): ReluxTask {
  return {
    id,
    title: `Task ${id}`,
    input: {},
    status,
    priority: 5,
    created_by: "operator",
    namespace_id: "default",
    created_at: "1",
    updated_at: "2",
    ...extra,
  };
}

// A research → implementation → testing chain: implementation depends on research,
// testing depends on implementation.
const CHAIN = orch([
  step("task_1", "completed", { role: "research", depends_on: [] }),
  step("task_2", "pending", { role: "implementation", depends_on: [0] }),
  step("task_3", "pending", { role: "testing", depends_on: [1] }),
]);

test("buildWorkGroups joins steps to LIVE task status (the board column wins over step.outcome)", () => {
  // task_2 is pending in the orchestration record, but the live board shows it
  // running — the live status must drive the bucket.
  const tasks = [
    task("task_1", "completed"),
    task("task_2", "running"),
    task("task_3", "queued"),
  ];
  const [g] = buildWorkGroups([CHAIN], tasks);
  assert.equal(g.id, "orch_0001");
  assert.equal(g.goal, "Ship the thing");
  assert.equal(g.children.length, 3);
  assert.deepEqual(g.children.map((c) => c.status), ["completed", "running", "queued"]);
  assert.deepEqual(g.children.map((c) => c.bucket), ["done", "running", "open"]);
  assert.equal(g.hasLiveChildren, true);
  // Progress reflects the LIVE statuses: 1 done, 1 running, 0 blocked, 1 open.
  assert.deepEqual(g.progress, { total: 3, done: 1, running: 1, blocked: 0, open: 1 });
});

test("a child absent from the board falls back to the durable step outcome", () => {
  // No live tasks at all → every child resolves via its step outcome.
  const [g] = buildWorkGroups([CHAIN], []);
  assert.equal(g.hasLiveChildren, false);
  assert.deepEqual(g.children.map((c) => c.status), [null, null, null]);
  // completed → done; pending → open; pending → open.
  assert.deepEqual(g.children.map((c) => c.bucket), ["done", "open", "open"]);
  assert.deepEqual(g.progress, { total: 3, done: 1, running: 0, blocked: 0, open: 2 });
});

test("a failed/blocked board status lands in the attention (blocked) bucket", () => {
  const tasks = [
    task("task_1", "completed"),
    task("task_2", "failed"),
    task("task_3", "blocked"),
  ];
  const [g] = buildWorkGroups([CHAIN], tasks);
  assert.deepEqual(g.children.map((c) => c.bucket), ["done", "blocked", "blocked"]);
  assert.deepEqual(g.progress, { total: 3, done: 1, running: 0, blocked: 2, open: 0 });
});

test("blocked-by / blocking resolve from depends_on indices to sibling task ids", () => {
  const [g] = buildWorkGroups([CHAIN], []);
  // research: nothing upstream, blocks implementation.
  assert.deepEqual(g.children[0].blockedBy, []);
  assert.deepEqual(g.children[0].blocking, ["task_2"]);
  // implementation: blocked by research, blocks testing.
  assert.deepEqual(g.children[1].blockedBy, ["task_1"]);
  assert.deepEqual(g.children[1].blocking, ["task_3"]);
  // testing: blocked by implementation, blocks nothing.
  assert.deepEqual(g.children[2].blockedBy, ["task_2"]);
  assert.deepEqual(g.children[2].blocking, []);
  // The human labels.
  assert.equal(blockedByLabel(g.children[1]), "blocked by task_1");
  assert.equal(blockingLabel(g.children[1]), "blocks task_3");
  assert.equal(blockedByLabel(g.children[0]), null);
  assert.equal(blockingLabel(g.children[2]), null);
});

test("live task assignment wins over the step's recorded agent for the child", () => {
  const tasks = [task("task_1", "completed", { assigned_agent: "agent_live" })];
  const [g] = buildWorkGroups(
    [orch([step("task_1", "completed", { agent_id: "agent_step" })])],
    tasks,
  );
  assert.equal(g.children[0].assignedAgent, "agent_live");
  // Absent from board → the step's agent_id.
  const [g2] = buildWorkGroups([orch([step("task_9", "pending", { agent_id: "agent_step" })])], []);
  assert.equal(g2.children[0].assignedAgent, "agent_step");
});

test("groupProgress tallies the four board buckets from a child list", () => {
  const children = buildWorkGroups([CHAIN], [
    task("task_1", "completed"),
    task("task_2", "running"),
    task("task_3", "waiting_for_approval"),
  ])[0].children;
  assert.deepEqual(groupProgress(children), { total: 3, done: 1, running: 1, blocked: 1, open: 0 });
});

test("progressSegments drops zero-count buckets and orders done→running→blocked→open", () => {
  const segs = progressSegments({ total: 4, done: 2, running: 1, blocked: 1, open: 0 });
  assert.deepEqual(segs.map((s) => s.bucket), ["done", "running", "blocked"]);
  assert.deepEqual(segs.map((s) => s.count), [2, 1, 1]);
  assert.equal(segs[0].pct, 50);
  assert.equal(segs[1].pct, 25);
  // An all-empty group never divides by zero.
  assert.deepEqual(progressSegments({ total: 0, done: 0, running: 0, blocked: 0, open: 0 }), []);
});

test("groupProgressLabel reads as a compact done count with an attention tail", () => {
  assert.equal(groupProgressLabel({ total: 5, done: 2, running: 1, blocked: 1, open: 1 }), "2/5 done · 1 running · 1 blocked · 1 open");
  assert.equal(groupProgressLabel({ total: 3, done: 3, running: 0, blocked: 0, open: 0 }), "3/3 done");
});

test("nonEmptyGroups drops a planned orchestration with no committed steps", () => {
  const planned = orch([], { id: "orch_0002", status: "planned" });
  const groups = buildWorkGroups([CHAIN, planned], []);
  assert.equal(groups.length, 2);
  assert.deepEqual(nonEmptyGroups(groups).map((g) => g.id), ["orch_0001"]);
});

test("groupForTask finds the parent group of a child task, null for standalone", () => {
  const groups = buildWorkGroups([CHAIN], []);
  assert.equal(groupForTask(groups, "task_2")?.id, "orch_0001");
  assert.equal(groupForTask(groups, "task_999"), null);
});

test("bucketTone / bucketColorVar map buckets to the shared B&W vocabulary", () => {
  assert.equal(bucketTone("done"), "done");
  assert.equal(bucketTone("running"), "in_progress");
  assert.equal(bucketTone("blocked"), "blocked");
  assert.equal(bucketTone("open"), "backlog");
  assert.equal(bucketColorVar("done"), "var(--ok)");
  assert.equal(bucketColorVar("blocked"), "var(--err)");
});

test("empty inputs yield no groups (the honest 'no sub-work yet' state upstream)", () => {
  assert.deepEqual(buildWorkGroups([], []), []);
  assert.deepEqual(nonEmptyGroups([] as WorkGroup[]), []);
});
