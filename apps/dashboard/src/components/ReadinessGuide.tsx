import { Link } from "react-router-dom";
import type { ReadinessItem, ReadinessReport } from "../readiness";

// The first-run / operational readiness surface (RELUX_MASTER_PLAN §11 Dashboard
// / §22 Home). A single compact, app-like card — NOT a marketing hero and never
// nested cards. It renders whatever ./readiness derived from live control-plane
// state: honest pass/warn/fail per capability, with an action link to the exact
// page that fixes each one, plus the single clearest first action.
//
// Two modes, decided by ./readiness (no blockers ⇒ ready):
//   - SETUP (a blocker exists): show the checklist prominently so the operator
//     finishes setup, then the first action.
//   - OPERATIONAL (nothing blocks): show a concise one-line summary + the first
//     action, with the full checks tucked behind a native <details> disclosure
//     and any "attention" items surfaced quietly — never a nag.

const ICON: Record<ReadinessItem["status"], string> = {
  done: "✓",
  todo: "✗",
  warn: "!",
  link: "→",
  info: "ℹ",
};

function ItemRow({ item, onRetry }: { item: ReadinessItem; onRetry?: () => void }) {
  return (
    <li className="readiness-item">
      <span className={`readiness-icon ${item.status}`}>{ICON[item.status]}</span>
      <div className="readiness-body">
        <div className="readiness-line">
          {item.linkTo ? (
            <Link to={item.linkTo} className="readiness-label">
              {item.label}
            </Link>
          ) : (
            <span className="readiness-label">{item.label}</span>
          )}
          {/* A failed read's honest fix is to re-run it — wire the row's Retry to
              the page's Refresh handler. */}
          {item.retry && onRetry && (
            <button className="btn ghost sm readiness-retry" onClick={onRetry}>
              Retry
            </button>
          )}
          {item.cta && item.linkTo && (
            <Link to={item.linkTo} className="readiness-cta">
              <button className="btn ghost sm">{item.cta} →</button>
            </Link>
          )}
        </div>
        <div className="readiness-description">{item.description}</div>
      </div>
    </li>
  );
}

export function ReadinessGuide({
  report,
  loading,
  onRefresh,
}: {
  report: ReadinessReport | null;
  loading?: boolean;
  onRefresh?: () => void;
}) {
  ensureStyles();

  const header = (
    <div className="row" style={{ alignItems: "center", marginBottom: 8 }}>
      <h3 style={{ margin: 0 }}>Readiness</h3>
      {report && (
        <span
          className={
            "badge " +
            (report.degraded ? "in_progress" : report.ready ? "done" : "todo")
          }
          style={{ marginLeft: 8 }}
        >
          {report.degraded ? "degraded" : report.ready ? "operational" : "setup needed"}
        </span>
      )}
      <div className="spacer" style={{ flex: 1 }} />
      {onRefresh && (
        <button className="btn ghost sm" onClick={onRefresh} disabled={loading}>
          {loading ? "Loading…" : "Refresh"}
        </button>
      )}
    </div>
  );

  if (!report) {
    return (
      <div className="card readiness">
        {header}
        <div className="muted" style={{ fontSize: 13 }}>
          Checking readiness…
        </div>
      </div>
    );
  }

  const { ready, degraded, items, attention, firstAction, summary } = report;

  return (
    <div className="card readiness">
      {header}

      {degraded ? (
        // One or more reads failed: show the full checklist with the explicit
        // "… unavailable" rows (each with a Retry) — never a faked operational
        // summary built from partial data.
        <>
          <p className="muted" style={{ marginTop: 0, fontSize: 13 }}>
            Some readiness data could not be read. Showing what is available —
            retry to refresh.
          </p>
          <ul className="readiness-list">
            {items.map((item) => (
              <ItemRow key={item.id} item={item} onRetry={onRefresh} />
            ))}
          </ul>
        </>
      ) : ready ? (
        <>
          <p className="readiness-summary">
            <span className="readiness-icon done" style={{ marginRight: 8 }}>
              ✓
            </span>
            Set up — {summary}
          </p>

          {attention.length > 0 && (
            <ul className="readiness-list">
              {attention.map((item) => (
                <ItemRow key={item.id} item={item} onRetry={onRefresh} />
              ))}
            </ul>
          )}

          <details className="readiness-details">
            <summary>All checks ({items.length})</summary>
            <ul className="readiness-list">
              {items.map((item) => (
                <ItemRow key={item.id} item={item} onRetry={onRefresh} />
              ))}
            </ul>
          </details>
        </>
      ) : (
        <>
          <p className="muted" style={{ marginTop: 0, fontSize: 13 }}>
            Finish these steps to get Relux working end-to-end.
          </p>
          <ul className="readiness-list">
            {items.map((item) => (
              <ItemRow key={item.id} item={item} onRetry={onRefresh} />
            ))}
          </ul>
        </>
      )}

      <div className="row wrap" style={{ gap: 8, marginTop: 10, alignItems: "center" }}>
        <span className="muted" style={{ fontSize: 12 }}>
          First action:
        </span>
        <Link to={firstAction.linkTo}>
          <button className="btn sm">{firstAction.label} →</button>
        </Link>
      </div>
    </div>
  );
}

// Inject the readiness styles once (idempotent), matching the B&W, restrained
// aesthetic of the dashboard. Status color is reserved for meaning only.
let injected = false;
function ensureStyles() {
  if (injected || typeof document === "undefined") return;
  injected = true;
  const el = document.createElement("style");
  el.innerText = `
  .readiness-summary { margin: 0 0 8px; font-size: 13px; display: flex; align-items: center; }
  .readiness-list { list-style: none; padding: 0; margin: 0; }
  .readiness-item { display: flex; align-items: flex-start; gap: 8px; margin-bottom: 8px; }
  .readiness-icon {
    flex: 0 0 18px; width: 18px; height: 18px; border-radius: 50%;
    display: inline-flex; justify-content: center; align-items: center;
    font-size: 11px; font-weight: bold; line-height: 1;
    border: 1px solid var(--border-color, #d0d0d0); color: var(--text-muted, #666);
  }
  .readiness-icon.done { background: var(--green-600, #1a7f37); border-color: transparent; color: #fff; }
  .readiness-icon.todo { background: var(--red-600, #b42318); border-color: transparent; color: #fff; }
  .readiness-icon.warn { background: var(--yellow-600, #b54708); border-color: transparent; color: #fff; }
  .readiness-icon.link { border-color: var(--border-color, #d0d0d0); color: var(--text-muted, #666); }
  .readiness-icon.info { border-color: var(--border-color, #d0d0d0); color: var(--text-muted, #666); }
  .readiness-body { flex: 1 1 auto; min-width: 0; }
  .readiness-line { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  .readiness-label { font-weight: 600; font-size: 13px; }
  .readiness-cta button { padding: 1px 8px; }
  .readiness-retry { padding: 1px 8px; }
  .readiness-description { color: var(--text-muted, #666); font-size: 12px; line-height: 1.5; margin-top: 2px; }
  .readiness-details { margin-top: 4px; }
  .readiness-details > summary { cursor: pointer; font-size: 12px; color: var(--text-muted, #666); margin-bottom: 8px; }
  `;
  document.head.appendChild(el);
}
