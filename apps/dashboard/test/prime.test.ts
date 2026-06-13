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
  requestedToolLabel,
  afterActionLabel,
  pendingClarificationLabel,
  updateChangeSummary,
  updateProvenance,
  contextReadsUsedLabel,
  contextReadsHadMiss,
  contextReadDetail,
  boundedContextReads,
  formatToolOutput,
  githubPluginInstallAction,
  agentCreatedAction,
  adapterBrandLabel,
  isCapabilityGrantSuggestion,
  isRunOrchestrationSuggestion,
  agentCreatedView,
  PRIME_GREETING,
  PRIME_HINT,
  PRIME_PLACEHOLDER,
  PRIME_SUGGESTIONS,
} from "../src/prime.ts";
import type { ReluxPendingClarification, ReluxPrimeContextRead, ReluxPrimeProposal, ReluxPrimeSuggestion, ReluxPrimeTaskSlots, ReluxPrimeTaskUpdate, ReluxPrimeTurn } from "../src/api.ts";

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

test("requestedToolLabel names the governed write tool, and is null when absent", () => {
  // The server attaches `requested_tool` only when a write tool genuinely drove the turn.
  assert.equal(requestedToolLabel("task.update"), "requested tool: task.update");
  assert.equal(requestedToolLabel("plugin.install"), "requested tool: plugin.install");
  // No honored write tool (a vetoed request, or a normal turn) -> nothing to attribute.
  assert.equal(requestedToolLabel(undefined), null);
  assert.equal(requestedToolLabel("   "), null);
});

test("afterActionLabel names the brain that re-worded a post-execution confirmation, null when absent", () => {
  // The server attaches `after_action_source` only when a brain shaped an ACTIONFUL turn's
  // confirmation after the action ran. The chip reads as wording provenance.
  assert.equal(
    afterActionLabel("anthropic/claude-3.5-haiku"),
    "after-action wording · anthropic/claude-3.5-haiku",
  );
  assert.equal(afterActionLabel("Claude CLI"), "after-action wording · Claude CLI");
  // The reply stayed deterministic (no brain / any failure) -> nothing to attribute.
  assert.equal(afterActionLabel(undefined), null);
  assert.equal(afterActionLabel("   "), null);
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

// Hermes-first chat copy: Prime is presented as a GENERAL local AI agent / chat
// companion, not a company / work-board manager (docs/prime-processing-audit.md
// "Hermes-first general agent"). The intro/hint/placeholder lead with normal
// conversation; the first example chips are general-chat prompts, with the
// control-plane examples kept but secondary.
test("PRIME_GREETING introduces a general agent, not a work-board operator", () => {
  const g = PRIME_GREETING.toLowerCase();
  assert.match(g, /agent/, "Prime is framed as a general agent");
  // It must NOT open by demanding work / setup like the old operator copy.
  assert.doesNotMatch(g, /the local relux operator/);
  assert.doesNotMatch(g, /tell me to create a task/);
  // Conversation framing leads (chat / ask / talk), control-plane comes after.
  assert.match(g, /\b(chat|ask|talk)\b/);
});

test("PRIME_HINT says casual conversation creates nothing", () => {
  const h = PRIME_HINT.toLowerCase();
  assert.match(h, /chat|ask|brainstorm/);
  assert.match(h, /won't create or run|creates? nothing|won't create/);
});

test("PRIME_PLACEHOLDER is a general prompt, not a create-task command", () => {
  assert.doesNotMatch(PRIME_PLACEHOLDER, /create a task to summarize the README/);
  assert.match(PRIME_PLACEHOLDER.toLowerCase(), /ask anything|message prime/);
});

test("PRIME_SUGGESTIONS lead with general chat before control-plane work", () => {
  // The very first chip is a general-capability / chat prompt, not a work command.
  assert.doesNotMatch(PRIME_SUGGESTIONS[0].toLowerCase(), /create a task|assign |start it/);
  assert.match(PRIME_SUGGESTIONS[0].toLowerCase(), /what can you do|help me think|chat/);
  // The work examples are still present (control-plane stays discoverable, just secondary).
  assert.ok(
    PRIME_SUGGESTIONS.some((s) => s.toLowerCase().includes("create a task")),
    "control-plane examples remain available",
  );
});

test("PRIME_SUGGESTIONS carry no contextless cold-start work commands", () => {
  // The chips shown on an EMPTY conversation must not be commands that have no
  // referent on a fresh board: "start it" / "why did it fail?" answer "nothing to
  // start" / "nothing failed", and "orchestrate …" would mint briefs on one click —
  // all of which make Prime read like a work-board bot at first impression. The
  // contextual work CTAs live UNDER a reply instead (§10.5, §11.1, §17.1).
  for (const s of PRIME_SUGGESTIONS) {
    const lc = s.toLowerCase();
    assert.doesNotMatch(lc, /^start it$|^orchestrate\b|why did it fail/, `cold-start chip must not be a contextless work command: ${s}`);
  }
  // At most ONE explicit-work example, and it is not first (conversation leads).
  const workChips = PRIME_SUGGESTIONS.filter((s) => s.toLowerCase().includes("create a task"));
  assert.equal(workChips.length, 1, "exactly one explicit-work example chip");
  assert.notEqual(PRIME_SUGGESTIONS[0].toLowerCase(), workChips[0].toLowerCase(), "the work example is never the first chip");
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

// formatToolOutput renders a tool's shaped result chat-naturally and bounded — never
// the raw transport envelope (docs/mcp.md "Invocation"). Used by both the ran-tool
// result block and the post-approval result inside the chat approval card.
test("formatToolOutput surfaces the shaped result text without wrapper braces", () => {
  // The shaped MCP/tool envelope { result: <text> } shows the text directly.
  assert.equal(formatToolOutput({ result: "all systems nominal" }), "all systems nominal");
  // structuredContent is appended as compact JSON only when present.
  const withStructured = formatToolOutput({ result: "found 2 rows", structuredContent: { rows: 2 } });
  assert.ok(withStructured.startsWith("found 2 rows"));
  assert.ok(withStructured.includes("\"rows\": 2"));
});

test("formatToolOutput handles plain strings, plain objects, and empty output", () => {
  assert.equal(formatToolOutput("hello"), "hello");
  // A plain plugin tool's structured output (no `result` key) is pretty JSON.
  assert.equal(formatToolOutput({ said: "hi" }), "{\n  \"said\": \"hi\"\n}");
  // Empty / absent output renders nothing, so the caller shows no result block.
  assert.equal(formatToolOutput(undefined), "");
  assert.equal(formatToolOutput(null), "");
});

test("formatToolOutput clamps a pathologically large output so it never floods chat", () => {
  const huge = formatToolOutput({ result: "x".repeat(10000) });
  assert.ok(huge.length <= 4000);
  assert.ok(huge.endsWith("…"));
});

test("githubPluginInstallAction extracts the canonical repo + proposed id from the action", () => {
  const got = githubPluginInstallAction({
    type: "install_plugin_from_github",
    repo_url: "https://github.com/nousresearch/hermes-agent",
    plugin_id: "relux-plugin-hermes-agent",
  });
  assert.deepEqual(got, {
    repoUrl: "https://github.com/nousresearch/hermes-agent",
    pluginId: "relux-plugin-hermes-agent",
  });
});

test("githubPluginInstallAction returns null for a non-import / absent / malformed action", () => {
  assert.equal(githubPluginInstallAction(null), null);
  assert.equal(githubPluginInstallAction(undefined), null);
  assert.equal(githubPluginInstallAction({ type: "grant_permission" }), null);
  // Right type but no repo URL → null (the card never trusts an unshaped action).
  assert.equal(githubPluginInstallAction({ type: "install_plugin_from_github" }), null);
});

// ── Agent-creation result card (RELUX_MASTER_PLAN §6, §7.3, §7.5, §8.1) ───────
// The "Prime created an operative" card must read HONESTLY: it surfaces the adapter the
// operative actually runs on, names a requested sensitive capability as needing setup
// (never granted on creation), and is shown ONLY on a real agent-creation turn. These pin
// that the card never invents an outcome Prime did not return (§17.1).

// A minimal agent-creation turn. Only the fields the pure builder reads are set; the cast
// keeps the fixture small without re-stating every wire field.
function agentTurn(extra: Partial<ReluxPrimeTurn> = {}): ReluxPrimeTurn {
  return {
    intent: "agent_creation",
    reply: "Creating agent \"researcher\" on the local adapter.",
    disposition: "executed",
    action: { type: "create_agent", name: "researcher", adapter_plugin: "relux-adapter-local-prime" },
    created_task: null,
    started_run: null,
    created_agent: "researcher",
    approval: null,
    ...extra,
  } as unknown as ReluxPrimeTurn;
}

test("agentCreatedAction extracts the name + adapter from a create_agent action", () => {
  assert.deepEqual(
    agentCreatedAction({ type: "create_agent", name: "Researcher", adapter_plugin: "relux-adapter-claude-cli" }),
    { name: "Researcher", adapterPlugin: "relux-adapter-claude-cli" },
  );
  // Wrong type / absent → null; never trusts an unshaped action.
  assert.equal(agentCreatedAction(null), null);
  assert.equal(agentCreatedAction({ type: "grant_permission" }), null);
});

test("adapterBrandLabel maps known adapters to a human brand and passes others through", () => {
  assert.equal(adapterBrandLabel("relux-adapter-claude-cli"), "Claude");
  assert.equal(adapterBrandLabel("relux-adapter-codex-cli"), "Codex");
  assert.equal(adapterBrandLabel("relux-adapter-local-prime"), "Local (deterministic)");
  assert.equal(adapterBrandLabel("relux-adapter-custom-xyz"), "relux-adapter-custom-xyz");
});

test("isCapabilityGrantSuggestion matches only the approval-gated grant pre-fill", () => {
  const grant: ReluxPrimeSuggestion = {
    label: "Grant GitHub access to researcher",
    message: "grant tool:relux-tools-github:access to researcher",
    send: false,
  };
  assert.equal(isCapabilityGrantSuggestion(grant), true);
  // A send:true chip is never a grant pre-fill, even if it mentions granting.
  assert.equal(isCapabilityGrantSuggestion({ label: "x", message: "grant it", send: true }), false);
  // A non-grant pre-fill is excluded.
  assert.equal(isCapabilityGrantSuggestion({ label: "Start the run", message: "start it", send: false }), false);
});

test("isRunOrchestrationSuggestion matches only the run chip for the given orchestration id", () => {
  // The kernel's exact "Run this orchestration" chip — an immediate run command.
  const runChip: ReluxPrimeSuggestion = {
    label: "Run this orchestration",
    message: "run orchestration orch_0001",
    send: true,
  };
  assert.equal(isRunOrchestrationSuggestion(runChip, "orch_0001"), true);
  // A chip for a DIFFERENT orchestration must not be filtered.
  assert.equal(isRunOrchestrationSuggestion(runChip, "orch_0002"), false);
  // The "Hire a … agent" pre-fill (send:false) is never the run chip.
  assert.equal(
    isRunOrchestrationSuggestion(
      { label: "Hire a documentation agent", message: "create a documentation agent", send: false },
      "orch_0001",
    ),
    false,
  );
  // A look-alike message that is not the exact run command is excluded.
  assert.equal(
    isRunOrchestrationSuggestion(
      { label: "x", message: "run orchestration orch_0001 now", send: true },
      "orch_0001",
    ),
    false,
  );
});

test("agentCreatedView surfaces the deterministic adapter and no setup when nothing was requested", () => {
  const view = agentCreatedView(agentTurn());
  assert.ok(view);
  assert.equal(view!.agentId, "researcher");
  assert.equal(view!.name, "researcher");
  assert.equal(view!.adapterId, "relux-adapter-local-prime");
  assert.equal(view!.adapterLabel, "Local (deterministic)");
  assert.equal(view!.capabilitiesNeedSetup, false);
  assert.equal(view!.grants.length, 0);
  // No brain slots on the deterministic path → no provenance.
  assert.equal(view!.brainSource, null);
});

test("agentCreatedView flags requested capabilities as needing setup and carries the grant follow-ups", () => {
  const view = agentCreatedView(
    agentTurn({
      suggested_actions: [
        { label: "Grant GitHub access to researcher", message: "grant tool:relux-tools-github:access to researcher", send: false },
      ],
    }),
  );
  assert.ok(view);
  assert.equal(view!.capabilitiesNeedSetup, true);
  assert.equal(view!.grants.length, 1);
  assert.equal(view!.grants[0].label, "Grant GitHub access to researcher");
});

test("agentCreatedView prefers the brain-validated adapter slot and folds in role/persona", () => {
  const view = agentCreatedView(
    agentTurn({
      agent_slots: {
        name: "Researcher",
        id: "researcher",
        description: "Reads GitHub and drafts PRs",
        adapter: "relux-adapter-claude-cli",
        persona: "Methodical and concise",
        source: "Claude CLI",
      },
    }),
  );
  assert.ok(view);
  // The brain-validated adapter wins over the action's pre-brain default.
  assert.equal(view!.adapterId, "relux-adapter-claude-cli");
  assert.equal(view!.adapterLabel, "Claude");
  assert.equal(view!.name, "Researcher");
  assert.equal(view!.description, "Reads GitHub and drafts PRs");
  assert.equal(view!.persona, "Methodical and concise");
  assert.equal(view!.brainSource, "Claude CLI");
});

test("agentCreatedView returns null for a non-creation turn or a duplicate-name refusal", () => {
  // Casual ideation must render as normal chat, never an action card.
  assert.equal(agentCreatedView(agentTurn({ intent: "brainstorming", created_agent: null })), null);
  // A duplicate-name refusal is an agent_creation INTENT but created nothing.
  assert.equal(agentCreatedView(agentTurn({ disposition: "answered", created_agent: null })), null);
});
