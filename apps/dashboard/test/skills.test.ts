// Unit tests for the pure crew skills/tags parsing helpers (apps/dashboard/src/skills.ts).
// Pure module → runs under node --strip-types (the .ts test path), no React/DOM.
//
// Run: `npm test` (auto-discovered) or
//   node --test --experimental-strip-types test/skills.test.ts

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  slugifySkill,
  parseSkillsInput,
  formatSkillsInput,
  MAX_SKILL_CHARS,
  MAX_SKILLS,
} from "../src/skills.ts";

test("slugifySkill reduces a token to the backend slug shape", () => {
  assert.equal(slugifySkill("Rust"), "rust");
  assert.equal(slugifySkill("  back end "), "back-end");
  assert.equal(slugifySkill("Data_Science!!"), "data-science");
  // Nothing valid → empty (the caller drops it).
  assert.equal(slugifySkill("💥🔥"), "");
  // Over-long is clamped, not rejected.
  assert.equal(slugifySkill("a".repeat(100)).length, MAX_SKILL_CHARS);
});

test("parseSkillsInput splits, slugifies, dedups, and bounds", () => {
  assert.deepEqual(parseSkillsInput("Rust, back end, rust"), ["rust", "back-end"]);
  // Empty fragments (trailing comma) are dropped.
  assert.deepEqual(parseSkillsInput("research, , ,frontend"), ["research", "frontend"]);
  // Newlines also separate.
  assert.deepEqual(parseSkillsInput("a\nb"), ["a", "b"]);
  // Capped at MAX_SKILLS.
  const many = Array.from({ length: MAX_SKILLS + 5 }, (_, i) => `s${i}`).join(",");
  assert.equal(parseSkillsInput(many).length, MAX_SKILLS);
  // Empty input → empty list.
  assert.deepEqual(parseSkillsInput("   "), []);
});

test("formatSkillsInput renders the comma-separated edit form", () => {
  assert.equal(formatSkillsInput(["rust", "frontend"]), "rust, frontend");
  assert.equal(formatSkillsInput([]), "");
  assert.equal(formatSkillsInput(undefined), "");
});

test("round-trip: format then parse is stable", () => {
  const skills = ["research", "rust", "data-science"];
  assert.deepEqual(parseSkillsInput(formatSkillsInput(skills)), skills);
});
