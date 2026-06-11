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
import {
  isElevatedPermission,
  permissionInvalidReason,
} from "../governance";
import { parseSkillsInput, formatSkillsInput } from "../skills";

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
                  <GovernanceSection
                    agentId={agent.id}
                    permissions={agent.permissions ?? []}
                    onChanged={afterChange}
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
                  <SkillChips skills={agent.skills ?? []} />
                  <PermissionsList permissions={agent.permissions ?? []} />
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
  const [skillsText, setSkillsText] = useState(formatSkillsInput(agent?.skills));
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
      // Parsed, deduped, bounded slug list (the backend re-validates and is authoritative).
      const skills = parseSkillsInput(skillsText);
      if (mode === "create") {
        const body: ReluxAgentConfig = { name, role, persona, skills };
        if (id.trim()) body.id = id.trim();
        if (adapter) body.adapter_plugin = adapter;
        await reluxWork.createAgent(body);
        // Reset the create form for the next member.
        setName("");
        setId("");
        setRole("");
        setPersona("");
        setSkillsText("");
        setAdapter("");
      } else if (agent) {
        // Send every field so an empty value is a deliberate clear (persona/skills) or
        // keeps the current value. The backend leaves absent fields unchanged; a present
        // (possibly empty) skills array REPLACES the whole list.
        const body: ReluxAgentConfig = { name, role, persona, status, skills };
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
        <label htmlFor={`${idPrefix}-skills`}>Skills / Tags (comma-separated, optional):</label>
        <input
          id={`${idPrefix}-skills`}
          type="text"
          value={skillsText}
          onChange={(e) => setSkillsText(e.target.value)}
          placeholder="e.g. research, rust, frontend"
        />
        <p className="muted" style={{ fontSize: 11, marginTop: 2 }}>
          Specialties used to route work to a specialist. Each becomes a short slug
          (lowercase, hyphenated); duplicates and invalid entries are dropped server-side.
        </p>
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

// Compact skill/tag chips for a crew card. Skills are bounded slugs used to route work
// to a specialist during assignment matching; rendered as small muted chips (color is
// reserved for meaning, so these stay monochrome). An agent with no skills shows nothing.
function SkillChips({ skills }: { skills: string[] }) {
  if (skills.length === 0) {
    return (
      <p>
        <strong>Skills:</strong> none
      </p>
    );
  }
  return (
    <div style={{ marginBottom: 8 }}>
      <strong style={{ fontSize: 13 }}>Skills:</strong>{" "}
      <span style={{ display: "inline-flex", flexWrap: "wrap", gap: 4, verticalAlign: "middle" }}>
        {skills.map((s) => (
          <span key={s} className="badge mono skill-chip" style={{ fontSize: 11 }}>
            {s}
          </span>
        ))}
      </span>
    </div>
  );
}

// Compact, read-only explicit-permission display for a crew card. Least privilege:
// this is the agent's full effective power (there are no implicit capabilities), so a
// short list reads honestly. An elevated (control-plane) grant is flagged so the
// operator can see it at a glance.
function PermissionsList({ permissions }: { permissions: string[] }) {
  if (permissions.length === 0) {
    return (
      <p>
        <strong>Permissions:</strong> none (least privilege)
      </p>
    );
  }
  return (
    <div>
      <p style={{ marginBottom: 4 }}>
        <strong>Permissions ({permissions.length}):</strong>
      </p>
      <ul className="perm-list" style={{ margin: 0, paddingLeft: 16, fontSize: 12 }}>
        {permissions.map((p) => (
          <li key={p} className="mono" style={{ listStyle: "disc" }}>
            {p}
            {isElevatedPermission(p) && (
              <span className="badge warn" style={{ marginLeft: 6 }}>
                elevated
              </span>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

// The compact Governance section on the edit card: view/grant/revoke an agent's
// EXPLICIT permissions. The operator console is the human approval (the same gate as
// clicking the button), so grant/revoke act immediately and are audited by the kernel
// — this is made explicit in the copy. Dangerous (control-plane) grants require a
// deliberate confirm; nothing dangerous is ever auto-granted (the create form grants
// only the minimal echo tool). docs/relix-dashboard-design.md §9 / §9.1.
function GovernanceSection({
  agentId,
  permissions,
  onChanged,
}: {
  agentId: string;
  permissions: string[];
  onChanged: () => void;
}) {
  const [newPerm, setNewPerm] = useState("");
  const [busy, setBusy] = useState<string | null>(null); // the permission currently mutating
  const [error, setError] = useState<string | null>(null);

  const invalidReason = newPerm.trim() ? permissionInvalidReason(newPerm) : null;

  async function grant() {
    const perm = newPerm.trim();
    const reason = permissionInvalidReason(perm);
    if (reason) {
      setError(reason);
      return;
    }
    if (
      isElevatedPermission(perm) &&
      !window.confirm(
        `"${perm}" is an elevated (control-plane) capability. Grant it to this agent now? ` +
          `This acts immediately as you and is audited.`,
      )
    ) {
      return;
    }
    setBusy(perm);
    setError(null);
    try {
      await reluxWork.grantPermission(agentId, perm);
      setNewPerm("");
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Grant failed.");
    } finally {
      setBusy(null);
    }
  }

  async function revoke(perm: string) {
    if (
      !window.confirm(
        `Revoke "${perm}" from this agent? It loses that capability immediately (you can re-grant it).`,
      )
    ) {
      return;
    }
    setBusy(perm);
    setError(null);
    try {
      await reluxWork.revokePermission(agentId, perm);
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Revoke failed.");
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="governance-section" style={{ marginTop: 12, borderTop: "1px solid var(--border, #2a2a2a)", paddingTop: 10 }}>
      <h4 style={{ margin: "0 0 4px" }}>Governance — permissions</h4>
      <p className="muted" style={{ fontSize: 12, marginTop: 0 }}>
        Grant/revoke acts immediately as you and is audited. Elevated (control-plane)
        capabilities ask for confirmation. Least privilege: an agent has only what is
        listed here.
      </p>
      {error && <div className="error-message">{error}</div>}
      {permissions.length === 0 ? (
        <p className="muted" style={{ fontSize: 12 }}>No explicit permissions.</p>
      ) : (
        <ul className="perm-list" style={{ margin: "4px 0", paddingLeft: 0, listStyle: "none" }}>
          {permissions.map((p) => (
            <li
              key={p}
              style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 4 }}
            >
              <span className="mono" style={{ fontSize: 12, flex: 1 }}>
                {p}
                {isElevatedPermission(p) && (
                  <span className="badge warn" style={{ marginLeft: 6 }}>elevated</span>
                )}
              </span>
              <button
                type="button"
                className="btn ghost sm"
                onClick={() => void revoke(p)}
                disabled={busy !== null}
              >
                {busy === p ? "…" : "Remove"}
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="form-group" style={{ marginTop: 6 }}>
        <label htmlFor={`gov-${agentId}-add`}>Add permission:</label>
        <div style={{ display: "flex", gap: 8 }}>
          <input
            id={`gov-${agentId}-add`}
            type="text"
            className="mono"
            value={newPerm}
            onChange={(e) => setNewPerm(e.target.value)}
            placeholder="e.g. tool:relux-tools-github:read"
            style={{ flex: 1 }}
          />
          <button
            type="button"
            className="btn sm"
            onClick={() => void grant()}
            disabled={busy !== null || !!invalidReason || !newPerm.trim()}
          >
            {busy === newPerm.trim() ? "…" : "Add"}
          </button>
        </div>
        {invalidReason && (
          <p className="muted" style={{ fontSize: 11, marginTop: 2 }}>{invalidReason}</p>
        )}
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
