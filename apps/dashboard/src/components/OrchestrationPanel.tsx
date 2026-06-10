import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import {
  reluxOrchestration,
  ApiError,
  type ReluxOrchestration,
  type ReluxOrchestrationPlan,
  type ReluxOrchestrationBatchResult,
} from "../api";
import {
  orchestrationStatusTone,
  stepOutcomeTone,
  orchestrationProgressLabel,
  canRunOrchestration,
  orchestrationNextAction,
  groupStepsByAgent,
  stepLifecycle,
  stepLifecycleTone,
  stepDependencyLabel,
  orchestrationReadiness,
} from "../orchestration";

// Orchestration panel (master plan section 10.4 Delegation Rules, section 15
// multi-agent workloads): Prime as an orchestrator. The operator types a goal,
// previews the multi-agent plan Prime would create (briefs across agents),
// commits it, and runs a governed batch — all without touching the CLI. Every
// row renders only what the kernel recorded; nothing here fabricates an outcome.

export function OrchestrationPanel() {
  const [goal, setGoal] = useState("");
  const [plan, setPlan] = useState<ReluxOrchestrationPlan | null>(null);
  const [list, setList] = useState<ReluxOrchestration[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [lastBatch, setLastBatch] = useState<ReluxOrchestrationBatchResult | null>(null);

  async function refresh() {
    try {
      const l = await reluxOrchestration.list();
      // Newest first (ids are zero-padded so lexical desc == newest).
      l.sort((a, b) => (a.id < b.id ? 1 : a.id > b.id ? -1 : 0));
      setList(l);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to load orchestrations");
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  async function preview() {
    if (!goal.trim() || busy) return;
    setBusy("preview");
    setError(null);
    setPlan(null);
    try {
      setPlan(await reluxOrchestration.preview(goal.trim()));
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to preview plan");
    } finally {
      setBusy(null);
    }
  }

  async function create() {
    if (!goal.trim() || busy) return;
    setBusy("create");
    setError(null);
    try {
      await reluxOrchestration.create(goal.trim());
      setGoal("");
      setPlan(null);
      await refresh();
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to create orchestration");
    } finally {
      setBusy(null);
    }
  }

  async function run(id: string) {
    if (busy) return;
    setBusy(`run:${id}`);
    setError(null);
    setLastBatch(null);
    try {
      const result = await reluxOrchestration.run(id);
      setLastBatch(result);
      await refresh();
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to run orchestration");
    } finally {
      setBusy(null);
    }
  }

  const planIsMultiAgent = plan != null && plan.steps.length >= 2;

  return (
    <div className="card">
      <h3>Orchestration (multi-agent)</h3>
      <p className="muted" style={{ fontSize: 12, marginTop: 0 }}>
        Give Prime a goal with multiple steps and it splits the work into briefs
        across agents. Preview the plan, create it, then run a governed batch.
        Briefs run through each agent&apos;s own adapter — CLI agents need their
        runtime enabled first.
      </p>

      {error && (
        <div className="banner err" style={{ fontSize: 12 }}>
          {error}
        </div>
      )}

      <div className="row wrap" style={{ gap: 8, alignItems: "center" }}>
        <input
          className="input"
          style={{ flex: 1, minWidth: 260 }}
          placeholder='e.g. "research the options, implement a prototype, and write the docs"'
          value={goal}
          onChange={(e) => setGoal(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void preview();
            }
          }}
        />
        <button className="btn" onClick={() => void preview()} disabled={!goal.trim() || busy != null}>
          {busy === "preview" ? "..." : "Preview plan"}
        </button>
      </div>

      {plan && (
        <div className="card" style={{ marginTop: 10, background: "var(--bg)" }}>
          {planIsMultiAgent ? (
            <>
              <div className="muted" style={{ fontSize: 11, marginBottom: 6 }}>
                Prime would create {plan.steps.length} briefs:
              </div>
              {plan.steps.map((s, i) => {
                const deps = s.depends_on ?? [];
                return (
                  <div key={i} className="row wrap" style={{ gap: 8, fontSize: 12, marginBottom: 4 }}>
                    <span className="mono muted" style={{ fontSize: 10 }}>
                      {i + 1}.
                    </span>
                    <span className="badge backlog" style={{ fontSize: 9 }}>
                      {s.role}
                    </span>
                    <span>{s.title}</span>
                    {deps.length > 0 && (
                      <span className="muted" style={{ fontSize: 10 }}>
                        waits on {deps.map((j) => `#${j + 1}`).join(", ")}
                      </span>
                    )}
                    <span className="muted" style={{ marginLeft: "auto" }}>
                      → {s.agent_id ?? "prime (no specialist; hire one for this role)"}
                    </span>
                  </div>
                );
              })}
              {plan.notes.map((n, i) => (
                <div key={i} className="muted" style={{ fontSize: 11, marginTop: 4 }}>
                  note: {n}
                </div>
              ))}
              <div className="row" style={{ gap: 8, marginTop: 10 }}>
                <button className="btn" onClick={() => void create()} disabled={busy != null}>
                  {busy === "create" ? "Creating..." : "Create this orchestration"}
                </button>
              </div>
            </>
          ) : (
            <div className="muted" style={{ fontSize: 12 }}>
              This goal reads as a single piece of work, not something to split
              across agents. Give it distinct steps, or create one task on the{" "}
              <Link to="/work" className="link">
                Work
              </Link>{" "}
              board.
            </div>
          )}
        </div>
      )}

      {lastBatch && (
        <div className="banner" style={{ fontSize: 12, marginTop: 10 }}>
          <strong>Last batch:</strong> {lastBatch.summary} <br />
          <span className="muted" style={{ fontSize: 11 }}>
            {lastBatch.rounds ?? 0} round(s), up to {lastBatch.concurrency ?? 2} brief(s) at a
            time
            {(lastBatch.waiting ?? 0) > 0 ? ` · ${lastBatch.waiting} waiting on a dependency` : ""}
            {(lastBatch.dependency_blocked ?? 0) > 0
              ? ` · ${lastBatch.dependency_blocked} blocked by a failed dependency`
              : ""}
          </span>
          <br />
          {lastBatch.per_agent.map((line, i) => (
            <span key={i} className="mono" style={{ display: "block", fontSize: 11 }}>
              {line}
            </span>
          ))}
          <div style={{ marginTop: 4 }}>Next: {lastBatch.next_action}</div>
        </div>
      )}

      <div style={{ marginTop: 16 }}>
        <h4 style={{ marginBottom: 6 }}>Orchestrations</h4>
        {list.length === 0 ? (
          <div className="muted" style={{ fontSize: 12 }}>
            No orchestrations yet.
          </div>
        ) : (
          list.map((o) => (
            <OrchestrationRow
              key={o.id}
              o={o}
              onRun={() => void run(o.id)}
              running={busy === `run:${o.id}`}
              disabled={busy != null}
            />
          ))
        )}
      </div>
    </div>
  );
}

function OrchestrationRow({
  o,
  onRun,
  running,
  disabled,
}: {
  o: ReluxOrchestration;
  onRun: () => void;
  running: boolean;
  disabled: boolean;
}) {
  const [open, setOpen] = useState(false);
  const groups = groupStepsByAgent(o);
  const readiness = orchestrationReadiness(o);
  // The dependency-aware shape of remaining work, shown so an operator sees what
  // is runnable now vs still gated before pressing Run.
  const readinessBits = [
    readiness.ready > 0 ? `${readiness.ready} ready` : null,
    readiness.waiting > 0 ? `${readiness.waiting} waiting on a dependency` : null,
    readiness.blocked > 0 ? `${readiness.blocked} blocked` : null,
  ].filter(Boolean);
  // Index briefs by task id so a row can resolve its own position for the
  // dependency-aware lifecycle (which reads the whole step set).
  const indexOfStep = new Map(o.steps.map((s, i) => [s.task_id, i] as const));
  return (
    <div className="card" style={{ marginBottom: 8, padding: 10 }}>
      <div className="row wrap" style={{ gap: 8, alignItems: "center" }}>
        <span className={"badge " + orchestrationStatusTone(o.status)} style={{ fontSize: 9 }}>
          {o.status.replace(/_/g, " ")}
        </span>
        <span className="mono" style={{ fontSize: 11 }}>
          {o.id}
        </span>
        <span style={{ fontSize: 13 }}>{o.goal}</span>
        <span className="muted" style={{ fontSize: 11, marginLeft: "auto" }}>
          {orchestrationProgressLabel(o)}
        </span>
      </div>

      <div className="muted" style={{ fontSize: 11, marginTop: 4 }}>
        {orchestrationNextAction(o)}
      </div>

      {readinessBits.length > 0 && (
        <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
          {readinessBits.join(" · ")}
        </div>
      )}

      <div className="row" style={{ gap: 8, marginTop: 8 }}>
        <button
          className="btn"
          onClick={onRun}
          disabled={disabled || !canRunOrchestration(o)}
          title={
            canRunOrchestration(o)
              ? "Run a governed batch of the pending briefs"
              : "No pending briefs to run"
          }
        >
          {running ? "Running..." : o.status === "planned" ? "Run orchestration" : "Continue"}
        </button>
        <button className="btn ghost" onClick={() => setOpen((v) => !v)}>
          {open ? "Hide briefs" : "Show briefs"}
        </button>
      </div>

      {open && (
        <div style={{ marginTop: 10 }}>
          {groups.map((g) => (
            <div key={g.agentId} style={{ marginBottom: 8 }}>
              <div className="row" style={{ gap: 6, alignItems: "center" }}>
                <span className="mono" style={{ fontSize: 11 }}>
                  {g.agentId}
                </span>
                <Link to="/crew" className="link" style={{ fontSize: 10 }}>
                  view agent
                </Link>
              </div>
              {g.steps.map((s) => {
                const idx = indexOfStep.get(s.task_id) ?? -1;
                const lifecycle = idx >= 0 ? stepLifecycle(o, idx) : "waiting";
                const depLabel = stepDependencyLabel(o, s);
                return (
                  <div
                    key={s.task_id}
                    className="row wrap"
                    style={{ gap: 8, fontSize: 12, padding: "2px 0 2px 12px" }}
                  >
                    <span className={"badge " + stepOutcomeTone(s.outcome)} style={{ fontSize: 9 }}>
                      {s.outcome}
                    </span>
                    {/* The derived lifecycle adds the dependency-aware state the raw
                        outcome can't show: a pending brief is "ready" or "waiting". */}
                    {s.outcome === "pending" && (
                      <span
                        className={"badge " + stepLifecycleTone(lifecycle)}
                        style={{ fontSize: 9 }}
                        title="dependency-aware state"
                      >
                        {lifecycle}
                      </span>
                    )}
                    <span className="badge backlog" style={{ fontSize: 9 }}>
                      {s.role}
                    </span>
                    <Link to="/work" className="mono" style={{ fontSize: 11 }}>
                      {s.task_id}
                    </Link>
                    <span>{s.title}</span>
                    {typeof s.round === "number" && (
                      <span className="muted" style={{ fontSize: 10 }}>
                        round {s.round}
                      </span>
                    )}
                    {s.run_id && (
                      <Link to="/work" className="mono muted" style={{ fontSize: 10 }}>
                        {s.run_id}
                      </Link>
                    )}
                    {depLabel && (
                      <span className="muted" style={{ fontSize: 10 }}>
                        {depLabel}
                      </span>
                    )}
                    {s.note && (
                      <span className="muted" style={{ fontSize: 11, width: "100%" }}>
                        {s.note}
                      </span>
                    )}
                  </div>
                );
              })}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
