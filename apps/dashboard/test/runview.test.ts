import { test } from "node:test";
import assert from "node:assert/strict";
import {
  runStatusTone,
  formatRunDuration,
  canRetryRun,
  runMetricsLine,
  phaseLabel,
  isRunInFlight,
  eventPayloadPreview,
  toolCallSummary,
  reviewApplyAvailability,
  REVIEW_APPLY_UNAVAILABLE_REASON,
} from "../src/runview.ts";

// The Work page's run-depth view must read HONESTLY: it only formats/classifies
// what the backend recorded, never fabricates progress or metrics, and offers a
// Retry only for runs the backend marked retryable. These assertions pin that.

test("runStatusTone maps known statuses and falls back neutrally", () => {
  assert.equal(runStatusTone("completed"), "done");
  assert.equal(runStatusTone("running"), "running");
  assert.equal(runStatusTone("failed"), "backlog");
  assert.equal(runStatusTone(undefined), "backlog");
});

test("formatRunDuration only renders a real measured value", () => {
  assert.equal(formatRunDuration(undefined), null); // local echo path: no duration
  assert.equal(formatRunDuration(null), null);
  assert.equal(formatRunDuration(-5), null);
  assert.equal(formatRunDuration(450), "450 ms");
  assert.equal(formatRunDuration(8123), "8.1 s");
  assert.equal(formatRunDuration(42000), "42 s");
  assert.equal(formatRunDuration(95000), "1m 35s");
});

test("canRetryRun prefers the backend retryable flag, falls back to failed", () => {
  // Run detail carries the server-derived flag — trust it exactly.
  assert.equal(canRetryRun({ id: "r1", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "failed", retryable: false } as any), false);
  assert.equal(canRetryRun({ id: "r1", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "failed", retryable: true } as any), true);
  // List shape (no flag): only a failed run is retryable.
  assert.equal(canRetryRun({ id: "r1", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "failed" } as any), true);
  assert.equal(canRetryRun({ id: "r1", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "completed" } as any), false);
});

test("runMetricsLine only shows metrics the adapter actually reported", () => {
  assert.equal(runMetricsLine({ id: "r", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "completed" } as any), null);
  assert.equal(
    runMetricsLine({ id: "r", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "completed", cost: 0.0125, usage: { num_turns: 3, output_tokens: 210 } } as any),
    "$0.0125 · 3 turns · 210 output tokens",
  );
});

test("phaseLabel humanizes event kinds and falls back to status", () => {
  assert.equal(phaseLabel("adapter_spawn", "running"), "Spawning adapter");
  assert.equal(phaseLabel("run_failed", "failed"), "Failed");
  assert.equal(phaseLabel(undefined, "running"), "running");
  assert.equal(phaseLabel("some_future_kind", "running"), "some_future_kind");
});

test("isRunInFlight is true only for non-terminal states", () => {
  assert.equal(isRunInFlight("running"), true);
  assert.equal(isRunInFlight("pending"), true);
  assert.equal(isRunInFlight("completed"), false);
  assert.equal(isRunInFlight("failed"), false);
});

test("eventPayloadPreview drops bulky stdout/stderr and nulls", () => {
  assert.equal(eventPayloadPreview(null), null);
  assert.equal(eventPayloadPreview({ stdout: "huge", stderr: "" }), null);
  const preview = eventPayloadPreview({ stdout: "huge", exit_code: 0, structured: true });
  assert.ok(preview && preview.includes("exit_code"));
  assert.ok(preview && !preview.includes("huge"));
});

const ev = (kind: string): any => ({ id: "e", run_id: "r", ts: "t", kind, source: "kernel", message: "" });

test("toolCallSummary counts only real tool events, and is null when there are none", () => {
  assert.equal(toolCallSummary(undefined), null);
  assert.equal(toolCallSummary([]), null);
  // run_started/adapter_output are NOT tool calls — never fabricate one from them.
  assert.equal(toolCallSummary([ev("run_started"), ev("adapter_output"), ev("run_completed")]), null);
  assert.equal(toolCallSummary([ev("tool_call")]), "1 tool call");
  assert.equal(toolCallSummary([ev("tool_call"), ev("tool_call")]), "2 tool calls");
  assert.equal(
    toolCallSummary([ev("tool_call"), ev("tool_call_denied"), ev("tool_call_failed"), ev("tool_call_failed")]),
    "1 tool call · 1 denied · 2 failed",
  );
});

test("reviewApplyAvailability is honestly unavailable for a plain Relux run", () => {
  const base = { id: "r", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "completed" } as any;
  const verdict = reviewApplyAvailability(base);
  assert.equal(verdict.available, false);
  assert.equal(verdict.reason, REVIEW_APPLY_UNAVAILABLE_REASON);
  // The reason must name where the capability actually lives and why ids don't cross.
  assert.match(verdict.reason, /read-only execution records/);
  assert.match(verdict.reason, /legacy Runs surface/);
  assert.match(verdict.reason, /not Relux run ids/);
});

test("reviewApplyAvailability flips to available only when a real artifact set is present", () => {
  const withArtifacts = {
    id: "r", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "completed",
    artifacts: [{ rel_path: "src/main.rs" }],
  } as any;
  assert.equal(reviewApplyAvailability(withArtifacts).available, true);
  // An empty artifact array is not a capability — stay honest.
  assert.equal(reviewApplyAvailability({ ...withArtifacts, artifacts: [] }).available, false);
});
