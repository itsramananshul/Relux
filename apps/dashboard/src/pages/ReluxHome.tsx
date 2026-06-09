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

interface Stat {
  label: string;
  value: number;
  hint: string;
}

function statsFrom(s: ReluxState): Stat[] {
  return [
    { label: "Plugins", value: s.installed_plugins, hint: "installed capabilities" },
    { label: "Agents", value: s.agents, hint: "configured actors" },
    { label: "Tasks", value: s.tasks, hint: "units of work" },
    { label: "Runs", value: s.runs, hint: "execution attempts" },
    { label: "Open tasks", value: s.open_tasks, hint: "not yet done" },
    { label: "Active runs", value: s.active_runs, hint: "running now" },
    { label: "Waiting approval", value: s.waiting_approval, hint: "need a decision" },
    { label: "Pending approvals", value: s.pending_approvals, hint: "queued for a human" },
  ];
}

export function ReluxHome() {
  const state = useAsync<ReluxState>(() => reluxPlugins.state(), []);
  const plugins = useAsync<ReluxPlugin[]>(() => reluxPlugins.list(), []);

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
      ) : state.data ? (
        <div className="grid cols-4">
          {statsFrom(state.data).map((s) => (
            <div className="card" key={s.label}>
              <h3 style={{ marginBottom: 6 }}>{s.label}</h3>
              <div className="stat">{s.value}</div>
              <div className="muted" style={{ fontSize: 11, marginTop: 4 }}>{s.hint}</div>
            </div>
          ))}
        </div>
      ) : (
        <div className="card">
          <div className="loading">Loading control-plane state...</div>
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
