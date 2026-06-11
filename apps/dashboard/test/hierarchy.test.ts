// Unit tests for the pure crew org-lattice helpers (apps/dashboard/src/hierarchy.ts).
// Pure module → runs under node --strip-types (the .ts test path), no React/DOM.
//
// Run: `npm test` (auto-discovered) or
//   node --test --experimental-strip-types test/hierarchy.test.ts

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  descendantIds,
  managerOptions,
  leadLabel,
  directReportsSummary,
  type HierAgent,
} from "../src/hierarchy.ts";

// director ← lead ← ic ; peer ← director (peer is a sibling of lead under director).
const ROSTER: HierAgent[] = [
  { id: "director", name: "Director" },
  { id: "lead", name: "Lead", reports_to: "director" },
  { id: "ic", name: "IC", reports_to: "lead" },
  { id: "peer", name: "Peer", reports_to: "director" },
];

test("descendantIds collects the whole Branch, excluding the root", () => {
  assert.deepEqual([...descendantIds(ROSTER, "director")].sort(), ["ic", "lead", "peer"]);
  assert.deepEqual([...descendantIds(ROSTER, "lead")].sort(), ["ic"]);
  assert.deepEqual([...descendantIds(ROSTER, "ic")].sort(), []);
});

test("descendantIds stays total under a stray cycle", () => {
  const cyclic: HierAgent[] = [
    { id: "a", name: "A", reports_to: "b" },
    { id: "b", name: "B", reports_to: "a" },
  ];
  // Must not hang; both nodes are each other's descendant here.
  assert.deepEqual([...descendantIds(cyclic, "a")].sort(), ["a", "b"]);
});

test("managerOptions excludes self and its own branch (no obvious cycle)", () => {
  // Editing 'director': lead, ic, peer are all in its branch → only itself excluded too,
  // leaving nobody eligible.
  assert.deepEqual(managerOptions(ROSTER, "director").map((a) => a.id), []);
  // Editing 'lead': exclude lead + ic (its branch); director and peer remain.
  assert.deepEqual(managerOptions(ROSTER, "lead").map((a) => a.id), ["director", "peer"]);
  // Editing 'ic' (a leaf): everyone except ic is eligible.
  assert.deepEqual(managerOptions(ROSTER, "ic").map((a) => a.id), ["director", "lead", "peer"]);
});

test("managerOptions returns the whole roster on create (no self yet)", () => {
  assert.deepEqual(managerOptions(ROSTER, undefined).map((a) => a.id), [
    "director",
    "lead",
    "ic",
    "peer",
  ]);
});

test("leadLabel prefers the resolved name, falls back to id, else none", () => {
  assert.equal(leadLabel("lead", "Lead"), "Lead");
  assert.equal(leadLabel("lead", ""), "lead");
  assert.equal(leadLabel("lead", undefined), "lead");
  assert.equal(leadLabel(undefined, "ignored"), "none");
});

test("directReportsSummary pluralizes the first-level count", () => {
  assert.equal(directReportsSummary(undefined), "none");
  assert.equal(directReportsSummary([]), "none");
  assert.equal(directReportsSummary(["a"]), "1 report");
  assert.equal(directReportsSummary(["a", "b"]), "2 reports");
});
