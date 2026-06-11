import { test } from "node:test";
import assert from "node:assert/strict";
import {
  buildReadiness,
  adapterWorkItem,
  crewItem,
  pluginToolItem,
  deriveFirstAction,
  brainLabel,
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

test("brainLabel renders each brain honestly", () => {
  assert.equal(brainLabel(null), "unknown");
  assert.equal(brainLabel(ai({ brain: "local" })), "Local (deterministic)");
  assert.equal(brainLabel(ai({ brain: "claude_cli" })), "Claude CLI");
  assert.equal(brainLabel(ai({ brain: "openrouter" })), "OpenRouter");
});
