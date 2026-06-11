import { test } from "node:test";
import assert from "node:assert/strict";
import {
  countNoun,
  proposalSummary,
  hasSteps,
  stepDisplayTitle,
  proposalDisplaySummary,
  polishProvenance,
  intentProvenance,
} from "../src/prime.ts";
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

// Advisory polish is PRESENTATION ONLY: a polished title/summary overrides what is
// shown, but the authoritative step/agent/order data is untouched (§10, §17.1).
test("stepDisplayTitle prefers the polished title only for the matching step index", () => {
  const p = proposal({
    polish: { step_titles: [{ index: 2, title: "Build a working prototype" }] },
  });
  // Step 1 has no polished title -> the authoritative title shows.
  assert.equal(stepDisplayTitle(p, p.steps[0]), "research the options");
  // Step 2 has a polished title -> the refined wording shows.
  assert.equal(stepDisplayTitle(p, p.steps[1]), "Build a working prototype");
});

test("stepDisplayTitle falls back to the authoritative title when polish is absent or blank", () => {
  const plain = proposal();
  assert.equal(stepDisplayTitle(plain, plain.steps[0]), "research the options");
  const blank = proposal({ polish: { step_titles: [{ index: 1, title: "   " }] } });
  assert.equal(stepDisplayTitle(blank, blank.steps[0]), "research the options");
});

test("proposalDisplaySummary prefers the polished summary, else the deterministic line", () => {
  const polished = proposal({ polish: { summary: "A clear two-stage path to a beta." } });
  assert.equal(proposalDisplaySummary(polished), "A clear two-stage path to a beta.");
  // No polish -> the authoritative count line is shown unchanged.
  assert.equal(proposalDisplaySummary(proposal()), "2 steps across 2 agents.");
});

// Provenance must read HONESTLY for both brains: the OpenRouter HTTP path stamps a
// model id, the local CLI path stamps a friendly label, and an unpolished plan has
// nothing to attribute (§10 planning layer, §11.1, §17.1).
test("polishProvenance shows the OpenRouter model id", () => {
  const p = proposal({ polish: { summary: "x", model: "anthropic/claude-3.5-haiku" } });
  assert.equal(polishProvenance(p), "anthropic/claude-3.5-haiku");
});

test("polishProvenance shows the CLI brain label", () => {
  assert.equal(polishProvenance(proposal({ polish: { summary: "x", model: "Claude CLI" } })), "Claude CLI");
  assert.equal(polishProvenance(proposal({ polish: { summary: "x", model: "Codex CLI" } })), "Codex CLI");
});

// Intent provenance must read HONESTLY: the brain chip shows ONLY when a configured
// brain genuinely classified the intent (`intent_source === "brain"`); a deterministic
// turn — no brain, or a proposal the safety gate vetoed — attributes nothing, so the
// card never overclaims a brain decision (§10.1, §17.1).
test("intentProvenance shows a label only when the brain decided the intent", () => {
  assert.equal(intentProvenance("brain"), "brain-classified");
  // Deterministic / absent / unknown sources attribute nothing.
  assert.equal(intentProvenance("deterministic"), null);
  assert.equal(intentProvenance(undefined), null);
  assert.equal(intentProvenance(""), null);
});

test("polishProvenance is null without a polish overlay and generic when the source is unrecorded", () => {
  // No overlay -> nothing to attribute, so the card renders no provenance.
  assert.equal(polishProvenance(proposal()), null);
  // An overlay from an older kernel that did not stamp `model` -> a generic label,
  // not a blank or a crash.
  assert.equal(polishProvenance(proposal({ polish: { summary: "x" } })), "AI brain");
  assert.equal(polishProvenance(proposal({ polish: { summary: "x", model: "   " } })), "AI brain");
});
