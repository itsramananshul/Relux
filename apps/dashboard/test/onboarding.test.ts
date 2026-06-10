import { test } from "node:test";
import assert from "node:assert/strict";
import {
  primeBrainStep,
  anyCliOnPath,
  CLAUDE_ADAPTER_ID,
  CODEX_ADAPTER_ID,
} from "../src/onboarding.ts";

// The first-run "connect Prime to a brain" step is the heart of Relux onboarding:
// given the live AI status + adapter list, it must report an HONEST readiness and,
// when not ready, the EXACT next step — always routed to /health (Prime Brain),
// never the legacy Crew path. These assertions pin that behavior so a regression
// (a misrouted link, a "done" shown for an unusable brain) fails loudly.

// Minimal builders shaped like the real API types (the test is not type-checked;
// it exercises runtime behavior only).
function aiStatus(over = {}) {
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

test("every brain step routes to /health (Prime Brain), never /crew", () => {
  const cases = [
    primeBrainStep(null, null),
    primeBrainStep(aiStatus(), null),
    primeBrainStep(aiStatus({ brain: "claude_cli" }), null),
    primeBrainStep(aiStatus({ brain: "openrouter" }), null),
  ];
  for (const step of cases) {
    assert.equal(step.linkTo, "/health", `${step.label} must link to /health`);
    assert.equal(step.id, "prime-brain");
  }
});

test("unreachable AI status still yields an actionable link (never hidden)", () => {
  const step = primeBrainStep(null, null);
  assert.equal(step.status, "link");
  assert.match(step.description, /Prime Brain/);
});

test("local brain with no CLI on PATH guides the user to install first", () => {
  const step = primeBrainStep(aiStatus({ brain: "local" }), [
    adapter(CLAUDE_ADAPTER_ID),
    adapter(CODEX_ADAPTER_ID),
  ]);
  assert.equal(step.status, "link");
  assert.match(step.description, /install the Claude or/i);
});

test("local brain with a CLI already on PATH says 'detected' and one click away", () => {
  const step = primeBrainStep(aiStatus({ brain: "local" }), [
    adapter(CLAUDE_ADAPTER_ID, { available_on_path: true, state: "disabled" }),
  ]);
  assert.equal(step.status, "link");
  assert.match(step.description, /already on your PATH/i);
});

test("claude_cli brain that is available is DONE", () => {
  const step = primeBrainStep(aiStatus({ brain: "claude_cli", mode: "claude_cli" }), [
    adapter(CLAUDE_ADAPTER_ID, { available_on_path: true, enabled: true, configured: true, state: "available" }),
  ]);
  assert.equal(step.status, "done");
  assert.match(step.label, /Claude CLI/);
  assert.match(step.description, /answering through your local Claude CLI/);
});

test("claude_cli brain not on PATH is TODO with the PATH next step", () => {
  const step = primeBrainStep(aiStatus({ brain: "claude_cli" }), [
    adapter(CLAUDE_ADAPTER_ID, { state: "missing_binary", available_on_path: false }),
  ]);
  assert.equal(step.status, "todo");
  assert.match(step.description, /not on your PATH/);
});

test("claude_cli brain on PATH but disabled is TODO with the enable next step", () => {
  const step = primeBrainStep(aiStatus({ brain: "claude_cli" }), [
    adapter(CLAUDE_ADAPTER_ID, { state: "disabled", available_on_path: true, enabled: false }),
  ]);
  assert.equal(step.status, "todo");
  assert.match(step.description, /Use Claude CLI for Prime/);
});

test("codex_cli brain with no adapter installed is TODO and says so", () => {
  const step = primeBrainStep(aiStatus({ brain: "codex_cli" }), []);
  assert.equal(step.status, "todo");
  assert.match(step.label, /Codex CLI/);
  assert.match(step.description, /not installed/);
});

test("openrouter configured and enabled is DONE with the model", () => {
  const step = primeBrainStep(
    aiStatus({ brain: "openrouter", configured: true, disabled: false, model: "anthropic/claude-3.5" }),
    null,
  );
  assert.equal(step.status, "done");
  assert.match(step.description, /anthropic\/claude-3\.5/);
});

test("openrouter selected without a key is TODO", () => {
  const step = primeBrainStep(aiStatus({ brain: "openrouter", configured: false }), null);
  assert.equal(step.status, "todo");
  assert.match(step.description, /no API key/);
});

test("openrouter configured but disabled is TODO (re-enable)", () => {
  const step = primeBrainStep(aiStatus({ brain: "openrouter", configured: true, disabled: true }), null);
  assert.equal(step.status, "todo");
  assert.match(step.description, /disabled/);
});

test("anyCliOnPath detects either CLI and ignores off-PATH adapters", () => {
  assert.equal(anyCliOnPath(null), false);
  assert.equal(anyCliOnPath([adapter(CLAUDE_ADAPTER_ID, { available_on_path: false })]), false);
  assert.equal(anyCliOnPath([adapter(CODEX_ADAPTER_ID, { available_on_path: true })]), true);
});
