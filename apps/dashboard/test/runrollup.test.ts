// Unit tests for the per-subtree run/cost rollup helper (runrollup.ts), pinning the
// doc-specified honesty semantics (docs/relix-dashboard-design.md §6 "live cost
// (tokens + spend) for the subtree"): sum ONLY runs that reported each metric, track
// coverage, and report "unavailable" (not a fabricated zero) when no run reported a
// figure. Pure helpers — run under `node --strip-types`, no DOM (see docs note
// dashboard-test-tsx-vs-ts-split).
//
// Run: `npm test` (auto-discovered) or `node --test test/runrollup.test.ts`.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  runBucket,
  rollupRuns,
  runRollupChips,
  formatCostUsd,
  formatDurationMs,
  formatTokens,
  adhocSubtreeTaskIds,
  type RunRollup,
} from "../src/runrollup.ts";
import type { ReluxRun } from "../src/api.ts";

// A minimal run factory — only the fields the rollup reads.
function run(partial: Partial<ReluxRun> & { task_id: string; status: string }): ReluxRun {
  return {
    id: partial.id ?? `run_${Math.round(0)}`,
    agent_id: "agent_1",
    adapter_plugin: "relux-adapter-claude-cli",
    ...partial,
  } as ReluxRun;
}

test("runBucket maps lifecycle status to the three health buckets", () => {
  assert.equal(runBucket("completed"), "done");
  assert.equal(runBucket("failed"), "failed");
  // A cancelled RUN did not complete its work → counts as not-completed (not "done"
  // like the task board's cancelled task).
  assert.equal(runBucket("cancelled"), "failed");
  assert.equal(runBucket("running"), "active");
  assert.equal(runBucket("pending"), "active");
  assert.equal(runBucket("waiting_for_approval"), "active");
  assert.equal(runBucket("something_new"), "active");
});

test("rollupRuns counts only runs whose task_id is in the subtree", () => {
  const runs = [
    run({ id: "r1", task_id: "t1", status: "completed" }),
    run({ id: "r2", task_id: "t2", status: "running" }),
    run({ id: "r3", task_id: "tOTHER", status: "failed" }), // not in the subtree
  ];
  const r = rollupRuns(runs, ["t1", "t2"]);
  assert.equal(r.runs, 2);
  assert.equal(r.done, 1);
  assert.equal(r.active, 1);
  assert.equal(r.failed, 0);
});

test("rollupRuns tallies mixed statuses (cancelled folds into failed)", () => {
  const runs = [
    run({ id: "r1", task_id: "t1", status: "completed" }),
    run({ id: "r2", task_id: "t1", status: "failed" }),
    run({ id: "r3", task_id: "t1", status: "cancelled" }),
    run({ id: "r4", task_id: "t1", status: "running" }),
    run({ id: "r5", task_id: "t1", status: "waiting_for_approval" }),
  ];
  const r = rollupRuns(runs, new Set(["t1"]));
  assert.equal(r.runs, 5);
  assert.equal(r.done, 1);
  assert.equal(r.failed, 2); // failed + cancelled
  assert.equal(r.active, 2); // running + waiting
});

test("rollupRuns sums cost only over runs that reported one, tracking coverage", () => {
  const runs = [
    run({ id: "r1", task_id: "t1", status: "completed", cost: 0.012 }),
    run({ id: "r2", task_id: "t1", status: "completed", cost: 0.008 }),
    run({ id: "r3", task_id: "t1", status: "completed" }), // no cost reported
  ];
  const r = rollupRuns(runs, ["t1"]);
  assert.equal(r.costRuns, 2);
  assert.equal(r.costKnown, true);
  assert.ok(Math.abs(r.costUsd - 0.02) < 1e-9);
});

test("rollupRuns: cost is UNAVAILABLE (not zero) when no run reported a cost", () => {
  const runs = [
    run({ id: "r1", task_id: "t1", status: "completed" }),
    run({ id: "r2", task_id: "t1", status: "running" }),
  ];
  const r = rollupRuns(runs, ["t1"]);
  assert.equal(r.costKnown, false);
  assert.equal(r.costRuns, 0);
  assert.equal(r.costUsd, 0); // the accumulator is 0, but costKnown=false marks it unavailable
});

test("rollupRuns: a genuine reported cost of 0 is KNOWN, distinct from unavailable", () => {
  const runs = [run({ id: "r1", task_id: "t1", status: "completed", cost: 0 })];
  const r = rollupRuns(runs, ["t1"]);
  assert.equal(r.costKnown, true);
  assert.equal(r.costRuns, 1);
  assert.equal(r.costUsd, 0);
});

test("rollupRuns sums real duration only over runs that measured one", () => {
  const runs = [
    run({ id: "r1", task_id: "t1", status: "completed", duration_ms: 8000 }),
    run({ id: "r2", task_id: "t1", status: "completed", duration_ms: 2000 }),
    run({ id: "r3", task_id: "t1", status: "completed" }), // local echo — no duration
  ];
  const r = rollupRuns(runs, ["t1"]);
  assert.equal(r.durationKnown, true);
  assert.equal(r.durationRuns, 2);
  assert.equal(r.durationMs, 10000);
});

test("rollupRuns sums tokens from usage (input + output) only when present", () => {
  const runs = [
    run({ id: "r1", task_id: "t1", status: "completed", usage: { input_tokens: 1200, output_tokens: 340 } }),
    run({ id: "r2", task_id: "t1", status: "completed", usage: { input_tokens: 100, output_tokens: 50 } }),
    run({ id: "r3", task_id: "t1", status: "completed" }), // no usage
    run({ id: "r4", task_id: "t1", status: "completed", usage: { note: "no numeric tokens" } }), // no token fields
  ];
  const r = rollupRuns(runs, ["t1"]);
  assert.equal(r.tokensKnown, true);
  assert.equal(r.tokenRuns, 2);
  assert.equal(r.tokens, 1690);
});

test("rollupRuns on an empty subtree is an honest all-zero, nothing known", () => {
  const r = rollupRuns([], ["t1"]);
  assert.equal(r.runs, 0);
  assert.equal(r.costKnown, false);
  assert.equal(r.durationKnown, false);
  assert.equal(r.tokensKnown, false);
});

test("runRollupChips: empty subtree shows a single 'no runs yet' chip", () => {
  const chips = runRollupChips(rollupRuns([], ["t1"]));
  assert.equal(chips.length, 1);
  assert.equal(chips[0].label, "no runs yet");
});

test("runRollupChips: cost-present subtree shows run count, cost, duration, tokens", () => {
  const runs = [
    run({ id: "r1", task_id: "t1", status: "completed", cost: 0.012, duration_ms: 8000, usage: { input_tokens: 1200, output_tokens: 340 } }),
    run({ id: "r2", task_id: "t1", status: "failed", cost: 0.003, duration_ms: 1000 }),
  ];
  const chips = runRollupChips(rollupRuns(runs, ["t1"]));
  const labels = chips.map((c) => c.label);
  assert.ok(labels.includes("2 runs"));
  assert.ok(labels.includes("1 failed"));
  assert.ok(labels.some((l) => l.startsWith("$")), "a cost chip is present");
  assert.ok(labels.some((l) => l.endsWith("tok")), "a token chip is present");
  // the failed chip carries the semantic 'failed' tone
  assert.equal(chips.find((c) => c.label === "1 failed")?.tone, "failed");
});

test("runRollupChips: cost-absent subtree shows an honest 'cost unavailable' chip, no duration/token chips", () => {
  const runs = [
    run({ id: "r1", task_id: "t1", status: "completed" }),
    run({ id: "r2", task_id: "t1", status: "running" }),
  ];
  const chips = runRollupChips(rollupRuns(runs, ["t1"]));
  const labels = chips.map((c) => c.label);
  assert.ok(labels.includes("cost unavailable"));
  assert.ok(!labels.some((l) => l.startsWith("$")), "no fabricated dollar figure");
  assert.ok(!labels.some((l) => l.endsWith("tok")), "no token chip when none reported");
  assert.ok(labels.includes("1 active"));
});

test("runRollupChips: partial cost coverage is disclosed in the tooltip", () => {
  const runs = [
    run({ id: "r1", task_id: "t1", status: "completed", cost: 0.01 }),
    run({ id: "r2", task_id: "t1", status: "completed" }), // no cost
  ];
  const costChip = runRollupChips(rollupRuns(runs, ["t1"])).find((c) => c.label.startsWith("$"));
  assert.ok(costChip);
  assert.match(costChip!.title, /from 1\/2 runs/);
});

test("formatters render compactly", () => {
  assert.equal(formatCostUsd(0), "$0.00");
  assert.equal(formatCostUsd(0.0123), "$0.0123");
  assert.equal(formatCostUsd(1.5), "$1.50");
  assert.equal(formatDurationMs(500), "500ms");
  assert.equal(formatDurationMs(8100), "8.1s");
  assert.equal(formatDurationMs(95000), "1m 35s");
  assert.equal(formatTokens(540), "540");
  assert.equal(formatTokens(1540), "1.5k");
  assert.equal(formatTokens(2_300_000), "2.3M");
});

test("adhocSubtreeTaskIds includes the parent plus its children", () => {
  assert.deepEqual(adhocSubtreeTaskIds("p1", ["c1", "c2"]), ["p1", "c1", "c2"]);
  assert.deepEqual(adhocSubtreeTaskIds("p1", []), ["p1"]);
});
