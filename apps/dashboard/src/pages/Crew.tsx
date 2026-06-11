import { useState, useMemo } from "react";
import { Link } from "react-router-dom";
import {
  reluxWork,
  reluxAdapters,
  type ReluxAgent,
  type ReluxAgentConfig,
  type ReluxTask,
  type ReluxAdapterStatus,
} from "../api";
import { useAsync } from "../components/common";
import { ADAPTER_STATE_LABEL } from "../plugins";

type Agent = ReluxAgent;

// NOTE: this page deliberately does NOT use react-router's `useLoaderData()`.
// The SPA mounts under a plain <BrowserRouter> (a declarative router, not a data
// router built with createBrowserRouter), so calling `useLoaderData()` here threw
// "useLoaderData must be used within a data router" on mount — an uncaught render
// error that white-screened the whole /crew route (the reported blank page).
// Crew loads its own data through the same `useAsync` hook every other Relux page
// uses, so it renders a real view (loading / error / empty / list) regardless of
// router wiring or API state.

// The statuses an operator may set on a crew member. Machine-driven states (Error)
// are not offered — they flow from the run lifecycle, not manual config. Matches the
// backend allowlist in crates/relux-kernel/src/agent_config.rs.
const SETTABLE_STATUSES = ["active", "paused", "disabled"] as const;

export function Crew() {
  const {
    data: agentsData,
    loading: agentsLoading,
    error: agentsError,
    reload: reloadAgents,
  } = useAsync<Agent[]>(() => reluxWork.listAgents(), []);
  const agents = agentsData ?? [];

  // The adapter roster powers the adapter picker in the create/edit form (the
  // allowlist a chosen adapter must resolve to, mirrored from the backend).
  const { data: adaptersData } = useAsync<ReluxAdapterStatus[]>(
    () => reluxAdapters.list(),
    [],
  );
  const adapters = adaptersData ?? [];

  const [editingId, setEditingId] = useState<string | null>(null);

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

  const afterChange = () => {
    reloadAgents(); // refresh the roster so the edit/new member shows
    reloadTasks(); // and the per-agent task counts
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
            agents.map((agent) =>
              editingId === agent.id ? (
                <div key={agent.id} className="agent-card">
                  <h3>Edit {agent.name} ({agent.id})</h3>
                  <CrewMemberForm
                    mode="edit"
                    agent={agent}
                    adapters={adapters}
                    onSaved={() => {
                      setEditingId(null);
                      afterChange();
                    }}
                    onCancel={() => setEditingId(null)}
                  />
                </div>
              ) : (
                <div key={agent.id} className="agent-card">
                  <h3>{agent.name} ({agent.id})</h3>
                  <p><strong>Role:</strong> {agent.description || "N/A"}</p>
                  {agent.persona && (
                    <p style={{ fontStyle: "italic" }}>
                      <strong>Persona:</strong> {agent.persona}
                    </p>
                  )}
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
                  <div style={{ marginTop: 8 }}>
                    <button className="btn ghost sm" onClick={() => setEditingId(agent.id)}>
                      Edit
                    </button>
                  </div>
                </div>
              ),
            )
          )}
        </div>
      </div>

      <AdaptersSection />

      <div className="section">
        <h2>Create New Crew Member</h2>
        <p className="muted" style={{ fontSize: 13, marginTop: -8 }}>
          Configure an operative directly: a display name, what it does, an optional
          persona (operating style), and which adapter runs its work. The id is derived
          from the name when you leave it blank.
        </p>
        <CrewMemberForm mode="create" adapters={adapters} onSaved={afterChange} />
      </div>
    </div>
  );
}

// Shared create/edit form for a crew member. In `create` mode it posts a new agent;
// in `edit` mode it patches the given agent. Validation errors from the backend are
// surfaced verbatim (honest 400s: duplicate id/name, unknown adapter, bad status).
function CrewMemberForm({
  mode,
  agent,
  adapters,
  onSaved,
  onCancel,
}: {
  mode: "create" | "edit";
  agent?: ReluxAgent;
  adapters: ReluxAdapterStatus[];
  onSaved: () => void;
  onCancel?: () => void;
}) {
  const [name, setName] = useState(agent?.name ?? "");
  const [id, setId] = useState("");
  const [role, setRole] = useState(agent?.description ?? "");
  const [persona, setPersona] = useState(agent?.persona ?? "");
  const [adapter, setAdapter] = useState(agent?.adapter_plugin ?? "");
  const [status, setStatus] = useState((agent?.status ?? "active").toLowerCase());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const idPrefix = mode === "edit" && agent ? `edit-${agent.id}` : "create";

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      if (mode === "create") {
        const body: ReluxAgentConfig = { name, role, persona };
        if (id.trim()) body.id = id.trim();
        if (adapter) body.adapter_plugin = adapter;
        await reluxWork.createAgent(body);
        // Reset the create form for the next member.
        setName("");
        setId("");
        setRole("");
        setPersona("");
        setAdapter("");
      } else if (agent) {
        // Send every field so an empty value is a deliberate clear (persona) or
        // keeps the current value. The backend leaves absent fields unchanged.
        const body: ReluxAgentConfig = { name, role, persona, status };
        if (adapter) body.adapter_plugin = adapter;
        await reluxWork.updateAgent(agent.id, body);
      }
      onSaved();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Save failed.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={submit} className="create-agent-form">
      {error && <div className="error-message">{error}</div>}
      <div className="form-group">
        <label htmlFor={`${idPrefix}-name`}>Name:</label>
        <input
          id={`${idPrefix}-name`}
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          required
        />
      </div>
      {mode === "create" && (
        <div className="form-group">
          <label htmlFor={`${idPrefix}-id`}>Id (optional, derived from name):</label>
          <input
            id={`${idPrefix}-id`}
            type="text"
            value={id}
            onChange={(e) => setId(e.target.value)}
            placeholder="e.g. research-bot"
          />
        </div>
      )}
      <div className="form-group">
        <label htmlFor={`${idPrefix}-role`}>Role / Description:</label>
        <input
          id={`${idPrefix}-role`}
          type="text"
          value={role}
          onChange={(e) => setRole(e.target.value)}
        />
      </div>
      <div className="form-group">
        <label htmlFor={`${idPrefix}-persona`}>Persona (operating style, optional):</label>
        <textarea
          id={`${idPrefix}-persona`}
          value={persona}
          rows={3}
          onChange={(e) => setPersona(e.target.value)}
          placeholder="e.g. Methodical and concise; cites sources; asks before risky steps."
        />
      </div>
      <div className="form-group">
        <label htmlFor={`${idPrefix}-adapter`}>Adapter / Runtime:</label>
        <select
          id={`${idPrefix}-adapter`}
          value={adapter}
          onChange={(e) => setAdapter(e.target.value)}
        >
          <option value="">Default (local Prime)</option>
          {adapters.map((a) => (
            <option key={a.plugin_id} value={a.plugin_id}>
              {a.adapter_name} — {ADAPTER_STATE_LABEL[a.state]}
            </option>
          ))}
        </select>
      </div>
      {mode === "edit" && (
        <div className="form-group">
          <label htmlFor={`${idPrefix}-status`}>Status:</label>
          <select
            id={`${idPrefix}-status`}
            value={SETTABLE_STATUSES.includes(status as (typeof SETTABLE_STATUSES)[number]) ? status : "active"}
            onChange={(e) => setStatus(e.target.value)}
          >
            {SETTABLE_STATUSES.map((s) => (
              <option key={s} value={s}>
                {s.charAt(0).toUpperCase() + s.slice(1)}
              </option>
            ))}
          </select>
        </div>
      )}
      <div style={{ display: "flex", gap: 8 }}>
        <button type="submit" className="btn primary" disabled={busy}>
          {busy ? "Saving…" : mode === "create" ? "Create Agent" : "Save Changes"}
        </button>
        {onCancel && (
          <button type="button" className="btn ghost" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
        )}
      </div>
    </form>
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

  // Shared with the Plugins page (single source of truth) so the two adapter
  // surfaces never disagree on what each runtime state is called.
  const stateLabel = ADAPTER_STATE_LABEL;

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
