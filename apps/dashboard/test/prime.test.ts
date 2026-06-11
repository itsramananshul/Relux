import { test } from "node:test";
import assert from "node:assert/strict";
import { countNoun, proposalSummary, hasSteps } from "../src/prime.ts";
import type { ReluxPrimeProposal } from "../src/api.ts";

// The plan proposal card must read HONESTLY: the summary reflects the actual step
// and agent counts, a single-step proposal is steered to the one-task path (not
// fanned into a storm), and the step list only renders when there are real steps.
// These pin that the card never invents shape Prime did not return (§10.5, §17.1).

function proposal(extra: Partial<ReluxPrimeProposal> = {}): ReluxPrimeProposal {
  return {
    goal: "ship the beta",
    multi_step: true,
    steps: [
      { index: 1, title: "research the options", role: "research", agent: "research-agent" },
      { index: 2, title: "build a prototype", role: "implementation", agent: "prime" },
    ],
    agents: ["research-agent", "prime"],
    ...extra,
  };
}

test("countNoun pluralizes only when the count is not one", () => {
  assert.equal(countNoun(1, "step"), "1 step");
  assert.equal(countNoun(2, "step"), "2 steps");
  assert.equal(countNoun(0, "agent"), "0 agents");
});

test("proposalSummary reflects the real step and agent counts for a multi-step plan", () => {
  assert.equal(proposalSummary(proposal()), "2 steps across 2 agents.");
  const big = proposal({
    steps: [
      { index: 1, title: "a", role: "research", agent: "r" },
      { index: 2, title: "b", role: "implementation", agent: "c" },
      { index: 3, title: "c", role: "documentation", agent: "d" },
    ],
    agents: ["r", "c", "d"],
  });
  assert.equal(proposalSummary(big), "3 steps across 3 agents.");
});

test("a single-step proposal is steered to the one-task path, not fanned out", () => {
  const single = proposal({ multi_step: false, steps: [], agents: [] });
  assert.match(proposalSummary(single), /single task/);
  assert.equal(hasSteps(single), false, "a single-step proposal renders no step list");
});

test("hasSteps is true only for a genuine multi-step plan with steps", () => {
  assert.equal(hasSteps(proposal()), true);
  // A proposal flagged multi_step but carrying no steps still renders no table —
  // the card never shows an empty step list.
  assert.equal(hasSteps(proposal({ steps: [] })), false);
});
