// Unit tests for the Board Oversight v1 pure helpers (src/oversight.ts).
//
// Pure logic only — no React, no DOM — so it runs under `node --test
// --experimental-strip-types` without the esbuild render harness. The component
// wiring is covered by work-render.test.mjs; this pins the bucketing + summary
// semantics the board depends on.

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  taskBucket,
  bucketTasks,
  oversightCountChips,
  hasOversightAttention,
  continuationActionLabel,
} from "../src/oversight.ts";

test("taskBucket maps every TaskStatus to a visible column", () => {
  assert.equal(taskBucket("created"), "open");
  assert.equal(taskBucket("queued"), "open");
  assert.equal(taskBucket("leased"), "running");
  assert.equal(taskBucket("running"), "running");
  assert.equal(taskBucket("waiting_for_tool"), "running");
  // The reported gap: these were computed into an unrendered "other" bucket.
  assert.equal(taskBucket("waiting_for_approval"), "blocked");
  assert.equal(taskBucket("blocked"), "blocked");
  assert.equal(taskBucket("failed"), "blocked");
  assert.equal(taskBucket("completed"), "done");
  assert.equal(taskBucket("cancelled"), "done");
  assert.equal(taskBucket("expired"), "done");
});

test("an unknown/future status never drops off the board", () => {
  assert.equal(taskBucket("some_new_status"), "done");
});

test("bucketTasks partitions in one pass and keeps blocked/failed visible", () => {
  const mk = (id: string, status: string) => ({ id, status }) as never;
  const cols = bucketTasks([
    mk("t1", "created"),
    mk("t2", "running"),
    mk("t3", "blocked"),
    mk("t4", "failed"),
    mk("t5", "completed"),
    mk("t6", "waiting_for_approval"),
  ]);
  assert.deepEqual(cols.open.map((t) => t.id), ["t1"]);
  assert.deepEqual(cols.running.map((t) => t.id), ["t2"]);
  // blocked + failed + waiting-on-approval all land in the one attention column.
  assert.deepEqual(cols.blocked.map((t) => t.id), ["t3", "t4", "t6"]);
  assert.deepEqual(cols.done.map((t) => t.id), ["t5"]);
});

const COUNTS = {
  db_path: "x",
  plugins: 0,
  installed_plugins: 0,
  namespaces: 0,
  agents: 0,
  tasks: 0,
  runs: 0,
  approvals: 0,
  open_tasks: 3,
  active_runs: 2,
  waiting_approval: 1,
  blocked: 4,
  failed: 5,
  pending_approvals: 1,
};

test("oversightCountChips surfaces the operational counts in order", () => {
  const chips = oversightCountChips(COUNTS);
  assert.deepEqual(
    chips.map((c) => [c.label, c.value]),
    [
      ["Active runs", 2],
      ["Open tasks", 3],
      ["Blocked", 4],
      ["Failed", 5],
      ["Waiting approval", 1],
      ["Pending approvals", 1],
    ],
  );
  // Active runs uses the "running" tone so the live count reads as live.
  assert.equal(chips[0].tone, "running");
});

test("hasOversightAttention is true only with real actionable state", () => {
  assert.equal(hasOversightAttention(null), false);
  assert.equal(
    hasOversightAttention({
      counts: COUNTS,
      active_runs: [],
      attention_runs: [],
      pending_approvals: [],
      continuation: null,
    }),
    false,
  );
  assert.equal(
    hasOversightAttention({
      counts: COUNTS,
      active_runs: [],
      attention_runs: [],
      pending_approvals: [],
      continuation: { id: "cont_1", reason: "tool-call limit", observation_count: 2, extended_used: false, awaiting_approval: false },
    }),
    true,
  );
});

test("continuationActionLabel distinguishes limit-paused from approval-paused", () => {
  const limit = continuationActionLabel({
    id: "cont_1",
    reason: "tool-call limit",
    observation_count: 3,
    extended_used: false,
    awaiting_approval: false,
  });
  assert.match(limit, /Continue resumes/);
  assert.match(limit, /3 observations gathered/);

  const approval = continuationActionLabel({
    id: "cont_2",
    reason: "awaiting tool approval",
    observation_count: 1,
    extended_used: true,
    awaiting_approval: true,
  });
  assert.match(approval, /approve the pending tool first/);
  assert.match(approval, /1 observation gathered/);
});
