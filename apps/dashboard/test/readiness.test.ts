import { test } from "node:test";
import assert from "node:assert/strict";
import {
  buildReadiness,
  adapterWorkItem,
  crewItem,
  pluginToolItem,
  stateUnavailableItem,
  deriveFirstAction,
  brainLabel,
  brainIsBlocked,
  tryPrimeItem,
  continuationItem,
  attentionWorkItem,
} from "../src/readiness.ts";
import { CLAUDE_ADAPTER_ID, CODEX_ADAPTER_ID } from "../src/onboarding.ts";

// The readiness report is the honesty contract of Relux Home: every item is
// derived from live control-plane state, a SELECTED-but-broken brain is the only
// blocker (a local brain WORKS), an installed-but-unconfigured wrapper/runtime is
// "attention" not "ready", and there is always one clear first action. These
// assertions pin the four required states (fresh/local-only, Claude available but
// disabled, metadata plugin needs config, fully ready) plus the blocker + first
// action so a regression (a faked green check, a misrouted link) fails loudly.
//
// The test is not type-checked; builders only shape the runtime fields used.

function state(over = {}) {
  return {
    db_path: "x",
    plugins: 0,
    installed_plugins: 0,
    namespaces: 1,
    agents: 0,
    tasks: 0,
    runs: 0,
    approvals: 0,
    open_tasks: 0,
    active_runs: 0,
    waiting_approval: 0,
    blocked: 0,
    failed: 0,
    pending_approvals: 0,
    ...over,
  };
}

function ai(over = {}) {
  return {
    mode: "deterministic",
    brain: "local",
    configured: false,
    disabled: false,
    model: "openai/gpt-4o-mini",
    timeout_ms: 30000,
    reason: "no key",
    ...over,
  };
}

function adapter(id, over = {}) {
  return {
    plugin_id: id,
    adapter_name: id,
    kind: "adapter",
    configured: false,
    enabled: false,
    command: id === CLAUDE_ADAPTER_ID ? "claude" : "codex",
    available_on_path: false,
    resolved_path: null,
    timeout_seconds: 120,
    max_output_bytes: 65536,
    working_dir: null,
    state: "missing_binary",
    detail: "",
    ...over,
  };
}

function wrapperPlugin(over = {}) {
  return {
    id: "relux-thing",
    name: "Thing",
    description: "",
    kind: "ToolSet",
    version: "0.1.0",
    enabled: true,
    source_kind: "Github",
    source_label: "gh",
    install_dir: "/x",
    protected: false,
    bundled: false,
    generated: true,
    tool_count: 0,
    ...over,
  };
}

function tool(executable, over = {}) {
  return {
    plugin_id: "relux-thing",
    tool_name: "do",
    description: "",
    permission: "thing.do",
    risk: "low",
    source_kind: "Github",
    installed: true,
    enabled: true,
    protected: false,
    executable,
    ...over,
  };
}

// ── State 1: fresh / local-only ──────────────────────────────────────────────

test("fresh local-only instance is operational with no blockers", () => {
  const r = buildReadiness({
    state: state(),
    ai: ai({ brain: "local" }),
    adapters: [],
    plugins: [],
    tools: [],
  });
  // Local Prime is a WORKING brain — nothing blocks, so the caller shows the
  // operational summary, not a nag.
  assert.equal(r.ready, true);
  assert.equal(r.blockers.length, 0);
  // The brain item is a recommendation (link), never a failure.
  const brain = r.items.find((i) => i.id === "prime-brain");
  assert.equal(brain.status, "link");
  // With no tasks yet, the clearest first action is to ask Prime.
  assert.equal(r.firstAction.linkTo, "/prime");
  assert.match(r.firstAction.label, /first task/i);
});

test("fresh crew item explains the local Prime fallback (not a blocker)", () => {
  const item = crewItem(state({ agents: 0 }));
  assert.equal(item.status, "link");
  assert.equal(item.linkTo, "/crew");
  assert.match(item.description, /built-in operative/i);
});

// ── State 2: Claude available but the adapter is disabled ────────────────────

test("Claude CLI detected on PATH but disabled is an actionable link to Crew", () => {
  const item = adapterWorkItem([
    adapter(CLAUDE_ADAPTER_ID, { available_on_path: true, state: "disabled" }),
  ]);
  assert.equal(item.status, "link");
  assert.equal(item.linkTo, "/crew");
  assert.match(item.description, /not enabled/i);
  assert.match(item.cta ?? "", /Enable on Crew/i);
});

test("an enabled+on-PATH adapter reads as a ready real-work path (done)", () => {
  const item = adapterWorkItem([
    adapter(CODEX_ADAPTER_ID, { available_on_path: true, enabled: true, state: "available" }),
  ]);
  assert.equal(item.status, "done");
  assert.match(item.description, /Codex CLI/);
});

test("no CLI anywhere is an optional link, never a blocker", () => {
  const item = adapterWorkItem([]);
  assert.equal(item.status, "link");
  assert.match(item.description, /optional/i);
});

// ── State 3: a metadata-only plugin needs configuration ──────────────────────

test("a generated metadata-only wrapper is attention, never ready", () => {
  const item = pluginToolItem([wrapperPlugin()], []);
  assert.equal(item.status, "warn");
  assert.equal(item.linkTo, "/plugins");
  assert.match(item.description, /metadata-only wrapper/i);
});

test("the wrapper shows as attention in the full report but does not block", () => {
  const r = buildReadiness({
    state: state(),
    ai: ai({ brain: "local" }),
    adapters: [],
    plugins: [wrapperPlugin()],
    tools: [],
  });
  assert.equal(r.ready, true); // warn ≠ blocker
  assert.equal(r.blockers.length, 0);
  assert.equal(r.attention.length, 1);
  assert.equal(r.attention[0].id, "plugins-tools");
});

test("tools needing a runtime are attention with a Plugins fix", () => {
  const item = pluginToolItem([], [tool("runtime_not_configured")]);
  assert.equal(item.status, "warn");
  assert.match(item.description, /loopback runtime/i);
});

test("an unavailable tools probe stays honest (info), never 'no tools'", () => {
  const item = pluginToolItem([], null);
  assert.equal(item.status, "info");
  assert.match(item.description, /unavailable/i);
});

// ── State 4: fully ready ─────────────────────────────────────────────────────

test("a fully-configured instance is operational with a useful summary", () => {
  const r = buildReadiness({
    state: state({ agents: 2, tasks: 3, open_tasks: 1 }),
    ai: ai({ brain: "claude_cli", mode: "claude_cli" }),
    adapters: [
      adapter(CLAUDE_ADAPTER_ID, {
        available_on_path: true,
        enabled: true,
        configured: true,
        state: "available",
      }),
    ],
    plugins: [],
    tools: [tool("ready"), tool("ready"), tool("needs_approval")],
  });
  assert.equal(r.ready, true);
  assert.equal(r.blockers.length, 0);
  assert.equal(r.attention.length, 0);
  assert.equal(r.items.find((i) => i.id === "prime-brain").status, "done");
  assert.equal(r.items.find((i) => i.id === "run-real-work").status, "done");
  assert.equal(r.items.find((i) => i.id === "crew").status, "done");
  assert.equal(r.items.find((i) => i.id === "plugins-tools").status, "done");
  // The summary is honest and secret-free.
  assert.match(r.summary, /Claude CLI/);
  assert.match(r.summary, /2 agents/);
  assert.match(r.summary, /2 tools ready/);
  // An approval-gated tool is noted, not counted as ready.
  assert.match(r.items.find((i) => i.id === "plugins-tools").description, /approval/i);
});

// ── Blocker + first-action priority ──────────────────────────────────────────

test("OpenRouter selected without a key is the only thing that blocks setup", () => {
  const r = buildReadiness({
    state: state(),
    ai: ai({ brain: "openrouter", configured: false }),
    adapters: [],
    plugins: [],
    tools: [],
  });
  assert.equal(r.ready, false);
  assert.equal(r.blockers.length, 1);
  assert.equal(r.blockers[0].id, "prime-brain");
  assert.equal(r.blockers[0].linkTo, "/health");
});

test("first action prioritises a pending decision, then work in flight", () => {
  assert.equal(deriveFirstAction(state({ pending_approvals: 2 })).linkTo, "/approvals");
  assert.equal(deriveFirstAction(state({ active_runs: 1 })).linkTo, "/work");
  assert.equal(deriveFirstAction(state({ tasks: 5, open_tasks: 2 })).linkTo, "/work");
  assert.equal(deriveFirstAction(state({ tasks: 0 })).linkTo, "/prime");
  assert.equal(deriveFirstAction(null).linkTo, "/prime");
});

// ── Read-failure honesty: loading vs FAILED must read differently ────────────
//
// The trap this pins: a null read because the request is still in flight must
// stay "Checking readiness…" (report null at the call site, or a neutral info
// row), while a null read because the request FAILED (settled) must become an
// explicit, retryable "… unavailable" row — never an indefinite checking text and
// never a faked-ready green badge.

test("a failed state read degrades the whole report honestly (not ready)", () => {
  const r = buildReadiness({
    state: null,
    ai: ai({ brain: "local" }),
    adapters: [],
    plugins: [],
    tools: [],
    failed: { state: true },
  });
  assert.equal(r.degraded, true);
  assert.equal(r.ready, false); // a partial report must not fake "operational"
  const row = r.items.find((i) => i.id === "state-unavailable");
  assert.ok(row, "an explicit State unavailable row is present");
  assert.equal(row.status, "warn");
  assert.equal(row.retry, true);
  assert.equal(row.linkTo, "/health");
  assert.match(row.description, /partial/i);
});

test("a null state WITHOUT a failed flag stays loading (no unavailable row)", () => {
  // The caller has not yet learned the read failed — it is still in flight — so
  // the report must not assert "State unavailable".
  const r = buildReadiness({
    state: null,
    ai: ai({ brain: "local" }),
    adapters: [],
    plugins: [],
    tools: [],
  });
  assert.equal(r.degraded, false);
  assert.equal(r.items.find((i) => i.id === "state-unavailable"), undefined);
});

test("stateUnavailableItem is a retryable warn that points at Health", () => {
  const item = stateUnavailableItem();
  assert.equal(item.status, "warn");
  assert.equal(item.retry, true);
  assert.equal(item.linkTo, "/health");
});

test("a FAILED tools read is an explicit retryable row, not the loading text", () => {
  const item = pluginToolItem([], null, { tools: true });
  assert.equal(item.status, "warn");
  assert.equal(item.retry, true);
  assert.match(item.label, /Tools unavailable/i);
  assert.match(item.description, /Could not read/i);
});

test("a still-loading tools probe stays a neutral info row (no retry, no failure)", () => {
  const item = pluginToolItem([], null);
  assert.equal(item.status, "info");
  assert.notEqual(item.retry, true);
});

test("a FAILED plugin-list read says so instead of inferring 'no plugins'", () => {
  const item = pluginToolItem(null, null, { plugins: true });
  assert.equal(item.status, "warn");
  assert.equal(item.retry, true);
  assert.match(item.label, /Plugins unavailable/i);
});

test("a FAILED adapter read is honest, not a guessed 'install a CLI'", () => {
  const item = adapterWorkItem(null, true);
  assert.equal(item.status, "warn");
  assert.equal(item.retry, true);
  assert.match(item.label, /unavailable/i);
  // A null adapter list WITHOUT the failed flag keeps the prior optional-link copy.
  const loading = adapterWorkItem(null);
  assert.equal(loading.status, "link");
  assert.notEqual(loading.retry, true);
});

test("a degraded report is reported degraded even with no blockers", () => {
  const r = buildReadiness({
    state: state(),
    ai: ai({ brain: "local" }),
    adapters: [],
    plugins: [],
    tools: null,
    failed: { tools: true },
  });
  assert.equal(r.blockers.length, 0);
  assert.equal(r.degraded, true);
  assert.equal(r.ready, false);
});

test("brainLabel renders each brain honestly", () => {
  assert.equal(brainLabel(null), "unknown");
  assert.equal(brainLabel(ai({ brain: "local" })), "Local (deterministic)");
  assert.equal(brainLabel(ai({ brain: "claude_cli" })), "Claude CLI");
  assert.equal(brainLabel(ai({ brain: "openrouter" })), "OpenRouter");
});

// ── Guided "try Prime" step (the first useful turn) ──────────────────────────
//
// The answer to "what do I do?". It only appears once the brain is connected,
// reads "done" once any real work exists, and is a recommendation (link), never a
// blocker, when the brain is ready but nothing has happened yet.

function continuation(over = {}) {
  return {
    id: "cont_0001",
    reason: "autonomy ceiling reached",
    observation_count: 3,
    extended_used: false,
    awaiting_approval: false,
    ...over,
  };
}

test("brainIsBlocked is true only for a selected-but-unusable brain", () => {
  assert.equal(brainIsBlocked(ai({ brain: "local" }), []), false);
  assert.equal(brainIsBlocked(ai({ brain: "openrouter", configured: false }), []), true);
  // Unknown ai (still loading / failed) is never asserted as blocked.
  assert.equal(brainIsBlocked(null, []), false);
});

test("try-Prime is hidden until the brain is connected", () => {
  const item = tryPrimeItem(
    state(),
    ai({ brain: "openrouter", configured: false }),
    [],
  );
  assert.equal(item, null);
});

test("try-Prime is the recommended next step when ready with no work yet", () => {
  const item = tryPrimeItem(state({ tasks: 0, runs: 0 }), ai({ brain: "local" }), []);
  assert.ok(item);
  assert.equal(item.status, "link");
  assert.equal(item.linkTo, "/prime");
  assert.match(item.label, /Ask Prime/i);
  assert.match(item.description, /what tools do you have/i);
});

test("try-Prime reads done once any task or run exists", () => {
  const fromTask = tryPrimeItem(state({ tasks: 1 }), ai({ brain: "local" }), []);
  const fromRun = tryPrimeItem(state({ runs: 1 }), ai({ brain: "local" }), []);
  assert.equal(fromTask.status, "done");
  assert.equal(fromRun.status, "done");
});

// ── Paused continuation (resume a paused Prime tool loop) ────────────────────

test("continuationItem is null when nothing is paused", () => {
  assert.equal(continuationItem(null), null);
  assert.equal(continuationItem(undefined), null);
});

test("a paused continuation is attention routed to Work", () => {
  const item = continuationItem(continuation());
  assert.ok(item);
  assert.equal(item.status, "warn");
  assert.equal(item.linkTo, "/work");
  assert.match(item.label, /Resume paused work/i);
  assert.match(item.description, /no work is re-run|already gathered/i);
});

test("a continuation awaiting approval says to approve first", () => {
  const item = continuationItem(continuation({ awaiting_approval: true }));
  assert.match(item.label, /Approve, then resume/i);
  assert.match(item.description, /waiting on your approval/i);
});

// ── Blocked / failed work needs attention ────────────────────────────────────

test("attentionWorkItem is null when no work is stuck", () => {
  assert.equal(attentionWorkItem(state()), null);
  assert.equal(attentionWorkItem(null), null);
});

test("blocked or failed work is attention routed to the Inbox", () => {
  const item = attentionWorkItem(state({ blocked: 2, failed: 1 }));
  assert.ok(item);
  assert.equal(item.status, "warn");
  assert.equal(item.linkTo, "/inbox");
  assert.match(item.description, /2 blocked tasks/);
  assert.match(item.description, /1 failed run/);
});

// ── First-action priority with the new stages ────────────────────────────────

test("first action fixes a broken brain before anything else", () => {
  const fa = deriveFirstAction(
    state({ pending_approvals: 3 }),
    ai({ brain: "openrouter", configured: false }),
    [],
  );
  assert.equal(fa.linkTo, "/health");
  assert.match(fa.label, /brain/i);
});

test("first action surfaces a paused continuation and stuck work in order", () => {
  // Paused work outranks active runs.
  const paused = deriveFirstAction(state({ active_runs: 2 }), ai({ brain: "local" }), [], continuation());
  assert.equal(paused.linkTo, "/work");
  assert.match(paused.label, /paused/i);
  // Stuck work routes to the Inbox.
  const stuck = deriveFirstAction(state({ blocked: 1 }), ai({ brain: "local" }), [], null);
  assert.equal(stuck.linkTo, "/inbox");
});

test("the legacy one-argument deriveFirstAction is unchanged", () => {
  // Existing callers pass only state — the brain/continuation branches must stay
  // dormant so behaviour is byte-for-byte the prior contract.
  assert.equal(deriveFirstAction(state({ pending_approvals: 2 })).linkTo, "/approvals");
  assert.equal(deriveFirstAction(state({ active_runs: 1 })).linkTo, "/work");
  assert.equal(deriveFirstAction(state({ tasks: 5, open_tasks: 2 })).linkTo, "/work");
  assert.equal(deriveFirstAction(state({ tasks: 0 })).linkTo, "/prime");
  assert.equal(deriveFirstAction(null).linkTo, "/prime");
});

// ── Integration: the new stages surface as attention but never block ─────────

test("a paused continuation surfaces as attention without blocking a ready instance", () => {
  const r = buildReadiness({
    state: state({ tasks: 2 }),
    ai: ai({ brain: "local" }),
    adapters: [],
    plugins: [],
    tools: [],
    continuation: continuation(),
  });
  assert.equal(r.ready, true); // warn ≠ blocker
  assert.equal(r.blockers.length, 0);
  assert.ok(r.attention.find((i) => i.id === "paused-continuation"));
  assert.equal(r.firstAction.linkTo, "/work");
});

test("blocked work surfaces as attention and drives the first action to the Inbox", () => {
  const r = buildReadiness({
    state: state({ tasks: 3, blocked: 1 }),
    ai: ai({ brain: "local" }),
    adapters: [],
    plugins: [],
    tools: [],
  });
  assert.equal(r.ready, true);
  assert.ok(r.attention.find((i) => i.id === "work-attention"));
  assert.equal(r.firstAction.linkTo, "/inbox");
});

test("the guided try-Prime step appears once the brain is connected", () => {
  const r = buildReadiness({
    state: state(),
    ai: ai({ brain: "local" }),
    adapters: [],
    plugins: [],
    tools: [],
  });
  const tp = r.items.find((i) => i.id === "try-prime");
  assert.ok(tp, "try-Prime is part of the guided journey");
  assert.equal(tp.status, "link");
});
