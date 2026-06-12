import { useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { useAsync } from "../components/common";
import {
  reluxInbox,
  reluxWork,
  type ReluxInbox,
  type ReluxInboxItem,
  type ReluxInboxActionKind,
} from "../api";
import {
  groupInbox,
  inboxActionSpec,
  inboxActionTarget,
  inboxInvestigationInput,
  inboxSeverityLabel,
  inboxSeverityTone,
  inboxAgeBucket,
  inboxAgeBucketLabel,
  inboxAgeTone,
  inboxAgeDetail,
  filterInbox,
  inboxFilterCount,
  searchInbox,
  inboxSearchEmptyMessage,
  buildInboxGroups,
  INBOX_FILTERS,
  type InboxFilter,
  type InboxGroupCard,
} from "../inbox";
import { buildInvestigationSeed, stashInvestigationSeed } from "../investigateseed";
import { ApprovalInlineDecisions } from "../components/ApprovalInlineDecisions";
import type { ReluxDiagnostic } from "../api";

// The cross-Guild Inbox (docs/relix-dashboard-design.md §5 "The Inbox (the
// operator's home)"; docs/relix-execution-and-issue-design.md §3.3b "Only the
// escalation kind reaches the Inbox — transient failures retry silently"). One
// dense, prioritized attention queue over the WHOLE Guild: pending approvals,
// hard-failed runs, blocked work, and a paused Prime loop — fed by the read-only
// /v1/relux/inbox projection. Every action button reuses an EXISTING route or
// surface; nothing here grants new authority or auto-runs anything.

// The inline result of a POST action on one row (retry / reopen / reopen & run) —
// a compact, shaped message, never the raw route envelope.
type ActionState =
  | { status: "idle" }
  | { status: "busy"; kind: ReluxInboxActionKind }
  | { status: "ok"; message: string }
  | { status: "err"; message: string };

// The inline diagnostic-narrative state (Analyze failure → read-only diagnose).
type DiagState =
  | { status: "loading" }
  | { status: "done"; result: ReluxDiagnostic }
  | { status: "error"; message: string };

// The compact ageing / SLA badge on a row. The kernel uses a deterministic LOGICAL
// clock (not wall-clock), so the age is a count of kernel events ("ticks") since the
// item began needing attention — never a real-time duration or deadline. An item with
// no anchor honestly reads "age unavailable" instead of inventing one (§6.11).
function AgeTicksBadge({ ageTicks }: { ageTicks: number | null | undefined }) {
  const bucket = inboxAgeBucket(ageTicks);
  const detail = inboxAgeDetail(ageTicks);
  if (bucket === "unknown") {
    return (
      <span
        className="mono muted"
        style={{ fontSize: 9 }}
        title="No timestamp anchor for this item — its age can't be measured, so none is shown (no fabricated deadline)."
      >
        age unavailable
      </span>
    );
  }
  const tip =
    `${detail ?? ""} since it needed attention. The kernel uses a deterministic ` +
    `logical clock, so this counts kernel events (ticks), not wall-clock time.`;
  return (
    <span className={"badge " + inboxAgeTone(bucket)} style={{ fontSize: 9 }} title={tip}>
      {inboxAgeBucketLabel(bucket)}
      {detail ? ` · ${detail}` : ""}
    </span>
  );
}

function AgeBadge({ item }: { item: ReluxInboxItem }) {
  return <AgeTicksBadge ageTicks={item.age_ticks} />;
}

// A collapsed stalled-subtree card (§6.11 cross-item grouping). The header summarizes
// the whole subtree — its root title, the WORST member severity, the OLDEST member age,
// and the per-kind counts — and is expandable to the full member rows. Actions are
// never hidden permanently: one click on the header reveals every member's controls.
export function SubtreeGroupCard({
  group,
  onActed,
  defaultOpen = false,
}: {
  group: InboxGroupCard;
  onActed: () => void;
  defaultOpen?: boolean;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const memberCount = group.items.length;
  return (
    <div className="card" style={{ padding: 12 }}>
      <button
        className="row"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        title={open ? "Collapse this subtree" : "Expand to act on each member"}
        style={{
          width: "100%",
          alignItems: "baseline",
          gap: 8,
          flexWrap: "wrap",
          background: "none",
          border: "none",
          padding: 0,
          cursor: "pointer",
          textAlign: "left",
        }}
      >
        <span className="mono muted" style={{ fontSize: 11 }}>{open ? "▾" : "▸"}</span>
        <span className={"badge " + inboxSeverityTone(group.topSeverity)} style={{ fontSize: 9 }}>
          {inboxSeverityLabel(group.topSeverity)}
        </span>
        <AgeTicksBadge ageTicks={group.topAgeTicks} />
        <span style={{ fontWeight: 600, fontSize: 13 }}>{group.title}</span>
        <span className="mono muted" style={{ fontSize: 11 }}>
          {memberCount} item{memberCount === 1 ? "" : "s"}
        </span>
        <div className="spacer" style={{ flex: 1 }} />
        <span className="mono muted" style={{ fontSize: 10 }}>
          {group.kindCounts.map((c) => `${c.count} ${c.label.toLowerCase()}`).join(" · ")}
        </span>
      </button>
      {!open && (
        <div className="muted" style={{ fontSize: 11, marginTop: 6 }}>
          A stalled subtree — expand to act on each member (every action is still here).
        </div>
      )}
      {open && (
        <div style={{ marginTop: 10 }}>
          {group.items.map((it) => (
            <InboxRow key={it.id} item={it} onActed={onActed} />
          ))}
        </div>
      )}
    </div>
  );
}

export function InboxRow({ item, onActed }: { item: ReluxInboxItem; onActed: () => void }) {
  const navigate = useNavigate();
  const [action, setAction] = useState<ActionState>({ status: "idle" });
  const [diag, setDiag] = useState<DiagState | null>(null);

  const busyKind = action.status === "busy" ? action.kind : null;

  // A pending-approval item carries the full approval record, so the row offers the
  // SAME inline decisions (approve & run / allow always / deny) the Work oversight
  // strip does — through the existing reluxApprovals routes, no new authority. When
  // the projection didn't embed it (older backend), we fall back to the generic
  // "Open approval" nav button so the row is never a dead end.
  const inlineApproval =
    item.kind === "pending_approval" ? item.approval ?? null : null;

  // A "post" action: call the existing route the operator explicitly clicked, show a
  // compact shaped result, then ask the page to refresh. Never auto-runs — the click
  // is the only trigger.
  async function runPost(kind: ReluxInboxActionKind) {
    setAction({ status: "busy", kind });
    try {
      if (kind === "retry") {
        if (!item.run_id) throw new Error("no run to retry");
        const res = await reluxWork.retryRun(item.run_id);
        setAction({ status: "ok", message: `Retrying as run ${res.run_id}.` });
      } else if (kind === "reopen") {
        if (!item.task_id) throw new Error("no task to reopen");
        await reluxWork.reopenTask(item.task_id);
        setAction({ status: "ok", message: "Reopened — re-queued for its operative." });
      } else if (kind === "reopen_and_run") {
        if (!item.task_id) throw new Error("no task to reopen");
        const res = await reluxWork.reopenAndRunTask(item.task_id);
        setAction({
          status: "ok",
          message: res.run_id
            ? `Reopened and started run ${res.run_id}.`
            : `Reopened — ${res.run_refused ?? "the run was refused; the reopened state is kept."}`,
        });
      } else {
        throw new Error(`unsupported post action ${kind}`);
      }
      onActed();
    } catch (e) {
      setAction({ status: "err", message: e instanceof Error ? e.message : "Action failed." });
    }
  }

  // Analyze failure: the read-only diagnostic narrative (POST /runs/:id/diagnose).
  // Mutates nothing; the result renders inline below the row.
  async function runDiagnose() {
    if (!item.run_id) return;
    setDiag({ status: "loading" });
    try {
      const result = await reluxWork.diagnoseRun(item.run_id);
      setDiag({ status: "done", result });
    } catch (e) {
      setDiag({ status: "error", message: e instanceof Error ? e.message : "Analysis failed." });
    }
  }

  // Investigate with Prime: seed the chat with the safe, redacted item context and
  // route there (the seed builder redacts; Prime is told to change nothing).
  function runInvestigate() {
    const input = inboxInvestigationInput(item);
    if (!input) return;
    const seed = buildInvestigationSeed(input);
    stashInvestigationSeed(window.sessionStorage, seed);
    navigate("/prime");
  }

  function onAction(kind: ReluxInboxActionKind) {
    const spec = inboxActionSpec(kind);
    if (spec.mode === "seed") {
      runInvestigate();
      return;
    }
    if (spec.mode === "nav") {
      const target = inboxActionTarget(item, kind);
      if (target) navigate(target);
      return;
    }
    // mode === "post"
    if (kind === "diagnose") {
      void runDiagnose();
      return;
    }
    void runPost(kind);
  }

  return (
    <div className="card sm" style={{ padding: 12, marginBottom: 8 }}>
      <div className="row" style={{ alignItems: "baseline", gap: 8, flexWrap: "wrap" }}>
        <span className={"badge " + inboxSeverityTone(item.severity)} style={{ fontSize: 9 }}>
          {inboxSeverityLabel(item.severity)}
        </span>
        <AgeBadge item={item} />
        <span style={{ fontWeight: 600, fontSize: 13 }}>{item.title}</span>
        <div className="spacer" style={{ flex: 1 }} />
        <span className="mono muted" style={{ fontSize: 10 }}>{item.id}</span>
      </div>
      <div className="muted" style={{ fontSize: 12, marginTop: 4, lineHeight: 1.5 }}>
        {item.summary}
      </div>
      {inlineApproval ? (
        // Inline approval decisions, reusing the shared controls (decide / execute /
        // allow-always). "Open approval →" always stays available for the full record.
        <div style={{ marginTop: 4 }}>
          <ApprovalInlineDecisions approval={inlineApproval} onDecided={onActed} />
          <div className="row" style={{ marginTop: 6 }}>
            <Link to={item.link} className="link" style={{ fontSize: 11 }}>
              Open approval →
            </Link>
          </div>
        </div>
      ) : (
        <div className="row wrap" style={{ gap: 6, marginTop: 10 }}>
          {item.actions.map((kind) => {
            const spec = inboxActionSpec(kind);
            const isBusy = busyKind === kind;
            const primary = kind === item.actions[0];
            return (
              <button
                key={kind}
                className={"btn sm" + (primary ? "" : " ghost")}
                title={spec.hint}
                disabled={action.status === "busy"}
                onClick={() => onAction(kind)}
              >
                {isBusy ? "…" : spec.label}
              </button>
            );
          })}
          <Link to={item.link} className="link" style={{ fontSize: 11, alignSelf: "center" }}>
            Open →
          </Link>
        </div>
      )}
      {action.status === "ok" && (
        <div className="banner ok" style={{ fontSize: 11, marginTop: 8 }}>{action.message}</div>
      )}
      {action.status === "err" && (
        <div className="banner err" style={{ fontSize: 11, marginTop: 8 }}>{action.message}</div>
      )}
      {diag && (
        <div
          role="note"
          aria-label="Diagnostic result"
          style={{ marginTop: 10, paddingTop: 8, borderTop: "1px solid var(--border)" }}
        >
          {diag.status === "loading" && (
            <div className="muted" style={{ fontSize: 11 }}>Analyzing the failure…</div>
          )}
          {diag.status === "error" && (
            <div className="banner err" style={{ fontSize: 11 }}>Analysis failed: {diag.message}</div>
          )}
          {diag.status === "done" && (
            <>
              <div
                className="muted"
                style={{ fontSize: 10, textTransform: "uppercase", letterSpacing: "0.05em", marginBottom: 4 }}
              >
                {diag.result.mode === "model" ? "Diagnostic narrative" : "Diagnostic unavailable"}
                {diag.result.mode === "model" && diag.result.model ? ` · ${diag.result.model}` : ""}
              </div>
              <div style={{ fontSize: 12, whiteSpace: "pre-wrap" }}>{diag.result.narrative}</div>
            </>
          )}
        </div>
      )}
    </div>
  );
}

export function Inbox() {
  const { data, loading, error, reload } = useAsync<ReluxInbox>(() => reluxInbox.get(), []);
  const [filter, setFilter] = useState<InboxFilter>("all");
  // Cross-item grouping (§6.11): collapse a stalled subtree (items whose tasks share a
  // parent_task edge) into one card. Default ON — the whole point of the queue is to
  // turn a fanned-out failure into one thing to look at. The toggle drops to the flat
  // per-kind sections for operators who want every row laid out.
  const [grouped, setGrouped] = useState(true);
  // The free-text search lives in the URL (`/inbox?q=…`) so a narrowed view is
  // shareable and survives refresh/back-forward (mirrors the Briefs `?brief=` /
  // Agents `?agent=` pattern). It is a purely local cut over the loaded items.
  const [searchParams, setSearchParams] = useSearchParams();
  const query = searchParams.get("q") ?? "";
  function setQuery(next: string) {
    setSearchParams(
      (prev) => {
        const p = new URLSearchParams(prev);
        if (next.trim()) p.set("q", next);
        else p.delete("q");
        return p;
      },
      { replace: true },
    );
  }

  const allItems = data?.items ?? [];
  // Search first (across all kinds), then the kind/overdue filter narrows it. The
  // filter chip counts reflect the SEARCHED set, so each chip honestly reads "how
  // many of this kind match your search".
  const searched = searchInbox(allItems, query);
  const visible = filterInbox(searched, filter);
  // Flat per-kind sections (toggle off) vs. collapsed subtree cards (toggle on). Both
  // operate on the SAME already-searched + filtered `visible` set, so a group only ever
  // holds matched items ("a group matches if any member matches" holds by construction).
  const groups = groupInbox(visible);
  const groupCards = buildInboxGroups(visible);
  // A search and/or the kind filter narrowed a non-empty queue to nothing (vs. a
  // globally empty Inbox) — the empty state names the active query/filter.
  const filteredToEmpty = allItems.length > 0 && visible.length === 0;

  return (
    <div className="grid" style={{ paddingBottom: 16 }}>
      <div className="row" style={{ alignItems: "center", marginBottom: 4 }}>
        <h3 style={{ margin: 0 }}>Attention queue</h3>
        <div className="spacer" style={{ flex: 1 }} />
        <button className="btn ghost sm" onClick={() => reload()} disabled={loading}>
          {loading ? "Refreshing…" : "Refresh"}
        </button>
      </div>
      <p className="muted" style={{ fontSize: 12, marginTop: 0, lineHeight: 1.6 }}>
        Everything across the Guild that needs you, most urgent first, then oldest-first within a
        severity — pending approvals, hard-failed runs, blocked work, and paused loops. Each row
        carries an ageing band (fresh / waiting / stale / overdue) measured in the kernel's logical
        clock ticks, not wall-clock time. Transient failures that retry on their own never appear
        here, so this stays signal, not noise. Every action reuses an existing control; nothing
        runs without your click.
      </p>

      {/* Search box — a purely local, free-text cut over the loaded items (title,
          summary, kind, severity, ids, failure class, action labels). The query
          lives in the URL so the narrowed view is shareable. */}
      <div className="row" style={{ gap: 6, marginBottom: 4, alignItems: "center" }}>
        <input
          type="search"
          className="input"
          aria-label="Search the attention queue"
          placeholder="Search the queue — title, id, kind, failure class, action…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          style={{ flex: 1, fontSize: 12 }}
        />
        {query && (
          <button className="btn ghost sm" onClick={() => setQuery("")} title="Clear the search">
            Clear
          </button>
        )}
      </div>

      {/* Filter chips — a cheap cut by kind, plus an overdue-only band. The active
          chip carries the live count (over the searched set); selecting one narrows
          the queue below. The Group toggle collapses stalled subtrees into one card. */}
      <div className="row wrap" style={{ gap: 6, marginBottom: 4, alignItems: "center" }}>
        {INBOX_FILTERS.map((f) => {
          const active = filter === f.key;
          const count = inboxFilterCount(searched, f.key);
          return (
            <button
              key={f.key}
              className={"btn sm" + (active ? "" : " ghost")}
              aria-pressed={active}
              onClick={() => setFilter(f.key)}
            >
              {f.label}
              <span className="mono muted" style={{ fontSize: 10, marginLeft: 6 }}>{count}</span>
            </button>
          );
        })}
        <div className="spacer" style={{ flex: 1 }} />
        <button
          className={"btn sm" + (grouped ? "" : " ghost")}
          aria-pressed={grouped}
          onClick={() => setGrouped((v) => !v)}
          title={
            grouped
              ? "Grouping related items (a stalled subtree collapses into one card). Click for a flat list."
              : "Showing every item flat, by kind. Click to collapse related subtrees into one card."
          }
        >
          {grouped ? "Grouped" : "Group related"}
        </button>
      </div>

      {error && (
        <div className="banner err" style={{ fontSize: 12 }}>
          Couldn't load the Inbox: {error}
        </div>
      )}

      {data && allItems.length === 0 && (
        <div className="card">
          <div className="empty" style={{ padding: 24, textAlign: "center" }}>
            <div style={{ fontSize: 28, marginBottom: 8 }}>✓</div>
            <div style={{ fontWeight: 600, marginBottom: 4 }}>Nothing needs you right now.</div>
            <div className="muted" style={{ fontSize: 12 }}>
              No pending approvals, no hard-failed runs, no blocked work, no paused loops. New
              escalations will appear here.
            </div>
          </div>
        </div>
      )}

      {filteredToEmpty && (
        <div className="card">
          <div className="empty" style={{ padding: 24, textAlign: "center" }}>
            <div style={{ fontWeight: 600, marginBottom: 4 }}>
              {inboxSearchEmptyMessage(filter, query)}
            </div>
            <div className="muted" style={{ fontSize: 12 }}>
              Other attention items are still queued — clear the{" "}
              {query ? "search or filter" : "filter"} to see them.
            </div>
          </div>
        </div>
      )}

      {/* Grouped view (default): a collapsed card per stalled subtree, with standalone
          items rendered as their own row — unrelated items are never falsely grouped.
          The flat view (toggle off) keeps the per-kind sections. */}
      {grouped
        ? groupCards.map((g) =>
            g.collapsible ? (
              <SubtreeGroupCard key={g.key} group={g} onActed={reload} />
            ) : (
              <InboxRow key={g.key} item={g.items[0]} onActed={reload} />
            ),
          )
        : groups.map((g) => (
            <div key={g.kind} className="card" style={{ padding: 12 }}>
              <div className="row" style={{ alignItems: "baseline", marginBottom: 8 }}>
                <h4 style={{ margin: 0 }}>{g.label}</h4>
                <span className="badge backlog" style={{ fontSize: 9, marginLeft: 8 }}>
                  {g.items.length}
                </span>
              </div>
              {g.items.map((it) => (
                <InboxRow key={it.id} item={it} onActed={reload} />
              ))}
            </div>
          ))}

      {data?.truncated && (
        <div className="muted" style={{ fontSize: 11 }}>
          Some items were capped to keep this list bounded — the rest are on the{" "}
          <Link to="/work" className="link">Work board</Link>.
        </div>
      )}
    </div>
  );
}
