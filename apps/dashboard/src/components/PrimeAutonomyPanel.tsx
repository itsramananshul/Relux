import { useState, useEffect } from "react";
import {
  reluxPrimeAutonomy,
  ReluxPrimeAutonomyConfig,
  ReluxPrimeAutonomyTickResult,
  ApiError,
} from "../api";

export function PrimeAutonomyPanel() {
  const [config, setConfig] = useState<ReluxPrimeAutonomyConfig | null>(null);
  const [lastTickResult, setLastTickResult] = useState<ReluxPrimeAutonomyTickResult | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [isUpdating, setIsUpdating] = useState(false);
  const [isTicking, setIsTicking] = useState(false);

  useEffect(() => {
    fetchStatus();
  }, []);

  async function fetchStatus() {
    setLoading(true);
    setError(null);
    try {
      const response = await reluxPrimeAutonomy.getStatus();
      setConfig(response.config);
      setLastTickResult(response.last_tick_result);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to fetch autonomy status");
    } finally {
      setLoading(false);
    }
  }

  async function updateConfig(updates: Partial<ReluxPrimeAutonomyConfig>) {
    setIsUpdating(true);
    setError(null);
    try {
      const updated = await reluxPrimeAutonomy.updateConfig(updates);
      setConfig(updated);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to update autonomy config");
    } finally {
      setIsUpdating(false);
    }
  }

  async function runOneTick() {
    setIsTicking(true);
    setError(null);
    try {
      const result = await reluxPrimeAutonomy.runTick();
      setLastTickResult(result);
      // Re-fetch full status to ensure config (last_tick_at/summary) is updated
      await fetchStatus();
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to run autonomy tick");
    } finally {
      setIsTicking(false);
    }
  }

  if (loading) {
    return <div className="card">Loading Prime Autonomy status...</div>;
  }

  if (error) {
    return <div className="banner err">Error: {error}</div>;
  }

  if (!config) {
    return <div className="card">No Prime Autonomy configuration found.</div>;
  }

  return (
    <div className="card">
      <h3>Prime Autonomy</h3>
      <p>
        <strong>Status:</strong>{" "}
        <span className={`badge ${config.enabled ? "success" : "todo"}`}>
          {config.enabled ? "Enabled" : "Disabled"}
        </span>
      </p>
      <div className="row wrap" style={{ gap: 8, marginTop: 10 }}>
        <button
          className="btn"
          onClick={() => updateConfig({ enabled: !config.enabled })}
          disabled={isUpdating}
        >
          {isUpdating ? "Updating..." : config.enabled ? "Disable" : "Enable"}
        </button>
        <button className="btn" onClick={runOneTick} disabled={isTicking}>
          {isTicking ? "Ticking..." : "Run one tick now"}
        </button>
      </div>

      <div style={{ marginTop: 20 }}>
        <h4>Configuration</h4>
        <div className="row" style={{ alignItems: "center", marginBottom: 10 }}>
          <label htmlFor="interval">Interval (seconds):</label>
          <input
            id="interval"
            type="number"
            value={config.interval_seconds}
            onChange={(e) => updateConfig({ interval_seconds: parseInt(e.target.value) || 5 })}
            min="5"
            className="input"
            style={{ width: 80, marginLeft: 10 }}
            disabled={isUpdating}
          />
        </div>
        <div className="row" style={{ alignItems: "center", marginBottom: 10 }}>
          <label htmlFor="maxTasks">Max Tasks per Tick:</label>
          <input
            id="maxTasks"
            type="number"
            value={config.max_tasks_per_tick}
            onChange={(e) => updateConfig({ max_tasks_per_tick: parseInt(e.target.value) || 1 })}
            min="1"
            max="25"
            className="input"
            style={{ width: 80, marginLeft: 10 }}
            disabled={isUpdating}
          />
        </div>
        <div className="row" style={{ alignItems: "center", marginBottom: 10 }}>
          <input
            id="autoAssign"
            type="checkbox"
            checked={config.auto_assign_unassigned}
            onChange={(e) => updateConfig({ auto_assign_unassigned: e.target.checked })}
            disabled={isUpdating}
          />
          <label htmlFor="autoAssign" style={{ marginLeft: 10 }}>
            Auto-assign unassigned tasks
          </label>
        </div>
      </div>

      {lastTickResult && (
        <div style={{ marginTop: 20 }}>
          <h4>Last Tick Result</h4>
          <p>
            <strong>Time:</strong> {new Date(lastTickResult.tick_at).toLocaleString()}
          </p>
          <p>
            <strong>Summary:</strong> {lastTickResult.summary}
          </p>
          <p>
            <strong>Tasks Run:</strong> {lastTickResult.tasks_run}
          </p>
          <p>
            <strong>Tasks Assigned:</strong> {lastTickResult.tasks_assigned}
          </p>
          {lastTickResult.skipped_reasons && lastTickResult.skipped_reasons.length > 0 && (
            <div>
              <strong>Skipped Reasons:</strong>
              <ul>
                {lastTickResult.skipped_reasons.map((reason, i) => (
                  <li key={i}>{reason}</li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}

      {config.last_tick_at && !lastTickResult && (
        <div style={{ marginTop: 20 }}>
          <h4>Last Known Tick State</h4>
          <p>
            <strong>Time:</strong> {new Date(config.last_tick_at).toLocaleString()}
          </p>
          <p>
            <strong>Summary:</strong> {config.last_tick_summary || "No summary available."}
          </p>        </div>
      )}
    </div>
  );
}
