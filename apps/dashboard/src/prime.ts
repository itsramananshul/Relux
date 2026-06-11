import type { ReluxPrimeProposal } from "./api";

// Pure helpers for rendering Prime's reviewable plan proposal as a card
// (RELUX_MASTER_PLAN §10 planning layer, §11.1 "Prime Chat"). The proposal is a
// PREVIEW: these functions only describe it. Nothing here commits work — the card
// surfaces Prime's existing "Create these tasks" suggestion as the lone commit
// path, and these helpers never fabricate a step or agent the proposal did not
// carry (§17.1: the UI renders only what Prime returned).

// "1 step" / "3 steps", "1 agent" / "2 agents" — a count with a correctly
// pluralized noun, so the summary line reads naturally for any size.
export function countNoun(n: number, noun: string): string {
  return `${n} ${noun}${n === 1 ? "" : "s"}`;
}

// The one-line summary under the proposal title. A genuine multi-step plan reads
// as "N steps across M agents"; a goal that does not split is steered to the
// one-task path honestly, never fanned into a storm (§10.5).
export function proposalSummary(p: ReluxPrimeProposal): string {
  if (!p.multi_step) {
    return "Reads as a single task — not a multi-step plan.";
  }
  return `${countNoun(p.steps.length, "step")} across ${countNoun(p.agents.length, "agent")}.`;
}

// Whether the card should render the step list. A single-step proposal carries no
// steps, so the card shows just the goal + the one-task route — no empty table.
export function hasSteps(p: ReluxPrimeProposal): boolean {
  return p.multi_step && p.steps.length > 0;
}
