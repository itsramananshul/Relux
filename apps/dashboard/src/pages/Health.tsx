import { useEffect, useState } from "react";
import {
  api,
  reluxTools,
  reluxAdapters,
  reluxPrimeAutonomy,
  type ReluxToolDescriptor,
  type ReluxAdapterStatus,
  type ReluxPrimeAutonomyStatusResponse,
} from "../api";
import { PrimeAiSettings } from "../components/PrimeAiSettings";

// Relux Health / diagnostics (RELUX_MASTER_PLAN §11.9, §22). The local
// readiness surface for the standalone product: state counts, plugin/tool/
// adapter status, Prime autonomy status, and the package/check command hints an
// operator runs before cutting a release. Everything here is backed ONLY by the
// local /v1/relux control plane - no Relix web bridge, no login.

interface HealthResponse {
  ok: boolean;
  version: string;
  db_path: string;
  db_ok: boolean;
  dashboard_bundle_present: boolean;
  installed_plugin_count: number;
  agent_count: number;
  task_count: number;
  run_count: number;
  ai_status: {
    mode: string;
    configured: boolean;
    disabled: boolean;
    model: string;
    timeout_ms: number;
    reason: string;
  };
  warnings: string[];
  errors: string[];
}

// The local readiness commands an operator runs before sharing a build. These
// are the documented Relux scripts (RELUX_MASTER_PLAN §22); shown verbatim so
// the Health page doubles as a release-readiness cheat sheet.
const READINESS_COMMANDS: { label: string; cmd: string }[] = [
  { label: "Kernel health", cmd: "cargo run -p relux-kernel -- health" },
  {
    label: "First-release check",
    cmd: "powershell -NoProfile -ExecutionPolicy Bypass -File scripts\\relux-first-release-check.ps1",
  },
  {
    label: "End-to-end smoke",
    cmd: "powershell -NoProfile -ExecutionPolicy Bypass -File scripts\\relux-e2e-smoke.ps1",
  },
  {
    label: "Package local bundle",
    cmd: "powershell -NoProfile -ExecutionPolicy Bypass -File scripts\\relux-package-local.ps1",
  },
];

export function Health() {
  const [healthData, setHealthData] = useState<HealthResponse | null>(null);
  const [tools, setTools] = useState<ReluxToolDescriptor[] | null>(null);
  const [adapters, setAdapters] = useState<ReluxAdapterStatus[] | null>(null);
  const [autonomy, setAutonomy] = useState<ReluxPrimeAutonomyStatusResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const fetchAll = async () => {
      try {
        // Health is the core readiness signal; the rest are best-effort so a
        // single unavailable surface degrades to a calm "—" rather than blanking
        // the whole page.
        const health = await api.get<HealthResponse>("/v1/relux/health");
        setHealthData(health);
        const [t, a, au] = await Promise.all([
          reluxTools.list().catch(() => null),
          reluxAdapters.list().catch(() => null),
          reluxPrimeAutonomy.getStatus().catch(() => null),
        ]);
        setTools(t);
        setAdapters(a);
        setAutonomy(au);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setLoading(false);
      }
    };
    fetchAll();
  }, []);

  if (loading) {
    return <div className="loading">Loading health status...</div>;
  }

  if (error) {
    return (
      <div className="banner err">
        Could not reach the Relux control plane ({error}). Start it with{" "}
        <span className="mono">cargo run -p relux-kernel -- serve</span>, then refresh.
      </div>
    );
  }

  if (!healthData) {
    return <div className="empty">No health data available.</div>;
  }

  const readyTools = (tools ?? []).filter((t) => t.executable === "ready").length;
  const enabledAdapters = (adapters ?? []).filter(
    (a) => a.state === "available" || a.state === "local_deterministic",
  ).length;
  const autonomyCfg = autonomy?.config;

  return (
    <div className="grid">
      <div className="card">
        <h3>Relux Health Status</h3>
        <div className="row" style={{ gap: 8, alignItems: "center", marginBottom: 8 }}>
          <span className={"badge " + (healthData.ok ? "done" : "blocked")}>
            {healthData.ok ? "OK" : "FAIL"}
          </span>
          <span className="muted">Version {healthData.version}</span>
        </div>
        <div className="muted mono" style={{ fontSize: 12, wordBreak: "break-all" }}>
          {healthData.db_path} ({healthData.db_ok ? "OK" : "FAIL"})
        </div>
        <div className="muted" style={{ marginTop: 8 }}>
          Dashboard bundle:{" "}
          <span className={"badge " + (healthData.dashboard_bundle_present ? "done" : "blocked")}>
            {healthData.dashboard_bundle_present ? "present" : "missing"}
          </span>
        </div>
      </div>

      <div className="card">
        <h3>Counts</h3>
        <div className="kpi-grid">
          <div><strong>{healthData.installed_plugin_count}</strong><span>Plugins</span></div>
          <div><strong>{healthData.agent_count}</strong><span>Agents</span></div>
          <div><strong>{healthData.task_count}</strong><span>Tasks</span></div>
          <div><strong>{healthData.run_count}</strong><span>Runs</span></div>
        </div>
      </div>

      <div className="card">
        <h3>Tools</h3>
        {tools == null ? (
          <div className="muted">—</div>
        ) : tools.length === 0 ? (
          <div className="empty">No tools discovered.</div>
        ) : (
          <>
            <div className="muted" style={{ fontSize: 12, marginBottom: 8 }}>
              {readyTools} of {tools.length} ready
            </div>
            <div className="table-scroll">
              <table className="table">
                <tbody>
                  {tools.map((t) => (
                    <tr key={`${t.plugin_id}/${t.tool_name}`}>
                      <td className="mono" style={{ fontSize: 12 }}>{t.tool_name}</td>
                      <td>
                        <span className={"badge " + (t.executable === "ready" ? "done" : "backlog")}>
                          {t.executable}
                        </span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </>
        )}
      </div>

      <div className="card">
        <h3>Adapters</h3>
        {adapters == null ? (
          <div className="muted">—</div>
        ) : adapters.length === 0 ? (
          <div className="empty">No adapters installed.</div>
        ) : (
          <>
            <div className="muted" style={{ fontSize: 12, marginBottom: 8 }}>
              {enabledAdapters} of {adapters.length} runnable
            </div>
            <div className="table-scroll">
              <table className="table">
                <tbody>
                  {adapters.map((a) => (
                    <tr key={a.plugin_id}>
                      <td className="mono" style={{ fontSize: 12 }}>{a.adapter_name}</td>
                      <td>
                        <span
                          className={
                            "badge " +
                            (a.state === "available" || a.state === "local_deterministic"
                              ? "done"
                              : a.state === "missing_binary"
                                ? "blocked"
                                : "backlog")
                          }
                        >
                          {a.state}
                        </span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </>
        )}
      </div>

      <div className="card">
        <h3>Prime Autonomy</h3>
        {autonomyCfg == null ? (
          <div className="muted">—</div>
        ) : (
          <div className="table-scroll">
            <table className="table">
              <tbody>
                <tr>
                  <td>Status</td>
                  <td>
                    <span className={"badge " + (autonomyCfg.enabled ? "done" : "backlog")}>
                      {autonomyCfg.enabled ? "enabled" : "disabled"}
                    </span>
                  </td>
                </tr>
                <tr><td>Interval</td><td>{autonomyCfg.interval_seconds}s</td></tr>
                <tr><td>Max tasks / tick</td><td>{autonomyCfg.max_tasks_per_tick}</td></tr>
                <tr><td>Auto-assign</td><td>{autonomyCfg.auto_assign_unassigned ? "Yes" : "No"}</td></tr>
                <tr><td>Last tick</td><td>{autonomyCfg.last_tick_at ?? "never"}</td></tr>
                <tr><td>Last summary</td><td>{autonomyCfg.last_tick_summary ?? "—"}</td></tr>
              </tbody>
            </table>
          </div>
        )}
      </div>

      <PrimeAiSettings />

      <div className="card">
        <h3>AI Status</h3>
        <div className="table-scroll">
          <table className="table">
            <tbody>
              <tr><td>Mode</td><td>{healthData.ai_status.mode}</td></tr>
              <tr><td>Configured</td><td>{healthData.ai_status.configured ? "Yes" : "No"}</td></tr>
              <tr><td>Disabled</td><td>{healthData.ai_status.disabled ? "Yes" : "No"}</td></tr>
              <tr><td>Model</td><td>{healthData.ai_status.model}</td></tr>
              <tr><td>Timeout</td><td>{healthData.ai_status.timeout_ms} ms</td></tr>
              <tr><td>Reason</td><td>{healthData.ai_status.reason}</td></tr>
            </tbody>
          </table>
        </div>
      </div>

      <div className="card">
        <h3>Release readiness</h3>
        <div className="grid" style={{ gap: 8 }}>
          {READINESS_COMMANDS.map((c) => (
            <div key={c.label} className="kv">
              <span className="muted" style={{ fontSize: 12 }}>{c.label}</span>
              <span className="mono" style={{ fontSize: 12, wordBreak: "break-all" }}>{c.cmd}</span>
            </div>
          ))}
        </div>
      </div>

      {healthData.warnings.length > 0 && (
        <div className="card">
          <h3>Warnings</h3>
          <ul>
            {healthData.warnings.map((warn, index) => (
              <li key={index}>{warn}</li>
            ))}
          </ul>
        </div>
      )}

      {healthData.errors.length > 0 && (
        <div className="card">
          <h3>Errors</h3>
          <ul>
            {healthData.errors.map((err, index) => (
              <li key={index}>{err}</li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
