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
  inboxAgeBucket,
  inboxAgeBucketLabel,
  inboxAgeTone,
  inboxAgeDetail,
  filterInbox,
  inboxFilterCount,
  inboxEmptyMessage,
  INBOX_AGE_THRESHOLDS,
  INBOX_FILTERS,
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

test("inboxAgeBucket bands the LOGICAL-tick age at the configured thresholds", () => {
  const { fresh, waiting, stale } = INBOX_AGE_THRESHOLDS;
  // Boundaries are half-open: [0,fresh) fresh, [fresh,waiting) waiting, etc.
  assert.equal(inboxAgeBucket(0), "fresh");
  assert.equal(inboxAgeBucket(fresh - 1), "fresh");
  assert.equal(inboxAgeBucket(fresh), "waiting");
  assert.equal(inboxAgeBucket(waiting - 1), "waiting");
  assert.equal(inboxAgeBucket(waiting), "stale");
  assert.equal(inboxAgeBucket(stale - 1), "stale");
  assert.equal(inboxAgeBucket(stale), "overdue");
  assert.equal(inboxAgeBucket(stale + 10_000), "overdue");
  // No anchor / nonsense age → unknown, never silently "fresh" (no fabricated age).
  assert.equal(inboxAgeBucket(null), "unknown");
  assert.equal(inboxAgeBucket(undefined), "unknown");
  assert.equal(inboxAgeBucket(-5), "unknown");
  assert.equal(inboxAgeBucket(Number.NaN), "unknown");
});

test("age bucket labels + tones are honest and restrained", () => {
  assert.equal(inboxAgeBucketLabel("fresh"), "Fresh");
  assert.equal(inboxAgeBucketLabel("waiting"), "Waiting");
  assert.equal(inboxAgeBucketLabel("stale"), "Stale");
  assert.equal(inboxAgeBucketLabel("overdue"), "Overdue");
  assert.equal(inboxAgeBucketLabel("unknown"), "Age unavailable");
  // Only the bands that need the operator sooner carry color; the rest stay faint.
  assert.equal(inboxAgeTone("overdue"), "blocked");
  assert.equal(inboxAgeTone("stale"), "in_progress");
  assert.equal(inboxAgeTone("waiting"), "backlog");
  assert.equal(inboxAgeTone("fresh"), "backlog");
  assert.equal(inboxAgeTone("unknown"), "backlog");
});

test("inboxAgeDetail reports ticks (never wall-clock units), or null when unknown", () => {
  assert.equal(inboxAgeDetail(1), "1 tick");
  assert.equal(inboxAgeDetail(0), "0 ticks");
  assert.equal(inboxAgeDetail(42), "42 ticks");
  assert.equal(inboxAgeDetail(7.9), "7 ticks"); // floored, still honest
  assert.equal(inboxAgeDetail(null), null);
  assert.equal(inboxAgeDetail(undefined), null);
  assert.equal(inboxAgeDetail(-1), null);
});

test("filterInbox keeps all / a kind / only overdue", () => {
  const items: ReluxInboxItem[] = [
    item({ id: "approval:1", kind: "pending_approval", age_ticks: 5 }),
    item({ id: "run:1", kind: "failed_run", age_ticks: INBOX_AGE_THRESHOLDS.stale + 1 }),
    item({ id: "task:1", kind: "blocked_task", age_ticks: 10 }),
    item({ id: "task:2", kind: "blocked_task", age_ticks: INBOX_AGE_THRESHOLDS.stale + 9 }),
  ];
  assert.equal(filterInbox(items, "all").length, 4);
  assert.deepEqual(
    filterInbox(items, "blocked_task").map((i) => i.id),
    ["task:1", "task:2"],
  );
  assert.deepEqual(
    filterInbox(items, "pending_approval").map((i) => i.id),
    ["approval:1"],
  );
  // Overdue is a band cut across kinds — only the items past the stale threshold.
  assert.deepEqual(
    filterInbox(items, "overdue").map((i) => i.id),
    ["run:1", "task:2"],
  );
  // Counts mirror the filter.
  assert.equal(inboxFilterCount(items, "all"), 4);
  assert.equal(inboxFilterCount(items, "overdue"), 2);
  assert.equal(inboxFilterCount(items, "paused_continuation"), 0);
});

test("INBOX_FILTERS covers all/kinds/overdue and inboxEmptyMessage reflects the active filter", () => {
  assert.deepEqual(
    INBOX_FILTERS.map((f) => f.key),
    ["all", "pending_approval", "failed_run", "blocked_task", "paused_continuation", "overdue"],
  );
  // Each filter has an honest, filter-specific empty line (not the global one).
  assert.match(inboxEmptyMessage("all"), /Nothing needs you/);
  assert.match(inboxEmptyMessage("pending_approval"), /approvals/i);
  assert.match(inboxEmptyMessage("failed_run"), /failed runs/i);
  assert.match(inboxEmptyMessage("blocked_task"), /blocked/i);
  assert.match(inboxEmptyMessage("paused_continuation"), /paused/i);
  assert.match(inboxEmptyMessage("overdue"), /overdue/i);
});
