import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import {
  reluxOrchestration,
  ApiError,
  type ReluxOrchestration,
  type ReluxOrchestrationPlan,
  type ReluxOrchestrationBatchResult,
  type ReluxOrchestrationJob,
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
  stepDurationLabel,
  orchestrationReadiness,
  jobIsActive,
  jobIsTerminal,
  jobIsCanceling,
  jobCanCancel,
  jobIsReconstructed,
  jobIsInterrupted,
  jobPendingCount,
  jobPhaseLabel,
  jobProgressLabel,
  jobRunningStepIds,
  runButtonLabel,
} from "../orchestration";

// How often to poll an in-flight orchestration job for live progress.
const JOB_POLL_MS = 1000;

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
  // Live background-job state, keyed by orchestration id (the backend guarantees
  // at most one active job per orchestration). Drives the non-blocking Run path.
  const [jobs, setJobs] = useState<Record<string, ReluxOrchestrationJob>>({});
  // Avoid setState after unmount when a poll resolves late.
  const mounted = useRef(true);
  // Hydrate the durable job status at most once per mount (see below).
  const hydrated = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  async function refresh() {
    try {
      const l = await reluxOrchestration.list();
      // Newest first (ids are zero-padded so lexical desc == newest).
      l.sort((a, b) => (a.id < b.id ? 1 : a.id > b.id ? -1 : 0));
      if (mounted.current) setList(l);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to load orchestrations");
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  // Hydrate the durable job status once after the first list load. A `running`
  // orchestration is the signal that a job was (or still is) in flight: polling
  // by orchestration id (`latestJob`) either reconnects to a live job — so the
  // poll effect resumes it after a page reload — or, when the in-memory registry
  // was lost to a server restart, returns the RECONSTRUCTED restart-honest status
  // ("interrupted" with the durable progress). Without this, the interrupted
  // callout would only ever appear in the same session that pressed Run; a
  // reload after a restart would silently hide it (RELUX_MASTER_PLAN Sec 15). A
  // 404 means "no job ever started" — we keep the planned record and show nothing.
  useEffect(() => {
    if (hydrated.current || list.length === 0) return;
    hydrated.current = true;
    void (async () => {
      for (const o of list) {
        if (o.status !== "running") continue;
        try {
          const j = await reluxOrchestration.latestJob(o.id);
          if (!mounted.current) return;
          // Never clobber a job we are already tracking (e.g. one just started).
          setJobs((prev) => (prev[o.id] ? prev : { ...prev, [o.id]: j }));
        } catch {
          /* 404 (no job started) or transient — fall back to the planned record */
        }
      }
    })();
  }, [list]);

  // Poll every active job until it finishes. On completion, fold the job's
  // aggregate result into the "Last batch" banner and refresh the durable record
  // so the per-brief outcomes/rounds shown are the recorded truth. A 404 means the
  // job was lost to a server restart — drop it and fall back to the record.
  useEffect(() => {
    const activeIds = Object.values(jobs)
      .filter((j) => jobIsActive(j))
      .map((j) => j.orchestration_id);
    if (activeIds.length === 0) return;
    let cancelled = false;
    const handle = window.setTimeout(async () => {
      const updates: Record<string, ReluxOrchestrationJob> = {};
      const drop: string[] = [];
      let terminalResult: ReluxOrchestrationBatchResult | null = null;
      for (const oid of activeIds) {
        try {
          const j = await reluxOrchestration.latestJob(oid);
          updates[oid] = j;
          if (jobIsTerminal(j.state) && j.result) terminalResult = j.result;
        } catch (e) {
          if (e instanceof ApiError && e.status === 404) drop.push(oid);
        }
      }
      if (cancelled || !mounted.current) return;
      setJobs((prev) => {
        const next = { ...prev, ...updates };
        for (const oid of drop) delete next[oid];
        return next;
      });
      const anyTerminal = Object.values(updates).some((j) => jobIsTerminal(j.state));
      if (terminalResult) setLastBatch(terminalResult);
      if (anyTerminal || drop.length > 0) await refresh();
    }, JOB_POLL_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(handle);
    };
  }, [jobs]);

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

  // Start a NON-BLOCKING run: kick off a background job and let the poll effect
  // drive it to completion. The button stays disabled while the job is active, so
  // a second click can't start a duplicate (the backend also rejects duplicates).
  async function run(id: string) {
    if (jobIsActive(jobs[id])) return;
    setError(null);
    setLastBatch(null);
    try {
      const started = await reluxOrchestration.runAsync(id);
      if (mounted.current) setJobs((prev) => ({ ...prev, [id]: started }));
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to start orchestration run");
    }
  }

  // Request cancellation of the active job for orchestration `id`. Cooperative and
  // honest: the worker finishes the in-flight round, then stops before the next
  // one and marks the job canceled. The poll effect keeps running (the job is
  // still active until the worker stops), so the live banner shows "Canceling…"
  // and then "Canceled". We optimistically fold the updated job in so the button
  // disables immediately.
  async function cancel(id: string) {
    const job = jobs[id];
    if (!jobCanCancel(job)) return;
    setError(null);
    try {
      const updated = await reluxOrchestration.cancelJob(job!.id);
      if (mounted.current) setJobs((prev) => ({ ...prev, [id]: updated }));
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to cancel orchestration run");
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
              job={jobs[o.id] ?? null}
              onRun={() => void run(o.id)}
              onCancel={() => void cancel(o.id)}
            />
          ))
        )}
      </div>
    </div>
  );
}

// Exported for the render/DOM verification (`test/render-interrupted.test.mjs`):
// it server-renders this row with a reconstructed-interrupted job fixture and
// asserts the visible interrupted callout + Continue button actually appear, so a
// regression that hides them (or a stale shipped bundle) is caught — not just the
// pure helpers in `orchestration.ts`.
export function OrchestrationRow({
  o,
  job,
  onRun,
  onCancel,
}: {
  o: ReluxOrchestration;
  job: ReluxOrchestrationJob | null;
  onRun: () => void;
  onCancel: () => void;
}) {
  const [open, setOpen] = useState(false);
  const groups = groupStepsByAgent(o);
  const readiness = orchestrationReadiness(o);
  const active = jobIsActive(job);
  // A restart-honest reconstructed status (no live worker) with pending briefs
  // left to resume. The kernel marks these with a synthetic `durable:` id, so we
  // render a distinct callout — never the live-job banner — and never present
  // that id as a live worker (RELUX_MASTER_PLAN Sec 15).
  const interrupted = jobIsReconstructed(job) && jobIsInterrupted(job);
  const pending = jobPendingCount(job);
  const runningIds = new Set(jobRunningStepIds(job));
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

      {/* Interrupted (restart-honest) callout: a prior run is no longer live and
          briefs remain. Distinct from the live-job banner — it explains what
          happened, shows the DURABLE progress (completed vs pending), labels the
          status as reconstructed (never a live worker id), and points at Continue
          to resume only the pending briefs (RELUX_MASTER_PLAN Sec 15). */}
      {interrupted ? (
        <div
          className="banner"
          style={{ fontSize: 11, marginTop: 6, borderColor: "var(--warn)" }}
          role="status"
        >
          <strong style={{ color: "var(--warn)" }}>
            Run interrupted — no live worker
          </strong>
          <div className="muted" style={{ fontSize: 10, marginTop: 2 }}>
            Reconstructed from the durable record (this is not a live run): a
            previous run finished, was canceled, or was lost to a server restart,
            and nothing is driving this orchestration now.
          </div>
          <div style={{ fontSize: 11, marginTop: 4 }}>
            {jobProgressLabel(job)}
            {pending > 0 ? ` · ${pending} pending` : ""}
          </div>
          {pending > 0 && (
            <div className="muted" style={{ fontSize: 10, marginTop: 2 }}>
              Continue starts a fresh run that resumes the {pending} pending
              brief{pending === 1 ? "" : "s"}; completed briefs are never re-run.
            </div>
          )}
        </div>
      ) : (
        /* Live job status: real phase/round/progress from the polled job, shown
           while a run is in flight (and the failure reason if it failed) — never a
           bare spinner. Hidden once a live job completes cleanly. */
        job &&
        (job.state !== "completed" || jobIsActive(job)) && (
          <div
            className={"banner" + (job.state === "failed" ? " err" : "")}
            style={{ fontSize: 11, marginTop: 6 }}
          >
            <strong>{jobPhaseLabel(job)}</strong>
            {jobProgressLabel(job) ? ` — ${jobProgressLabel(job)}` : ""}
            {job.last_event && (
              <div className="muted" style={{ fontSize: 10, marginTop: 2 }}>
                {job.last_event}
              </div>
            )}
            {job.error && (
              <div style={{ fontSize: 10, marginTop: 2 }}>{job.error}</div>
            )}
          </div>
        )
      )}

      <div className="row" style={{ gap: 8, marginTop: 8 }}>
        <button
          className="btn"
          onClick={onRun}
          disabled={active || !canRunOrchestration(o)}
          title={
            active
              ? "A run is already in progress"
              : canRunOrchestration(o)
                ? "Run a governed batch of the pending briefs"
                : "No pending briefs to run"
          }
        >
          {runButtonLabel(o, job)}
        </button>
        {/* Cancel is offered only while a job is active. It is cooperative: the
            worker finishes the in-flight round, then stops before the next one and
            marks the job canceled — it never kills a running brief. */}
        {jobIsActive(job) && (
          <button
            className="btn ghost"
            onClick={onCancel}
            disabled={!jobCanCancel(job)}
            title={
              jobIsCanceling(job)
                ? "Canceling — finishing the in-flight round, then stopping"
                : "Stop after the in-flight round; remaining briefs stay pending"
            }
          >
            {jobIsCanceling(job) ? "Canceling…" : "Cancel"}
          </button>
        )}
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
                // Recorded run duration (started → finished), shown only once a
                // brief has finished — never a fabricated live timer.
                const duration = stepDurationLabel(s);
                return (
                  <div
                    key={s.task_id}
                    className="row wrap"
                    style={{ gap: 8, fontSize: 12, padding: "2px 0 2px 12px" }}
                  >
                    <span className={"badge " + stepOutcomeTone(s.outcome)} style={{ fontSize: 9 }}>
                      {s.outcome}
                    </span>
                    {/* A brief the live job is executing this round shows a real
                        "running" badge (from the polled job snapshot), not a guess. */}
                    {active && s.outcome === "pending" && runningIds.has(s.task_id) && (
                      <span className="badge in_progress" style={{ fontSize: 9 }} title="running now">
                        running
                      </span>
                    )}
                    {/* The derived lifecycle adds the dependency-aware state the raw
                        outcome can't show: a pending brief is "ready" or "waiting". */}
                    {s.outcome === "pending" && !runningIds.has(s.task_id) && (
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
                    {duration && (
                      <span
                        className="muted"
                        style={{ fontSize: 10 }}
                        title="recorded run duration (started → finished)"
                      >
                        {duration}
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
