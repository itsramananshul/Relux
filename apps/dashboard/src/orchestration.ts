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
  ReluxOrchestrationStatus,
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
