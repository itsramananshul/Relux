// Pure, framework-free helpers for the cross-Guild Inbox surface
// (docs/relix-dashboard-design.md §5 "The Inbox (the operator's home)";
// docs/relix-execution-and-issue-design.md §3.3b "Only the escalation kind reaches
// the Inbox — transient failures retry silently — so the Inbox stays signal").
//
// The kernel's read-only /v1/relux/inbox projection already decides WHICH items
// need attention and the recommended action KINDS (each backed by an existing
// route). This module is the presentation half: it maps each action kind to its
// button label + how it is invoked (a POST to an existing route, a navigation to
// the surface that owns the control, or a Prime investigation seed), groups items
// into dense sections, and derives the badge count / tones. It invents no action a
// backend route does not back, and mutates nothing.
//
// Kept dependency-free of React/DOM (it only type-imports the api + seed shapes,
// which are erased) so the mapping is unit-tested under `node --strip-types` (see
// the docs note dashboard-test-tsx-vs-ts-split). The Inbox page renders these.

import type { ReluxInbox, ReluxInboxItem, ReluxInboxKind, ReluxInboxActionKind } from "./api";
import type { InvestigationSeedInput } from "./investigateseed.ts";

// How an Inbox action button is invoked. "post" calls an existing mutating route
// (the operator's click is the only trigger — never auto-run); "nav" routes to the
// surface that already owns the richer control; "seed" seeds Prime with a safe,
// redacted investigation prompt and navigates to the chat.
export type InboxActionMode = "post" | "nav" | "seed";

// The static display + invocation spec for one action kind. The per-item TARGET
// (which run/task id, which path) is resolved separately by `inboxActionTarget`, so
// this table stays a pure constant the test can pin.
export interface InboxActionSpec {
  kind: ReluxInboxActionKind;
  label: string;
  hint: string;
  mode: InboxActionMode;
}

// The action vocabulary — every kind the projection can emit has exactly one spec,
// so a missing entry is a bug (the unit test asserts totality). Each `mode` is the
// SAFE invocation: a read-only POST (diagnose), a re-queue/retry POST the operator
// explicitly clicked, a navigation, or a Prime seed. None grants new authority.
const ACTION_SPECS: Record<ReluxInboxActionKind, InboxActionSpec> = {
  open_approval: {
    kind: "open_approval",
    label: "Open approval",
    hint: "Go to Approvals to decide this pending gate (approve / allow-always / deny).",
    mode: "nav",
  },
  retry: {
    kind: "retry",
    label: "Retry",
    hint: "Start a fresh attempt of this failed run through the existing retry route.",
    mode: "post",
  },
  reopen: {
    kind: "reopen",
    label: "Reopen",
    hint: "Re-queue this blocked task so its operative can run it again (running stays separate).",
    mode: "post",
  },
  reopen_and_run: {
    kind: "reopen_and_run",
    label: "Reopen & run",
    hint: "Re-queue the task and run it now through the same run gate (no bypass).",
    mode: "post",
  },
  diagnose: {
    kind: "diagnose",
    label: "Analyze failure",
    hint: "Ask the configured brain for a concise written diagnosis (read-only — changes nothing).",
    mode: "post",
  },
  investigate: {
    kind: "investigate",
    label: "Investigate with Prime",
    hint: "Open Prime pre-loaded with this item to debug it conversationally — Prime answers, it doesn't change anything.",
    mode: "seed",
  },
  continue: {
    kind: "continue",
    label: "Open continuation",
    hint: "Go to the Work board to resume the paused Prime loop where it stopped.",
    mode: "nav",
  },
  inspect: {
    kind: "inspect",
    label: "Inspect",
    hint: "Open the Work board to read this item's transcript / detail.",
    mode: "nav",
  },
};

// The display + invocation spec for an action kind. Throws for an unknown kind
// (caught by the totality test) so a new server action can't silently render blank.
export function inboxActionSpec(kind: ReluxInboxActionKind): InboxActionSpec {
  const spec = ACTION_SPECS[kind];
  if (!spec) throw new Error(`no inbox action spec for kind "${kind}"`);
  return spec;
}

// The navigation TARGET for a "nav"/"seed" action on a specific item, or null for a
// "post" action (which calls a route, not a route change). Keeps the page's wiring
// honest: a nav button always knows where it goes, derived from the item + kind.
export function inboxActionTarget(
  item: ReluxInboxItem,
  kind: ReluxInboxActionKind,
): string | null {
  switch (kind) {
    case "open_approval":
      return "/approvals";
    case "investigate":
      return "/prime";
    case "continue":
      // The resumable-continuation Continue control (with its full context + result
      // handling) lives in the Work oversight strip — route there rather than firing
      // a bare resume from a compact row.
      return "/work";
    case "inspect":
      return item.link || "/work";
    default:
      // retry / reopen / reopen_and_run / diagnose are POSTs, not navigations.
      return null;
  }
}

// One rendered Inbox section: a kind, its human heading, and the items in it. Only
// non-empty groups are produced, in the fixed priority order below.
export interface InboxGroup {
  kind: ReluxInboxKind;
  label: string;
  items: ReluxInboxItem[];
}

// The fixed section order + heading per kind: what needs a decision first
// (approvals), then what broke (failed runs), then what is stuck (blocked work),
// then the paused loops.
const GROUP_ORDER: { kind: ReluxInboxKind; label: string }[] = [
  { kind: "pending_approval", label: "Approvals" },
  { kind: "failed_run", label: "Failed runs" },
  { kind: "blocked_task", label: "Blocked work" },
  { kind: "paused_continuation", label: "Paused loops" },
];

// Group the flat item list into ordered, non-empty sections. Item order WITHIN a
// section is preserved (the backend already sorted by severity then recency).
export function groupInbox(items: ReluxInboxItem[]): InboxGroup[] {
  const out: InboxGroup[] = [];
  for (const g of GROUP_ORDER) {
    const inGroup = items.filter((it) => it.kind === g.kind);
    if (inGroup.length > 0) out.push({ kind: g.kind, label: g.label, items: inGroup });
  }
  return out;
}

// The sidebar badge count: how many things need the operator. Just the item count
// (the projection already excludes silently self-healing work), clamped to ≥ 0.
export function inboxBadgeCount(inbox: ReluxInbox | null | undefined): number {
  const n = inbox?.items?.length ?? 0;
  return n > 0 ? n : 0;
}

// The B&W badge tone for a severity, mapping onto the existing chip vocabulary
// (styles.css `.badge <tone>`): a critical escalation reads as the error tone, a
// warn as the blocked tone, an info as the neutral queued tone — never a loud fill.
export function inboxSeverityTone(
  severity: ReluxInboxItem["severity"],
): "failed" | "blocked" | "queued" {
  switch (severity) {
    case "critical":
      return "failed";
    case "warn":
      return "blocked";
    default:
      return "queued";
  }
}

// A short human label for a severity (the row's leading chip text).
export function inboxSeverityLabel(severity: ReluxInboxItem["severity"]): string {
  switch (severity) {
    case "critical":
      return "Critical";
    case "warn":
      return "Needs attention";
    default:
      return "Info";
  }
}

// ---------------------------------------------------------------------------
// Ageing / SLA (docs/relix-dashboard-design.md §6.11 "triage SLAs / ageing").
//
// IMPORTANT — the kernel uses a DETERMINISTIC LOGICAL CLOCK (crates/relux-kernel/
// src/clock.rs), not wall-clock time. The backend's `age_ticks` is therefore the
// number of kernel EVENTS (ticks) that have elapsed since an item began needing
// attention — NOT a count of real seconds. So this surface never claims a wall-clock
// duration or a real deadline; it buckets the honest logical age into urgency bands
// and, when an item carries no anchor at all, says so ("age unavailable") rather than
// inventing one (the §6.11 / mission "no fake deadlines" rule).
// ---------------------------------------------------------------------------

// The urgency band for an item's logical age. "unknown" = no anchor to measure from.
export type InboxAgeBucket = "fresh" | "waiting" | "stale" | "overdue" | "unknown";

// Static, configurable thresholds in LOGICAL-CLOCK TICKS (kernel events), not wall
// seconds. An item younger than `fresh` ticks is fresh; `< waiting` is waiting; `<
// stale` is stale; at/above `stale` it is overdue. Tunable in one place.
export const INBOX_AGE_THRESHOLDS = {
  fresh: 30,
  waiting: 120,
  stale: 600,
} as const;

// Bucket a logical-tick age. A missing / non-finite / negative age is honestly
// "unknown" (the row shows "age unavailable"), never silently treated as fresh.
export function inboxAgeBucket(ageTicks: number | null | undefined): InboxAgeBucket {
  if (ageTicks == null || !Number.isFinite(ageTicks) || ageTicks < 0) return "unknown";
  if (ageTicks < INBOX_AGE_THRESHOLDS.fresh) return "fresh";
  if (ageTicks < INBOX_AGE_THRESHOLDS.waiting) return "waiting";
  if (ageTicks < INBOX_AGE_THRESHOLDS.stale) return "stale";
  return "overdue";
}

// The short human word for a bucket (the age badge's text).
export function inboxAgeBucketLabel(bucket: InboxAgeBucket): string {
  switch (bucket) {
    case "fresh":
      return "Fresh";
    case "waiting":
      return "Waiting";
    case "stale":
      return "Stale";
    case "overdue":
      return "Overdue";
    default:
      return "Age unavailable";
  }
}

// The restrained B&W badge tone for an age bucket, onto the existing chip vocabulary
// (styles.css): overdue reads as the error/blocked tone, stale as the warn
// (in_progress) tone, and fresh/waiting/unknown stay faint (backlog) — color is
// reserved for the bands that actually need the operator sooner.
export function inboxAgeTone(
  bucket: InboxAgeBucket,
): "blocked" | "in_progress" | "backlog" {
  switch (bucket) {
    case "overdue":
      return "blocked";
    case "stale":
      return "in_progress";
    default:
      return "backlog";
  }
}

// The honest tick-count detail for a row (e.g. "12 ticks"), or null when no age is
// available. Deliberately uses "ticks" (logical clock events), never "seconds" /
// "min", so it can't be misread as wall-clock time.
export function inboxAgeDetail(ageTicks: number | null | undefined): string | null {
  if (ageTicks == null || !Number.isFinite(ageTicks) || ageTicks < 0) return null;
  const n = Math.floor(ageTicks);
  return `${n} tick${n === 1 ? "" : "s"}`;
}

// ---------------------------------------------------------------------------
// Filtering (docs/relix-dashboard-design.md §6.11 "filtering of the queue").
// ---------------------------------------------------------------------------

// The Inbox filter: "all", one of the item kinds, or the cheap "overdue" band cut.
export type InboxFilter = ReluxInboxKind | "all" | "overdue";

export interface InboxFilterSpec {
  key: InboxFilter;
  label: string;
}

// The filter chips shown above the queue, in a stable order: All, then per-kind, then
// the overdue-only band cut.
export const INBOX_FILTERS: InboxFilterSpec[] = [
  { key: "all", label: "All" },
  { key: "pending_approval", label: "Approvals" },
  { key: "failed_run", label: "Failed runs" },
  { key: "blocked_task", label: "Blocked" },
  { key: "paused_continuation", label: "Paused" },
  { key: "overdue", label: "Overdue" },
];

// Apply a filter to the item list. "all" passes everything; "overdue" keeps only the
// items whose logical age is in the overdue band; any kind keeps only that kind. Pure
// and order-preserving (the backend already sorted by severity then oldest-first).
export function filterInbox(
  items: ReluxInboxItem[],
  filter: InboxFilter,
): ReluxInboxItem[] {
  if (filter === "all") return items;
  if (filter === "overdue") {
    return items.filter((it) => inboxAgeBucket(it.age_ticks) === "overdue");
  }
  return items.filter((it) => it.kind === filter);
}

// How many items a filter would show — for the count on each filter chip.
export function inboxFilterCount(items: ReluxInboxItem[], filter: InboxFilter): number {
  return filterInbox(items, filter).length;
}

// The human label for a filter key (the chip text), looked up from the single
// source above so the empty-state copy and the chip never drift.
export function inboxFilterLabel(filter: InboxFilter): string {
  return INBOX_FILTERS.find((f) => f.key === filter)?.label ?? "All";
}

// ---------------------------------------------------------------------------
// Search (docs/relix-dashboard-design.md §6.11 "cross-project search of the
// queue"). A purely LOCAL, free-text filter over the items already loaded — it
// fetches nothing and grants no authority; it only narrows what is shown.
// ---------------------------------------------------------------------------

// Normalize a raw query: trimmed + lowercased for case-insensitive matching. An
// absent / blank query normalizes to "" (which matches everything).
export function normalizeInboxQuery(query: string | null | undefined): string {
  return (query ?? "").trim().toLowerCase();
}

// The searchable text blob for one item: its identity (the category-prefixed id
// + every related id), its kind/severity, its title/summary/failure class, and
// its action button labels — so a search by "retry", "auth_required", a run id,
// or a word in the title all hit. Lowercased once for the predicate below.
export function inboxItemSearchText(item: ReluxInboxItem): string {
  const parts: (string | null | undefined)[] = [
    item.title,
    item.summary,
    item.kind,
    item.severity,
    item.id,
    item.task_id,
    item.run_id,
    item.approval_id,
    item.continuation_id,
    item.failure_class,
    ...item.actions.map((kind) => inboxActionSpec(kind).label),
  ];
  return parts.filter((p) => typeof p === "string" && p.length > 0).join(" ").toLowerCase();
}

// Does an item match the free-text query? Empty query → everything matches.
// Multi-term is AND (every whitespace-separated term must appear somewhere in
// the item's text), so "task auth" narrows to items mentioning both.
export function inboxItemMatchesQuery(item: ReluxInboxItem, query: string): boolean {
  const q = normalizeInboxQuery(query);
  if (!q) return true;
  const text = inboxItemSearchText(item);
  return q.split(/\s+/).every((term) => text.includes(term));
}

// Apply the free-text search to the list. Empty query passes everything through
// unchanged; otherwise keeps only matching items, order-preserving (the backend's
// severity-then-oldest order is retained). Pure — composes with filterInbox.
export function searchInbox(items: ReluxInboxItem[], query: string): ReluxInboxItem[] {
  const q = normalizeInboxQuery(query);
  if (!q) return items;
  return items.filter((it) => inboxItemMatchesQuery(it, q));
}

// The honest empty-state line when a search (and/or a kind/overdue filter) has
// narrowed a non-empty queue to nothing. Names the active query verbatim and, when
// a filter is also active, the filter — so the operator sees exactly what they're
// looking through (vs. the global all-clear). Falls back to the filter-specific
// line when no query is set.
export function inboxSearchEmptyMessage(filter: InboxFilter, query: string): string {
  const raw = (query ?? "").trim();
  if (raw) {
    const scope = filter === "all" ? "" : ` in ${inboxFilterLabel(filter)}`;
    return `No items match '${raw}'${scope}.`;
  }
  return inboxEmptyMessage(filter);
}

// The honest empty-state line for the ACTIVE filter, so an empty filtered view reads
// correctly ("No overdue items", not the global "Nothing needs you").
export function inboxEmptyMessage(filter: InboxFilter): string {
  switch (filter) {
    case "pending_approval":
      return "No pending approvals need you right now.";
    case "failed_run":
      return "No hard-failed runs need attention.";
    case "blocked_task":
      return "No blocked work is waiting on a decision.";
    case "paused_continuation":
      return "No paused Prime loops are waiting.";
    case "overdue":
      return "Nothing is overdue.";
    default:
      return "Nothing needs you right now.";
  }
}

// Build the safe, redacted Prime investigation seed input for an item (consumed by
// investigateseed.buildInvestigationSeed in the page). It carries only the item's
// identity + the deterministic summary the projection already redacted/bounded — no
// new context is fetched. A non-investigable kind (approval / continuation) returns
// null, so the page never offers a seed it can't fill honestly.
export function inboxInvestigationInput(
  item: ReluxInboxItem,
): InvestigationSeedInput | null {
  if (item.kind === "failed_run") {
    return {
      subject: "run",
      run: {
        id: item.run_id ?? item.id,
        status: "failed",
        failureClass: item.failure_class ?? null,
        summary: item.summary,
      },
      task: item.task_id ? { id: item.task_id } : null,
      classLabel: item.failure_class ?? "Failed run",
      rootCause: item.summary,
      recommendation:
        "Diagnose the failure from the transcript and decide whether to retry, reassign, or block.",
    };
  }
  if (item.kind === "blocked_task") {
    return {
      subject: "task",
      task: { id: item.task_id ?? item.id, title: item.title, status: "blocked" },
      run: item.run_id
        ? { id: item.run_id, status: "failed", failureClass: item.failure_class ?? null }
        : null,
      classLabel: item.failure_class ?? "Blocked",
      rootCause: item.summary,
      recommendation:
        "Decide whether to reopen (re-queue) the task, reassign it, or keep it blocked until the cause is fixed.",
    };
  }
  return null;
}
