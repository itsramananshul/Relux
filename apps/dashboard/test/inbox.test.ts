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
  inboxFilterLabel,
  inboxEmptyMessage,
  normalizeInboxQuery,
  inboxItemSearchText,
  inboxItemMatchesQuery,
  searchInbox,
  inboxSearchEmptyMessage,
  buildInboxGroups,
  inboxGroupKey,
  inboxKindLabel,
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

test("normalizeInboxQuery trims + lowercases; blank/absent → empty", () => {
  assert.equal(normalizeInboxQuery("  AuTh  "), "auth");
  assert.equal(normalizeInboxQuery(""), "");
  assert.equal(normalizeInboxQuery("   "), "");
  assert.equal(normalizeInboxQuery(null), "");
  assert.equal(normalizeInboxQuery(undefined), "");
});

test("inboxItemSearchText covers identity, content, and action labels", () => {
  const text = inboxItemSearchText(
    item({
      id: "run:abc",
      kind: "failed_run",
      severity: "critical",
      title: "Login flow broke",
      summary: "the adapter timed out",
      task_id: "tk-77",
      run_id: "run-abc",
      failure_class: "auth_required",
      actions: ["retry", "diagnose"],
    }),
  );
  // Identity + ids.
  assert.match(text, /run:abc/);
  assert.match(text, /tk-77/);
  assert.match(text, /run-abc/);
  // Kind + severity + failure class + content.
  assert.match(text, /failed_run/);
  assert.match(text, /critical/);
  assert.match(text, /auth_required/);
  assert.match(text, /login flow broke/);
  assert.match(text, /adapter timed out/);
  // Action button labels ("Retry", "Analyze failure"), lowercased.
  assert.match(text, /retry/);
  assert.match(text, /analyze failure/);
});

test("inboxItemMatchesQuery: empty matches all; case-insensitive; multi-term is AND", () => {
  const it = item({
    title: "Login flow broke",
    failure_class: "auth_required",
    run_id: "run-abc",
    actions: ["retry"],
  });
  assert.equal(inboxItemMatchesQuery(it, ""), true);
  assert.equal(inboxItemMatchesQuery(it, "   "), true);
  assert.equal(inboxItemMatchesQuery(it, "LOGIN"), true);
  assert.equal(inboxItemMatchesQuery(it, "auth_required"), true);
  assert.equal(inboxItemMatchesQuery(it, "run-abc"), true);
  // Multi-term AND: both terms must appear somewhere in the item's text.
  assert.equal(inboxItemMatchesQuery(it, "login auth"), true);
  assert.equal(inboxItemMatchesQuery(it, "login nope"), false);
  assert.equal(inboxItemMatchesQuery(it, "zzz"), false);
});

test("searchInbox filters by free text; empty query passes everything, order-preserving", () => {
  const items: ReluxInboxItem[] = [
    item({ id: "approval:1", kind: "pending_approval", title: "Approve hire" }),
    item({ id: "run:1", kind: "failed_run", failure_class: "auth_required", title: "Run broke" }),
    item({ id: "task:1", kind: "blocked_task", title: "Ship the auth page" }),
  ];
  // Empty query → unchanged list (same order, same length).
  assert.deepEqual(searchInbox(items, "").map((i) => i.id), ["approval:1", "run:1", "task:1"]);
  // "auth" hits the failure_class on run:1 AND the title on task:1.
  assert.deepEqual(searchInbox(items, "auth").map((i) => i.id), ["run:1", "task:1"]);
  // A specific id narrows to one.
  assert.deepEqual(searchInbox(items, "approval:1").map((i) => i.id), ["approval:1"]);
  // No match → empty.
  assert.equal(searchInbox(items, "nonesuch").length, 0);
});

test("inboxFilterLabel resolves the chip label, defaulting to All", () => {
  assert.equal(inboxFilterLabel("all"), "All");
  assert.equal(inboxFilterLabel("pending_approval"), "Approvals");
  assert.equal(inboxFilterLabel("overdue"), "Overdue");
});

test("inboxSearchEmptyMessage names the query + filter, or falls back to the filter line", () => {
  // No query → the existing filter-specific line.
  assert.equal(inboxSearchEmptyMessage("overdue", ""), inboxEmptyMessage("overdue"));
  assert.equal(inboxSearchEmptyMessage("all", "   "), inboxEmptyMessage("all"));
  // Query under "all" names the query verbatim, no scope clause.
  assert.match(inboxSearchEmptyMessage("all", "auth"), /No items match 'auth'\./);
  // Query under a filter names BOTH the query and the filter scope.
  const msg = inboxSearchEmptyMessage("failed_run", "auth");
  assert.match(msg, /No items match 'auth'/);
  assert.match(msg, /Failed runs/);
});

// --- Cross-item grouping (§6.11) ------------------------------------------------

test("inboxGroupKey: a parent_task edge keys the subtree; a parent-in-queue roots its own", () => {
  const parents = new Set<string>(["tk-parent"]);
  // A child with a parent_task → that subtree.
  assert.equal(
    inboxGroupKey(item({ id: "run:1", parent_task: "tk-parent" }), parents),
    "subtree:tk-parent",
  );
  // A blocked task that IS some item's parent → roots its own subtree.
  assert.equal(
    inboxGroupKey(item({ id: "task:tk-parent", kind: "blocked_task", task_id: "tk-parent" }), parents),
    "subtree:tk-parent",
  );
  // No edge and not a parent → standalone, keyed by its own id (never merges).
  assert.equal(inboxGroupKey(item({ id: "approval:9", kind: "pending_approval" }), parents), "solo:approval:9");
});

test("buildInboxGroups collapses a real subtree and leaves unrelated items standalone", () => {
  // A blocked parent task + a failed-run child + a blocked child, all under tk-root,
  // plus an UNRELATED approval. The three subtree members collapse; the approval stays
  // standalone (no fake grouping).
  const items: ReluxInboxItem[] = [
    item({ id: "task:tk-root", kind: "blocked_task", task_id: "tk-root", title: "Blocked: Ship the launch", severity: "warn", age_ticks: 50 }),
    item({ id: "run:1", kind: "failed_run", task_id: "tk-child-a", parent_task: "tk-root", severity: "critical", age_ticks: 10 }),
    item({ id: "task:tk-child-b", kind: "blocked_task", task_id: "tk-child-b", parent_task: "tk-root", severity: "warn", age_ticks: 200 }),
    item({ id: "approval:1", kind: "pending_approval", severity: "info", age_ticks: 5 }),
  ];
  const cards = buildInboxGroups(items);
  // Two cards: the collapsed subtree (root + 2 children) and the standalone approval.
  assert.equal(cards.length, 2);
  const subtree = cards.find((c) => c.key === "subtree:tk-root")!;
  assert.ok(subtree, "the subtree card exists");
  assert.equal(subtree.collapsible, true, "≥2 members → collapsible");
  assert.equal(subtree.items.length, 3, "root + both children are members");
  // Title is the root blocked task's own title, with the "Blocked: " prefix stripped.
  assert.equal(subtree.title, "Ship the launch");
  assert.equal(subtree.rootTaskId, "tk-root");
  // Rollups: WORST severity (the critical child) and OLDEST age (the 200-tick child).
  assert.equal(subtree.topSeverity, "critical");
  assert.equal(subtree.topAgeTicks, 200);
  // Per-kind counts: 2 blocked (root + child-b) and 1 failed.
  const counts = Object.fromEntries(subtree.kindCounts.map((c) => [c.kind, c.count]));
  assert.equal(counts["blocked_task"], 2);
  assert.equal(counts["failed_run"], 1);

  // The unrelated approval is its own non-collapsible solo card.
  const solo = cards.find((c) => c.key === "solo:approval:1")!;
  assert.ok(solo, "the approval is standalone");
  assert.equal(solo.collapsible, false);
  assert.equal(solo.items.length, 1);
});

test("buildInboxGroups: a single child whose parent is absent stays a standalone row", () => {
  // Only ONE item carries parent_task tk-x and the parent itself is not in the queue —
  // collapsing one item is pointless, so it is NOT collapsible (renders as a row).
  const cards = buildInboxGroups([
    item({ id: "run:1", kind: "failed_run", parent_task: "tk-x", parent_title: "Parent effort", age_ticks: 3 }),
  ]);
  assert.equal(cards.length, 1);
  assert.equal(cards[0].collapsible, false, "a lone subtree member is not worth collapsing");
  // A lone member's card title is the item's own title, not the subtree heading.
  assert.equal(cards[0].title, "t");
});

test("buildInboxGroups titles a subtree from parent_title when the root isn't in the queue", () => {
  // Two siblings under tk-y whose parent is NOT itself an attention item: the card is
  // titled from the backend-resolved parent_title, never blank.
  const cards = buildInboxGroups([
    item({ id: "run:1", kind: "failed_run", parent_task: "tk-y", parent_title: "Migration effort", age_ticks: 4 }),
    item({ id: "run:2", kind: "failed_run", parent_task: "tk-y", parent_title: "Migration effort", age_ticks: 9 }),
  ]);
  assert.equal(cards.length, 1);
  assert.equal(cards[0].collapsible, true);
  assert.equal(cards[0].title, "Migration effort");
});

test("buildInboxGroups preserves urgency order: a more-urgent group leads", () => {
  // A standalone critical run appears before a warn subtree because its first member is
  // more urgent (input is already backend-sorted; group order follows first appearance).
  const items: ReluxInboxItem[] = [
    item({ id: "run:hot", kind: "failed_run", severity: "critical", age_ticks: 1 }),
    item({ id: "task:p", kind: "blocked_task", task_id: "p", severity: "warn", age_ticks: 100 }),
    item({ id: "run:c", kind: "failed_run", parent_task: "p", severity: "warn", age_ticks: 80 }),
  ];
  const cards = buildInboxGroups(items);
  assert.deepEqual(cards.map((c) => c.key), ["solo:run:hot", "subtree:p"]);
});

test("buildInboxGroups: search/filter run FIRST, so a group only holds matched items", () => {
  const items: ReluxInboxItem[] = [
    item({ id: "run:1", kind: "failed_run", parent_task: "tk-root", title: "auth broke", age_ticks: 5 }),
    item({ id: "task:tk-child", kind: "blocked_task", task_id: "tk-child", parent_task: "tk-root", title: "Blocked: deploy", age_ticks: 6 }),
  ];
  // Mimic the page: search first, then group the visible subset. Only the matching
  // member survives, so the subtree degrades to a single standalone row.
  const cards = buildInboxGroups(searchInbox(items, "auth"));
  assert.equal(cards.length, 1);
  assert.equal(cards[0].items.length, 1);
  assert.equal(cards[0].collapsible, false, "one matched member → not a collapsed group");
});

test("inboxKindLabel resolves a section heading per kind", () => {
  assert.equal(inboxKindLabel("pending_approval"), "Approvals");
  assert.equal(inboxKindLabel("failed_run"), "Failed runs");
  assert.equal(inboxKindLabel("blocked_task"), "Blocked work");
  assert.equal(inboxKindLabel("paused_continuation"), "Paused loops");
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
