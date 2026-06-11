import { useState, useMemo } from "react";
import { Link } from "react-router-dom";
import {
  postJson,
  reluxWork,
  reluxAdapters,
  type ReluxAgent,
  type ReluxTask,
  type ReluxAdapterStatus,
} from "../api";
import { useAsync } from "../components/common";

type Agent = ReluxAgent;

// NOTE: this page deliberately does NOT use react-router's `useLoaderData()`.
// The SPA mounts under a plain <BrowserRouter> (a declarative router, not a data
// router built with createBrowserRouter), so calling `useLoaderData()` here threw
// "useLoaderData must be used within a data router" on mount — an uncaught render
// error that white-screened the whole /crew route (the reported blank page).
// Crew loads its own data through the same `useAsync` hook every other Relux page
// uses, so it renders a real view (loading / error / empty / list) regardless of
// router wiring or API state.

export function Crew() {
  const {
    data: agentsData,
    loading: agentsLoading,
    error: agentsError,
    reload: reloadAgents,
  } = useAsync<Agent[]>(() => reluxWork.listAgents(), []);
  const agents = agentsData ?? [];

  const [name, setName] = useState("");
  const [role, setRole] = useState("");
  const [createError, setCreateError] = useState<string | null>(null);

  const { data: tasks, error: tasksError, reload: reloadTasks } = useAsync<ReluxTask[]>(
    () => reluxWork.listTasks(),
    [],
  );

  const agentTaskCounts = useMemo(() => {
    const counts: Record<string, { queued: number; running: number }> = {};
    if (tasks) {
      for (const task of tasks) {
        if (task.assigned_agent) {
          if (!counts[task.assigned_agent]) {
            counts[task.assigned_agent] = { queued: 0, running: 0 };
          }
          if (task.status === "queued") {
            counts[task.assigned_agent].queued++;
          } else if (task.status === "running") {
            counts[task.assigned_agent].running++;
          }
        }
      }
    }
    return counts;
  }, [tasks]);

  const handleCreateAgent = async (e: React.FormEvent) => {
    e.preventDefault();
    setCreateError(null);
    try {
      await postJson<Agent>("/v1/relux/agents", { name, role });
      setName("");
      setRole("");
      reloadAgents(); // refresh the roster so the new member shows
      reloadTasks(); // and the per-agent task counts
    } catch (err) {
      console.error("Failed to create agent:", err);
      setCreateError(err instanceof Error ? err.message : "Failed to create agent.");
    }
  };

  // A created_at that isn't a parseable date must not throw inside render.
  const createdLabel = (raw: string): string => {
    const d = new Date(raw);
    return Number.isNaN(d.getTime()) ? raw || "—" : d.toLocaleString();
  };

  return (
    <div className="crew-page">
      <div className="section">
        <h2>Your Crew</h2>
        {agentsError && (
          <div className="error-message">
            Could not load your crew: {String(agentsError)}{" "}
            <button className="btn ghost sm" onClick={() => reloadAgents()}>
              Retry
            </button>
          </div>
        )}
        {tasksError && (
          <div className="error-message">
            Error loading task counts: {String(tasksError)}
          </div>
        )}
        <div className="agent-list">
          {agentsLoading && !agentsData ? (
            <p>Loading your crew&hellip;</p>
          ) : agents.length === 0 ? (
            <p>
              {agentsError
                ? "Crew unavailable — fix the error above and retry."
                : "No agents yet. Create one below to get started."}
            </p>
          ) : (
            agents.map((agent) => (
              <div key={agent.id} className="agent-card">
                <h3>{agent.name} ({agent.id})</h3>
                <p><strong>Role:</strong> {agent.description || "N/A"}</p>
                <p><strong>Status:</strong> {agent.status || "—"}</p>
                <p><strong>Adapter:</strong> {agent.adapter_plugin || "—"}</p>
                <p><strong>Permissions:</strong> {agent.permissions_summary || "N/A"}</p>
                <p>
                  <strong>Queued Tasks:</strong>{" "}
                  <Link to={`/work?agentId=${agent.id}&status=queued`} className="link">
                    {agentTaskCounts[agent.id]?.queued || 0}
                  </Link>
                </p>
                <p>
                  <strong>Running Tasks:</strong>{" "}
                  <Link to={`/work?agentId=${agent.id}&status=running`} className="link">
                    {agentTaskCounts[agent.id]?.running || 0}
                  </Link>
                </p>
                <p className="created-at">Created: {createdLabel(agent.created_at)}</p>
              </div>
            ))
          )}
        </div>
      </div>

      <AdaptersSection />

      <div className="section">
        <h2>Create New Crew Member</h2>
        {createError && <div className="error-message">{createError}</div>}
        <form onSubmit={handleCreateAgent} className="create-agent-form">
          <div className="form-group">
            <label htmlFor="agent-name">Name:</label>
            <input
              id="agent-name"
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              required
            />
          </div>
          <div className="form-group">
            <label htmlFor="agent-role">Role/Description (optional):</label>
            <input
              id="agent-role"
              type="text"
              value={role}
              onChange={(e) => setRole(e.target.value)}
            />
          </div>
          <button type="submit" className="btn primary">Create Agent</button>
        </form>
      </div>
    </div>
  );
}

// Adapter runtime controls: which adapters can actually run an assigned task.
// CLI adapters are DISABLED BY DEFAULT and spawn a local binary only when an
// operator explicitly enables them. Relux runs them in a non-interactive,
// non-bypass mode and never passes --dangerously-skip-permissions.
function AdaptersSection() {
  const { data: adapters, loading, error, reload } = useAsync<ReluxAdapterStatus[]>(
    () => reluxAdapters.list(),
    [],
  );

  return (
    <div className="section">
      <h2>Adapters</h2>
      <p className="muted" style={{ fontSize: 13, marginTop: -8 }}>
        Adapters decide how an assigned task runs. The real product path is a
        coding-agent CLI: <strong>Claude</strong> or <strong>Codex</strong>. A CLI
        adapter spawns a local binary &mdash; <strong>disabled by default</strong>.
        Enabling one means <em>Relux will run that local CLI when an assigned task
        starts</em>, in a non-interactive, non-bypass mode (it never passes{" "}
        <span className="mono">--dangerously-skip-permissions</span>).
      </p>
      <p className="muted" style={{ fontSize: 13, marginTop: -4 }}>
        <strong>Onboarding:</strong> install and log in to the Claude CLI
        (<span className="mono">claude</span>) or the Codex CLI
        (<span className="mono">codex</span>) so it is on your PATH, then enable the
        matching adapter below. These CLIs use their own local login &mdash; there
        is no API key to paste in Relux for them.
      </p>
      {error && (
        <div className="error-message">Error loading adapters: {String(error)}</div>
      )}
      {loading && !adapters ? (
        <p>Loading adapters&hellip;</p>
      ) : adapters && adapters.length > 0 ? (
        <div className="agent-list">
          {adapters.map((a) => (
            <AdapterCard key={a.plugin_id} adapter={a} onChange={reload} />
          ))}
        </div>
      ) : (
        <p>No adapter plugins installed.</p>
      )}
    </div>
  );
}

function AdapterCard({
  adapter,
  onChange,
}: {
  adapter: ReluxAdapterStatus;
  onChange: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const isLocal = adapter.state === "local_deterministic";

  const stateLabel: Record<ReluxAdapterStatus["state"], string> = {
    local_deterministic: "Local (deterministic)",
    available: "Enabled — ready",
    missing_binary: "Enabled — binary missing",
    disabled: "Configured — disabled",
    needs_configuration: "Disabled (default)",
  };

  async function enable() {
    setBusy(true);
    setError(null);
    try {
      await reluxAdapters.set(adapter.plugin_id, { enabled: true });
      onChange();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Enable failed");
    } finally {
      setBusy(false);
    }
  }

  async function disable() {
    setBusy(true);
    setError(null);
    try {
      await reluxAdapters.set(adapter.plugin_id, { enabled: false });
      onChange();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Disable failed");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="agent-card">
      <h3>{adapter.adapter_name}</h3>
      <p className="mono" style={{ fontSize: 11, opacity: 0.7 }}>{adapter.plugin_id}</p>
      <p><strong>Status:</strong> {stateLabel[adapter.state]}</p>
      {!isLocal && (
        <>
          <p><strong>Kind:</strong> {adapter.kind ?? "—"}</p>
          <p>
            <strong>Binary:</strong> {adapter.command ?? "—"}{" "}
            {adapter.command &&
              (adapter.available_on_path ? "(on PATH)" : "(NOT on PATH)")}
          </p>
          {adapter.timeout_seconds != null && (
            <p><strong>Timeout:</strong> {adapter.timeout_seconds}s</p>
          )}
        </>
      )}
      <p className="muted" style={{ fontSize: 12 }}>{adapter.detail}</p>
      {error && <div className="error-message">{error}</div>}
      {!isLocal && (
        <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
          {adapter.enabled ? (
            <button className="btn ghost sm" onClick={() => void disable()} disabled={busy}>
              {busy ? "…" : "Disable"}
            </button>
          ) : (
            <button className="btn sm" onClick={() => void enable()} disabled={busy}>
              {busy ? "…" : "Enable"}
            </button>
          )}
        </div>
      )}
    </div>
  );
}
