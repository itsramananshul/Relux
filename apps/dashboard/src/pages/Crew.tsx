import { useState, useMemo } from "react";
import { Link } from "react-router-dom";
import {
  reluxWork,
  reluxAdapters,
  agentSelfAssignTask,
  agentSelfManagerGrant,
  agentSelfManagerRevoke,
  type ReluxAgent,
  type ReluxAgentConfig,
  type ReluxAgentPermissions,
  type ReluxAgentPreset,
  type ReluxAgentTokenMeta,
  type ReluxMintedAgentToken,
  type ReluxTask,
  type ReluxAdapterStatus,
} from "../api";
import { useAsync } from "../components/common";
import { ADAPTER_STATE_LABEL } from "../plugins";
import {
  isElevatedPermission,
  isManagerSubtree,
  isScopedWildcard,
  permissionInvalidReason,
  managerGrantAvailability,
  parseTokenTtlSecs,
  assignTaskFormReason,
  managerGrantFormReason,
  managerRevokeFormReason,
  assignTaskCurlSnippet,
  managerGrantCurlSnippet,
  managerRevokeCurlSnippet,
  AGENT_SELF_ASSIGN_TASK_ROUTE,
  AGENT_SELF_MANAGER_GRANT_ROUTE,
  AGENT_SELF_MANAGER_REVOKE_ROUTE,
  type ManagerGrantAgent,
} from "../governance";
import { parseSkillsInput, formatSkillsInput } from "../skills";
import { applyPreset, presetFieldsDirty } from "../presets";
import { managerOptions, leadLabel, directReportsSummary } from "../hierarchy";
import { adapterBrandLabel } from "../prime";

type Agent = ReluxAgent;

// A created_at that isn't a parseable date must not throw inside render — a missing/odd
// value falls back to the raw string (or an em dash). Module-level + pure so the Crew
// member card can be rendered and unit-tested in isolation.
export function createdLabel(raw: string): string {
  const d = new Date(raw);
  return Number.isNaN(d.getTime()) ? raw || "—" : d.toLocaleString();
}

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
                    roster={agents}
                    onSaved={() => {
                      setEditingId(null);
                      afterChange();
                    }}
                    onCancel={() => setEditingId(null)}
                  />
                  <GovernanceSection
                    agentId={agent.id}
                    permissions={agent.permissions ?? []}
                    roster={agents}
                    onChanged={afterChange}
                  />
                </div>
              ) : (
                <CrewMemberCard
                  key={agent.id}
                  agent={agent}
                  queued={agentTaskCounts[agent.id]?.queued || 0}
                  running={agentTaskCounts[agent.id]?.running || 0}
                  onEdit={() => setEditingId(agent.id)}
                />
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
        <CrewMemberForm
          mode="create"
          adapters={adapters}
          roster={agents}
          onSaved={afterChange}
        />
      </div>
    </div>
  );
}

// One read-only crew member card. Renders an operative cleanly REGARDLESS of which
// optional fields the record carries — a missing role/persona/status/adapter/skills/Lead
// falls back to an honest placeholder, never a blank or a throw. The adapter shows a human
// brand (e.g. "Claude") next to its raw plugin id so the runtime is legible the same way
// Prime names it when it hires the operative; permissions read as least-privilege when
// empty (the honest setup hint). Exported so the created-agent render test can mount a
// populated card directly (useAsync never fetches under renderToStaticMarkup).
// docs/relix-dashboard-design.md (Crew); RELUX_MASTER_PLAN §6, §7.3, §8.1.
export function CrewMemberCard({
  agent,
  queued,
  running,
  onEdit,
}: {
  agent: ReluxAgent;
  queued: number;
  running: number;
  onEdit: () => void;
}) {
  const adapterId = agent.adapter_plugin || "";
  return (
    <div className="agent-card">
      <h3>{agent.name} ({agent.id})</h3>
      <p><strong>Role:</strong> {agent.description || "N/A"}</p>
      {agent.persona && (
        <p style={{ fontStyle: "italic" }}>
          <strong>Persona:</strong> {agent.persona}
        </p>
      )}
      <p><strong>Status:</strong> {agent.status || "—"}</p>
      <p>
        <strong>Adapter:</strong>{" "}
        {adapterId ? (
          <>
            {adapterBrandLabel(adapterId)}{" "}
            <span className="mono muted" style={{ fontSize: 11 }}>{adapterId}</span>
          </>
        ) : (
          "—"
        )}
      </p>
      <p>
        <strong>Reports to (Lead):</strong>{" "}
        {agent.reports_to ? (
          <span className="mono">{leadLabel(agent.reports_to, agent.reports_to_name)}</span>
        ) : (
          <span className="muted">none (top-level)</span>
        )}
      </p>
      <p>
        <strong>Direct reports:</strong>{" "}
        {(agent.reports?.length ?? 0) === 0 ? (
          <span className="muted">none</span>
        ) : (
          <span title={(agent.reports ?? []).join(", ")}>
            {directReportsSummary(agent.reports)}
          </span>
        )}
      </p>
      <SkillChips skills={agent.skills ?? []} />
      <PermissionsList permissions={agent.permissions ?? []} />
      <p>
        <strong>Queued Tasks:</strong>{" "}
        <Link to={`/work?agentId=${agent.id}&status=queued`} className="link">
          {queued}
        </Link>
      </p>
      <p>
        <strong>Running Tasks:</strong>{" "}
        <Link to={`/work?agentId=${agent.id}&status=running`} className="link">
          {running}
        </Link>
      </p>
      <p className="created-at">Created: {createdLabel(agent.created_at)}</p>
      <div style={{ marginTop: 8 }}>
        <button className="btn ghost sm" onClick={onEdit}>
          Edit
        </button>
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
  roster,
  onSaved,
  onCancel,
}: {
  mode: "create" | "edit";
  agent?: ReluxAgent;
  adapters: ReluxAdapterStatus[];
  // The live roster, used to populate the "Reports to (Lead)" picker. In edit mode the
  // agent itself and its own Branch (descendants) are excluded so the dropdown can't
  // offer an obvious cycle (the backend re-validates regardless).
  roster: ReluxAgent[];
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
  const [reportsTo, setReportsTo] = useState(agent?.reports_to ?? "");
  const [presetId, setPresetId] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Eligible Leads: every crew member except (in edit mode) this operative and its own
  // Branch. Mirrors crates/relux-core/src/hierarchy.rs — the backend is authoritative.
  const leadChoices = useMemo(
    () => managerOptions(roster, mode === "edit" ? agent?.id : undefined),
    [roster, mode, agent?.id],
  );

  // Curated role presets for the create form (read-only, advisory). Edit mode never
  // offers presets — they seed a NEW member, they don't reshape an existing one.
  const { data: presetsData } = useAsync<ReluxAgentPreset[]>(
    () => (mode === "create" ? reluxWork.listAgentPresets() : Promise.resolve([])),
    [mode],
  );
  const presets = presetsData ?? [];

  const idPrefix = mode === "edit" && agent ? `edit-${agent.id}` : "create";

  // Apply the chosen preset: fill role/persona/skills (still editable). An explicit
  // action — never on render — and it confirms before overwriting fields the operator
  // already typed, so it cannot clobber work unexpectedly. It touches only these three
  // fields; name/id/adapter/status/permissions are never changed by a preset.
  function applyChosenPreset() {
    const preset = presets.find((p) => p.id === presetId);
    if (!preset) return;
    if (
      presetFieldsDirty({ role, persona, skills: skillsText }) &&
      !window.confirm(
        `Apply the "${preset.label}" preset? This replaces the current role, persona, ` +
          `and skills fields (you can still edit them before saving).`,
      )
    ) {
      return;
    }
    const filled = applyPreset(preset);
    setRole(filled.role);
    setPersona(filled.persona);
    setSkillsText(filled.skills);
    setError(null);
  }

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
        if (reportsTo) body.reports_to = reportsTo;
        await reluxWork.createAgent(body);
        // Reset the create form for the next member.
        setName("");
        setId("");
        setRole("");
        setPersona("");
        setSkillsText("");
        setAdapter("");
        setReportsTo("");
        setPresetId("");
      } else if (agent) {
        // Send every field so an empty value is a deliberate clear (persona/skills/Lead)
        // or keeps the current value. The backend leaves absent fields unchanged; a
        // present (possibly empty) skills array REPLACES the whole list, and a present
        // blank `reports_to` CLEARS the Lead (top-level).
        const body: ReluxAgentConfig = { name, role, persona, status, skills, reports_to: reportsTo };
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
      {mode === "create" && presets.length > 0 && (
        <div className="form-group">
          <label htmlFor={`${idPrefix}-preset`}>Role preset (optional):</label>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <select
              id={`${idPrefix}-preset`}
              value={presetId}
              onChange={(e) => setPresetId(e.target.value)}
              style={{ flex: 1 }}
            >
              <option value="">Start from scratch</option>
              {presets.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.label}
                </option>
              ))}
            </select>
            <button
              type="button"
              className="btn sm"
              onClick={applyChosenPreset}
              disabled={!presetId}
            >
              Apply
            </button>
          </div>
          <p className="muted" style={{ fontSize: 11, marginTop: 2 }}>
            {presets.find((p) => p.id === presetId)?.summary ??
              "Fills role, persona, and skills with a common crew type — still editable, and grants no permissions."}
          </p>
        </div>
      )}
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
      <div className="form-group">
        <label htmlFor={`${idPrefix}-reports-to`}>Reports to (Lead, optional):</label>
        <select
          id={`${idPrefix}-reports-to`}
          value={reportsTo}
          onChange={(e) => setReportsTo(e.target.value)}
        >
          <option value="">None (top-level)</option>
          {leadChoices.map((a) => (
            <option key={a.id} value={a.id}>
              {a.name} ({a.id})
            </option>
          ))}
        </select>
        <p className="muted" style={{ fontSize: 11, marginTop: 2 }}>
          The operative this one escalates to. An operative cannot report to itself or
          into its own branch (those are excluded above); the server re-validates and
          rejects any reporting cycle. This sets chain-of-command only — it does NOT widen
          any permission.
        </p>
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
            {isScopedWildcard(p) && (
              <span className="badge" style={{ marginLeft: 6 }}>
                scope: all tools in plugin
              </span>
            )}
            {isManagerSubtree(p) && (
              <span className="badge" style={{ marginLeft: 6 }}>
                scope: manager subtree
              </span>
            )}
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
  roster,
  onChanged,
}: {
  agentId: string;
  permissions: string[];
  roster: Agent[];
  onChanged: () => void;
}) {
  const [newPerm, setNewPerm] = useState("");
  const [busy, setBusy] = useState<string | null>(null); // the permission currently mutating
  const [error, setError] = useState<string | null>(null);

  const invalidReason = newPerm.trim() ? permissionInvalidReason(newPerm) : null;

  // Whether to offer the operator-assisted "Grant as manager" affordance for THIS agent
  // (it holds a live manager-subtree scope over a non-empty Branch). Mirrors the backend
  // authority gate so the affordance never appears when the kernel would only 403.
  const thisAgent = useMemo<ManagerGrantAgent>(
    () => ({
      id: agentId,
      status: roster.find((a) => a.id === agentId)?.status,
      permissions,
      reports_to: roster.find((a) => a.id === agentId)?.reports_to,
    }),
    [agentId, permissions, roster],
  );
  const managerGrant = useMemo(
    () => managerGrantAvailability(thisAgent, roster as ManagerGrantAgent[]),
    [thisAgent, roster],
  );

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
        listed here. A grant is an exact capability (e.g.{" "}
        <span className="mono">tool:relux-tools-github:create_pr</span>) or a single
        plugin scope (<span className="mono">tool:&lt;plugin-id&gt;:*</span>) that
        authorizes every tool in that plugin. No global or partial wildcards. Revoke
        removes exactly the row you grant — a scope and its tools are separate rows.
      </p>
      <p className="muted" style={{ fontSize: 12, marginTop: 4 }}>
        <strong>Advanced — manager scope.</strong> A manager-subtree grant{" "}
        (<span className="mono">agent:&lt;manager-id&gt;:subtree:&lt;action&gt;</span>,
        e.g. <span className="mono">agent:lead-1:subtree:grant_permission</span>) lets a
        live manager perform <span className="mono">&lt;action&gt;</span> on operatives
        inside its OWN Branch (its <span className="mono">reports_to</span> subtree) — never
        siblings, its own managers, or itself. Granted to that manager. Today the enforced
        action is <span className="mono">grant_permission</span> (a manager granting a
        permission to a subordinate); the manager id must be the operative you grant it to.
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
                {isScopedWildcard(p) && (
                  <span className="badge" style={{ marginLeft: 6 }}>scope: all tools in plugin</span>
                )}
                {isManagerSubtree(p) && (
                  <span className="badge" style={{ marginLeft: 6 }}>scope: manager subtree</span>
                )}
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
            placeholder="e.g. tool:relux-tools-github:read or tool:relux-tools-github:*"
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
      <ManagerGrantPanel
        managerId={agentId}
        available={managerGrant.available}
        reason={managerGrant.reason}
        targets={managerGrant.targets}
        roster={roster}
        onChanged={onChanged}
      />
      <AgentTokenPanel agentId={agentId} />
      <ManagerTokenActionsPanel agentId={agentId} targets={managerGrant.targets} />
    </div>
  );
}

// Per-agent access tokens (the first per-agent auth identity). The operator mints a
// bounded, hashed-at-rest, revocable token that authenticates a request AS this agent
// on the tiny agent-self route subset (`/v1/relux/agents/me*`) — notably the
// manager-grant-as-self path, where a manager drives its own grant with NO operator in
// the loop. The raw token is shown EXACTLY ONCE at mint (copy-once) and never again;
// the dashboard only ever lists non-secret metadata. docs/HERMES_OPENCLAW_DEEP_AUDIT.md §19.
function AgentTokenPanel({ agentId }: { agentId: string }) {
  const { data: tokens, loading, error, reload } = useAsync<ReluxAgentTokenMeta[]>(
    () => reluxWork.listAgentTokens(agentId),
    [agentId],
  );
  const [label, setLabel] = useState("");
  const [ttlDays, setTtlDays] = useState("");
  const [busy, setBusy] = useState(false);
  const [mintError, setMintError] = useState<string | null>(null);
  // The just-minted raw token, shown once behind a copy-once warning. Cleared on dismiss
  // — it is never re-fetchable (the backend stores only a hash).
  const [justMinted, setJustMinted] = useState<ReluxMintedAgentToken | null>(null);
  const [copied, setCopied] = useState(false);

  const fmt = (secs: number): string => {
    try {
      return new Date(secs * 1000).toLocaleString();
    } catch {
      return String(secs);
    }
  };

  async function mint() {
    setBusy(true);
    setMintError(null);
    try {
      const ttlSecs = parseTokenTtlSecs(ttlDays);
      const minted = await reluxWork.mintAgentToken(agentId, label.trim(), ttlSecs);
      setJustMinted(minted);
      setCopied(false);
      setLabel("");
      setTtlDays("");
      reload();
    } catch (e) {
      setMintError(e instanceof Error ? e.message : "Mint failed.");
    } finally {
      setBusy(false);
    }
  }

  async function revoke(tokenId: string) {
    if (
      !window.confirm(
        `Revoke token "${tokenId}"? Any request using it stops authenticating immediately.`,
      )
    ) {
      return;
    }
    setBusy(true);
    setMintError(null);
    try {
      await reluxWork.revokeAgentToken(agentId, tokenId);
      reload();
    } catch (e) {
      setMintError(e instanceof Error ? e.message : "Revoke failed.");
    } finally {
      setBusy(false);
    }
  }

  async function copyToken() {
    if (!justMinted) return;
    try {
      await navigator.clipboard.writeText(justMinted.token);
      setCopied(true);
    } catch {
      // Clipboard may be unavailable; the operator can still select the text manually.
      setCopied(false);
    }
  }

  return (
    <div
      className="agent-token-panel"
      style={{ marginTop: 12, borderTop: "1px dashed var(--border, #2a2a2a)", paddingTop: 10 }}
    >
      <h4 style={{ margin: "0 0 4px" }}>Access tokens (per-agent auth)</h4>
      <p className="muted" style={{ fontSize: 12, marginTop: 0 }}>
        Mint a bounded, revocable token that authenticates a request <strong>as this agent</strong>{" "}
        on the agent-self routes only (e.g. <span className="mono">POST /v1/relux/agents/me/manager-grant</span>,
        where a manager drives its own Branch grant with no operator in the loop). It can never
        reach the operator console. The token is stored only as a hash and{" "}
        <strong>shown to you exactly once</strong> at mint — copy it then.
      </p>
      {justMinted && (
        <div
          className="token-once"
          style={{
            margin: "8px 0",
            padding: 8,
            border: "1px solid var(--warn, #b58900)",
            borderRadius: 4,
          }}
        >
          <p style={{ margin: "0 0 4px", fontSize: 12 }}>
            <strong>Copy this token now — it will never be shown again.</strong>
          </p>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <code className="mono" style={{ flex: 1, wordBreak: "break-all", fontSize: 12 }}>
              {justMinted.token}
            </code>
            <button type="button" className="btn sm" onClick={() => void copyToken()}>
              {copied ? "Copied" : "Copy"}
            </button>
            <button
              type="button"
              className="btn ghost sm"
              onClick={() => {
                setJustMinted(null);
                setCopied(false);
              }}
            >
              Dismiss
            </button>
          </div>
          <p className="muted" style={{ fontSize: 11, marginTop: 4 }}>
            token id <span className="mono">{justMinted.token_id}</span> · expires {fmt(justMinted.expires_at)}
          </p>
        </div>
      )}
      {mintError && <div className="error-message">{mintError}</div>}
      {error && <div className="error-message">{error}</div>}
      {loading ? (
        <p className="muted" style={{ fontSize: 12 }}>Loading tokens…</p>
      ) : !tokens || tokens.length === 0 ? (
        <p className="muted" style={{ fontSize: 12 }}>No active tokens.</p>
      ) : (
        <ul className="token-list" style={{ margin: "4px 0", paddingLeft: 0, listStyle: "none" }}>
          {tokens.map((t) => (
            <li
              key={t.token_id}
              style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 4 }}
            >
              <span style={{ flex: 1, fontSize: 12 }}>
                <span className="mono">{t.token_id}</span>
                {t.label ? ` · ${t.label}` : ""}
                <span className="muted"> · expires {fmt(t.expires_at)}</span>
              </span>
              <button
                type="button"
                className="btn ghost sm"
                onClick={() => void revoke(t.token_id)}
                disabled={busy}
              >
                Revoke
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="form-group" style={{ marginTop: 6 }}>
        <label htmlFor={`tok-${agentId}-label`}>Mint a token:</label>
        <div style={{ display: "flex", gap: 8 }}>
          <input
            id={`tok-${agentId}-label`}
            type="text"
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            placeholder="label (e.g. ci-runner)"
            style={{ flex: 1 }}
          />
          <input
            type="number"
            min="1"
            value={ttlDays}
            onChange={(e) => setTtlDays(e.target.value)}
            placeholder="days (opt)"
            style={{ width: 96 }}
          />
          <button type="button" className="btn sm" onClick={() => void mint()} disabled={busy}>
            {busy ? "…" : "Mint"}
          </button>
        </div>
      </div>
    </div>
  );
}

// A compact, HONEST manager-actions panel for the per-agent token path. It documents the
// three agent-self routes a token unlocks (`manager-grant` / `assign-task` /
// `manager-revoke`), shows copy-paste curl snippets that embed NO secret (the token is the
// `$RELUX_AGENT_TOKEN` shell var), and offers a local test form for EACH action. The forms
// require the operator to PASTE the raw token deliberately — the dashboard cannot reuse a
// minted token (only its hash is stored, copy-once), so it never fakes the credential, and
// each drives the bearer path (`agentSelfAssignTask` / `agentSelfManagerGrant` /
// `agentSelfManagerRevoke`, `credentials: "omit"`) so the operator session is NOT used to
// bypass the token. The acting manager is always the token subject; the kernel re-checks
// own-Branch + Active + the matching `agent:<id>:subtree:<action>` scope. Each form keeps its
// OWN pasted token and clears it the moment the request returns.
// docs/HERMES_OPENCLAW_DEEP_AUDIT.md §19 / §20 / §21 / §22.
export function ManagerTokenActionsPanel({
  agentId,
  targets,
}: {
  agentId: string;
  targets: string[];
}) {
  // assign-task form state.
  const [open, setOpen] = useState(false);
  const [token, setToken] = useState("");
  const [taskId, setTaskId] = useState("");
  const [target, setTarget] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<ReluxTask | null>(null);
  const [snippetCopied, setSnippetCopied] = useState(false);

  // manager-grant form state (its own token + fields, so each form is self-contained and
  // the operator pastes the credential deliberately into the action it is testing).
  const [grantOpen, setGrantOpen] = useState(false);
  const [grantToken, setGrantToken] = useState("");
  const [grantTarget, setGrantTarget] = useState("");
  const [permission, setPermission] = useState("");
  const [grantBusy, setGrantBusy] = useState(false);
  const [grantError, setGrantError] = useState<string | null>(null);
  const [grantResult, setGrantResult] = useState<ReluxAgentPermissions | null>(null);
  const [grantSnippetCopied, setGrantSnippetCopied] = useState(false);

  // manager-revoke form state (its own token + fields, same copy-once discipline).
  const [revokeOpen, setRevokeOpen] = useState(false);
  const [revokeToken, setRevokeToken] = useState("");
  const [revokeTarget, setRevokeTarget] = useState("");
  const [revokePermission, setRevokePermission] = useState("");
  const [revokeBusy, setRevokeBusy] = useState(false);
  const [revokeError, setRevokeError] = useState<string | null>(null);
  const [revokeResult, setRevokeResult] = useState<ReluxAgentPermissions | null>(null);
  const [revokeSnippetCopied, setRevokeSnippetCopied] = useState(false);

  const notReady = assignTaskFormReason(token, taskId, target);
  const snippet = assignTaskCurlSnippet(taskId, target);

  const grantNotReady = managerGrantFormReason(grantToken, grantTarget, permission);
  const grantSnippet = managerGrantCurlSnippet(grantTarget, permission);

  const revokeNotReady = managerRevokeFormReason(revokeToken, revokeTarget, revokePermission);
  const revokeSnippet = managerRevokeCurlSnippet(revokeTarget, revokePermission);

  async function copySnippet() {
    try {
      await navigator.clipboard.writeText(snippet);
      setSnippetCopied(true);
    } catch {
      setSnippetCopied(false);
    }
  }

  async function copyGrantSnippet() {
    try {
      await navigator.clipboard.writeText(grantSnippet);
      setGrantSnippetCopied(true);
    } catch {
      setGrantSnippetCopied(false);
    }
  }

  async function copyRevokeSnippet() {
    try {
      await navigator.clipboard.writeText(revokeSnippet);
      setRevokeSnippetCopied(true);
    } catch {
      setRevokeSnippetCopied(false);
    }
  }

  async function runAssign() {
    const reason = assignTaskFormReason(token, taskId, target);
    if (reason) {
      setError(reason);
      return;
    }
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      const updated = await agentSelfAssignTask(token.trim(), taskId.trim(), target.trim());
      setResult(updated);
      // The credential has done its single job — drop it from state immediately so a
      // pasted token does not linger in memory longer than the request needs it.
      setToken("");
    } catch (e) {
      setError(e instanceof Error ? e.message : "Assignment failed.");
    } finally {
      setBusy(false);
    }
  }

  async function runGrant() {
    const reason = managerGrantFormReason(grantToken, grantTarget, permission);
    if (reason) {
      setGrantError(reason);
      return;
    }
    setGrantBusy(true);
    setGrantError(null);
    setGrantResult(null);
    try {
      const updated = await agentSelfManagerGrant(
        grantToken.trim(),
        grantTarget.trim(),
        permission.trim(),
      );
      setGrantResult(updated);
      // Same copy-once discipline as the assign form — clear the pasted token immediately.
      setGrantToken("");
    } catch (e) {
      setGrantError(e instanceof Error ? e.message : "Grant failed.");
    } finally {
      setGrantBusy(false);
    }
  }

  async function runRevoke() {
    const reason = managerRevokeFormReason(revokeToken, revokeTarget, revokePermission);
    if (reason) {
      setRevokeError(reason);
      return;
    }
    setRevokeBusy(true);
    setRevokeError(null);
    setRevokeResult(null);
    try {
      const updated = await agentSelfManagerRevoke(
        revokeToken.trim(),
        revokeTarget.trim(),
        revokePermission.trim(),
      );
      setRevokeResult(updated);
      // Same copy-once discipline — clear the pasted token immediately.
      setRevokeToken("");
    } catch (e) {
      setRevokeError(e instanceof Error ? e.message : "Revoke failed.");
    } finally {
      setRevokeBusy(false);
    }
  }

  return (
    <div
      className="manager-token-actions-panel"
      style={{ marginTop: 12, borderTop: "1px dashed var(--border, #2a2a2a)", paddingTop: 10 }}
    >
      <h4 style={{ margin: "0 0 4px" }}>Manager actions (token-authenticated)</h4>
      <p className="muted" style={{ fontSize: 12, marginTop: 0 }}>
        A per-agent token authenticates a request <strong>as this agent</strong> on two
        agent-self manager routes, each needing the matching{" "}
        <span className="mono">agent:{agentId}:subtree:&lt;action&gt;</span> scope (own Branch ·
        live · scoped — the kernel re-checks it):
      </p>
      <ul className="muted" style={{ fontSize: 12, margin: "0 0 6px", paddingLeft: 18 }}>
        <li>
          <span className="mono">POST {AGENT_SELF_MANAGER_GRANT_ROUTE}</span> — grant a permission
          to a Branch operative (<span className="mono">grant_permission</span>).
        </li>
        <li>
          <span className="mono">POST {AGENT_SELF_ASSIGN_TASK_ROUTE}</span> — assign a live task to
          a Branch operative (<span className="mono">assign_task</span>).
        </li>
        <li>
          <span className="mono">POST {AGENT_SELF_MANAGER_REVOKE_ROUTE}</span> — revoke an explicit
          permission from a Branch operative (<span className="mono">revoke_permission</span>).
        </li>
      </ul>
      <p className="muted" style={{ fontSize: 11, marginTop: 0 }}>
        The raw token is shown <strong>once</strong> at mint and stored only as a hash, so the
        dashboard cannot replay it — paste it yourself to test. These routes never accept the
        operator session; only the bearer token acts.
      </p>

      <details
        open={open}
        onToggle={(e) => setOpen((e.currentTarget as HTMLDetailsElement).open)}
        style={{ marginTop: 6 }}
      >
        <summary style={{ cursor: "pointer", fontSize: 12 }}>
          Test <span className="mono">assign-task</span> with a token
        </summary>

        <div className="form-group" style={{ marginTop: 8 }}>
          <label htmlFor={`mta-${agentId}-token`}>Raw agent token (paste once):</label>
          <input
            id={`mta-${agentId}-token`}
            type="password"
            autoComplete="off"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            placeholder="relux_agt_…"
            style={{ width: "100%" }}
          />
        </div>
        <div className="form-group" style={{ marginTop: 6 }}>
          <label htmlFor={`mta-${agentId}-task`}>Task id:</label>
          <input
            id={`mta-${agentId}-task`}
            type="text"
            className="mono"
            value={taskId}
            onChange={(e) => setTaskId(e.target.value)}
            placeholder="e.g. task_0001"
            style={{ width: "100%" }}
          />
        </div>
        <div className="form-group" style={{ marginTop: 6 }}>
          <label htmlFor={`mta-${agentId}-target`}>Target subordinate id:</label>
          {targets.length > 0 ? (
            <select
              id={`mta-${agentId}-target`}
              value={target}
              onChange={(e) => setTarget(e.target.value)}
              style={{ width: "100%" }}
            >
              <option value="">— choose a Branch operative —</option>
              {targets.map((id) => (
                <option key={id} value={id}>{id}</option>
              ))}
            </select>
          ) : (
            <input
              id={`mta-${agentId}-target`}
              type="text"
              className="mono"
              value={target}
              onChange={(e) => setTarget(e.target.value)}
              placeholder="a subordinate in this agent's Branch"
              style={{ width: "100%" }}
            />
          )}
        </div>

        {error && <div className="error-message">{error}</div>}
        {result && (
          <div
            className="assign-result"
            style={{ margin: "6px 0", fontSize: 12, color: "var(--ok, #2aa198)" }}
          >
            Assigned <span className="mono">{result.id}</span> →{" "}
            <span className="mono">{result.assigned_agent ?? "—"}</span> · status{" "}
            <span className="mono">{result.status}</span>
          </div>
        )}

        <button
          type="button"
          className="btn sm"
          onClick={() => void runAssign()}
          disabled={busy || !!notReady}
          style={{ marginTop: 4 }}
        >
          {busy ? "…" : "Assign as manager (token)"}
        </button>
        {notReady && (
          <p className="muted" style={{ fontSize: 11, marginTop: 2 }}>{notReady}</p>
        )}

        <div style={{ marginTop: 10 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <span className="muted" style={{ fontSize: 11 }}>
              Or call it yourself (no secret in this snippet):
            </span>
            <button type="button" className="btn ghost sm" onClick={() => void copySnippet()}>
              {snippetCopied ? "Copied" : "Copy curl"}
            </button>
          </div>
          <pre
            className="mono"
            style={{
              fontSize: 11,
              whiteSpace: "pre-wrap",
              wordBreak: "break-all",
              background: "var(--code-bg, #111)",
              padding: 8,
              borderRadius: 4,
              marginTop: 4,
            }}
          >
            {snippet}
          </pre>
          <p className="muted" style={{ fontSize: 11, marginTop: 2 }}>
            Set <span className="mono">RELUX_AGENT_TOKEN</span> to the raw token, then run the
            above.
          </p>
        </div>
      </details>

      <details
        open={grantOpen}
        onToggle={(e) => setGrantOpen((e.currentTarget as HTMLDetailsElement).open)}
        style={{ marginTop: 6 }}
      >
        <summary style={{ cursor: "pointer", fontSize: 12 }}>
          Test <span className="mono">manager-grant</span> with a token
        </summary>

        <div className="form-group" style={{ marginTop: 8 }}>
          <label htmlFor={`mta-${agentId}-grant-token`}>Raw agent token (paste once):</label>
          <input
            id={`mta-${agentId}-grant-token`}
            type="password"
            autoComplete="off"
            value={grantToken}
            onChange={(e) => setGrantToken(e.target.value)}
            placeholder="relux_agt_…"
            style={{ width: "100%" }}
          />
        </div>
        <div className="form-group" style={{ marginTop: 6 }}>
          <label htmlFor={`mta-${agentId}-grant-target`}>Target subordinate id:</label>
          {targets.length > 0 ? (
            <select
              id={`mta-${agentId}-grant-target`}
              value={grantTarget}
              onChange={(e) => setGrantTarget(e.target.value)}
              style={{ width: "100%" }}
            >
              <option value="">— choose a Branch operative —</option>
              {targets.map((id) => (
                <option key={id} value={id}>{id}</option>
              ))}
            </select>
          ) : (
            <input
              id={`mta-${agentId}-grant-target`}
              type="text"
              className="mono"
              value={grantTarget}
              onChange={(e) => setGrantTarget(e.target.value)}
              placeholder="a subordinate in this agent's Branch"
              style={{ width: "100%" }}
            />
          )}
        </div>
        <div className="form-group" style={{ marginTop: 6 }}>
          <label htmlFor={`mta-${agentId}-grant-permission`}>Permission to grant:</label>
          <input
            id={`mta-${agentId}-grant-permission`}
            type="text"
            className="mono"
            value={permission}
            onChange={(e) => setPermission(e.target.value)}
            placeholder="e.g. tool:relux-tools-echo:say"
            style={{ width: "100%" }}
          />
        </div>

        {grantError && <div className="error-message">{grantError}</div>}
        {grantResult && (
          <div
            className="grant-result"
            style={{ margin: "6px 0", fontSize: 12, color: "var(--ok, #2aa198)" }}
          >
            Granted to <span className="mono">{grantResult.agent_id}</span> · now holds{" "}
            <span className="mono">{grantResult.permissions.length}</span> explicit permission
            {grantResult.permissions.length === 1 ? "" : "s"}.
          </div>
        )}

        <button
          type="button"
          className="btn sm"
          onClick={() => void runGrant()}
          disabled={grantBusy || !!grantNotReady}
          style={{ marginTop: 4 }}
        >
          {grantBusy ? "…" : "Grant as manager (token)"}
        </button>
        {grantNotReady && (
          <p className="muted" style={{ fontSize: 11, marginTop: 2 }}>{grantNotReady}</p>
        )}

        <p className="muted" style={{ fontSize: 11, marginTop: 8 }}>
          The <strong>token subject</strong> is the acting manager — the kernel reads it from
          the bearer token, never the form — and re-checks own Branch · live ·{" "}
          <span className="mono">agent:{agentId}:subtree:grant_permission</span>. The operator
          cookie cannot stand in: this form sends only the bearer.
        </p>

        <div style={{ marginTop: 6 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <span className="muted" style={{ fontSize: 11 }}>
              Or call it yourself (no secret in this snippet):
            </span>
            <button type="button" className="btn ghost sm" onClick={() => void copyGrantSnippet()}>
              {grantSnippetCopied ? "Copied" : "Copy curl"}
            </button>
          </div>
          <pre
            className="mono"
            style={{
              fontSize: 11,
              whiteSpace: "pre-wrap",
              wordBreak: "break-all",
              background: "var(--code-bg, #111)",
              padding: 8,
              borderRadius: 4,
              marginTop: 4,
            }}
          >
            {grantSnippet}
          </pre>
        </div>
      </details>

      <details
        open={revokeOpen}
        onToggle={(e) => setRevokeOpen((e.currentTarget as HTMLDetailsElement).open)}
        style={{ marginTop: 6 }}
      >
        <summary style={{ cursor: "pointer", fontSize: 12 }}>
          Test <span className="mono">manager-revoke</span> with a token
        </summary>

        <div className="form-group" style={{ marginTop: 8 }}>
          <label htmlFor={`mta-${agentId}-revoke-token`}>Raw agent token (paste once):</label>
          <input
            id={`mta-${agentId}-revoke-token`}
            type="password"
            autoComplete="off"
            value={revokeToken}
            onChange={(e) => setRevokeToken(e.target.value)}
            placeholder="relux_agt_…"
            style={{ width: "100%" }}
          />
        </div>
        <div className="form-group" style={{ marginTop: 6 }}>
          <label htmlFor={`mta-${agentId}-revoke-target`}>Target subordinate id:</label>
          {targets.length > 0 ? (
            <select
              id={`mta-${agentId}-revoke-target`}
              value={revokeTarget}
              onChange={(e) => setRevokeTarget(e.target.value)}
              style={{ width: "100%" }}
            >
              <option value="">— choose a Branch operative —</option>
              {targets.map((id) => (
                <option key={id} value={id}>{id}</option>
              ))}
            </select>
          ) : (
            <input
              id={`mta-${agentId}-revoke-target`}
              type="text"
              className="mono"
              value={revokeTarget}
              onChange={(e) => setRevokeTarget(e.target.value)}
              placeholder="a subordinate in this agent's Branch"
              style={{ width: "100%" }}
            />
          )}
        </div>
        <div className="form-group" style={{ marginTop: 6 }}>
          <label htmlFor={`mta-${agentId}-revoke-permission`}>Permission to revoke (exact):</label>
          <input
            id={`mta-${agentId}-revoke-permission`}
            type="text"
            className="mono"
            value={revokePermission}
            onChange={(e) => setRevokePermission(e.target.value)}
            placeholder="e.g. tool:relux-tools-echo:say"
            style={{ width: "100%" }}
          />
        </div>

        {revokeError && <div className="error-message">{revokeError}</div>}
        {revokeResult && (
          <div
            className="revoke-result"
            style={{ margin: "6px 0", fontSize: 12, color: "var(--ok, #2aa198)" }}
          >
            Revoked from <span className="mono">{revokeResult.agent_id}</span> · now holds{" "}
            <span className="mono">{revokeResult.permissions.length}</span> explicit permission
            {revokeResult.permissions.length === 1 ? "" : "s"}.
          </div>
        )}

        <button
          type="button"
          className="btn sm"
          onClick={() => void runRevoke()}
          disabled={revokeBusy || !!revokeNotReady}
          style={{ marginTop: 4 }}
        >
          {revokeBusy ? "…" : "Revoke as manager (token)"}
        </button>
        {revokeNotReady && (
          <p className="muted" style={{ fontSize: 11, marginTop: 2 }}>{revokeNotReady}</p>
        )}

        <p className="muted" style={{ fontSize: 11, marginTop: 8 }}>
          The <strong>token subject</strong> is the acting manager — the kernel reads it from
          the bearer token, never the form — and re-checks own Branch · live ·{" "}
          <span className="mono">agent:{agentId}:subtree:revoke_permission</span>. It removes the{" "}
          <strong>exact</strong> stored permission only (no pattern expansion); a permission the
          operative does not hold is an honest 404.
        </p>

        <div style={{ marginTop: 6 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <span className="muted" style={{ fontSize: 11 }}>
              Or call it yourself (no secret in this snippet):
            </span>
            <button type="button" className="btn ghost sm" onClick={() => void copyRevokeSnippet()}>
              {revokeSnippetCopied ? "Copied" : "Copy curl"}
            </button>
          </div>
          <pre
            className="mono"
            style={{
              fontSize: 11,
              whiteSpace: "pre-wrap",
              wordBreak: "break-all",
              background: "var(--code-bg, #111)",
              padding: 8,
              borderRadius: 4,
              marginTop: 4,
            }}
          >
            {revokeSnippet}
          </pre>
        </div>
      </details>
    </div>
  );
}

// The operator-assisted "Grant as manager" affordance. Shown only when THIS agent holds a
// live manager-subtree `grant_permission` scope over a non-empty Branch (otherwise the
// honest unavailable reason is shown). The operator authorizes acting AS the manager; the
// backend (`POST /v1/relux/agents/:id/manager-grant`) still enforces the real own-Branch +
// Active + scope rule, so this never widens what the manager itself could do. HONEST: no
// per-agent auth identity yet, so the operator stands in for the manager.
// docs/HERMES_OPENCLAW_DEEP_AUDIT.md §19.
function ManagerGrantPanel({
  managerId,
  available,
  reason,
  targets,
  roster,
  onChanged,
}: {
  managerId: string;
  available: boolean;
  reason: string;
  targets: string[];
  roster: Agent[];
  onChanged: () => void;
}) {
  const [target, setTarget] = useState("");
  const [perm, setPerm] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const nameFor = (id: string): string => {
    const a = roster.find((r) => r.id === id);
    return a ? `${a.name} (${a.id})` : id;
  };
  const invalidReason = perm.trim() ? permissionInvalidReason(perm) : null;

  async function grantAsManager() {
    const p = perm.trim();
    const r = permissionInvalidReason(p);
    if (r) {
      setError(r);
      return;
    }
    if (!target) {
      setError("Choose a subordinate in this manager's Branch.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await reluxWork.managerGrantPermission(managerId, target, p);
      setPerm("");
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Manager grant failed.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      className="manager-grant-panel"
      style={{ marginTop: 12, borderTop: "1px dashed var(--border, #2a2a2a)", paddingTop: 10 }}
    >
      <h4 style={{ margin: "0 0 4px" }}>Grant as manager</h4>
      {!available ? (
        <p className="muted" style={{ fontSize: 12, marginTop: 0 }}>
          Unavailable — {reason}
        </p>
      ) : (
        <>
          <p className="muted" style={{ fontSize: 12, marginTop: 0 }}>
            You authorize acting <strong>as this manager</strong> to grant a capability to one
            of its own-Branch operatives. The kernel still enforces the manager-subtree rule
            (own Branch · live · scoped) — you cannot grant where the manager itself could not.
            <span className="badge" style={{ marginLeft: 6 }}>operator stands in (no per-agent auth yet)</span>
          </p>
          {error && <div className="error-message">{error}</div>}
          <div className="form-group" style={{ marginTop: 6 }}>
            <label htmlFor={`mg-${managerId}-target`}>Subordinate:</label>
            <select
              id={`mg-${managerId}-target`}
              value={target}
              onChange={(e) => setTarget(e.target.value)}
              style={{ width: "100%" }}
            >
              <option value="">— choose a Branch operative —</option>
              {targets.map((id) => (
                <option key={id} value={id}>{nameFor(id)}</option>
              ))}
            </select>
          </div>
          <div className="form-group" style={{ marginTop: 6 }}>
            <label htmlFor={`mg-${managerId}-perm`}>Permission to grant:</label>
            <div style={{ display: "flex", gap: 8 }}>
              <input
                id={`mg-${managerId}-perm`}
                type="text"
                className="mono"
                value={perm}
                onChange={(e) => setPerm(e.target.value)}
                placeholder="e.g. tool:relux-tools-github:read"
                style={{ flex: 1 }}
              />
              <button
                type="button"
                className="btn sm"
                onClick={() => void grantAsManager()}
                disabled={busy || !!invalidReason || !perm.trim() || !target}
              >
                {busy ? "…" : "Grant"}
              </button>
            </div>
            {invalidReason && (
              <p className="muted" style={{ fontSize: 11, marginTop: 2 }}>{invalidReason}</p>
            )}
          </div>
        </>
      )}
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
