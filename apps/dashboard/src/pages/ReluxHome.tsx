import { Link } from "react-router-dom";
import {
  reluxPlugins,
  reluxAi,
  reluxAdapters,
  reluxTools,
  reluxOrchestration,
  type ReluxPlugin,
  type ReluxState,
  type ReluxAiStatus,
  type ReluxAdapterStatus,
  type ReluxToolDescriptor,
  type ReluxOrchestration,
} from "../api";
import { useAsync } from "../components/common";
import { buildReadiness } from "../readiness";
import { ReadinessGuide } from "../components/ReadinessGuide";
import {
  activeOrchestration,
  orchestrationHeadline,
  orchestrationNextAction,
  orchestrationProgressLabel,
  orchestrationStatusTone,
} from "../orchestration";

// Relux Home (RELUX_MASTER_PLAN section 11 Dashboard, section 2 North Star): the first
// screen of the standalone Relux product. It is backed ONLY by the local
// /v1/relux control plane - `state` for the grounded counts and `plugins` for
// the installed list - so it works the moment `relux-kernel serve` is up, with
// no bridge and no login. It says what Relux is (a local, Prime-centered control
// plane) and points at the two things you can do right now: talk to Prime and
// manage plugins.

export function ReluxHome() {
  const state = useAsync<ReluxState>(() => reluxPlugins.state(), []);
  const plugins = useAsync<ReluxPlugin[]>(() => reluxPlugins.list(), []);
  const ai = useAsync<ReluxAiStatus>(() => reluxAi.status(), []);
  const adapters = useAsync<ReluxAdapterStatus[]>(() => reluxAdapters.list(), []);
  const tools = useAsync<ReluxToolDescriptor[]>(() => reluxTools.list(), []);
  const orchestrations = useAsync<ReluxOrchestration[]>(() => reluxOrchestration.list(), []);

  // The whole readiness report is derived (pure) from the live control-plane
  // reads — the brain, the real-work adapter, crew, plugins/tools and any
  // pending approvals — so the guide reflects HONEST readiness, never a faked
  // green check. Null state means the control plane was not reachable; we render
  // the loading report and the honest error banner below.
  const report = state.data
    ? buildReadiness({
        state: state.data,
        ai: ai.data,
        adapters: adapters.data,
        plugins: plugins.data,
        tools: tools.error ? null : tools.data,
      })
    : null;

  const refreshAll = () => {
    state.reload();
    plugins.reload();
    ai.reload();
    adapters.reload();
    tools.reload();
  };

  return (
    <div className="grid">
      {/* What Relux is — the product framing, grounded and local-first. */}
      <div className="card">
        <div className="row" style={{ alignItems: "center", marginBottom: 8 }}>
          <h3 style={{ margin: 0 }}>Relux - local control plane</h3>
          <div className="spacer" style={{ flex: 1 }} />
          <button className="btn ghost sm" onClick={refreshAll} disabled={state.loading}>
            {state.loading ? "Loading..." : "Refresh"}
          </button>
        </div>
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.6 }}>
          Relux is a Prime-centered control plane for agentic work, running locally
          on your machine. Talk to <strong>Prime</strong> to inspect state, create
          tasks, and start runs; run real work through a <strong>Claude</strong> or{" "}
          <strong>Codex</strong> adapter; install <strong>plugins</strong> to add
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
        <ReadinessGuide report={report} loading={state.loading} onRefresh={refreshAll} />
      )}

      {/* Multi-agent orchestration at a glance — Prime coordinating the fleet. */}
      <OrchestrationHomeCard orchestrations={orchestrations.data} loading={orchestrations.loading} />

      {/* The real product path: run work through a coding-agent adapter. */}
      <div className="card">
        <h3 style={{ marginTop: 0 }}>Run real work: Claude / Codex adapters</h3>
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.6 }}>
          Prime can drive a real coding-agent CLI to execute assigned tasks. This is
          the recommended path:
        </p>
        <ol className="muted" style={{ fontSize: 13, lineHeight: 1.7, marginTop: 0, paddingLeft: 18 }}>
          <li>
            Install and log in to the <strong>Claude CLI</strong>{" "}
            (<span className="mono">claude</span>) or the <strong>Codex CLI</strong>{" "}
            (<span className="mono">codex</span>) so it is on your PATH. They use
            their own local login — no API key goes into Relux.
          </li>
          <li>
            On <Link to="/crew" className="link">Crew → Adapters</Link>, enable the
            adapter. It is disabled by default and runs the CLI in a safe,
            non-bypass mode.
          </li>
          <li>
            Create a task on <Link to="/work" className="link">Work</Link>, assign it
            to an agent using that adapter, and run it.
          </li>
        </ol>
        <p className="muted" style={{ fontSize: 12, marginTop: 4 }}>
          Prefer natural Prime chat? Add an OpenRouter API key under{" "}
          <Link to="/health" className="link">Health → Prime AI settings</Link>{" "}
          (optional; Prime stays deterministic and grounded without it).
        </p>
      </div>

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

// Multi-agent orchestration summary (master plan section 10.4, section 15). Shows
// the most relevant orchestration — the newest unfinished one — with its progress
// and the exact next human action, linking to the Prime page where it is run.
// Grounded entirely in what the control plane returned; hidden when there is none.
function OrchestrationHomeCard({
  orchestrations,
  loading,
}: {
  orchestrations: ReluxOrchestration[] | null | undefined;
  loading: boolean;
}) {
  const list = orchestrations ?? [];
  const active = activeOrchestration(list);
  const headline = orchestrationHeadline(list);
  return (
    <div className="card">
      <div className="row" style={{ alignItems: "center", marginBottom: 8 }}>
        <h3 style={{ margin: 0 }}>Orchestration (multi-agent)</h3>
        <div className="spacer" style={{ flex: 1 }} />
        <Link to="/prime" className="link" style={{ fontSize: 12 }}>
          Open in Prime →
        </Link>
      </div>
      {!active ? (
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.6 }}>
          {loading
            ? "Loading orchestrations..."
            : "No orchestrations yet. On the Prime page, give Prime a multi-step goal (e.g. “research the options, implement a prototype, and write the docs”) and it will split the work into briefs across agents."}
        </p>
      ) : (
        <>
          {headline && (
            <p className="muted" style={{ marginTop: 0, marginBottom: 8, fontSize: 12 }}>
              {headline}
            </p>
          )}
          <div className="row wrap" style={{ gap: 8, alignItems: "center" }}>
            <span className={"badge " + orchestrationStatusTone(active.status)} style={{ fontSize: 9 }}>
              {active.status.replace(/_/g, " ")}
            </span>
            <span className="mono" style={{ fontSize: 11 }}>
              {active.id}
            </span>
            <span style={{ fontSize: 13 }}>{active.goal}</span>
            <span className="muted" style={{ fontSize: 11, marginLeft: "auto" }}>
              {orchestrationProgressLabel(active)}
            </span>
          </div>
          <div className="muted" style={{ fontSize: 12, marginTop: 6 }}>
            Next: {orchestrationNextAction(active)}
          </div>
        </>
      )}
    </div>
  );
}
