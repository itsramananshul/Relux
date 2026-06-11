// Unit tests for the pure crew role-preset helpers (apps/dashboard/src/presets.ts).
// Pure module → runs under node --strip-types (the .ts test path), no React/DOM.
//
// Run: `npm test` (auto-discovered) or
//   node --test --experimental-strip-types test/presets.test.ts

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  applyPreset,
  presetFieldsDirty,
  type AgentPreset,
} from "../src/presets.ts";

const RESEARCHER: AgentPreset = {
  id: "researcher",
  label: "Researcher",
  summary: "Investigates questions and gathers cited sources.",
  role: "Investigates questions and gathers sources.",
  persona: "Methodical and thorough; cites sources.",
  skills: ["research", "analysis", "writing"],
};

test("applyPreset fills role/persona and renders skills as comma-separated text", () => {
  const fields = applyPreset(RESEARCHER);
  assert.equal(fields.role, "Investigates questions and gathers sources.");
  assert.equal(fields.persona, "Methodical and thorough; cites sources.");
  assert.equal(fields.skills, "research, analysis, writing");
});

test("applyPreset tolerates a missing skills array", () => {
  const fields = applyPreset({ ...RESEARCHER, skills: undefined as unknown as string[] });
  assert.equal(fields.skills, "");
});

test("applyPreset touches ONLY role/persona/skills (no other field key)", () => {
  // Safety: a preset can never reach name/id/adapter/status/permissions — the helper
  // returns exactly the three editable fields and nothing else.
  const fields = applyPreset(RESEARCHER);
  assert.deepEqual(Object.keys(fields).sort(), ["persona", "role", "skills"]);
});

test("presetFieldsDirty is false for empty/whitespace-only fields", () => {
  assert.equal(presetFieldsDirty({ role: "", persona: "", skills: "" }), false);
  assert.equal(presetFieldsDirty({ role: "  ", persona: "\n", skills: " " }), false);
});

test("presetFieldsDirty is true when any preset-managed field has content", () => {
  assert.equal(presetFieldsDirty({ role: "x", persona: "", skills: "" }), true);
  assert.equal(presetFieldsDirty({ role: "", persona: "y", skills: "" }), true);
  assert.equal(presetFieldsDirty({ role: "", persona: "", skills: "rust" }), true);
});
