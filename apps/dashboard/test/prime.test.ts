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
  slotProvenance,
  brainSourceLabel,
  replyPolishLabel,
  decisionSourceLabel,
  pendingClarificationLabel,
  updateChangeSummary,
  updateProvenance,
  contextReadsUsedLabel,
  contextReadsHadMiss,
  contextReadDetail,
  boundedContextReads,
} from "../src/prime.ts";
import type { ReluxPendingClarification, ReluxPrimeContextRead, ReluxPrimeProposal, ReluxPrimeTaskSlots, ReluxPrimeTaskUpdate } from "../src/api.ts";

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
test("slotProvenance shows the brain label only when slots are present", () => {
  const slots = (extra: Partial<ReluxPrimeTaskSlots> = {}): ReluxPrimeTaskSlots => ({
    title: "Fix the login redirect bug",
    ...extra,
  });
  // The OpenRouter model id or the CLI brain label is surfaced verbatim.
  assert.equal(slotProvenance(slots({ source: "anthropic/claude-3.5-haiku" })), "anthropic/claude-3.5-haiku");
  assert.equal(slotProvenance(slots({ source: "Claude CLI" })), "Claude CLI");
  // No slots → no chip (never overclaim a brain decision).
  assert.equal(slotProvenance(undefined), null);
  // An older kernel that left `source` unset degrades to a generic label.
  assert.equal(slotProvenance(slots()), "AI brain");
  assert.equal(slotProvenance(slots({ source: "   " })), "AI brain");
});

test("brainSourceLabel is the shared provenance rule for agent/admin slot cards", () => {
  // The stamped model id / CLI label is surfaced verbatim.
  assert.equal(brainSourceLabel("anthropic/claude-3.5-haiku"), "anthropic/claude-3.5-haiku");
  assert.equal(brainSourceLabel("Codex CLI"), "Codex CLI");
  // An unstamped / blank source degrades to the generic label (the caller only
  // renders the card when the kernel attached a validated slot object, so a label is
  // always shown — never null — for agent/admin cards).
  assert.equal(brainSourceLabel(undefined), "AI brain");
  assert.equal(brainSourceLabel("   "), "AI brain");
});

test("intentProvenance shows a label only when the brain decided the intent", () => {
  assert.equal(intentProvenance("brain"), "brain-classified");
  // Deterministic / absent / unknown sources attribute nothing.
  assert.equal(intentProvenance("deterministic"), null);
  assert.equal(intentProvenance(undefined), null);
  assert.equal(intentProvenance(""), null);
});

// A brain-polished clarify/brainstorm reply shows a small chip ONLY when the server
// attached `reply_polish` (i.e. a configured brain re-worded the turn through the
// validated wording path). The label distinguishes a clarifying question from a
// brainstorm reply and carries the brain source; an absent overlay attributes nothing,
// so the chip never overclaims a brain decision on a deterministic-worded turn.
test("replyPolishLabel reads honestly per kind and source, and is null when absent", () => {
  assert.equal(
    replyPolishLabel({ kind: "clarification", source: "anthropic/claude-3.5-haiku" }),
    "brain-worded question · anthropic/claude-3.5-haiku",
  );
  assert.equal(
    replyPolishLabel({ kind: "brainstorm", source: "Claude CLI" }),
    "brain-worded reply · Claude CLI",
  );
  // No overlay -> nothing to attribute.
  assert.equal(replyPolishLabel(undefined), null);
  // An unstamped source degrades to the generic brain label, never blank.
  assert.equal(replyPolishLabel({ kind: "clarification", source: "   " }), "brain-worded question · AI brain");
});

test("decisionSourceLabel names the one unified decision, and is null when absent", () => {
  // A single unified brain call carried multiple proposals -> one concise chip.
  assert.equal(
    decisionSourceLabel("anthropic/claude-3.5-haiku"),
    "one brain decision · anthropic/claude-3.5-haiku",
  );
  assert.equal(decisionSourceLabel("Claude CLI"), "one brain decision · Claude CLI");
  // No unified decision (serial calls / no brain) -> nothing to attribute.
  assert.equal(decisionSourceLabel(undefined), null);
  assert.equal(decisionSourceLabel("   "), null);
});

test("polishProvenance is null without a polish overlay and generic when the source is unrecorded", () => {
  // No overlay -> nothing to attribute, so the card renders no provenance.
  assert.equal(polishProvenance(proposal()), null);
  // An overlay from an older kernel that did not stamp `model` -> a generic label,
  // not a blank or a crash.
  assert.equal(polishProvenance(proposal({ polish: { summary: "x" } })), "AI brain");
  assert.equal(polishProvenance(proposal({ polish: { summary: "x", model: "   " } })), "AI brain");
});

// Multi-turn clarify memory: the "waiting for: …" chip only appears while Prime is
// actually expecting an answer, and it names what is still needed so the user knows
// what to type next. Pins that the chip never appears without a pending clarification.
function pending(extra: Partial<ReluxPendingClarification> = {}): ReluxPendingClarification {
  return {
    original_message: "assign this to researcher",
    intent: "assign_task",
    needs: "task id",
    question: "Which task should I assign?",
    created_at_secs: 1,
    expires_at_secs: 901,
    source: "deterministic",
    ...extra,
  };
}

test("pendingClarificationLabel names the missing field while a clarification is pending", () => {
  assert.equal(pendingClarificationLabel(pending()), "waiting for: task id");
  assert.equal(pendingClarificationLabel(pending({ needs: "agent" })), "waiting for: agent");
  // No pending clarification -> no chip.
  assert.equal(pendingClarificationLabel(undefined), null);
  // A pending record with an empty `needs` still reads sensibly, never blank.
  assert.equal(pendingClarificationLabel(pending({ needs: "   " })), "waiting for your answer");
});

// By-id task update card: the change summary renders the applied rows honestly, and the
// brain provenance chip appears ONLY when a configured brain resolved the change.
function update(extra: Partial<ReluxPrimeTaskUpdate> = {}): ReluxPrimeTaskUpdate {
  return {
    task_id: "task_0001",
    changes: [
      { field: "priority", value: "8" },
      { field: "status", value: "blocked" },
    ],
    ...extra,
  };
}

test("updateChangeSummary renders the applied change rows and updateProvenance gates the chip", () => {
  assert.equal(updateChangeSummary(update()), "priority → 8, status → blocked");
  // No update -> nothing to render.
  assert.equal(updateChangeSummary(undefined), "");
  // A deterministically-parsed update shows the card but NO brain chip.
  assert.equal(updateProvenance(update()), null);
  // A brain-resolved update surfaces the stamped provenance label.
  assert.equal(updateProvenance(update({ source: "anthropic/claude-3.5-haiku" })), "anthropic/claude-3.5-haiku");
  // An unstamped source still reads as the generic brain label, never blank.
  assert.equal(updateProvenance(update({ source: "brain" })), "brain");
  assert.equal(updateProvenance(undefined), null);
});

// READ-ONLY context provenance: when a configured brain inspected live state through the
// governed read-only tool loop before answering, the chip names the DISTINCT tools used,
// flags an honest miss, and the detail list is clamped and bounded so the chat is never
// flooded and no raw JSON is dumped (§10.1, §17.1). These pin that the chip never appears
// without a real read and never fabricates a tool the loop did not run.
function read(extra: Partial<ReluxPrimeContextRead> = {}): ReluxPrimeContextRead {
  return { tool: "get_task", ok: true, summary: 'task_0001: "Fix the login redirect" [queued]', ...extra };
}

test("contextReadsUsedLabel names the distinct tools consulted, in order, else null", () => {
  assert.equal(
    contextReadsUsedLabel([read({ tool: "list_tasks" }), read({ tool: "get_task" })]),
    "used: list_tasks, get_task",
  );
  // Duplicate tool names collapse to one entry (the brain may look at the same tool twice).
  assert.equal(
    contextReadsUsedLabel([read({ tool: "get_task" }), read({ tool: "get_task" }), read({ tool: "list_agents" })]),
    "used: get_task, list_agents",
  );
  // A long loop is bounded — only the first four distinct tools are named, the rest collapse.
  assert.equal(
    contextReadsUsedLabel([
      read({ tool: "board_summary" }),
      read({ tool: "list_tasks" }),
      read({ tool: "get_task" }),
      read({ tool: "list_agents" }),
      read({ tool: "get_agent" }),
      read({ tool: "list_runs" }),
    ]),
    "used: board_summary, list_tasks, get_task, list_agents, +2 more",
  );
  // No reads (or only blank tool names) -> no chip.
  assert.equal(contextReadsUsedLabel(undefined), null);
  assert.equal(contextReadsUsedLabel([]), null);
  assert.equal(contextReadsUsedLabel([read({ tool: "   " })]), null);
});

test("contextReadsHadMiss flags an honest miss, never hides one", () => {
  assert.equal(contextReadsHadMiss([read()]), false);
  assert.equal(contextReadsHadMiss([read(), read({ ok: false, summary: "no task task_9999" })]), true);
  assert.equal(contextReadsHadMiss(undefined), false);
  assert.equal(contextReadsHadMiss([]), false);
});

test("contextReadDetail clamps a long summary and is honest about a miss", () => {
  // A normal short summary passes through trimmed.
  assert.equal(contextReadDetail(read()), 'task_0001: "Fix the login redirect" [queued]');
  // An over-long summary is clamped with an ellipsis (never an unbounded blob / raw JSON).
  const long = "x".repeat(300);
  const detail = contextReadDetail(read({ summary: long }));
  assert.ok(detail.length <= 160, `detail must be bounded, got ${detail.length}`);
  assert.ok(detail.endsWith("…"), "a clamped summary ends with an ellipsis");
  // An empty summary falls back to an honest placeholder per ok/miss — never blank.
  assert.equal(contextReadDetail(read({ summary: "" })), "(no detail)");
  assert.equal(contextReadDetail(read({ ok: false, summary: "" })), "(not found)");
});

test("boundedContextReads caps the detail list and reports the hidden count", () => {
  const many = Array.from({ length: 11 }, (_, i) => read({ summary: `read ${i}` }));
  const { shown, hidden } = boundedContextReads(many);
  assert.equal(shown.length, 8, "the detail list is capped at the client bound");
  assert.equal(hidden, 3, "the overflow is reported honestly");
  // A short list shows everything with nothing hidden.
  const few = boundedContextReads([read(), read({ tool: "list_runs" })]);
  assert.equal(few.shown.length, 2);
  assert.equal(few.hidden, 0);
  // Empty / absent -> nothing shown, nothing hidden.
  assert.deepEqual(boundedContextReads(undefined), { shown: [], hidden: 0 });
  assert.deepEqual(boundedContextReads([]), { shown: [], hidden: 0 });
});
