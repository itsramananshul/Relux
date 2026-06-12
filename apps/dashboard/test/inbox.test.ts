// Unit tests for the cross-Guild Inbox pure helpers (src/inbox.ts).
//
// Pure logic only — no React, no DOM — so it runs under `node --test
// --experimental-strip-types` without the esbuild render harness. The page wiring
// is covered by inbox-render.test.mjs; this pins the action-kind mapping, the
// nav-target resolution, the grouping order, and the per-kind action gating the
// Inbox depends on.

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  inboxActionSpec,
  inboxActionTarget,
  groupInbox,
  inboxBadgeCount,
  inboxSeverityTone,
  inboxSeverityLabel,
  inboxInvestigationInput,
  type InboxActionMode,
} from "../src/inbox.ts";
import type { ReluxInboxItem, ReluxInboxActionKind } from "../src/api.ts";

const ALL_ACTION_KINDS: ReluxInboxActionKind[] = [
  "open_approval",
  "retry",
  "reopen",
  "reopen_and_run",
  "diagnose",
  "investigate",
  "continue",
  "inspect",
];

function item(over: Partial<ReluxInboxItem>): ReluxInboxItem {
  return {
    id: "x:1",
    kind: "failed_run",
    severity: "warn",
    title: "t",
    summary: "s",
    actions: [],
    link: "/work",
    ...over,
  };
}

test("every action kind has a spec (totality — no blank button)", () => {
  for (const kind of ALL_ACTION_KINDS) {
    const spec = inboxActionSpec(kind);
    assert.equal(spec.kind, kind);
    assert.ok(spec.label.length > 0, `${kind} has a label`);
    assert.ok(spec.hint.length > 0, `${kind} has a hint`);
  }
});

test("action modes: mutations are post, navigations are nav, investigate is seed", () => {
  const expected: Record<ReluxInboxActionKind, InboxActionMode> = {
    open_approval: "nav",
    retry: "post",
    reopen: "post",
    reopen_and_run: "post",
    diagnose: "post",
    investigate: "seed",
    continue: "nav",
    inspect: "nav",
  };
  for (const kind of ALL_ACTION_KINDS) {
    assert.equal(inboxActionSpec(kind).mode, expected[kind], `${kind} mode`);
  }
});

test("inboxActionTarget routes nav/seed actions and is null for post actions", () => {
  const failed = item({ kind: "failed_run", link: "/work", run_id: "run-1", task_id: "tk-1" });
  assert.equal(inboxActionTarget(failed, "investigate"), "/prime");
  assert.equal(inboxActionTarget(failed, "inspect"), "/work");
  assert.equal(inboxActionTarget(item({ kind: "pending_approval" }), "open_approval"), "/approvals");
  assert.equal(inboxActionTarget(item({ kind: "paused_continuation" }), "continue"), "/work");
  // POST actions are route CALLS, not navigations — no target.
  assert.equal(inboxActionTarget(failed, "retry"), null);
  assert.equal(inboxActionTarget(failed, "reopen"), null);
  assert.equal(inboxActionTarget(failed, "diagnose"), null);
});

test("inspect target honors the item's own link", () => {
  assert.equal(inboxActionTarget(item({ link: "/approvals" }), "inspect"), "/approvals");
  // A missing link falls back to the Work board, never an empty href.
  assert.equal(inboxActionTarget(item({ link: "" }), "inspect"), "/work");
});

test("groupInbox produces only non-empty groups in the fixed priority order", () => {
  const items: ReluxInboxItem[] = [
    item({ id: "task:1", kind: "blocked_task" }),
    item({ id: "approval:1", kind: "pending_approval" }),
    item({ id: "run:1", kind: "failed_run" }),
    item({ id: "continuation:1", kind: "paused_continuation" }),
    item({ id: "approval:2", kind: "pending_approval" }),
  ];
  const groups = groupInbox(items);
  assert.deepEqual(
    groups.map((g) => g.kind),
    ["pending_approval", "failed_run", "blocked_task", "paused_continuation"],
  );
  // The approvals group keeps BOTH approvals, in input order.
  assert.deepEqual(groups[0].items.map((i) => i.id), ["approval:1", "approval:2"]);
});

test("groupInbox omits kinds with no items", () => {
  const groups = groupInbox([item({ id: "run:1", kind: "failed_run" })]);
  assert.deepEqual(groups.map((g) => g.kind), ["failed_run"]);
});

test("inboxBadgeCount is the item count, clamped to ≥ 0", () => {
  assert.equal(inboxBadgeCount({ items: [item({}), item({})], truncated: false }), 2);
  assert.equal(inboxBadgeCount({ items: [], truncated: false }), 0);
  assert.equal(inboxBadgeCount(null), 0);
  assert.equal(inboxBadgeCount(undefined), 0);
});

test("severity maps to a restrained B&W tone + an honest label", () => {
  assert.equal(inboxSeverityTone("critical"), "failed");
  assert.equal(inboxSeverityTone("warn"), "blocked");
  assert.equal(inboxSeverityTone("info"), "queued");
  assert.equal(inboxSeverityLabel("critical"), "Critical");
  assert.equal(inboxSeverityLabel("warn"), "Needs attention");
  assert.equal(inboxSeverityLabel("info"), "Info");
});

test("inboxInvestigationInput is built only for investigable kinds", () => {
  const run = inboxInvestigationInput(
    item({ kind: "failed_run", run_id: "run-7", task_id: "tk-7", failure_class: "auth_required" }),
  );
  assert.ok(run, "failed run is investigable");
  assert.equal(run!.subject, "run");
  assert.equal(run!.run?.id, "run-7");
  assert.equal(run!.run?.failureClass, "auth_required");

  const task = inboxInvestigationInput(
    item({ kind: "blocked_task", task_id: "tk-9", title: "Ship it" }),
  );
  assert.ok(task, "blocked task is investigable");
  assert.equal(task!.subject, "task");
  assert.equal(task!.task?.id, "tk-9");

  // Approvals and continuations carry no failure to debug — no seed offered.
  assert.equal(inboxInvestigationInput(item({ kind: "pending_approval" })), null);
  assert.equal(inboxInvestigationInput(item({ kind: "paused_continuation" })), null);
});
