import { Link } from "react-router-dom";
import { reluxPlugins, type ReluxPlugin, type ReluxState } from "../api";
import { useAsync } from "../components/common";

// Relux Home (RELUX_MASTER_PLAN section 11 Dashboard, section 2 North Star): the first
// screen of the standalone Relux product. It is backed ONLY by the local
// /v1/relux control plane - `state` for the grounded counts and `plugins` for
// the installed list - so it works the moment `relux-kernel serve` is up, with
// no bridge and no login. It says what Relux is (a local, Prime-centered control
// plane) and points at the two things you can do right now: talk to Prime and
// manage plugins.

interface ChecklistItem {
  id: string;
  label: string;
  status: "todo" | "done" | "info" | "link";
  description: string;
  linkTo?: string;
}

function getFirstRunChecklist(s: ReluxState | null): ChecklistItem[] {
  if (!s) {
    return [
      { id: "loading", label: "Loading system state...", status: "info", description: "Fetching current Relux operational state." }
    ];
  }

  const checklist: ChecklistItem[] = [
    {
      id: "prime-available",
      label: "Prime is available",
      status: "done", // Prime is always available in the local control plane
      description: "Your local Relux operator is ready to chat.",
      linkTo: "/prime"
    },
    {
      id: "at-least-one-agent",
      label: "At least one agent exists",
      status: s.agents > 0 ? "done" : "todo",
      description: s.agents > 0 ? `You have ${s.agents} configured agent(s).` : "Create your first agent to delegate tasks.",
      linkTo: "/crew"
    },
    {
      id: "at-least-one-task",
      label: "At least one task exists",
      status: s.tasks > 0 ? "done" : "todo",
      description: s.tasks > 0 ? `You have ${s.tasks} total task(s).` : "Create a task for Prime or an agent to work on.",
      linkTo: "/work"
    },
    {
      id: "pending-approvals",
      label: "Pending approvals",
      status: s.pending_approvals > 0 ? "todo" : "done",
      description: s.pending_approvals > 0 ? `You have ${s.pending_approvals} pending approval(s) requiring your decision.` : "No pending approvals at the moment.",
      linkTo: "/approvals"
    },
    {
      id: "installed-plugins",
      label: "Plugins installed",
      status: s.installed_plugins > 0 ? "done" : "todo",
      description: s.installed_plugins > 0 ? `You have ${s.installed_plugins} plugin(s) installed, extending Relux capabilities.` : "Install plugins to add new tools and adapters.",
      linkTo: "/plugins"
    },
    {
      id: "health-status",
      label: "Check system health",
      status: "link",
      description: "Monitor the operational status and diagnostics of your Relux instance.",
      linkTo: "/health"
    },
  ];

  return checklist;
}

export function ReluxHome() {
  const state = useAsync<ReluxState>(() => reluxPlugins.state(), []);
  const plugins = useAsync<ReluxPlugin[]>(() => reluxPlugins.list(), []);

  const checklist = getFirstRunChecklist(state.data);

  return (
    <div className="grid">
      {/* What Relux is — the product framing, grounded and local-first. */}
      <div className="card">
        <div className="row" style={{ alignItems: "center", marginBottom: 8 }}>
          <h3 style={{ margin: 0 }}>Relux - local control plane</h3>
          <div className="spacer" style={{ flex: 1 }} />
          <button
            className="btn ghost sm"
            onClick={() => {
              state.reload();
              plugins.reload();
            }}
            disabled={state.loading}
          >
            {state.loading ? "Loading..." : "Refresh"}
          </button>
        </div>
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.6 }}>
          Relux is a Prime-centered control plane for agentic work, running locally
          on your machine. Talk to <strong>Prime</strong> to inspect state, create
          tasks, and start runs; install <strong>plugins</strong> to add
          capabilities. Everything here is served by <span className="mono">relux-kernel</span>{" "}
          - no login, no external bridge.
        </p>
        <div className="row wrap" style={{ gap: 8, marginTop: 4 }}>
          <Link to="/prime">
            <button className="btn sm">Talk to Prime →</button>
          </Link>
          <Link to="/work">
            <button className="btn ghost sm">Go to Work →</button>
          </Link>
          <Link to="/crew">
            <button className="btn ghost sm">Manage crew →</button>
          </Link>
          <Link to="/plugins">
            <button className="btn ghost sm">Manage plugins →</button>
          </Link>
          <Link to="/approvals">
            <button className="btn ghost sm">Manage approvals →</button>
          </Link>
          <Link to="/health">
            <button className="btn ghost sm">Check health →</button>
          </Link>
        </div>
      </div>

      {state.error ? (
        <div className="card">
          <div className="banner err" style={{ fontSize: 12, marginBottom: 0 }}>
            Could not reach the Relux control plane ({state.error}). Start it with{" "}
            <span className="mono">cargo run -p relux-kernel -- serve</span> (listens on{" "}
            <span className="mono">127.0.0.1:19891</span>), then refresh.
          </div>
        </div>
      ) : (
        <div className="card">
          <h3>First-run checklist</h3>
          <ul className="checklist">
            {checklist.map((item) => (
              <li key={item.id} className="checklist-item">
                <span className={`checklist-icon ${item.status}`}>
                  {item.status === "done" && "✓"}
                  {item.status === "todo" && "✗"}
                  {item.status === "info" && "…"}
                  {item.status === "link" && "→"}
                </span>
                {item.linkTo ? (
                  <Link to={item.linkTo} className="checklist-label">
                    {item.label}
                  </Link>
                ) : (
                  <span className="checklist-label">{item.label}</span>
                )}
                <span className="checklist-description">{item.description}</span>
              </li>
            ))}
            <li className="checklist-item">
              <span className="checklist-icon info">ℹ</span>
              <span className="checklist-label">Tasks status overview</span>
              <span className="checklist-description">
                <Link to="/work" className="link">
                  Open tasks: {state.data?.open_tasks ?? 0}
                </Link>
                {" · "}
                <Link to="/work" className="link">
                  Active runs: {state.data?.active_runs ?? 0}
                </Link>
                {" · "}
                <Link to="/work" className="link">
                  Waiting approval: {state.data?.waiting_approval ?? 0}
                </Link>
              </span>
            </li>
          </ul>
        </div>
      )}

      {/* Installed plugins at a glance — the capability surface. */}
      <div className="card">
        <div className="row" style={{ alignItems: "center", marginBottom: 8 }}>
          <h3 style={{ margin: 0 }}>Installed plugins</h3>
          <div className="spacer" style={{ flex: 1 }} />
          <Link to="/plugins" className="link" style={{ fontSize: 12 }}>
            Manage →
          </Link>
        </div>
        {plugins.error ? (
          <div className="muted" style={{ fontSize: 12 }}>
            Plugin list unavailable ({plugins.error}).
          </div>
        ) : (plugins.data ?? []).length === 0 ? (
          <div className="empty">
            {plugins.loading ? "Loading plugins..." : "No plugins installed yet."}
          </div>
        ) : (
          <div className="grid" style={{ gap: 8 }}>
            {(plugins.data ?? []).map((p) => (
              <div className="kv" key={p.id}>
                <span>
                  <strong>{p.name || p.id}</strong>{" "}
                  <span className="mono muted" style={{ fontSize: 11 }}>
                    {p.id}
                  </span>
                </span>
                <span>
                  <span className="muted" style={{ fontSize: 12 }}>
                    {p.kind} · v{p.version}
                  </span>
                  <span className={"badge " + (p.enabled ? "done" : "backlog")} style={{ marginLeft: 8 }}>
                    {p.enabled ? "enabled" : "disabled"}
                  </span>
                  {p.protected && (
                    <span className="badge" style={{ marginLeft: 6 }} title="Bundled fixture">
                      protected
                    </span>
                  )}
                </span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

// Add some basic styles for the checklist
const styleSheet = document.createElement("style");
styleSheet.innerText = `
  .checklist {
    list-style: none;
    padding: 0;
    margin: 0;
  }
  .checklist-item {
    display: flex;
    align-items: center;
    margin-bottom: 8px;
    font-size: 14px;
  }
  .checklist-icon {
    width: 20px;
    height: 20px;
    border-radius: 50%;
    display: flex;
    justify-content: center;
    align-items: center;
    margin-right: 8px;
    font-weight: bold;
    color: var(--text-color);
  }
  .checklist-icon.done {
    background-color: var(--green-600); /* Example green */
    color: white;
  }
  .checklist-icon.todo {
    background-color: var(--yellow-600); /* Example yellow */
    color: black;
  }
  .checklist-icon.info {
    background-color: var(--blue-600); /* Example blue */
    color: white;
  }
  .checklist-icon.link {
    background-color: var(--gray-500); /* Example gray for links */
    color: white;
  }
  .checklist-label {
    flex-shrink: 0;
    margin-right: 8px;
    font-weight: 600;
  }
  .checklist-description {
    color: var(--text-muted);
  }
  .checklist-item .link {
    color: var(--link-color);
    text-decoration: underline;
  }
`;
document.head.appendChild(styleSheet);
