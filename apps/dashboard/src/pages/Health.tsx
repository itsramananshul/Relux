import { useEffect, useState } from "react";
import { api } from "../api";

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

export function Health() {
  const [healthData, setHealthData] = useState<HealthResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const fetchHealth = async () => {
      try {
        const data = await api.get<HealthResponse>("/v1/relux/health");
        setHealthData(data);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setLoading(false);
      }
    };
    fetchHealth();
  }, []);

  if (loading) {
    return <div className="loading">Loading health status...</div>;
  }

  if (error) {
    return <div className="banner err">Error: {error}</div>;
  }

  if (!healthData) {
    return <div className="empty">No health data available.</div>;
  }

  const statusColor = healthData.ok ? "green" : "red";

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
          Dashboard bundle: <span style={{ color: statusColor }}>{healthData.dashboard_bundle_present ? "present" : "missing"}</span>
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
