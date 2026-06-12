import { test } from "node:test";
import assert from "node:assert/strict";
import {
  buildToolPickerOptions,
  buildToolRunTaskPayload,
  MAX_TOOL_RUN_STEPS,
  type McpServerDiscovery,
  type ToolRunStep,
} from "../src/toolruntask.ts";
import { toolReadiness } from "../src/plugins.ts";
import type { ReluxToolDescriptor } from "../src/api.ts";

// The tool-run-task builder must fail closed the SAME way the kernel does
// (`relux_core::TaskToolPlan::validate` + `CreateTaskReq`): one valid step is a
// `tool_call`, two-to-five a `tool_plan`, and every invalid shape (no title, no
// step, >5 steps, an unchosen tool, bad JSON args) is rejected BEFORE any POST so
// the UI never sends a request the backend would 400. These assertions pin that.

function step(over: Partial<ToolRunStep> = {}): ToolRunStep {
  return { plugin: "mcp:fs", tool: "search", argsText: "", ...over };
}

test("one step builds a tool_call (not a tool_plan)", () => {
  const r = buildToolRunTaskPayload("find files", [step({ argsText: '{ "q": "files" }' })]);
  assert.ok(r.ok);
  assert.equal(r.payload.title, "find files");
  assert.deepEqual(r.payload.tool_call, { plugin: "mcp:fs", tool: "search", args: { q: "files" } });
  assert.equal(r.payload.tool_plan, undefined);
});

test("multiple steps build a tool_plan (not a tool_call)", () => {
  const r = buildToolRunTaskPayload("two-step", [
    step({ argsText: '{ "q": "a" }' }),
    step({ argsText: '{ "q": "b" }' }),
  ]);
  assert.ok(r.ok);
  assert.equal(r.payload.tool_call, undefined);
  assert.ok(Array.isArray(r.payload.tool_plan));
  assert.equal(r.payload.tool_plan?.length, 2);
  assert.deepEqual(r.payload.tool_plan?.[0], { plugin: "mcp:fs", tool: "search", args: { q: "a" } });
  assert.deepEqual(r.payload.tool_plan?.[1], { plugin: "mcp:fs", tool: "search", args: { q: "b" } });
});

test("blank args default to {} (canonical empty, matching the kernel default)", () => {
  const r = buildToolRunTaskPayload("t", [step({ argsText: "   " })]);
  assert.ok(r.ok);
  assert.deepEqual(r.payload.tool_call?.args, {});
});

test("title is trimmed and required", () => {
  const blank = buildToolRunTaskPayload("   ", [step()]);
  assert.ok(!blank.ok);
  assert.match(blank.error, /title is required/i);

  const ok = buildToolRunTaskPayload("  hello  ", [step()]);
  assert.ok(ok.ok);
  assert.equal(ok.payload.title, "hello");
});

test("at least one step is required", () => {
  const r = buildToolRunTaskPayload("t", []);
  assert.ok(!r.ok);
  assert.match(r.error, /at least one tool step/i);
});

test("more than MAX_TOOL_RUN_STEPS steps is rejected (never silently truncated)", () => {
  const tooMany: ToolRunStep[] = Array.from({ length: MAX_TOOL_RUN_STEPS + 1 }, () => step());
  const r = buildToolRunTaskPayload("t", tooMany);
  assert.ok(!r.ok);
  assert.match(r.error, new RegExp(`at most ${MAX_TOOL_RUN_STEPS}`));
});

test("exactly MAX_TOOL_RUN_STEPS steps is accepted (boundary)", () => {
  const max: ToolRunStep[] = Array.from({ length: MAX_TOOL_RUN_STEPS }, () => step());
  const r = buildToolRunTaskPayload("t", max);
  assert.ok(r.ok);
  assert.equal(r.payload.tool_plan?.length, MAX_TOOL_RUN_STEPS);
});

test("a step with no tool chosen is rejected, naming the step", () => {
  const r = buildToolRunTaskPayload("t", [step(), step({ plugin: "", tool: "" })]);
  assert.ok(!r.ok);
  assert.match(r.error, /Step 2/);
  assert.match(r.error, /choose a tool/i);
});

test("an empty plugin or tool (whitespace) is rejected", () => {
  const blankPlugin = buildToolRunTaskPayload("t", [step({ plugin: "   " })]);
  assert.ok(!blankPlugin.ok);
  const blankTool = buildToolRunTaskPayload("t", [step({ tool: "   " })]);
  assert.ok(!blankTool.ok);
});

test("invalid JSON args are rejected, naming the step", () => {
  const r = buildToolRunTaskPayload("t", [step(), step({ argsText: "{ not json" })]);
  assert.ok(!r.ok);
  assert.match(r.error, /Step 2/);
  assert.match(r.error, /valid JSON/i);
});

test("plugin and tool are trimmed in the emitted directive", () => {
  const r = buildToolRunTaskPayload("t", [step({ plugin: "  mcp:fs  ", tool: "  search  " })]);
  assert.ok(r.ok);
  assert.deepEqual(r.payload.tool_call, { plugin: "mcp:fs", tool: "search", args: {} });
});

test("args may be a non-object JSON value (the kernel arg is any Value)", () => {
  const r = buildToolRunTaskPayload("t", [step({ argsText: '["a","b"]' })]);
  assert.ok(r.ok);
  assert.deepEqual(r.payload.tool_call?.args, ["a", "b"]);
});

// ── Tool picker (installed plugin tools + live MCP-discovered tools) ──────────
// `buildToolPickerOptions` is what gives the "Create a tool-run task" picker its
// entries. It MUST merge installed plugin tools with the tools a live discovery
// surfaced from each ENABLED MCP server (keyed `mcp:<server>`), label a gated tool
// honestly, never silently drop a failed or disabled server, and produce options
// whose (plugin, tool) round-trip into a directive the kernel routes as an MCP call
// (`plugin_id = "mcp:<server>"`). These pin that wiring.

// Gating mirrors the Tools list exactly: a tool is gated iff `toolReadiness` says it
// is not runnable (only `executable: "ready"` is runnable).
const isGated = (t: ReluxToolDescriptor) => !toolReadiness(t).runnable;

function descriptor(over: Partial<ReluxToolDescriptor> = {}): ReluxToolDescriptor {
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

test("merges installed plugin tools with tools discovered from an enabled MCP server", () => {
  const installed = [descriptor({ plugin_id: "relux-tools-fs", tool_name: "read" })];
  const mcp: McpServerDiscovery[] = [
    {
      serverId: "fs-helper",
      enabled: true,
      tools: [descriptor({ plugin_id: "mcp:fs-helper", tool_name: "search", executable: "ready" })],
    },
  ];
  const model = buildToolPickerOptions(installed, mcp, isGated);

  // Both sources appear, tagged by source.
  const plugin = model.options.find((o) => o.source === "plugin");
  const mcpOpt = model.options.find((o) => o.source === "mcp");
  assert.ok(plugin, "installed plugin tool must be offered");
  assert.ok(mcpOpt, "MCP-discovered tool must be offered");

  // The MCP option uses the stable `mcp:<server>` plugin id and the tool name, and
  // reads as `mcp:<server>/<tool>` so it is unmistakable in the dropdown.
  assert.equal(mcpOpt.plugin, "mcp:fs-helper");
  assert.equal(mcpOpt.tool, "search");
  assert.equal(mcpOpt.key, "mcp:fs-helper search");
  assert.match(mcpOpt.label, /mcp:fs-helper\/search/);

  // No failed / disabled notes for a clean enabled discovery.
  assert.equal(model.failures.length, 0);
  assert.equal(model.disabledServers.length, 0);
});

test("an unclassified (needs_approval) MCP tool is offered, labelled 'needs approval'", () => {
  const mcp: McpServerDiscovery[] = [
    {
      serverId: "fs-helper",
      enabled: true,
      tools: [
        descriptor({ plugin_id: "mcp:fs-helper", tool_name: "write", risk: "medium", executable: "needs_approval" }),
      ],
    },
  ];
  const model = buildToolPickerOptions([], mcp, isGated);
  assert.equal(model.options.length, 1);
  assert.equal(model.options[0].gated, true);
  assert.match(model.options[0].label, /needs approval/);
});

test("a failed MCP discovery is surfaced as a warning, not silently dropped", () => {
  const mcp: McpServerDiscovery[] = [
    { serverId: "down-server", enabled: true, failed: true, error: "502 unreachable" },
  ];
  const model = buildToolPickerOptions([], mcp, isGated);
  // No options from a failed server, but it is named in `failures` (with its reason).
  assert.equal(model.options.length, 0);
  assert.equal(model.failures.length, 1);
  assert.equal(model.failures[0].serverId, "down-server");
  assert.match(model.failures[0].error ?? "", /unreachable/);
  assert.equal(model.disabledServers.length, 0);
});

test("a disabled MCP server is surfaced as info, not failed and not dropped", () => {
  const mcp: McpServerDiscovery[] = [{ serverId: "off-server", enabled: false }];
  const model = buildToolPickerOptions([], mcp, isGated);
  assert.equal(model.options.length, 0);
  assert.equal(model.failures.length, 0);
  assert.deepEqual(model.disabledServers, ["off-server"]);
});

test("an MCP option round-trips into a directive with plugin 'mcp:<server>' and the tool name", () => {
  const mcp: McpServerDiscovery[] = [
    {
      serverId: "fs-helper",
      enabled: true,
      tools: [descriptor({ plugin_id: "mcp:fs-helper", tool_name: "search", executable: "ready" })],
    },
  ];
  const opt = buildToolPickerOptions([], mcp, isGated).options[0];
  // The form turns the chosen option into a step (plugin + tool) and builds the body.
  const built = buildToolRunTaskPayload("search the docs", [
    { plugin: opt.plugin, tool: opt.tool, argsText: '{ "q": "files" }' },
  ]);
  assert.ok(built.ok);
  assert.deepEqual(built.payload.tool_call, {
    plugin: "mcp:fs-helper",
    tool: "search",
    args: { q: "files" },
  });
});

test("the same tool from two sources is listed once (deduped by plugin+tool)", () => {
  const installed = [descriptor({ plugin_id: "mcp:fs-helper", tool_name: "search" })];
  const mcp: McpServerDiscovery[] = [
    {
      serverId: "fs-helper",
      enabled: true,
      tools: [descriptor({ plugin_id: "mcp:fs-helper", tool_name: "search" })],
    },
  ];
  const model = buildToolPickerOptions(installed, mcp, isGated);
  const matches = model.options.filter((o) => o.key === "mcp:fs-helper search");
  assert.equal(matches.length, 1);
});
