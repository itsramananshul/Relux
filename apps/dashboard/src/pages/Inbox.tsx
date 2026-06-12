import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
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
} from "../inbox";
import { buildInvestigationSeed, stashInvestigationSeed } from "../investigateseed";
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

function InboxRow({ item, onActed }: { item: ReluxInboxItem; onActed: () => void }) {
  const navigate = useNavigate();
  const [action, setAction] = useState<ActionState>({ status: "idle" });
  const [diag, setDiag] = useState<DiagState | null>(null);

  const busyKind = action.status === "busy" ? action.kind : null;

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
        <span style={{ fontWeight: 600, fontSize: 13 }}>{item.title}</span>
        <div className="spacer" style={{ flex: 1 }} />
        <span className="mono muted" style={{ fontSize: 10 }}>{item.id}</span>
      </div>
      <div className="muted" style={{ fontSize: 12, marginTop: 4, lineHeight: 1.5 }}>
        {item.summary}
      </div>
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
  const groups = data ? groupInbox(data.items) : [];

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
        Everything across the Guild that needs you, most urgent first — pending approvals,
        hard-failed runs, blocked work, and paused loops. Transient failures that retry on their
        own never appear here, so this stays signal, not noise. Every action reuses an existing
        control; nothing runs without your click.
      </p>

      {error && (
        <div className="banner err" style={{ fontSize: 12 }}>
          Couldn't load the Inbox: {error}
        </div>
      )}

      {data && data.items.length === 0 && (
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

      {groups.map((g) => (
        <div key={g.kind} className="card" style={{ padding: 12 }}>
          <div className="row" style={{ alignItems: "baseline", marginBottom: 8 }}>
            <h4 style={{ margin: 0 }}>{g.label}</h4>
            <span className="badge backlog" style={{ fontSize: 9, marginLeft: 8 }}>{g.items.length}</span>
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
