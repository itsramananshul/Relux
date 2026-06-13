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
  stepLifecycle,
  stepLifecycleTone,
  stepDependencyLabel,
  orchestrationReadiness,
  orchestrationRounds,
  stepDurationLabel,
  orchestrationAssignmentSummary,
  stepIsPrimeFallback,
} from "../src/orchestration.ts";
import type { ReluxOrchestration, ReluxOrchestrationStep } from "../src/api.ts";

// The orchestration view must read HONESTLY: progress is computed from the actual
// step set, the next action matches the recorded status, and "active" surfaces the
// right plan. These assertions pin that.

function step(
  taskId: string,
  agentId: string,
  outcome: ReluxOrchestrationStep["outcome"],
  extra: Partial<ReluxOrchestrationStep> = {},
): ReluxOrchestrationStep {
  return {
    task_id: taskId,
    agent_id: agentId,
    role: "implementation",
    title: `Brief ${taskId}`,
    outcome,
    ...extra,
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

test("orchestrationAssignmentSummary splits specialists from Prime fallbacks", () => {
  // research + documentation fell back to Prime (no specialist); implementation
  // landed on a real specialist. A general brief on Prime is NOT a missing hire.
  const o = orch("orch_0001", "planned", [
    step("t1", "prime", "pending", { role: "research" }),
    step("t2", "code-agent", "pending", { role: "implementation" }),
    step("t3", "prime", "pending", { role: "documentation" }),
    step("t4", "prime", "pending", { role: "general" }),
  ]);
  const summary = orchestrationAssignmentSummary(o);
  assert.deepEqual(summary.assignedAgents, ["code-agent"]);
  assert.deepEqual(summary.unassignedRoles, ["research", "documentation"]);
  // The per-step predicate matches: a Prime non-general brief is a fallback; a real
  // specialist or a general brief is not.
  assert.equal(stepIsPrimeFallback(o.steps[0]), true);
  assert.equal(stepIsPrimeFallback(o.steps[1]), false);
  assert.equal(stepIsPrimeFallback(o.steps[3]), false, "a general brief needs no specialist");
});

test("orchestrationAssignmentSummary de-dupes and is empty when fully staffed", () => {
  const o = orch("orch_0002", "planned", [
    step("t1", "research-agent", "pending", { role: "research" }),
    step("t2", "research-agent", "pending", { role: "research" }),
  ]);
  const summary = orchestrationAssignmentSummary(o);
  assert.deepEqual(summary.assignedAgents, ["research-agent"]);
  assert.deepEqual(summary.unassignedRoles, []);
});

test("stepLifecycle derives ready vs waiting from dependencies", () => {
  // step 1 depends on step 0. While step 0 is pending, step 1 is "waiting"; once
  // step 0 completes, step 1 becomes "ready".
  const waiting = orch("orch_0001", "running", [
    step("t0", "research-agent", "pending"),
    step("t1", "code-agent", "pending", { depends_on: [0] }),
  ]);
  assert.equal(stepLifecycle(waiting, 0), "ready"); // no deps
  assert.equal(stepLifecycle(waiting, 1), "waiting"); // dep not done

  const ready = orch("orch_0002", "running", [
    step("t0", "research-agent", "completed"),
    step("t1", "code-agent", "pending", { depends_on: [0] }),
  ]);
  assert.equal(stepLifecycle(ready, 1), "ready"); // dep completed

  // A failed/blocked dependency makes a still-pending dependent read as blocked.
  const upstreamFailed = orch("orch_0003", "needs_attention", [
    step("t0", "research-agent", "failed"),
    step("t1", "code-agent", "pending", { depends_on: [0] }),
  ]);
  assert.equal(stepLifecycle(upstreamFailed, 1), "blocked");

  // Terminal outcomes pass through unchanged.
  assert.equal(
    stepLifecycle(orch("o", "completed", [step("t", "a", "completed")]), 0),
    "completed",
  );
});

test("stepLifecycleTone maps derived states to badge tones", () => {
  assert.equal(stepLifecycleTone("completed"), "done");
  assert.equal(stepLifecycleTone("ready"), "in_progress");
  assert.equal(stepLifecycleTone("failed"), "blocked");
  assert.equal(stepLifecycleTone("blocked"), "blocked");
  assert.equal(stepLifecycleTone("waiting"), "backlog");
});

test("stepDependencyLabel names upstream task ids or hides when independent", () => {
  const o = orch("orch_0001", "running", [
    step("t0", "research-agent", "completed"),
    step("t1", "code-agent", "pending", { depends_on: [0] }),
  ]);
  assert.equal(stepDependencyLabel(o, o.steps[0]), null); // independent
  assert.equal(stepDependencyLabel(o, o.steps[1]), "waits on t0");
});

test("orchestrationReadiness tallies ready/waiting/blocked from the step set", () => {
  const o = orch("orch_0001", "running", [
    step("t0", "research-agent", "completed"),
    step("t1", "code-agent", "pending", { depends_on: [0] }), // ready
    step("t2", "qa-agent", "pending", { depends_on: [1] }), // waiting on t1
    step("t3", "doc-agent", "blocked"),
  ]);
  assert.deepEqual(orchestrationReadiness(o), {
    ready: 1,
    waiting: 1,
    blocked: 1,
    completed: 1,
    failed: 0,
  });
});

test("orchestrationRounds groups briefs by their recorded round, in order", () => {
  const o = orch("orch_0001", "completed", [
    step("t0", "a", "completed", { round: 1 }),
    step("t1", "b", "completed", { round: 1 }),
    step("t2", "c", "completed", { round: 2 }),
    step("t3", "d", "pending"), // never ran -> omitted
  ]);
  const rounds = orchestrationRounds(o);
  assert.deepEqual(
    rounds.map((r) => r.round),
    [1, 2],
  );
  assert.equal(rounds[0].steps.length, 2);
  assert.equal(rounds[1].steps.length, 1);
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

// --- Non-blocking orchestration job helpers --------------------------------

import {
  jobIsActive,
  jobIsTerminal,
  jobIsCanceling,
  jobCanCancel,
  jobIsReconstructed,
  jobIsInterrupted,
  jobPendingCount,
  jobPhaseLabel,
  jobProgressLabel,
  jobRunningStepIds,
  runButtonLabel,
} from "../src/orchestration.ts";
import type {
  ReluxOrchestrationJob,
  ReluxJobStepStatus,
} from "../src/api.ts";

function jobStep(
  taskId: string,
  outcome: ReluxJobStepStatus["outcome"],
): ReluxJobStepStatus {
  return { task_id: taskId, agent_id: "prime", title: `Brief ${taskId}`, outcome };
}

function job(
  state: ReluxOrchestrationJob["state"],
  extra: Partial<ReluxOrchestrationJob> = {},
): ReluxOrchestrationJob {
  return {
    id: "job_0001",
    orchestration_id: "orch_0001",
    state,
    max: 25,
    concurrency: 2,
    current_round: 0,
    ran: 0,
    completed: 0,
    failed: 0,
    blocked: 0,
    steps: [],
    ...extra,
  };
}

test("jobIsActive is true only while queued or running", () => {
  assert.equal(jobIsActive(job("queued")), true);
  assert.equal(jobIsActive(job("running")), true);
  assert.equal(jobIsActive(job("completed")), false);
  assert.equal(jobIsActive(job("failed")), false);
  assert.equal(jobIsActive(null), false);
  assert.equal(jobIsActive(undefined), false);
});

test("jobIsTerminal is true for completed, failed, canceled, or interrupted", () => {
  assert.equal(jobIsTerminal("completed"), true);
  assert.equal(jobIsTerminal("failed"), true);
  assert.equal(jobIsTerminal("canceled"), true);
  // A restart-reconstructed job is terminal too (no live worker), so the UI stops
  // polling and falls back to the durable record.
  assert.equal(jobIsTerminal("interrupted"), true);
  assert.equal(jobIsTerminal("running"), false);
  assert.equal(jobIsTerminal("queued"), false);
  assert.equal(jobIsTerminal(undefined), false);
});

test("interrupted is not active and never cancelable (no live worker)", () => {
  assert.equal(jobIsActive(job("interrupted")), false);
  assert.equal(jobCanCancel(job("interrupted")), false);
  assert.equal(jobIsCanceling(job("interrupted", { cancel_requested: true })), false);
});

test("jobCanCancel only while active and no cancel pending", () => {
  assert.equal(jobCanCancel(job("running")), true);
  assert.equal(jobCanCancel(job("queued")), true);
  // Once a cancel is requested, the control is no longer offered.
  assert.equal(jobCanCancel(job("running", { cancel_requested: true })), false);
  // Terminal jobs can't be canceled.
  assert.equal(jobCanCancel(job("completed")), false);
  assert.equal(jobCanCancel(job("canceled")), false);
  assert.equal(jobCanCancel(null), false);
});

test("jobIsCanceling is true only while active with a pending cancel", () => {
  assert.equal(jobIsCanceling(job("running", { cancel_requested: true })), true);
  assert.equal(jobIsCanceling(job("queued", { cancel_requested: true })), true);
  assert.equal(jobIsCanceling(job("running")), false);
  // A canceled (terminal) job is no longer "canceling".
  assert.equal(jobIsCanceling(job("canceled", { cancel_requested: true })), false);
  assert.equal(jobIsCanceling(null), false);
});

test("jobPhaseLabel surfaces canceling and canceled states", () => {
  assert.equal(
    jobPhaseLabel(job("running", { cancel_requested: true, current_round: 0 })),
    "Canceling…",
  );
  assert.equal(
    jobPhaseLabel(job("running", { cancel_requested: true, current_round: 3 })),
    "Canceling — finishing round 3",
  );
  assert.equal(jobPhaseLabel(job("canceled")), "Canceled");
});

test("jobPhaseLabel reflects the real lifecycle, not a spinner", () => {
  assert.equal(jobPhaseLabel(job("queued")), "Queued");
  assert.equal(jobPhaseLabel(job("running", { current_round: 0 })), "Running — starting");
  assert.equal(jobPhaseLabel(job("running", { current_round: 2 })), "Running — round 2");
  assert.equal(jobPhaseLabel(job("completed")), "Completed");
  assert.equal(jobPhaseLabel(job("failed")), "Failed");
  assert.equal(jobPhaseLabel(job("interrupted")), "Interrupted — no live worker");
  assert.equal(jobPhaseLabel(null), "");
});

test("jobProgressLabel summarizes ran/completed/failed/blocked from the job", () => {
  const j = job("running", {
    ran: 3,
    completed: 2,
    failed: 1,
    steps: [
      jobStep("t0", "completed"),
      jobStep("t1", "failed"),
      jobStep("t2", "running"),
      jobStep("t3", "pending"),
    ],
  });
  assert.equal(jobProgressLabel(j), "3/4 briefs run · 2 completed · 1 failed");
  assert.equal(jobProgressLabel(null), "");
});

test("jobRunningStepIds returns only briefs the job is executing now", () => {
  const j = job("running", {
    steps: [jobStep("t0", "completed"), jobStep("t1", "running"), jobStep("t2", "pending")],
  });
  assert.deepEqual(jobRunningStepIds(j), ["t1"]);
  assert.deepEqual(jobRunningStepIds(null), []);
});

test("runButtonLabel tracks the live job and resting verb", () => {
  const planned = orch("orch_0001", "planned", [step("t0", "a", "pending")]);
  const running = orch("orch_0001", "running", [
    step("t0", "a", "completed"),
    step("t1", "b", "pending"),
  ]);
  // No job: resting verb depends on whether there is recorded progress.
  assert.equal(runButtonLabel(planned, null), "Run orchestration");
  assert.equal(runButtonLabel(running, null), "Continue");
  // Active job: live phase wins.
  assert.equal(runButtonLabel(planned, job("queued")), "Queued...");
  assert.equal(runButtonLabel(planned, job("running")), "Running...");
  // After a failure: offer a retry.
  assert.equal(runButtonLabel(running, job("failed")), "Retry");
});

// --- Restart-honest reconstructed status (interrupted jobs) ----------------
// When the in-memory job registry is lost (server restart), a poll by
// orchestration id RECONSTRUCTS a job-like status from the durable record, with a
// synthetic `durable:<id>` id and state "interrupted" if briefs remain. These pin
// that the UI labels it honestly, treats it as terminal (so polling stops — no
// broken loop), and re-offers Continue to resume.

// A reconstructed interrupted job: synthetic id, no live worker, pending briefs.
function reconstructedInterrupted(): ReluxOrchestrationJob {
  return job("interrupted", {
    id: "durable:orch_0001",
    ran: 2,
    completed: 2,
    current_round: 1,
    steps: [
      jobStep("t0", "completed"),
      jobStep("t1", "completed"),
      jobStep("t2", "pending"),
      jobStep("t3", "pending"),
    ],
  });
}

test("jobIsReconstructed detects the synthetic durable id, not a live worker", () => {
  assert.equal(jobIsReconstructed(reconstructedInterrupted()), true);
  // A live worker's process-local id is never treated as reconstructed.
  assert.equal(jobIsReconstructed(job("running", { id: "job_0001" })), false);
  assert.equal(jobIsReconstructed(job("completed", { id: "job_0007" })), false);
  assert.equal(jobIsReconstructed(null), false);
  assert.equal(jobIsReconstructed(undefined), false);
});

test("jobIsInterrupted is true only for the interrupted state", () => {
  assert.equal(jobIsInterrupted(reconstructedInterrupted()), true);
  assert.equal(jobIsInterrupted(job("running")), false);
  assert.equal(jobIsInterrupted(job("completed")), false);
  assert.equal(jobIsInterrupted(null), false);
});

test("jobPendingCount counts the briefs a Continue run would resume", () => {
  assert.equal(jobPendingCount(reconstructedInterrupted()), 2);
  assert.equal(jobPendingCount(job("completed", { steps: [jobStep("t0", "completed")] })), 0);
  assert.equal(jobPendingCount(null), 0);
});

test("an interrupted reconstructed job is terminal and never active/cancelable", () => {
  const j = reconstructedInterrupted();
  // Terminal => the poll effect stops (it only polls active jobs): no broken loop.
  assert.equal(jobIsTerminal(j.state), true);
  assert.equal(jobIsActive(j), false);
  assert.equal(jobCanCancel(j), false);
  assert.equal(jobIsCanceling({ ...j, cancel_requested: true }), false);
});

test("the poll effect schedules nothing for a reconstructed interrupted job", () => {
  // Mirrors OrchestrationPanel's poll gate: only jobs that are still active drive a
  // timeout. A reconstructed interrupted job must not, or the UI would poll forever.
  const jobs: Record<string, ReluxOrchestrationJob> = {
    orch_0001: reconstructedInterrupted(),
  };
  const activeIds = Object.values(jobs)
    .filter((j) => jobIsActive(j))
    .map((j) => j.orchestration_id);
  assert.deepEqual(activeIds, []);
});

test("an interrupted run re-offers Continue to resume the pending briefs", () => {
  // The orchestration record after a partial run: some briefs completed, some
  // pending, status still "running" (no worker is driving it now).
  const partiallyRun = orch("orch_0001", "running", [
    step("t0", "a", "completed"),
    step("t1", "a", "completed"),
    step("t2", "b", "pending"),
    step("t3", "b", "pending"),
  ]);
  assert.equal(canRunOrchestration(partiallyRun), true);
  assert.equal(runButtonLabel(partiallyRun, reconstructedInterrupted()), "Continue");
});

test("jobPhaseLabel for interrupted does not over-claim a restart as the only cause", () => {
  // Honest: the reconstructed state covers finished/canceled/restart-lost runs, so
  // the headline stays cause-neutral and the callout body carries the full reason.
  const label = jobPhaseLabel(reconstructedInterrupted());
  assert.equal(label, "Interrupted — no live worker");
  assert.doesNotMatch(label, /server restart/);
});

test("stepDurationLabel renders the recorded finished−started elapsed time", () => {
  // The kernel stamps both timestamps from its logical clock (ISO-8601-shaped),
  // so the label is the recorded delta — never wall-clock guessing.
  const ran = step("task_0001", "research-agent", "completed", {
    started_at: "2026-06-08T00:00:01Z",
    finished_at: "2026-06-08T00:00:09Z",
  });
  assert.equal(stepDurationLabel(ran), "8.0 s");
  const longer = step("task_0002", "doc-agent", "completed", {
    started_at: "2026-06-08T00:00:00Z",
    finished_at: "2026-06-08T00:02:05Z",
  });
  assert.equal(stepDurationLabel(longer), "2m 5s");
});

test("stepDurationLabel is null until a brief has actually finished", () => {
  // A pending brief has no timing yet; a brief the worker only just started has a
  // start but no finish. Neither fabricates a live ticking duration.
  assert.equal(stepDurationLabel(step("task_0001", "a", "pending")), null);
  const started = step("task_0002", "a", "pending", {
    started_at: "2026-06-08T00:00:01Z",
  });
  assert.equal(stepDurationLabel(started), null);
});

test("stepDurationLabel refuses unparseable or backwards timestamps", () => {
  const garbage = step("task_0001", "a", "completed", {
    started_at: "not-a-time",
    finished_at: "2026-06-08T00:00:09Z",
  });
  assert.equal(stepDurationLabel(garbage), null);
  // A finish before the start is incoherent — show nothing rather than a negative.
  const backwards = step("task_0002", "a", "completed", {
    started_at: "2026-06-08T00:00:09Z",
    finished_at: "2026-06-08T00:00:01Z",
  });
  assert.equal(stepDurationLabel(backwards), null);
});
