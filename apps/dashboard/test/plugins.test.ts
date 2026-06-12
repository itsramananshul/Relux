import { test } from "node:test";
import assert from "node:assert/strict";
import {
  pluginCategory,
  pluginKindLabel,
  pluginStatus,
  pluginNextStep,
  canConfigureTools,
  installResultSummary,
  visibleTools,
  isRunnableTool,
  toolReadiness,
  adapterStatusBadge,
  ADAPTER_STATE_LABEL,
  mcpServerStatusBadge,
} from "../src/plugins.ts";

// The Plugins page must read HONESTLY: a generated metadata-only wrapper is never
// shown as ready/enabled, and its next step is "add a manifest" (it has no tools),
// not "configure a runtime". A real ToolSet keeps the runtime call-to-action. The
// Tools list shows only runnable tools by default. These assertions pin that so a
// regression (a wrapper badged "enabled", a misleading runtime CTA) fails loudly.

// Minimal builders shaped like the real API types (runtime-only; not type-checked).
function plugin(over = {}) {
  return {
    id: "relux-tools-demo",
    name: "Demo",
    description: "",
    kind: "ToolSet",
    version: "0.1.0",
    enabled: true,
    source_kind: "Github",
    source_label: "https://github.com/owner/demo",
    install_dir: "/data/demo",
    protected: false,
    bundled: false,
    generated: false,
    tool_count: 1,
    ...over,
  };
}

function tool(over = {}) {
  return {
    plugin_id: "relux-tools-demo",
    tool_name: "demo.run",
    description: "",
    permission: "tool:relux-tools-demo:run",
    risk: "low",
    source_kind: "github",
    installed: true,
    enabled: true,
    protected: false,
    executable: "ready",
    ...over,
  };
}

function mcpServer(over = {}) {
  return {
    id: "fs-helper",
    transport: "http_loopback",
    endpoint: "http://127.0.0.1:8000/mcp",
    description: "local fs",
    enabled: true,
    timeout_ms: 10000,
    status: "configured",
    ...over,
  };
}

test("an MCP server badge reflects only its config (configured vs disabled), never faked ready", () => {
  const on = mcpServerStatusBadge(mcpServer({ enabled: true }));
  assert.equal(on.label, "configured");
  assert.equal(on.variant, "ok");
  const off = mcpServerStatusBadge(mcpServer({ enabled: false }));
  assert.equal(off.label, "disabled");
  assert.equal(off.variant, "muted");
});

// A discovered MCP tool flows through the SAME `toolReadiness` classifier a plugin
// tool uses, keyed off the kernel's `executable`. An unclassified MCP tool is
// `needs_approval` (gated, but can request a per-call approval); a classified
// low-risk + auto-approve tool is `ready` (directly invocable). This pins the
// invoke-surface integration so a regression (a gated MCP tool shown runnable, or
// a ready one shown as "not callable") fails loudly.
function mcpTool(over = {}) {
  return tool({
    plugin_id: "mcp:fs-helper",
    tool_name: "search",
    permission: "tool:mcp-fs-helper:search",
    source_kind: "Mcp",
    risk: "medium",
    executable: "needs_approval",
    ...over,
  });
}

test("an unclassified MCP tool is gated (needs approval), never directly runnable", () => {
  const r = toolReadiness(mcpTool());
  assert.equal(r.runnable, false);
  assert.equal(r.canRequestApproval, true);
  assert.equal(r.label, "needs approval");
  assert.equal(isRunnableTool(mcpTool()), false);
});

test("a classified low-risk auto-approve MCP tool is ready to invoke directly", () => {
  const r = toolReadiness(mcpTool({ executable: "ready", risk: "low" }));
  assert.equal(r.runnable, true);
  assert.equal(r.canRequestApproval, false);
  assert.equal(r.label, "ready");
  assert.equal(isRunnableTool(mcpTool({ executable: "ready" })), true);
});

test("a generated wrapper is categorized as a wrapper regardless of kind", () => {
  assert.equal(pluginCategory(plugin({ generated: true })), "wrapper");
  assert.equal(pluginCategory(plugin({ generated: true, kind: "ToolSet" })), "wrapper");
  assert.equal(pluginKindLabel(plugin({ generated: true })), "Metadata-only wrapper");
});

test("real adapters and toolsets keep their honest kind labels", () => {
  assert.equal(pluginCategory(plugin({ kind: "Adapter" })), "adapter");
  assert.equal(pluginKindLabel(plugin({ kind: "Adapter" })), "Adapter");
  assert.equal(pluginCategory(plugin({ kind: "ToolSet" })), "toolset");
  assert.equal(pluginKindLabel(plugin({ kind: "ToolSet" })), "ToolSet");
});

test("a metadata-only wrapper with NO tools is NEVER shown as ready/enabled", () => {
  const s = pluginStatus(plugin({ generated: true, enabled: true, tool_count: 0 }));
  assert.equal(s.label, "Needs configuration");
  assert.equal(s.variant, "warn");
  assert.notEqual(s.variant, "ok");
});

test("a wrapper the operator has added tools to behaves like a toolset", () => {
  // Once tools exist (tool_count > 0), the wrapper is no longer a dead-end: it
  // shows the plain enabled/disabled status like any ToolSet.
  const s = pluginStatus(plugin({ generated: true, enabled: true, tool_count: 1 }));
  assert.equal(s.label, "enabled");
  assert.equal(s.variant, "ok");
});

test("a real enabled toolset shows enabled; a disabled one shows disabled", () => {
  assert.equal(pluginStatus(plugin({ enabled: true })).label, "enabled");
  assert.equal(pluginStatus(plugin({ enabled: true })).variant, "ok");
  assert.equal(pluginStatus(plugin({ enabled: false })).label, "disabled");
  assert.equal(pluginStatus(plugin({ enabled: false })).variant, "muted");
});

test("a 0-tool wrapper's next step is add-manifest, and it explains the dead-end", () => {
  const step = pluginNextStep(plugin({ generated: true, tool_count: 0 }));
  assert.equal(step.kind, "add-manifest");
  assert.equal(step.cta, "Configure");
  assert.match(step.detail, /declares no tools/i);
  assert.match(step.detail, /tool definition/i);
});

test("a wrapper with tools added now points at the loopback runtime step", () => {
  const step = pluginNextStep(plugin({ generated: true, tool_count: 2 }));
  assert.equal(step.kind, "configure-runtime");
  assert.equal(step.cta, "Runtime");
  assert.match(step.detail, /2 tools/);
});

test("a real non-bundled toolset keeps the runtime call-to-action", () => {
  const step = pluginNextStep(plugin({ kind: "ToolSet", tool_count: 3 }));
  assert.equal(step.kind, "configure-runtime");
  assert.equal(step.cta, "Runtime");
  assert.match(step.detail, /3 tools/);
});

test("canConfigureTools: wrappers + non-bundled toolsets yes; bundled + adapters no", () => {
  assert.equal(canConfigureTools(plugin({ generated: true, tool_count: 0 })), true);
  assert.equal(canConfigureTools(plugin({ kind: "ToolSet" })), true);
  // Bundled/protected fixtures and adapters are refused (kernel rejects them too).
  assert.equal(canConfigureTools(plugin({ protected: true, bundled: true })), false);
  assert.equal(canConfigureTools(plugin({ kind: "Adapter" })), false);
});

test("a bundled toolset has no next step (built-in, runnable)", () => {
  const step = pluginNextStep(plugin({ protected: true, bundled: true }));
  assert.equal(step.kind, "none");
});

test("a non-bundled adapter points to the Crew page, not loopback runtime", () => {
  const step = pluginNextStep(plugin({ kind: "Adapter", protected: false }));
  assert.equal(step.kind, "configure-adapter");
  assert.match(step.detail, /Crew/);
});

test("a bundled (protected) adapter is configurable, NOT a dead-end locked row", () => {
  // The shipped Claude/Codex CLIs are protected (can't be removed) but must still
  // expose a real Configure path — the mission's "mysterious protected rows with no
  // path to use them" is the exact regression this pins.
  const step = pluginNextStep(plugin({ kind: "Adapter", protected: true }));
  assert.equal(step.kind, "configure-adapter");
  assert.notEqual(step.kind, "none");
  assert.match(step.detail, /Crew/);
  assert.match(step.detail, /Prime's brain|Settings/);
});

test("tools list shows only runnable tools by default, with a hidden count", () => {
  const tools = [
    tool({ tool_name: "a", executable: "ready" }),
    tool({ tool_name: "b", executable: "runtime_not_configured" }),
    tool({ tool_name: "c", executable: "not_implemented" }),
  ];
  const def = visibleTools(tools, false);
  assert.equal(def.shown.length, 1);
  assert.equal(def.shown[0].tool_name, "a");
  assert.equal(def.hiddenCount, 2);

  const all = visibleTools(tools, true);
  assert.equal(all.shown.length, 3);
  assert.equal(all.hiddenCount, 0);
});

test("install summary for a wrapper is honest: nothing runnable yet, add tools", () => {
  const s = installResultSummary(plugin({ generated: true, tool_count: 0 }));
  assert.equal(s.tone, "info");
  assert.match(s.headline, /metadata-only wrapper/i);
  assert.match(s.detail, /no tools|nothing is runnable/i);
});

test("install summary for a real toolset reports discovered tool count + runtime step", () => {
  const s = installResultSummary(plugin({ kind: "ToolSet", tool_count: 2 }));
  assert.equal(s.tone, "ok");
  assert.match(s.headline, /discovered 2 tools/);
  assert.match(s.detail, /loopback/i);
});

test("install summary for an adapter points at the Crew page", () => {
  const s = installResultSummary(plugin({ kind: "Adapter", tool_count: 0 }));
  assert.equal(s.tone, "ok");
  assert.match(s.headline, /adapter/i);
  assert.match(s.detail, /Crew/);
});

test("isRunnableTool is true only for ready tools", () => {
  assert.equal(isRunnableTool(tool({ executable: "ready" })), true);
  assert.equal(isRunnableTool(tool({ executable: "runtime_disabled" })), false);
  assert.equal(isRunnableTool(tool({ executable: "missing_permission" })), false);
});

// ── Tool readiness (the honest invocation surface) ────────────────────────────
// The single classifier that drives the Tools list: every non-ready state is a
// CLEAR refusal/disabled reason (never a blank "not callable"), and ONLY "ready"
// is runnable — matching what the kernel enforces in call_tool/invoke_tool.

test("a ready tool is runnable, ok-toned, and offers no next step", () => {
  const r = toolReadiness(tool({ executable: "ready" }));
  assert.equal(r.runnable, true);
  assert.equal(r.tone, "ok");
  assert.equal(r.label, "ready");
  assert.equal(r.nextStep, undefined);
});

test("a needs_approval tool is NOT runnable but CAN request a per-call approval", () => {
  // The exact regression the mission pins: a higher-risk configured tool must be
  // refused on the direct invoke path and SAY so — never blank, never pretend-run.
  // It is NOT directly runnable, but the per-call approval flow IS now available.
  const r = toolReadiness(tool({ executable: "needs_approval", risk: "high" }));
  assert.equal(r.runnable, false);
  assert.equal(r.canRequestApproval, true);
  assert.equal(r.label, "needs approval");
  assert.match(r.reason, /requires approval|refused/i);
  assert.match(r.reason, /high-risk/);
  // The next step points at the per-call approval flow (and still notes the
  // lower-risk alternative).
  assert.match(r.nextStep ?? "", /request a per-call approval/i);
  assert.match(r.nextStep ?? "", /lower its risk/i);
});

test("only needs_approval allows requesting a per-call approval", () => {
  // Every other state — runnable or not — must NOT offer the request-approval
  // form, so the gate is never bypassed and a ready tool just runs.
  for (const executable of [
    "ready",
    "runtime_not_configured",
    "runtime_disabled",
    "missing_permission",
    "not_implemented",
  ]) {
    const r = toolReadiness(tool({ executable }));
    assert.equal(r.canRequestApproval, false, `${executable} must not request approval`);
  }
});

test("a runtime_not_configured tool points at configuring a loopback runtime", () => {
  const r = toolReadiness(tool({ executable: "runtime_not_configured" }));
  assert.equal(r.runnable, false);
  assert.match(r.reason, /no runtime|never auto-runs/i);
  assert.match(r.nextStep ?? "", /loopback|runtime/i);
});

test("a runtime_disabled tool points at re-enabling the runtime", () => {
  const r = toolReadiness(tool({ executable: "runtime_disabled" }));
  assert.equal(r.runnable, false);
  assert.match(r.reason, /disabled/i);
  assert.match(r.nextStep ?? "", /re-enable/i);
});

test("a missing_permission tool names the missing permission and the fix", () => {
  const r = toolReadiness(
    tool({ executable: "missing_permission", permission: "tool:relux-tools-demo:run" }),
  );
  assert.equal(r.runnable, false);
  assert.match(r.reason, /tool:relux-tools-demo:run/);
  assert.match(r.nextStep ?? "", /grant/i);
});

test("a not_implemented tool is honestly muted with no next step", () => {
  const r = toolReadiness(tool({ executable: "not_implemented" }));
  assert.equal(r.runnable, false);
  assert.equal(r.tone, "muted");
  assert.match(r.reason, /no supported runtime/i);
  assert.equal(r.nextStep, undefined);
});

test("every non-ready readiness state carries a real reason (no blank refusal)", () => {
  for (const executable of [
    "needs_approval",
    "runtime_not_configured",
    "runtime_disabled",
    "missing_permission",
    "not_implemented",
  ]) {
    const r = toolReadiness(tool({ executable }));
    assert.equal(r.runnable, false, `${executable} must not be runnable`);
    assert.ok(r.reason.length > 0, `${executable} must explain itself`);
    assert.ok(r.label.length > 0, `${executable} must have a badge label`);
  }
});

// ── Live adapter runtime badge (Plugins page) ─────────────────────────────────
// An adapter row must surface its LIVE runtime state, not the static plugin
// record. These pin the label vocabulary (shared with Crew) and the honest
// fail-closed behavior: an unresolved probe is NEVER shown as ready/available.

function adapterRuntime(over = {}) {
  return {
    plugin_id: "relux-adapter-claude-cli",
    adapter_name: "Claude CLI",
    kind: "claude",
    configured: true,
    enabled: true,
    command: "claude",
    available_on_path: true,
    resolved_path: "/usr/local/bin/claude",
    timeout_seconds: 600,
    max_output_bytes: 1048576,
    working_dir: null,
    state: "available",
    detail: "Enabled. Relux will run 'claude' for assigned tasks.",
    ...over,
  };
}

test("an enabled, on-PATH adapter reads as available (ok), with the live detail", () => {
  const b = adapterStatusBadge(adapterRuntime({ state: "available" }));
  assert.equal(b.label, "Enabled — ready");
  assert.equal(b.variant, "ok");
  assert.match(b.title, /Relux will run/);
});

test("the local deterministic adapter reads as available (ok)", () => {
  const b = adapterStatusBadge(adapterRuntime({ state: "local_deterministic" }));
  assert.equal(b.label, "Local (deterministic)");
  assert.equal(b.variant, "ok");
});

test("an enabled adapter whose binary is missing reads as warn, not ok", () => {
  const b = adapterStatusBadge(adapterRuntime({ state: "missing_binary" }));
  assert.equal(b.label, "Enabled — binary missing");
  assert.equal(b.variant, "warn");
  assert.notEqual(b.variant, "ok");
});

test("a default/unconfigured CLI adapter reads as needs-configuration (warn)", () => {
  const b = adapterStatusBadge(adapterRuntime({ state: "needs_configuration" }));
  assert.equal(b.label, "Disabled (default)");
  assert.equal(b.variant, "warn");
});

test("a deliberately disabled adapter reads as muted, never ready", () => {
  const b = adapterStatusBadge(adapterRuntime({ state: "disabled" }));
  assert.equal(b.label, "Configured — disabled");
  assert.equal(b.variant, "muted");
  assert.notEqual(b.variant, "ok");
});

test("an unresolved adapter probe is honest 'status unavailable', NOT ready", () => {
  // undefined = the /v1/relux/adapters probe failed or no row matched this plugin.
  const b = adapterStatusBadge(undefined);
  assert.equal(b.label, "status unavailable");
  assert.equal(b.variant, "muted");
  assert.notEqual(b.variant, "ok");
  assert.match(b.title, /could not read/i);
});

test("every adapter state has a shared label (single source of truth with Crew)", () => {
  for (const state of [
    "local_deterministic",
    "available",
    "missing_binary",
    "disabled",
    "needs_configuration",
  ]) {
    assert.ok(ADAPTER_STATE_LABEL[state], `missing label for ${state}`);
  }
});
