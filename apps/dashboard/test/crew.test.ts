import { test } from "node:test";
import assert from "node:assert/strict";
import { isPrimeOnlyRoster, PRIME_AGENT_ID } from "../src/crew.ts";

// The roster-state derivation behind the Crew page's actionable empty state. Prime is
// always seeded, so a literally-empty list is the loading/error path, NOT "no crew yet";
// "only Prime" is the real signal to prompt the operator to build a crew. The test is
// not type-checked; the records only carry the `id` the helper reads.

test("PRIME_AGENT_ID matches the seeded control-plane operative id", () => {
  assert.equal(PRIME_AGENT_ID, "prime");
});

test("a roster of just Prime is the actionable empty state", () => {
  assert.equal(isPrimeOnlyRoster([{ id: "prime" }] as never), true);
});

test("a roster with any non-Prime operative is NOT prime-only", () => {
  assert.equal(isPrimeOnlyRoster([{ id: "prime" }, { id: "researcher" }] as never), false);
  assert.equal(isPrimeOnlyRoster([{ id: "researcher" }] as never), false);
});

test("an empty roster is not prime-only (that is the loading/error path, handled separately)", () => {
  // No Prime at all means the control plane was not reachable / not yet seeded; the page
  // shows loading or error there, never the 'build your crew' nudge.
  assert.equal(isPrimeOnlyRoster([]), false);
});
