// Unit tests for the inline approval action model (src/approvalactions.ts).
//
// Pure logic only — no React, no DOM — so it runs under `node --test
// --experimental-strip-types` without the esbuild render harness. The component
// wiring (which routes each action calls) is covered by
// oversight-approvals-render.test.mjs; this pins WHICH decisions are offered for
// each approval shape, which is the safety-critical part: the strip must never
// offer a decision the backend would reject for that record.

import { test } from "node:test";
import assert from "node:assert/strict";

import { approvalInlineActions } from "../src/approvalactions.ts";

const base = {
  id: "appr_1",
  requested_by: "prime",
  action: "delete the staging bucket",
  reason: "high-risk write",
  risk: "high",
  status: "pending",
  created_at: "2026-06-12T00:00:00Z",
} as never;

const withTi = {
  ...base,
  tool_invocation: {
    plugin_id: "relux-tools-github",
    tool_name: "delete_repo",
    agent_id: "agent_7",
    permission: "tool:relux-tools-github:delete_repo",
    risk: "high",
    args_preview: "{ repo: … }",
    args_sha256: "abc123",
    consumed: false,
    executable: false,
  },
} as never;

test("a per-call tool invocation gets the full inline set (approve&run / allow-always / deny)", () => {
  const a = approvalInlineActions(withTi);
  assert.equal(a.actionable, true);
  assert.deepEqual(a.approve, { kind: "approve_run", label: "Approve & run" });
  assert.equal(a.allowAlways, true);
  assert.equal(a.deny, true);
});

test("a generic approval gets approve + deny only (no run, no allow-always)", () => {
  const a = approvalInlineActions(base);
  assert.equal(a.actionable, true);
  assert.deepEqual(a.approve, { kind: "approve", label: "Approve" });
  // Allow-always 404s for a generic approval — it must NOT be offered.
  assert.equal(a.allowAlways, false);
  assert.equal(a.deny, true);
  // The honest caveat is surfaced so the operator knows nothing executes here.
  assert.match(a.reason, /nothing runs here/i);
});

test("a non-pending approval degrades to details-only with an honest reason", () => {
  const approved = approvalInlineActions({ ...base, status: "approved" } as never);
  assert.equal(approved.actionable, false);
  assert.equal(approved.approve, null);
  assert.equal(approved.allowAlways, false);
  assert.equal(approved.deny, false);
  assert.match(approved.reason, /already approved/i);

  const rejected = approvalInlineActions({ ...base, status: "rejected" } as never);
  assert.equal(rejected.actionable, false);
  assert.match(rejected.reason, /already rejected/i);
});
