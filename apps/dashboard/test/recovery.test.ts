// Unit tests for the recovery recommendation model (src/recovery.ts) — the §3.3b
// diagnosis-driven decision card content built deterministically from a run's
// structured failure class + retry/session state and a blocked task's reopen
// eligibility.
//
// Pure logic only — no React, no DOM — so it runs under `node --test
// --experimental-strip-types`. The card rendering is covered by
// work-recovery-render.test.mjs; this pins the classification + proposed-action
// semantics (the actions only ever reference an EXISTING route, and a no-action
// case explains what is missing).

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  assessRunRecovery,
  assessTaskRecovery,
  latestRunForTask,
  type RecoveryActionKind,
} from "../src/recovery.ts";
import type { ReluxRun, ReluxRunDetail, ReluxTask } from "../src/api.ts";

function run(over: Partial<ReluxRunDetail> = {}): ReluxRunDetail {
  return {
    id: "run_0001",
    task_id: "task_0001",
    agent_id: "prime",
    adapter_plugin: "claude-cli",
    status: "failed",
    ...over,
  } as ReluxRunDetail;
}

function task(over: Partial<ReluxTask> = {}): ReluxTask {
  return {
    id: "task_0001",
    title: "held work",
    input: {},
    status: "blocked",
    priority: 5,
    created_by: "operator",
    assigned_agent: "prime",
    namespace_id: "ns_root",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...over,
  };
}

function kinds(actions: { kind: RecoveryActionKind }[]): RecoveryActionKind[] {
  return actions.map((a) => a.kind);
}

// ── No card when there's nothing to recover ────────────────────────────────
test("assessRunRecovery returns null for a healthy (non-failed, classless) run", () => {
  assert.equal(assessRunRecovery(run({ status: "running", failure_class: undefined })), null);
  assert.equal(assessRunRecovery(run({ status: "completed", failure_class: undefined })), null);
});

// ── Adapter unavailable / missing ──────────────────────────────────────────
test("adapter_missing recommends configuring the adapter, then a retry", () => {
  const a = assessRunRecovery(run({ failure_class: "adapter_missing", retryable: true }))!;
  assert.equal(a.subject, "run");
  assert.match(a.classLabel, /Adapter not available/);
  assert.match(a.rootCause, /isn't installed or enabled/i);
  // The primary action is configuring the agent (an existing route), and a retry is offered.
  assert.equal(a.actions[0].kind, "configure_agent");
  assert.equal(a.actions[0].primary, true);
  assert.ok(kinds(a.actions).includes("retry_run"));
  assert.equal(a.missingInfo, null);
});

test("adapter_missing omits Retry when the run is not retry-eligible", () => {
  const a = assessRunRecovery(run({ failure_class: "adapter_missing", retryable: false }))!;
  assert.ok(!kinds(a.actions).includes("retry_run"));
  // Still offers the real fix (configure) + inspect — never an empty card.
  assert.ok(kinds(a.actions).includes("configure_agent"));
  assert.ok(kinds(a.actions).includes("inspect"));
});

// ── Auth required ──────────────────────────────────────────────────────────
test("auth_required recommends fixing the credential", () => {
  const a = assessRunRecovery(run({ failure_class: "auth_required", retryable: true }))!;
  assert.match(a.classLabel, /Authentication required/);
  assert.match(a.recommendation, /credential/i);
  assert.equal(a.actions[0].kind, "configure_agent");
});

// ── Permission denied → grant or reassign ──────────────────────────────────
test("permission_denied offers grant (configure) and reassign", () => {
  const a = assessRunRecovery(run({ failure_class: "permission_denied", retryable: true }))!;
  assert.match(a.classLabel, /Permission denied/);
  assert.ok(kinds(a.actions).includes("configure_agent"));
  assert.ok(kinds(a.actions).includes("reassign"));
});

// ── Transient / timeout → auto-retry lane ──────────────────────────────────
test("transient_provider with a scheduled retry says it self-heals + offers retry-now", () => {
  const a = assessRunRecovery(
    run({
      failure_class: "transient_provider",
      retryable: true,
      retry: { attempt: 1, max_attempts: 4, not_before_secs: 9_999_999_999, exhausted: false },
    }),
  )!;
  assert.match(a.recommendation, /self-heals|scheduled/i);
  assert.equal(a.actions[0].kind, "retry_run");
  assert.equal(a.actions[0].primary, true);
});

test("transient_provider with exhausted retries says the budget is spent", () => {
  const a = assessRunRecovery(
    run({
      failure_class: "timeout",
      retryable: true,
      retry: { attempt: 4, max_attempts: 4, exhausted: true },
    }),
  )!;
  assert.match(a.classLabel, /Timed out/);
  assert.match(a.recommendation, /spent|manually/i);
});

// ── Tool / output validation ───────────────────────────────────────────────
test("invalid_prompt leads with inspect, then retry/reassign", () => {
  const a = assessRunRecovery(run({ failure_class: "invalid_prompt", retryable: true }))!;
  assert.equal(a.actions[0].kind, "inspect");
  assert.equal(a.actions[0].primary, true);
  assert.ok(kinds(a.actions).includes("reassign"));
});

test("output_validation leads with inspect", () => {
  const a = assessRunRecovery(run({ failure_class: "output_validation", retryable: true }))!;
  assert.equal(a.actions[0].kind, "inspect");
  assert.match(a.classLabel, /Output validation/);
});

// ── Cancelled → terminal non-error ─────────────────────────────────────────
test("cancelled is framed as a non-error with a fresh-run option", () => {
  const a = assessRunRecovery(run({ status: "cancelled", failure_class: "cancelled", retryable: true }))!;
  assert.match(a.rootCause, /non-error/i);
  assert.equal(a.actions[0].kind, "retry_run");
  assert.match(a.actions[0].label, /Run again/);
});

// ── Unknown / unclassified failure ─────────────────────────────────────────
test("an unclassified failed run says what's missing and offers inspect", () => {
  const a = assessRunRecovery(run({ status: "failed", failure_class: undefined, retryable: true }))!;
  assert.match(a.classLabel, /Unknown failure/);
  assert.equal(a.actions[0].kind, "inspect");
  assert.ok(a.missingInfo && /no structured failure class/i.test(a.missingInfo));
});

test("a resumable run adds a resume_session action", () => {
  const a = assessRunRecovery(
    run({ failure_class: "unknown", retryable: true, resumable: true }),
  )!;
  assert.ok(kinds(a.actions).includes("resume_session"));
});

// ── Blocked task recovery ──────────────────────────────────────────────────
test("a blocked + assigned task recommends reopen & run, reopen, reassign", () => {
  const a = assessTaskRecovery(task({ status: "blocked", assigned_agent: "prime" }), null)!;
  assert.equal(a.subject, "task");
  assert.equal(a.classLabel, "Blocked");
  assert.equal(a.actions[0].kind, "reopen_and_run");
  assert.equal(a.actions[0].primary, true);
  assert.deepEqual(kinds(a.actions), ["reopen_and_run", "reopen", "reassign"]);
  assert.equal(a.missingInfo, null);
});

test("a blocked + UNASSIGNED task says assign first and surfaces the honest reason", () => {
  const a = assessTaskRecovery(task({ status: "blocked", assigned_agent: undefined }), null)!;
  assert.deepEqual(kinds(a.actions), ["reassign"]);
  assert.match(a.actions[0].label, /Assign operative/);
  assert.ok(a.missingInfo && /assign an operative/i.test(a.missingInfo));
});

test("a blocked task folds its last failed run's root cause into the card", () => {
  const lastRun = run({ id: "run_0009", failure_class: "adapter_missing", status: "failed" });
  const a = assessTaskRecovery(task({ status: "blocked", assigned_agent: "prime" }), lastRun)!;
  assert.match(a.rootCause, /last run stalled/i);
  assert.match(a.rootCause, /adapter/i);
});

test("assessTaskRecovery returns null for a non-blocked task", () => {
  for (const s of ["created", "queued", "running", "failed", "completed", "cancelled", "waiting_for_approval"]) {
    assert.equal(assessTaskRecovery(task({ status: s }), null), null, `status ${s} → no task card`);
  }
});

// ── latestRunForTask ───────────────────────────────────────────────────────
test("latestRunForTask picks the highest-id run for the task and ignores others", () => {
  const runs: ReluxRun[] = [
    run({ id: "run_0001", task_id: "task_0001" }),
    run({ id: "run_0007", task_id: "task_0001" }),
    run({ id: "run_0009", task_id: "task_OTHER" }),
    run({ id: "run_0003", task_id: "task_0001" }),
  ];
  assert.equal(latestRunForTask(runs, "task_0001")?.id, "run_0007");
  assert.equal(latestRunForTask(runs, "task_NONE"), null);
});
