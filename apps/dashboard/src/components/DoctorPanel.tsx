import { useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { reluxDoctor, type ReluxDoctorReport } from "../api";
import {
  severityBadgeClass,
  severityLabel,
  sortChecksBySeverity,
  doctorHeadline,
} from "../doctor";

// The operator **Doctor** panel (relix-dashboard-design.md §15). A compact,
// scan-friendly read-only diagnostics card on the Health page — NOT a hero. It
// renders the kernel's `/v1/relux/doctor` report verbatim: structured severity
// rows with a message and, where the kernel supplied one, a remediation line and
// an in-app action link (Health/Crew/Plugins/Approvals). The kernel does all the
// diagnostic logic; this component invents nothing.
//
// Honest failure: if the doctor read fails it shows the error and a Refresh, not
// a blank panel and never a faked-green report.

// Presentational card, split out (no fetch) so the render tests can drive every
// state — loading / error / ok / warn / fail — directly.
export function DoctorReportCard({
  report,
  loading,
  error,
  onRefresh,
}: {
  report: ReluxDoctorReport | null;
  loading?: boolean;
  error?: string | null;
  onRefresh?: () => void;
}) {
  const header = (
    <div className="row" style={{ alignItems: "center", marginBottom: 8 }}>
      <h3 style={{ margin: 0 }}>Doctor</h3>
      {report && (
        <span
          className={"badge " + severityBadgeClass(report.overall)}
          style={{ marginLeft: 8 }}
        >
          {severityLabel(report.overall)}
        </span>
      )}
      <div className="spacer" style={{ flex: 1 }} />
      {onRefresh && (
        <button className="btn ghost sm" onClick={onRefresh} disabled={loading}>
          {loading ? "Running…" : "Refresh"}
        </button>
      )}
    </div>
  );

  // Honest error — never a blank panel.
  if (error) {
    return (
      <div className="card">
        {header}
        <div className="banner err" style={{ marginBottom: 0 }}>
          Could not run diagnostics ({error}). Retry, or check the control plane is
          running.
        </div>
      </div>
    );
  }

  if (!report) {
    return (
      <div className="card">
        {header}
        <div className="muted" style={{ fontSize: 13 }}>
          {loading ? "Running diagnostics…" : "No diagnostics yet."}
        </div>
      </div>
    );
  }

  const rows = sortChecksBySeverity(report.checks);

  return (
    <div className="card">
      {header}
      <div className="muted" style={{ fontSize: 12, marginBottom: 8 }}>
        Read-only diagnostics — {doctorHeadline(report)}.
      </div>
      <ul className="doctor-list">
        {rows.map((c) => (
          <li key={c.id} className="doctor-item">
            <span className={"badge " + severityBadgeClass(c.severity)}>
              {severityLabel(c.severity)}
            </span>
            <div className="doctor-body">
              <div className="doctor-line">
                <span className="doctor-label">{c.label}</span>
                {c.action_link && (
                  <Link to={c.action_link} className="doctor-cta">
                    <button className="btn ghost sm">Fix →</button>
                  </Link>
                )}
              </div>
              <div className="doctor-message">{c.message}</div>
              {c.remediation && (
                <div className="doctor-remediation">{c.remediation}</div>
              )}
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
}

// Container: fetch the report on mount and on Refresh. Best-effort — a failed
// read becomes the card's honest error state.
export function DoctorPanel() {
  const [report, setReport] = useState<ReluxDoctorReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(() => {
    setLoading(true);
    setError(null);
    void (async () => {
      try {
        setReport(await reluxDoctor.get());
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
        setReport(null);
      } finally {
        setLoading(false);
      }
    })();
  }, []);

  useEffect(() => {
    reload();
  }, [reload]);

  ensureStyles();
  return (
    <DoctorReportCard
      report={report}
      loading={loading}
      error={error}
      onRefresh={reload}
    />
  );
}

// Inject the Doctor styles once (idempotent), matching the restrained B&W
// aesthetic and the readiness card's layout vocabulary.
let injected = false;
function ensureStyles() {
  if (injected || typeof document === "undefined") return;
  injected = true;
  const el = document.createElement("style");
  el.innerText = `
  .doctor-list { list-style: none; padding: 0; margin: 0; }
  .doctor-item { display: flex; align-items: flex-start; gap: 8px; margin-bottom: 10px; }
  .doctor-item .badge { flex: 0 0 auto; margin-top: 1px; }
  .doctor-body { flex: 1 1 auto; min-width: 0; }
  .doctor-line { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  .doctor-label { font-weight: 600; font-size: 13px; }
  .doctor-cta button { padding: 1px 8px; }
  .doctor-message { color: var(--text-muted, #666); font-size: 12px; line-height: 1.5; margin-top: 2px; }
  .doctor-remediation { font-size: 12px; line-height: 1.5; margin-top: 2px; }
  `;
  document.head.appendChild(el);
}
