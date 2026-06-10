// Pure, framework-free derivations for the multi-agent orchestration view
// (master plan section 10.4 Delegation Rules, section 15 multi-agent workloads).
//
// These mirror the backend's orchestration record and are unit-tested in
// `test/orchestration.test.ts`. Keeping them out of the React components means the
// "honest, grounded" rules — what counts as active, what the next human action is,
// how briefs group by agent — are pinned by tests and reused without a DOM.
// Nothing here invents data: every helper only classifies/aggregates what the API
// already returned.

import type {
  ReluxOrchestration,
  ReluxOrchestrationJob,
  ReluxJobState,
  ReluxOrchestrationStatus,
  ReluxOrchestrationStep,
  ReluxStepOutcome,
} from "./api";

// Badge tone for an orchestration status. Reuses the dashboard's shared tones so
// the orchestration view matches the rest of the B&W UI.
export function orchestrationStatusTone(
  status: ReluxOrchestrationStatus | undefined,
): "done" | "in_progress" | "in_review" | "backlog" {
  switch (status) {
    case "completed":
      return "done";
    case "running":
      return "in_progress";
    case "needs_attention":
      return "in_review";
    default:
      return "backlog"; // planned / unknown
  }
}

// Badge tone for a single brief's outcome.
export function stepOutcomeTone(
  outcome: ReluxStepOutcome | undefined,
): "done" | "blocked" | "backlog" {
  switch (outcome) {
    case "completed":
      return "done";
    case "failed":
      return "blocked";
    case "blocked":
      return "blocked";
    default:
      return "backlog"; // pending / unknown
  }
}

// Per-orchestration progress counts, computed from the step set (never trusted
// from a single summary field). `total` is the brief count; the rest are tallies.
export interface OrchestrationProgress {
  total: number;
  completed: number;
  pending: number;
  failed: number;
  blocked: number;
}

export function orchestrationProgress(o: ReluxOrchestration): OrchestrationProgress {
  const p: OrchestrationProgress = {
    total: o.steps.length,
    completed: 0,
    pending: 0,
    failed: 0,
    blocked: 0,
  };
  for (const s of o.steps) {
    if (s.outcome === "completed") p.completed += 1;
    else if (s.outcome === "failed") p.failed += 1;
    else if (s.outcome === "blocked") p.blocked += 1;
    else p.pending += 1;
  }
  return p;
}

// A compact "2/3 completed" style progress label.
export function orchestrationProgressLabel(o: ReluxOrchestration): string {
  const p = orchestrationProgress(o);
  return `${p.completed}/${p.total} completed`;
}

// The live dependency-aware lifecycle of one brief, derived from its own outcome
// plus its dependencies' outcomes. This is what the panel renders so an operator
// can see, before and after a run, which briefs are runnable now ("ready"), which
// are still gated ("waiting"), and which are done/failed/blocked. Mirrors the
// kernel scheduler's own readiness rule (run only when every dependency
// completed); it never invents state.
export type StepLifecycle =
  | "completed"
  | "failed"
  | "blocked"
  | "ready"
  | "waiting";

export function stepLifecycle(
  o: ReluxOrchestration,
  index: number,
): StepLifecycle {
  const step = o.steps[index];
  if (!step) return "waiting";
  if (step.outcome === "completed") return "completed";
  if (step.outcome === "failed") return "failed";
  if (step.outcome === "blocked") return "blocked";
  // Pending: classify by its dependencies. Treat an out-of-range index as
  // satisfied (defensive) so a malformed record never hides a runnable brief.
  const deps = step.depends_on ?? [];
  const depStates = deps.map((j) => o.steps[j]?.outcome);
  if (depStates.some((s) => s === "failed" || s === "blocked")) return "blocked";
  if (depStates.every((s) => s === undefined || s === "completed")) return "ready";
  return "waiting";
}

// Badge tone for a derived lifecycle state (reuses the shared B&W tones).
export function stepLifecycleTone(
  state: StepLifecycle,
): "done" | "in_progress" | "blocked" | "backlog" {
  switch (state) {
    case "completed":
      return "done";
    case "ready":
      return "in_progress";
    case "failed":
    case "blocked":
      return "blocked";
    default:
      return "backlog"; // waiting
  }
}

// A human label for a brief's dependencies, e.g. "waits on task_0003, task_0004"
// — or null when the brief is independent. Resolves indices to task ids so the
// row reads in product terms, not array offsets.
export function stepDependencyLabel(
  o: ReluxOrchestration,
  step: ReluxOrchestrationStep,
): string | null {
  const deps = step.depends_on ?? [];
  if (!deps.length) return null;
  const ids = deps
    .map((j) => o.steps[j]?.task_id)
    .filter((id): id is string => typeof id === "string");
  if (!ids.length) return null;
  return `waits on ${ids.join(", ")}`;
}

// Dependency-aware tallies for the orchestration: how many briefs are runnable
// now vs still gated, on top of the raw outcome counts. Computed from the step
// set, so it always matches the record.
export interface OrchestrationReadiness {
  ready: number;
  waiting: number;
  blocked: number;
  completed: number;
  failed: number;
}

export function orchestrationReadiness(
  o: ReluxOrchestration,
): OrchestrationReadiness {
  const r: OrchestrationReadiness = {
    ready: 0,
    waiting: 0,
    blocked: 0,
    completed: 0,
    failed: 0,
  };
  o.steps.forEach((_, i) => {
    switch (stepLifecycle(o, i)) {
      case "ready":
        r.ready += 1;
        break;
      case "waiting":
        r.waiting += 1;
        break;
      case "blocked":
        r.blocked += 1;
        break;
      case "completed":
        r.completed += 1;
        break;
      case "failed":
        r.failed += 1;
        break;
    }
  });
  return r;
}

// The distinct batch rounds an orchestration's briefs ran in, smallest first,
// each with the briefs recorded for that round. Briefs that have not run yet
// (no round) are omitted. Lets the panel show "round 1: …, round 2: …" honestly
// after a batch — real recorded data, never a fabricated timeline.
export function orchestrationRounds(
  o: ReluxOrchestration,
): { round: number; steps: ReluxOrchestrationStep[] }[] {
  const byRound = new Map<number, ReluxOrchestrationStep[]>();
  for (const s of o.steps) {
    if (typeof s.round === "number") {
      if (!byRound.has(s.round)) byRound.set(s.round, []);
      byRound.get(s.round)!.push(s);
    }
  }
  return [...byRound.keys()]
    .sort((a, b) => a - b)
    .map((round) => ({ round, steps: byRound.get(round)! }));
}

// True when an orchestration has briefs left to run (so the UI can offer a
// Run/Continue control). Completed orchestrations and ones with no pending briefs
// return false.
export function canRunOrchestration(o: ReluxOrchestration): boolean {
  return o.steps.some((s) => s.outcome === "pending");
}

// The single most relevant orchestration to surface on Home: prefer an unfinished
// one (planned / running / needs_attention), newest first by id; otherwise the
// newest overall. Returns null for an empty list. Ids are zero-padded
// (`orch_0001`) so lexical ordering matches creation order.
export function activeOrchestration(
  list: ReluxOrchestration[],
): ReluxOrchestration | null {
  if (!list.length) return null;
  const byIdDesc = [...list].sort((a, b) => (a.id < b.id ? 1 : a.id > b.id ? -1 : 0));
  const unfinished = byIdDesc.filter((o) => o.status !== "completed");
  return unfinished[0] ?? byIdDesc[0];
}

// A grounded, single-line next human action for an orchestration. Derived purely
// from its status + step tallies, so it always matches what was actually recorded.
export function orchestrationNextAction(o: ReluxOrchestration): string {
  const p = orchestrationProgress(o);
  switch (o.status) {
    case "completed":
      return "All briefs completed. Review the runs.";
    case "running":
      return `${p.pending} brief(s) pending — run the orchestration again to continue.`;
    case "needs_attention":
      return `${p.blocked + p.failed} brief(s) need attention: enable a blocked agent's runtime, reassign, or retry, then run again.`;
    default:
      return "Run the orchestration to start its briefs.";
  }
}

// Group an orchestration's briefs by their assigned agent, for the per-agent
// "who is doing what" view. Preserves first-seen agent order and per-agent brief
// order so the render is deterministic.
export function groupStepsByAgent(
  o: ReluxOrchestration,
): { agentId: string; steps: ReluxOrchestration["steps"] }[] {
  const order: string[] = [];
  const map = new Map<string, ReluxOrchestration["steps"]>();
  for (const s of o.steps) {
    if (!map.has(s.agent_id)) {
      map.set(s.agent_id, []);
      order.push(s.agent_id);
    }
    map.get(s.agent_id)!.push(s);
  }
  return order.map((agentId) => ({ agentId, steps: map.get(agentId)! }));
}

// --- Non-blocking orchestration jobs --------------------------------------
//
// These derive UI state from a polled background-job record. A job is the
// runtime twin of a governed batch run: the dashboard starts one, polls it until
// it finishes, and renders its real, recorded progress (phase, round, per-brief
// status) instead of a bare spinner. Every helper only classifies what the job
// already reported — nothing fabricates in-flight progress.

// True while a job is still working (queued or running), so the UI can disable a
// second Run/Continue and keep polling.
export function jobIsActive(job: ReluxOrchestrationJob | null | undefined): boolean {
  return job != null && (job.state === "queued" || job.state === "running");
}

// True once a job has stopped working (completed, failed, canceled, or
// interrupted); the UI then stops polling, refreshes the durable record, and
// re-enables Run/Continue. "interrupted" is the restart-honest reconstructed state
// (no live worker, pending briefs remain) — terminal for that job, resumable.
export function jobIsTerminal(state: ReluxJobState | undefined): boolean {
  return (
    state === "completed" ||
    state === "failed" ||
    state === "canceled" ||
    state === "interrupted"
  );
}

// True while a job is active and a cancel has already been requested: the worker
// is finishing its in-flight round before stopping. The UI shows "Canceling…" and
// disables a second Cancel.
export function jobIsCanceling(job: ReluxOrchestrationJob | null | undefined): boolean {
  return jobIsActive(job) && job?.cancel_requested === true;
}

// True when the UI should offer a Cancel control: the job is active and no cancel
// is pending yet.
export function jobCanCancel(job: ReluxOrchestrationJob | null | undefined): boolean {
  return jobIsActive(job) && job?.cancel_requested !== true;
}

// A short human phase label for the job, used in place of a spinner. Reflects the
// real lifecycle the worker reported, never a guessed step.
export function jobPhaseLabel(job: ReluxOrchestrationJob | null | undefined): string {
  if (!job) return "";
  // A pending cancel takes precedence over the running phase: the worker is
  // finishing the in-flight round before it stops.
  if (jobIsCanceling(job)) {
    return job.current_round > 0
      ? `Canceling — finishing round ${job.current_round}`
      : "Canceling…";
  }
  switch (job.state) {
    case "queued":
      return "Queued";
    case "running": {
      const base =
        job.current_round > 0
          ? `Running — round ${job.current_round}`
          : "Running — starting";
      // Surface the real OS-parallelism: how many briefs are in flight together
      // right now, and the round cap. Only when more than one runs at once, so a
      // single-brief round stays quiet.
      const inflight = job.steps.filter((s) => s.outcome === "running").length;
      return inflight > 1
        ? `${base} · ${inflight} briefs in parallel (cap ${job.concurrency})`
        : base;
    }
    case "completed":
      return "Completed";
    case "failed":
      return "Failed";
    case "canceled":
      return "Canceled";
    case "interrupted":
      // Reconstructed from the durable record after a restart (no live worker).
      return "Interrupted — run was lost to a server restart";
    default:
      return "";
  }
}

// A compact "3/5 briefs ran · 2 completed" progress line from the job's running
// tallies. Returns "" when there is nothing to show yet.
export function jobProgressLabel(job: ReluxOrchestrationJob | null | undefined): string {
  if (!job) return "";
  const total = job.steps.length;
  const bits = [`${job.ran}/${total} briefs run`];
  if (job.completed > 0) bits.push(`${job.completed} completed`);
  if (job.failed > 0) bits.push(`${job.failed} failed`);
  if (job.blocked > 0) bits.push(`${job.blocked} blocked`);
  return bits.join(" · ");
}

// The task ids of briefs the job is currently running this round (marked
// "running" in the job's step snapshot). Lets the panel highlight in-flight work.
export function jobRunningStepIds(job: ReluxOrchestrationJob | null | undefined): string[] {
  if (!job) return [];
  return job.steps.filter((s) => s.outcome === "running").map((s) => s.task_id);
}

// The Run/Continue button label given the current job and orchestration. While a
// job is active it reflects the live phase; otherwise it is the resting verb
// (Run for a fresh plan, Continue for one with progress, Retry after a failure).
export function runButtonLabel(
  o: ReluxOrchestration,
  job: ReluxOrchestrationJob | null | undefined,
): string {
  if (jobIsActive(job)) {
    return job!.state === "queued" ? "Queued..." : "Running...";
  }
  if (job?.state === "failed") return "Retry";
  if (o.status === "planned") return "Run orchestration";
  return "Continue";
}

// A one-line Home headline summarizing orchestration activity across the fleet,
// or null when there is nothing to surface (so Home can hide the card).
export function orchestrationHeadline(list: ReluxOrchestration[]): string | null {
  if (!list.length) return null;
  const active = list.filter((o) => o.status !== "completed").length;
  if (active === 0) {
    return `${list.length} orchestration${list.length === 1 ? "" : "s"}, all completed.`;
  }
  return `${active} active orchestration${active === 1 ? "" : "s"} across the fleet.`;
}
