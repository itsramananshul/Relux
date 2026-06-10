import { test } from "node:test";
import assert from "node:assert/strict";
import {
  orchestrationStatusTone,
  stepOutcomeTone,
  orchestrationProgress,
  orchestrationProgressLabel,
  canRunOrchestration,
  activeOrchestration,
  orchestrationNextAction,
  groupStepsByAgent,
  orchestrationHeadline,
} from "../src/orchestration.ts";
import type { ReluxOrchestration, ReluxOrchestrationStep } from "../src/api.ts";

// The orchestration view must read HONESTLY: progress is computed from the actual
// step set, the next action matches the recorded status, and "active" surfaces the
// right plan. These assertions pin that.

function step(
  taskId: string,
  agentId: string,
  outcome: ReluxOrchestrationStep["outcome"],
): ReluxOrchestrationStep {
  return {
    task_id: taskId,
    agent_id: agentId,
    role: "implementation",
    title: `Brief ${taskId}`,
    outcome,
  };
}

function orch(
  id: string,
  status: ReluxOrchestration["status"],
  steps: ReluxOrchestrationStep[],
): ReluxOrchestration {
  return {
    id,
    goal: "do the thing",
    created_by: "founder",
    namespace_id: "workspace",
    status,
    steps,
    notes: [],
    created_at: "t0",
    updated_at: "t0",
  };
}

test("status and outcome tones map known values and fall back neutrally", () => {
  assert.equal(orchestrationStatusTone("completed"), "done");
  assert.equal(orchestrationStatusTone("running"), "in_progress");
  assert.equal(orchestrationStatusTone("needs_attention"), "in_review");
  assert.equal(orchestrationStatusTone("planned"), "backlog");
  assert.equal(orchestrationStatusTone(undefined), "backlog");

  assert.equal(stepOutcomeTone("completed"), "done");
  assert.equal(stepOutcomeTone("failed"), "blocked");
  assert.equal(stepOutcomeTone("blocked"), "blocked");
  assert.equal(stepOutcomeTone("pending"), "backlog");
});

test("progress is computed from the step set, not a summary field", () => {
  const o = orch("orch_0001", "needs_attention", [
    step("task_0001", "prime", "completed"),
    step("task_0002", "code-agent", "failed"),
    step("task_0003", "research-agent", "blocked"),
    step("task_0004", "prime", "pending"),
  ]);
  const p = orchestrationProgress(o);
  assert.deepEqual(p, { total: 4, completed: 1, pending: 1, failed: 1, blocked: 1 });
  assert.equal(orchestrationProgressLabel(o), "1/4 completed");
});

test("canRunOrchestration is true only when a brief is still pending", () => {
  assert.equal(
    canRunOrchestration(orch("orch_0001", "running", [step("t1", "a", "pending")])),
    true,
  );
  assert.equal(
    canRunOrchestration(orch("orch_0002", "completed", [step("t1", "a", "completed")])),
    false,
  );
  // Needs-attention with only blocked/failed (no pending) cannot be "run" — it
  // needs a human action first.
  assert.equal(
    canRunOrchestration(orch("orch_0003", "needs_attention", [step("t1", "a", "blocked")])),
    false,
  );
});

test("activeOrchestration prefers the newest unfinished plan", () => {
  const list = [
    orch("orch_0001", "completed", [step("t1", "a", "completed")]),
    orch("orch_0002", "running", [step("t2", "a", "pending")]),
    orch("orch_0003", "completed", [step("t3", "a", "completed")]),
  ];
  assert.equal(activeOrchestration(list)?.id, "orch_0002");
  // All completed -> newest overall.
  const allDone = [
    orch("orch_0001", "completed", [step("t1", "a", "completed")]),
    orch("orch_0005", "completed", [step("t5", "a", "completed")]),
  ];
  assert.equal(activeOrchestration(allDone)?.id, "orch_0005");
  assert.equal(activeOrchestration([]), null);
});

test("next action matches the recorded status", () => {
  assert.match(
    orchestrationNextAction(orch("orch_0001", "planned", [step("t1", "a", "pending")])),
    /Run the orchestration to start/,
  );
  assert.match(
    orchestrationNextAction(
      orch("orch_0002", "running", [step("t1", "a", "completed"), step("t2", "a", "pending")]),
    ),
    /1 brief\(s\) pending/,
  );
  assert.match(
    orchestrationNextAction(
      orch("orch_0003", "needs_attention", [
        step("t1", "a", "blocked"),
        step("t2", "b", "failed"),
      ]),
    ),
    /2 brief\(s\) need attention/,
  );
  assert.match(
    orchestrationNextAction(orch("orch_0004", "completed", [step("t1", "a", "completed")])),
    /All briefs completed/,
  );
});

test("groupStepsByAgent preserves first-seen order and groups briefs", () => {
  const o = orch("orch_0001", "running", [
    step("t1", "research-agent", "completed"),
    step("t2", "code-agent", "pending"),
    step("t3", "research-agent", "pending"),
  ]);
  const groups = groupStepsByAgent(o);
  assert.deepEqual(
    groups.map((g) => g.agentId),
    ["research-agent", "code-agent"],
  );
  assert.equal(groups[0].steps.length, 2);
  assert.equal(groups[1].steps.length, 1);
});

test("headline summarizes fleet activity or hides when empty", () => {
  assert.equal(orchestrationHeadline([]), null);
  assert.equal(
    orchestrationHeadline([orch("orch_0001", "running", [step("t1", "a", "pending")])]),
    "1 active orchestration across the fleet.",
  );
  assert.equal(
    orchestrationHeadline([orch("orch_0001", "completed", [step("t1", "a", "completed")])]),
    "1 orchestration, all completed.",
  );
});
