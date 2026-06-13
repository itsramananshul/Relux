// Unit tests for the pure tool-glue editor helpers (RELUX_MASTER_PLAN §23, the
// `execute_code` foundation). `parseGlueSteps` must fail closed on every malformed shape
// BEFORE the UI POSTs to `/v1/relux/prime/glue/preview`, and normalize a valid program to
// the wire steps; `appendAbilityStep` must be non-destructive (never clobber the operator's
// hand-edited JSON). These pin both, React-free, the same way `toolruntask.test.ts` pins the
// tool-run builder. Run: `npm test` or `node --test test/toolglue.test.ts`.

import { test } from "node:test";
import assert from "node:assert/strict";
import { appendAbilityStep, EMPTY_GLUE_STEPS_TEXT, parseGlueSteps } from "../src/toolglue.ts";

test("a valid JSON array of steps parses to normalized wire steps", () => {
  const r = parseGlueSteps('[{ "plugin": "acme", "tool": "build", "args": { "x": 1 } }]');
  assert.ok(r.ok);
  assert.deepEqual(r.steps, [{ plugin: "acme", tool: "build", args: { x: 1 } }]);
});

test("omitted/null args normalize to {} (the kernel default)", () => {
  const omitted = parseGlueSteps('[{ "plugin": "acme", "tool": "build" }]');
  assert.ok(omitted.ok);
  assert.deepEqual(omitted.steps[0].args, {});
  const nulled = parseGlueSteps('[{ "plugin": "acme", "tool": "build", "args": null }]');
  assert.ok(nulled.ok);
  assert.deepEqual(nulled.steps[0].args, {});
});

test("plugin and tool are trimmed", () => {
  const r = parseGlueSteps('[{ "plugin": "  mcp:fs  ", "tool": "  search  " }]');
  assert.ok(r.ok);
  assert.equal(r.steps[0].plugin, "mcp:fs");
  assert.equal(r.steps[0].tool, "search");
});

test("a blank editor is an honest 'add a step' error, not an empty POST", () => {
  for (const t of ["", "   ", EMPTY_GLUE_STEPS_TEXT]) {
    const r = parseGlueSteps(t);
    assert.ok(!r.ok, `expected blank/empty "${t}" to fail`);
    assert.match(r.error, /at least one step/i);
  }
});

test("non-JSON, non-array, and non-object steps fail closed with a clear reason", () => {
  const bad = parseGlueSteps("not json");
  assert.ok(!bad.ok);
  assert.match(bad.error, /valid JSON/i);

  const obj = parseGlueSteps('{ "plugin": "acme", "tool": "build" }');
  assert.ok(!obj.ok);
  assert.match(obj.error, /array/i);

  const scalarStep = parseGlueSteps('["acme/build"]');
  assert.ok(!scalarStep.ok);
  assert.match(scalarStep.error, /Step 1.*object/i);
});

test("a step missing plugin or tool names the 1-based step", () => {
  const noTool = parseGlueSteps('[{ "plugin": "acme", "tool": "build" }, { "plugin": "acme" }]');
  assert.ok(!noTool.ok);
  assert.match(noTool.error, /Step 2/);
  assert.match(noTool.error, /plugin and tool/i);
});

test("appendAbilityStep seeds a single-step array from a blank editor", () => {
  const out = appendAbilityStep(EMPTY_GLUE_STEPS_TEXT, "acme", "build");
  const parsed = parseGlueSteps(out);
  assert.ok(parsed.ok);
  assert.deepEqual(parsed.steps, [{ plugin: "acme", tool: "build", args: {} }]);
});

test("appendAbilityStep appends to an existing valid array", () => {
  const first = appendAbilityStep(EMPTY_GLUE_STEPS_TEXT, "acme", "build");
  const second = appendAbilityStep(first, "mcp:fs", "search");
  const parsed = parseGlueSteps(second);
  assert.ok(parsed.ok);
  assert.equal(parsed.steps.length, 2);
  assert.deepEqual(parsed.steps[1], { plugin: "mcp:fs", tool: "search", args: {} });
});

test("appendAbilityStep is non-destructive on hand-edited invalid JSON", () => {
  const broken = '[{ "plugin": "acme", "tool": ';
  assert.equal(appendAbilityStep(broken, "acme", "build"), broken);
  const nonArray = '{ "plugin": "acme" }';
  assert.equal(appendAbilityStep(nonArray, "acme", "build"), nonArray);
});
