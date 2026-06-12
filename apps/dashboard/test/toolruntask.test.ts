import { test } from "node:test";
import assert from "node:assert/strict";
import {
  buildToolRunTaskPayload,
  MAX_TOOL_RUN_STEPS,
  type ToolRunStep,
} from "../src/toolruntask.ts";

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
