import { test } from "node:test";
import assert from "node:assert/strict";
import {
  pluginCategory,
  pluginKindLabel,
  pluginStatus,
  pluginNextStep,
  installResultSummary,
  visibleTools,
  isRunnableTool,
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

test("a metadata-only wrapper is NEVER shown as ready/enabled", () => {
  const s = pluginStatus(plugin({ generated: true, enabled: true }));
  assert.equal(s.label, "Needs configuration");
  assert.equal(s.variant, "warn");
  assert.notEqual(s.variant, "ok");
});

test("a real enabled toolset shows enabled; a disabled one shows disabled", () => {
  assert.equal(pluginStatus(plugin({ enabled: true })).label, "enabled");
  assert.equal(pluginStatus(plugin({ enabled: true })).variant, "ok");
  assert.equal(pluginStatus(plugin({ enabled: false })).label, "disabled");
  assert.equal(pluginStatus(plugin({ enabled: false })).variant, "muted");
});

test("a wrapper's next step is add-manifest, and it explains the runtime dead-end", () => {
  const step = pluginNextStep(plugin({ generated: true }));
  assert.equal(step.kind, "add-manifest");
  assert.equal(step.cta, "Set up");
  assert.match(step.detail, /declares no tools/i);
  assert.match(step.detail, /relux-plugin\.json/);
});

test("a real non-bundled toolset keeps the runtime call-to-action", () => {
  const step = pluginNextStep(plugin({ kind: "ToolSet", tool_count: 3 }));
  assert.equal(step.kind, "configure-runtime");
  assert.equal(step.cta, "Runtime");
  assert.match(step.detail, /3 tools/);
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
