import { useState, useMemo, useEffect, useRef } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { runIdFromSearch, taskIdFromSearch, workRunShareUrl } from "../routing";
import {
  latestReluxEventId,
  mergeReluxRunEvents,
  noActivityLabel,
} from "../reluxruntranscript";
import { reluxWork, reluxAudit, type ReluxTask, type ReluxRun, type ReluxAgent, type ReluxTaskDetail, type ReluxRunDetail, type ReluxAuditEntry, type ReluxRunEvent } from "../api";
import { useAsync } from "../components/common";
import {
  runStatusTone,
  formatRunDuration,
  canRetryRun,
  runMetricsLine,
  phaseLabel,
  isRunInFlight,
  eventPayloadPreview,
  toolCallSummary,
  reviewApplyAvailability,
  runArtifacts,
  artifactTypeLabel,
  runProposedChanges,
  proposedChangeStatusLabel,
  proposedChangeStatusTone,
  proposedChangeActionLabel,
  isCreateProposedChange,
  isRenameProposedChange,
  isDeleteProposedChange,
  proposedChangePathLabel,
  canReviewProposedChange,
  canApplyProposedChange,
  reviewableProposedChangeIndices,
  applyEligibleProposedChangeIndices,
  showBatchProposedChangeControls,
  failureClassLabel,
  failureClassTone,
  recoveryStatusLine,
} from "../runview";

// Relux Work page: standalone surface for tasks and runs.
// BACKED BY: /v1/relux/tasks, /v1/relux/runs.

export function Work() {
  const location = useLocation();
  const navigate = useNavigate();
  const queryParams = useMemo(() => new URLSearchParams(location.search), [location.search]);
  const filterAgentId = queryParams.get("agentId");
  const filterStatus = queryParams.get("status");
  // Run and task detail are both URL-driven and mutually exclusive:
  // `/work?run=<id>` opens that run's panel, `/work?task=<id>` opens that task's
  // panel. Making the URL the source of truth (rather than local state) lets a
  // deep link — an orchestration step's run_id, or the task link Prime shows after
  // creating a task — plus browser back/forward/refresh restore the same view. A
  // missing/empty param simply shows no detail panel.
  const selectedRunId = runIdFromSearch(location.search);
  const selectedTaskId = taskIdFromSearch(location.search);

  const { data: tasks, loading: loadingTasks, error: errorTasks, reload: reloadTasks } = useAsync<ReluxTask[]>(
    () => reluxWork.listTasks(),
    [],
  );
  const { data: runs, loading: loadingRuns, error: errorRuns, reload: reloadRuns } = useAsync<ReluxRun[]>(
    () => reluxWork.listRuns(),
    [],
  );
  const { data: agents, loading: loadingAgents, error: errorAgents, reload: reloadAgents } = useAsync<ReluxAgent[]>(
    () => reluxWork.listAgents(),
    [],
  );

  const [newTaskTitle, setNewTaskTitle] = useState("");
  const [creating, setCreating] = useState(false);

  // Point the URL at a run or a task detail (or clear it), preserving any other
  // Work filters in the querystring. Run and task panels are mutually exclusive,
  // so focusing one clears the other. This is the single way detail selection
  // changes, so it stays in sync with deep links and the browser history.
  const focusDetail = (kind: "run" | "task", id: string | null) => {
    const next = new URLSearchParams(location.search);
    next.delete("run");
    next.delete("task");
    if (id) next.set(kind, id);
    const search = next.toString();
    const target = search ? `?${search}` : "";
    if (target === (location.search ?? "")) return; // no-op: don't push a duplicate history entry
    navigate({ search: target }, { replace: false });
  };
  const setSelectedRunId = (runId: string | null) => focusDetail("run", runId);
  const setSelectedTaskId = (taskId: string | null) => focusDetail("task", taskId);

  async function createTask() {
    if (!newTaskTitle.trim()) return;
    setCreating(true);
    try {
      await reluxWork.createTask(newTaskTitle.trim());
      setNewTaskTitle("");
      reloadTasks();
    } catch (e) {
      alert(e instanceof Error ? e.message : "Create failed");
    } finally {
      setCreating(false);
    }
  }

  const columns = useMemo(() => {
    let list = tasks ?? [];

    if (filterAgentId) {
      list = list.filter(t => t.assigned_agent === filterAgentId);
    }
    if (filterStatus) {
      list = list.filter(t => t.status === filterStatus);
    }

    return {
      open: list.filter(t => t.status === "created" || t.status === "queued"),
      running: list.filter(t => t.status === "running"),
      done: list.filter(t => t.status === "completed"),
      other: list.filter(t => !["created", "queued", "running", "completed"].includes(t.status)),
    };
  }, [tasks, filterAgentId, filterStatus]);

  const error = errorTasks || errorRuns || errorAgents;
  const loading = (loadingTasks && !tasks) || (loadingRuns && !runs) || (loadingAgents && !agents);

  const handleReload = () => {
    reloadTasks();
    reloadRuns();
    reloadAgents();
  };

  // focusDetail already clears the other panel, so each inspect is a single nav.
  const handleInspectTask = (taskId: string) => setSelectedTaskId(taskId);
  const handleInspectRun = (runId: string) => setSelectedRunId(runId);

  return (
    <div className="grid">
      <div className="card">
        <div className="row" style={{ marginBottom: 8, alignItems: "center" }}>
          <h3 style={{ margin: 0 }}>Work</h3>
          <div className="spacer" style={{ flex: 1 }} />
          <button className="btn ghost sm" onClick={handleReload} disabled={loading}>
            {loading ? "Loading..." : "Refresh"}
          </button>
        </div>

        {error ? (
          <div className="banner err" style={{ fontSize: 12 }}>
            Could not reach the Relux API ({error}). Start it with{" "}
            <span className="mono">cargo run -p relux-kernel -- serve</span>.
          </div>
        ) : (
          <>
            <div className="card" style={{ marginBottom: 20, padding: 12 }}>
              <div className="row" style={{ gap: 8 }}>
                <input
                  className="input"
                  placeholder="Create a new task..."
                  value={newTaskTitle}
                  onChange={e => setNewTaskTitle(e.target.value)}
                  onKeyDown={e => e.key === "Enter" && void createTask()}
                  disabled={creating}
                />
                <button className="btn" onClick={() => void createTask()} disabled={creating || !newTaskTitle.trim()}>
                  {creating ? "..." : "Create"}
                </button>
              </div>
            </div>

            <div className="row wrap" style={{ gap: 16, alignItems: "flex-start" }}>
              <Column title="Open" tasks={columns.open} onAction={handleReload} onInspectTask={handleInspectTask} agents={agents || []} />
              <Column title="Running" tasks={columns.running} onAction={handleReload} onInspectTask={handleInspectTask} agents={agents || []} />
              <Column title="Done" tasks={columns.done} onAction={handleReload} onInspectTask={handleInspectTask} agents={agents || []} />
            </div>

            {(selectedTaskId || selectedRunId) && (
              <div className="card" style={{ marginTop: 24, padding: 16 }}>
                {selectedTaskId && (
                  <TaskDetailPanel taskId={selectedTaskId} onClose={() => setSelectedTaskId(null)} />
                )}
                {selectedRunId && (
                  <RunDetailPanel
                    runId={selectedRunId}
                    onClose={() => setSelectedRunId(null)}
                    onOpenRun={handleInspectRun}
                    onRetried={(newRunId) => {
                      handleReload();
                      setSelectedRunId(newRunId);
                    }}
                  />
                )}
              </div>
            )}

            <div className="card" style={{ marginTop: 24, padding: 16 }}>
              <h4 style={{ marginTop: 0 }}>Recent Runs</h4>
              {runs && runs.length > 0 ? (
                <div className="table-scroll">
                  <table className="table sm">
                    <thead>
                      <tr>
                        <th>Run ID</th>
                        <th>Task</th>
                        <th>Agent</th>
                        <th>Status</th>
                        <th>Summary</th>
                        <th>Actions</th>
                      </tr>
                    </thead>
                    <tbody>
                      {[...runs].reverse().map(run => (
                        <tr key={run.id}>
                          <td className="mono" style={{ fontSize: 11 }}>
                            {run.id}
                            {run.retried_from && (
                              <span className="muted" style={{ fontSize: 9, display: "block" }}>
                                retry of{" "}
                                {/* Lineage stays on the Work surface: the parent run is
                                    in the same Relux ledger, so inspect it via /work?run=. */}
                                <a
                                  className="link"
                                  href={`?run=${encodeURIComponent(run.retried_from)}`}
                                  onClick={(e) => { e.preventDefault(); handleInspectRun(run.retried_from!); }}
                                >
                                  {run.retried_from}
                                </a>
                              </span>
                            )}
                          </td>
                          <td className="mono" style={{ fontSize: 11 }}>{run.task_id}</td>
                          <td className="mono" style={{ fontSize: 11 }}>
                            {agents?.find(a => a.id === run.agent_id)?.name || run.agent_id}
                          </td>
                          <td>
                            <span className={`badge ${runStatusTone(run.status)}`}>
                              {run.status}
                            </span>
                          </td>
                          <td className="muted" style={{ fontSize: 11 }}>{run.summary || run.error || "-"}</td>
                          <td>
                            <button className="btn ghost sm" onClick={() => handleInspectRun(run.id)}>Inspect</button>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              ) : (
                <div className="empty sm">No runs yet.</div>
              )}
            </div>

            <AuditPanel />
          </>
        )}
      </div>
    </div>
  );
}

function Column({ title, tasks, onAction, onInspectTask, agents }: { title: string; tasks: ReluxTask[]; onAction: () => void; onInspectTask: (taskId: string) => void; agents: ReluxAgent[] }) {
  return (
    <div style={{ flex: 1, minWidth: 280 }}>
      <h4 style={{ marginBottom: 8, fontSize: 13, textTransform: "uppercase", letterSpacing: "0.05em" }}>
        {title} <span className="muted" style={{ fontWeight: 400 }}>{tasks.length}</span>
      </h4>
      <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        {tasks.map(t => (
          <TaskCard key={t.id} task={t} onAction={onAction} onInspectTask={onInspectTask} agents={agents} />
        ))}
        {tasks.length === 0 && <div className="empty sm" style={{ padding: 16 }}>No {title.toLowerCase()} tasks</div>}
      </div>
    </div>
  );
}

function TaskCard({ task, onAction, onInspectTask, agents }: { task: ReluxTask; onAction: () => void; onInspectTask: (taskId: string) => void; agents: ReluxAgent[] }) {
  const [busy, setBusy] = useState(false);
  const [selectedAgent, setSelectedAgent] = useState(task.assigned_agent || "");

  const assignedAgent = useMemo(() => agents.find(a => a.id === task.assigned_agent), [agents, task.assigned_agent]);

  async function startRun() {
    setBusy(true);
    try {
      await reluxWork.startTask(task.id);
      onAction(); // Reload tasks to reflect the change to Running
    } catch (e) {
      alert(e instanceof Error ? e.message : "Start failed");
    } finally {
      setBusy(false);
    }
  }

  async function executeAssignedRun() {
    setBusy(true);
    try {
      if (task.status === "created" || task.status === "queued") {
        // If task is created/queued, first start the run (moves it to Running status)
        await reluxWork.startTask(task.id);
      }
      // Then execute the run, which will complete it
      await reluxWork.executeAssignedTask(task.id);
      onAction(); // Reload tasks to reflect the completion
    } catch (e) {
      alert(e instanceof Error ? e.message : "Execution failed");
    } finally {
      setBusy(false);
    }
  }

  async function assignAgent(agentId: string) {
    setBusy(true);
    try {
      await reluxWork.assignTask(task.id, agentId);
      onAction(); // Reload tasks to reflect the change
    } catch (e) {
      alert(e instanceof Error ? e.message : "Assignment failed");
    } finally {
      setBusy(false);
    }
  }

  const isAssigned = !!task.assigned_agent;
  const isRunnableByAssignedAgent = isAssigned && task.status === "queued";

  return (
    <div className="card sm" style={{ padding: 12, border: "1px solid var(--border)" }}>
      <div className="row" style={{ marginBottom: 4 }}>
        <div className="mono muted" style={{ fontSize: 10 }}>{task.id}</div>
        <div className="spacer" style={{ flex: 1 }} />
        <div className={`badge sm ${task.status === "completed" ? "done" : task.status === "running" ? "running" : "backlog"}`} style={{ fontSize: 9 }}>
          {task.status}
        </div>
      </div>
      <div style={{ fontWeight: 600, fontSize: 13, marginBottom: 10, lineHeight: 1.4 }}>{task.title}</div>
      <div className="row" style={{ alignItems: "center", flexWrap: "wrap", gap: 8 }}>
        {isAssigned ? (
          <div className="mono muted" style={{ fontSize: 10 }}>Assigned: {assignedAgent?.name || task.assigned_agent}</div>
        ) : (
          <select
            className="input sm"
            style={{ fontSize: 10, padding: "4px 8px", minWidth: 100 }}
            value={selectedAgent}
            onChange={(e) => {
              setSelectedAgent(e.target.value);
              if (e.target.value) {
                void assignAgent(e.target.value);
              }
            }}
            disabled={busy || !agents.length}
          >
            <option value="">Assign agent...</option>
            {agents.map((agent) => (
              <option key={agent.id} value={agent.id}>
                {agent.name}
              </option>
            ))}
          </select>
        )}
        <div className="spacer" style={{ flex: 1 }} />
        {(task.status === "created" || task.status === "queued") && !isAssigned && (
          <button className="btn sm" style={{ height: 24, padding: "0 8px" }} onClick={() => void startRun()} disabled={busy}>
            {busy ? "..." : "Start (Prime)"}
          </button>
        )}
        {isRunnableByAssignedAgent && (
          <button className="btn sm" style={{ height: 24, padding: "0 8px" }} onClick={() => void executeAssignedRun()} disabled={busy}>
            {busy ? "..." : "Run (Assigned)"}
          </button>
        )}
        <button className="btn ghost sm" style={{ height: 24, padding: "0 8px" }} onClick={() => onInspectTask(task.id)}>Inspect</button>
      </div>
    </div>
  );
}

function TaskDetailPanel({ taskId, onClose }: { taskId: string; onClose: () => void }) {
  const { data: task, loading, error } = useAsync<ReluxTaskDetail>(
    () => reluxWork.getTask(taskId),
    [taskId],
  );

  return (
    <div style={{ paddingBottom: 16 }}>
      <div className="row" style={{ alignItems: "center", marginBottom: 12 }}>
        <h4 style={{ margin: 0 }}>Task Detail</h4>
        <div className="spacer" style={{ flex: 1 }} />
        <button className="btn ghost sm" onClick={onClose}>Close</button>
      </div>
      {loading ? (
        <div className="loading">Loading task details...</div>
      ) : error ? (
        <div className="banner err" style={{ fontSize: 12 }}>
          Error loading task: {String(error)}
        </div>
      ) : task ? (
        <div className="grid" style={{ gap: 8, fontSize: 12 }}>
          <div className="kv"><span>ID:</span><span className="mono">{task.id}</span></div>
          <div className="kv"><span>Title:</span><span>{task.title}</span></div>
          <div className="kv"><span>Status:</span><span>{task.status}</span></div>
          <div className="kv"><span>Priority:</span><span>{task.priority}</span></div>
          <div className="kv"><span>Created By:</span><span>{task.created_by}</span></div>
          <div className="kv"><span>Assigned Agent:</span><span>{task.assignee_name || task.assigned_agent || "N/A"}</span></div>
          <div className="kv"><span>Namespace ID:</span><span className="mono">{task.namespace_id}</span></div>
          <div className="kv"><span>Created At:</span><span>{new Date(task.created_at).toLocaleString()}</span></div>
          <div className="kv"><span>Updated At:</span><span>{new Date(task.updated_at).toLocaleString()}</span></div>
          <div className="kv stretch"><span>Input:</span><pre className="code" style={{ whiteSpace: "pre-wrap" }}>{JSON.stringify(task.input, null, 2)}</pre></div>
        </div>
      ) : (
        <div className="empty sm">No task details found.</div>
      )}
    </div>
  );
}

function RunDetailPanel({ runId, onClose, onOpenRun, onRetried }: { runId: string; onClose: () => void; onOpenRun: (runId: string) => void; onRetried: (newRunId: string) => void }) {
  const { data: run, loading: loadingRun, error: errorRun, reload: reloadRun } = useAsync<ReluxRunDetail>(
    () => reluxWork.getRun(runId),
    [runId],
  );
  // Incremental live-tail for the transcript. Instead of re-fetching the whole
  // transcript each poll (the old behavior), we keep the accumulated events and
  // re-fetch only the tail past `cursorRef` (the highest event id we hold),
  // merging the new events on. The first load (and any recovery) fetches the
  // full transcript by passing no cursor.
  const [events, setEvents] = useState<ReluxRunEvent[] | null>(null);
  const [eventsLoading, setEventsLoading] = useState(true);
  const [eventsError, setEventsError] = useState<string | null>(null);
  const cursorRef = useRef<string | null>(null);
  // Wall-clock instant of the last observed activity (a new transcript event or
  // a run phase/status change). Drives the honest "no activity" stalled signal —
  // the Relux event `ts` is a logical clock, so staleness must be measured here
  // against real time, never derived from `ts`.
  const [lastActivityAt, setLastActivityAt] = useState<number | null>(null);
  const [nowMs, setNowMs] = useState<number>(() => Date.now());
  const [retrying, setRetrying] = useState(false);
  // Copy-link state: the shareable absolute `/work?run=` URL is the same one a
  // deep link restores, so an operator can hand a run to a teammate. Reset when
  // the panel switches runs so a stale "copied" note never sticks.
  const [shareNote, setShareNote] = useState<string | null>(null);
  useEffect(() => { setShareNote(null); }, [runId]);

  async function copyLink() {
    const url = workRunShareUrl(runId, window.location.origin);
    try {
      await navigator.clipboard?.writeText(url);
      setShareNote("✓ link copied");
    } catch {
      // Clipboard blocked (insecure context / denied) — surface the URL inline
      // so it can still be copied by hand. Never silently fail.
      setShareNote(url);
    }
  }

  const inFlight = isRunInFlight(run?.status);

  // First load (and on run switch): fetch the FULL transcript, seed the cursor,
  // and mark activity. Resets the accumulated state so a different run never
  // shows the prior run's events.
  useEffect(() => {
    let on = true;
    setEvents(null);
    setEventsLoading(true);
    setEventsError(null);
    cursorRef.current = null;
    reluxWork
      .getRunEvents(runId)
      .then((evs) => {
        if (!on) return;
        setEvents(evs);
        cursorRef.current = latestReluxEventId(evs);
        setLastActivityAt(Date.now());
      })
      .catch((e) => {
        if (on) setEventsError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (on) setEventsLoading(false);
      });
    return () => {
      on = false;
    };
  }, [runId]);

  // Light incremental polling while the run is still in flight. Execution is
  // synchronous, so a run is usually already terminal when this panel opens;
  // this only keeps a panel left open during a long CLI run fresh. The run
  // record is small (re-fetched whole), but the transcript fetches ONLY the
  // tail past our cursor and merges it on — no full re-fetch, no fake progress.
  useEffect(() => {
    if (!inFlight) return;
    const t = setInterval(() => {
      reloadRun();
      reluxWork
        .getRunEvents(runId, cursorRef.current ?? undefined)
        .then((tail) => {
          if (tail.length === 0) return; // nothing new — let the stall signal grow
          setEvents((prev) => mergeReluxRunEvents(prev ?? [], tail));
          const next = latestReluxEventId(tail);
          if (next) cursorRef.current = next;
          setLastActivityAt(Date.now());
        })
        .catch(() => {
          // Transient poll error: keep the last good transcript rather than
          // clearing it. The next tick retries from the same cursor.
        });
    }, 1500);
    return () => clearInterval(t);
  }, [inFlight, runId, reloadRun]);

  // A run phase/status change is real activity even if no transcript event
  // arrived this tick. Resetting here also counts the panel opening as activity,
  // so the stall signal only fires after genuine silence.
  useEffect(() => {
    setLastActivityAt(Date.now());
  }, [run?.phase, run?.status]);

  // Tick a wall clock once a second while in flight so the "no activity for Xs"
  // signal ages live without re-fetching anything. Stops when the run settles.
  useEffect(() => {
    if (!inFlight) return;
    setNowMs(Date.now());
    const t = setInterval(() => setNowMs(Date.now()), 1000);
    return () => clearInterval(t);
  }, [inFlight]);

  // Honest stalled signal: in-flight but no new event/phase for a while. Null
  // while activity is recent (normal live indicator shown instead).
  const stalledNote = inFlight ? noActivityLabel(lastActivityAt, nowMs) : null;

  async function retry() {
    setRetrying(true);
    try {
      const res = await reluxWork.retryRun(runId);
      onRetried(res.run_id);
    } catch (e) {
      alert(e instanceof Error ? e.message : "Retry failed");
    } finally {
      setRetrying(false);
    }
  }

  const error = errorRun;
  const duration = run ? formatRunDuration(run.duration_ms) : null;
  const metrics = run ? runMetricsLine(run) : null;
  // Tool-call count is derived from the real transcript (not the run record), so
  // it only appears once the events have loaded and the kernel actually recorded
  // tool activity. §11.3 Active Runs lists "tool calls" as a run-depth field.
  const toolCalls = toolCallSummary(events);
  // Honest applicability of the legacy artifact/diff/apply/review affordances for
  // THIS run. Relux run records carry none of that data, so we surface the reason
  // instead of hiding it or wiring dead buttons.
  const reviewApply = run ? reviewApplyAvailability(run) : null;
  // Read-only artifact references the adapter declared in its result envelope
  // (§9.6 / §15). These are references (name/type/summary/source), NOT a diff or
  // an apply plan — we list them but apply stays unavailable (see reviewApply).
  const artifacts = run ? runArtifacts(run) : [];
  // Reviewable proposed file changes (master plan §15 diff/apply model). These
  // carry content + a baseline hash and drive the real approve/apply controls.
  const proposedChanges = run ? runProposedChanges(run) : [];
  // Per-change busy flag (by index), so one button shows a pending state without
  // freezing the whole panel.
  const [pcBusy, setPcBusy] = useState<number | null>(null);
  // Busy flag for a batch (multi-file) operation, so the batch toolbar shows a
  // pending state without colliding with the per-change buttons.
  const [batchBusy, setBatchBusy] = useState(false);
  // Indices for the batch toolbar: still-reviewable changes (Approve all) and
  // apply-eligible changes (Apply all approved). The backend re-validates both.
  const reviewableIndices = reviewableProposedChangeIndices(proposedChanges);
  const applyEligibleIndices = applyEligibleProposedChangeIndices(proposedChanges);

  async function reviewChange(index: number, decision: "approve" | "reject") {
    setPcBusy(index);
    try {
      await reluxWork.reviewProposedChange(runId, index, decision);
      reloadRun();
    } catch (e) {
      alert(e instanceof Error ? e.message : "Review failed");
    } finally {
      setPcBusy(null);
    }
  }

  async function applyChange(index: number) {
    setPcBusy(index);
    try {
      const res = await reluxWork.applyProposedChange(runId, index);
      reloadRun();
      alert(`Applied ${res.path} (${res.bytes} bytes).`);
    } catch (e) {
      // An honest refusal (conflict / no baseline / no workspace) surfaces here.
      alert(e instanceof Error ? e.message : "Apply failed");
      reloadRun();
    } finally {
      setPcBusy(null);
    }
  }

  // Approve every still-reviewable change. Approval touches no files, so doing it
  // sequentially is safe; the real all-or-nothing guarantee is on the apply below.
  async function approveAll(indices: number[]) {
    setBatchBusy(true);
    try {
      for (const i of indices) {
        await reluxWork.reviewProposedChange(runId, i, "approve");
      }
    } catch (e) {
      alert(e instanceof Error ? e.message : "Approve all failed");
    } finally {
      setBatchBusy(false);
      reloadRun();
    }
  }

  // Apply every approved change as ONE transaction. The backend writes all or
  // none — a single refusal (conflict / unsafe / duplicate / missing baseline)
  // leaves every file untouched and reports the honest reason here.
  async function applyAll(indices: number[]) {
    setBatchBusy(true);
    try {
      const res = await reluxWork.applyProposedChangeSet(runId, indices);
      reloadRun();
      alert(`Applied ${res.applied.length} file(s) as one transaction.`);
    } catch (e) {
      alert(e instanceof Error ? e.message : "Apply all failed (no files were changed)");
      reloadRun();
    } finally {
      setBatchBusy(false);
    }
  }

  return (
    <div style={{ paddingBottom: 16 }}>
      {/* Title + status/live-stalled cues group on the left; the Copy-link /
          Retry / Close controls stay together on the right. Two groups (the
          shared `.xtr-bar` split, not a flex-1 spacer) so a long stalled cue
          wraps within the meta group and the action buttons wrap as one unit in
          a narrow card — they never get squeezed or label-wrapped. Matches the
          legacy RunTranscript header (relix-dashboard-design §8 / §11). */}
      <div className="xtr-bar" style={{ marginBottom: 12 }}>
        <div className="xtr-bar-meta">
          <h4 style={{ margin: 0 }}>Run Detail</h4>
          {run && <span className={`badge ${runStatusTone(run.status)}`}>{run.status}</span>}
          {inFlight &&
            (stalledNote ? (
              /* Honest in-flight state: the stall signal reports real elapsed
                 silence (no new event/phase for a while). Same chip language as
                 the legacy RunTranscript header (◌ + the honest "no activity"
                 label, `badge in_progress`) so the two surfaces read identically
                 — never a fabricated progress bar (relix-dashboard-design §8 / §11). */
              <span
                className="badge in_progress"
                style={{ fontSize: 9, fontWeight: 600 }}
                title="real elapsed silence — no new event/phase has arrived for a while (not a guaranteed stall, just no observed activity)"
              >
                ◌ {stalledNote}
              </span>
            ) : (
              /* Live indicator, matching RunTranscript's `● live` chip (`badge
                 done` tone): the panel live-tails the transcript and re-polls the
                 run record while it is in flight. */
              <span
                className="badge done"
                style={{ fontSize: 9, fontWeight: 600 }}
                title="this run is in flight — the panel live-tails the transcript and re-polls the run record"
              >
                ● live
              </span>
            ))}
        </div>
        <div className="xtr-bar-actions">
          <button className="btn ghost sm" title="Copy a shareable link to this run" onClick={() => void copyLink()}>
            Copy link
          </button>
          {run && canRetryRun(run) && (
            <button className="btn sm" onClick={() => void retry()} disabled={retrying}>
              {retrying ? "Retrying…" : "Retry"}
            </button>
          )}
          <button className="btn ghost sm" onClick={onClose}>Close</button>
        </div>
      </div>
      {shareNote && (
        <div className="muted mono" style={{ fontSize: 11, marginBottom: 8, wordBreak: "break-all" }}>{shareNote}</div>
      )}
      {loadingRun && !run ? (
        <div className="loading">Loading run details...</div>
      ) : error ? (
        <div className="banner err" style={{ fontSize: 12 }}>
          Could not load run <span className="mono">{runId}</span> ({String(error)}).
          It may no longer exist or the Relux API is unreachable — use Close to go back.
        </div>
      ) : run ? (
        <div className="grid" style={{ gap: 8, fontSize: 12 }}>
          <div className="kv"><span>ID:</span><span className="mono">{run.id}</span></div>
          {run.task_title && <div className="kv"><span>Task:</span><span>{run.task_title}</span></div>}
          <div className="kv"><span>Task ID:</span><span className="mono">{run.task_id}</span></div>
          <div className="kv"><span>Agent ID:</span><span className="mono">{run.agent_id}</span></div>
          <div className="kv"><span>Adapter:</span><span className="mono">{run.adapter_plugin}</span></div>
          <div className="kv"><span>Phase:</span><span>{phaseLabel(run.phase, run.status)}</span></div>
          <div className="kv"><span>Duration:</span><span>{duration ?? "—"}</span></div>
          {metrics && <div className="kv"><span>Metrics:</span><span>{metrics}</span></div>}
          {toolCalls && <div className="kv"><span>Tool calls:</span><span>{toolCalls}</span></div>}
          {run.retried_from && (
            <div className="kv"><span>Retry of:</span>
              {/* Same Relux ledger → inspect the parent run in-shell via /work?run=. */}
              <a
                className="link mono"
                href={`?run=${encodeURIComponent(run.retried_from)}`}
                onClick={(e) => { e.preventDefault(); onOpenRun(run.retried_from!); }}
              >
                {run.retried_from}
              </a>
            </div>
          )}
          {/* Logical-sequence timestamps (ordering, not wall-clock). Real timing is "Duration" above. */}
          <div className="kv"><span>Sequence:</span><span className="mono">{run.started_at ?? "—"} → {run.ended_at ?? "(in progress)"}</span></div>
          {run.failure_class && (
            <div className="kv"><span>Failure class:</span>
              <span className={`badge ${failureClassTone(run.failure_class)}`} style={{ fontSize: 10 }}>
                {failureClassLabel(run.failure_class)}
              </span>
            </div>
          )}
          {run.failure_class && (() => {
            // Recovery status reads against the current wall clock; the kernel
            // owns the authoritative not-before instant, this is a display read.
            const line = recoveryStatusLine(run, Math.floor(Date.now() / 1000));
            return line ? <div className="kv"><span>Recovery:</span><span>{line}</span></div> : null;
          })()}
          {run.failure_reason && (
            <div className="kv stretch"><span>Failure reason:</span>
              <pre className="code" style={{ whiteSpace: "pre-wrap", color: "var(--err, #b00)" }}>{run.failure_reason}</pre>
            </div>
          )}
          {run.failure_remediation && (
            <div className="kv stretch"><span>Remediation:</span>
              <span className="muted">{run.failure_remediation}</span>
            </div>
          )}
          {run.summary && <div className="kv stretch"><span>Summary:</span><pre className="code" style={{ whiteSpace: "pre-wrap" }}>{run.summary}</pre></div>}
          {run.output_excerpt && (
            <div className="kv stretch"><span>Output excerpt:</span>
              <pre className="code" style={{ whiteSpace: "pre-wrap", maxHeight: 240, overflow: "auto" }}>{run.output_excerpt}</pre>
            </div>
          )}
          <h5 style={{ marginTop: 16, marginBottom: 8 }}>
            Transcript
            {/* The stall signal also rides next to the transcript header, where
                an operator watching the live tail is looking. Same badge chip
                language as the legacy RunTranscript stalled cue (◌ + the honest
                "no activity" label, `badge in_progress`) so the two transcript
                surfaces read identically (relix-dashboard-design §8 / §11). */}
            {stalledNote && (
              <span
                className="badge in_progress"
                style={{ fontSize: 9, fontWeight: 600, marginLeft: 8, verticalAlign: "middle" }}
                title="real elapsed silence — no new event/phase has arrived for a while (not a guaranteed stall, just no observed activity)"
              >
                ◌ {stalledNote}
              </span>
            )}
          </h5>
          {eventsLoading && !events ? (
            <div className="loading">Loading events...</div>
          ) : eventsError ? (
            <div className="banner err" style={{ fontSize: 12 }}>
              Error loading events: {String(eventsError)}
            </div>
          ) : events && events.length > 0 ? (
            <div className="table-scroll" style={{ maxHeight: 300 }}>
              <table className="table sm">
                <thead>
                  <tr>
                    <th>Seq</th>
                    <th>Phase</th>
                    <th>Source</th>
                    <th>Message</th>
                  </tr>
                </thead>
                <tbody>
                  {events.map((event) => {
                    const preview = eventPayloadPreview(event.payload);
                    return (
                      <tr key={event.id}>
                        <td className="mono" style={{ fontSize: 10 }}>{event.ts}</td>
                        <td>{phaseLabel(event.kind, undefined)}</td>
                        <td>{event.source}</td>
                        <td className="muted" style={{ fontSize: 11 }}>
                          {event.message}
                          {preview && (
                            <pre className="code" style={{ whiteSpace: "pre-wrap", marginTop: 4 }}>
                              {preview}
                            </pre>
                          )}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          ) : (
            <div className="empty sm">No events found for this run.</div>
          )}
          {/* Read-only artifact references the adapter declared in its result
              envelope. References only (name/type/summary/source) — no diff, no
              apply. Rendered when present; otherwise an honest empty state. */}
          <h5 style={{ marginTop: 16, marginBottom: 8 }}>Artifacts</h5>
          {artifacts.length > 0 ? (
            <div className="table-scroll" style={{ maxHeight: 240 }}>
              <table className="table sm">
                <thead>
                  <tr>
                    <th>Name</th>
                    <th>Type</th>
                    <th>Summary</th>
                    <th>Source</th>
                  </tr>
                </thead>
                <tbody>
                  {artifacts.map((a, i) => (
                    <tr key={`${a.name}-${i}`}>
                      <td className="mono" style={{ fontSize: 11 }}>
                        {a.name}
                        {a.path && a.path !== a.name && (
                          <div className="muted" style={{ fontSize: 10 }}>{a.path}</div>
                        )}
                      </td>
                      <td>
                        {artifactTypeLabel(a.type)}
                        {typeof a.bytes === "number" && (
                          <div className="muted" style={{ fontSize: 10 }}>{a.bytes} B</div>
                        )}
                      </td>
                      <td className="muted" style={{ fontSize: 11 }}>
                        {a.summary ?? "—"}
                        {a.truncated && <span title="truncated"> …</span>}
                      </td>
                      <td className="muted" style={{ fontSize: 11 }}>{a.source}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <div className="empty sm">No artifacts declared for this run.</div>
          )}
          {/* Reviewable proposed file changes (master plan §15 diff/apply model):
              full-content replacements with a baseline hash. Approve a change to
              enable Apply; applying writes into the run's controlled workspace
              root after a baseline-conflict check. Refusals are shown honestly. */}
          <h5 style={{ marginTop: 16, marginBottom: 8 }}>Proposed Changes</h5>
          {/* Batch (multi-file) controls: only shown when a run has more than one
              proposed change (a single change uses the per-change flow below).
              "Apply all approved" writes every approved change as ONE all-or-
              nothing transaction — the backend writes all or none. */}
          {showBatchProposedChangeControls(proposedChanges) && (
            <div className="row" style={{ alignItems: "center", gap: 8, marginBottom: 8 }}>
              <span className="muted" style={{ fontSize: 11 }}>
                {proposedChanges.length} changes
              </span>
              <div className="spacer" style={{ flex: 1 }} />
              {reviewableIndices.length > 0 && (
                <button
                  className="btn ghost sm"
                  disabled={batchBusy || pcBusy !== null}
                  title="Approve every change still awaiting review"
                  onClick={() => void approveAll(reviewableIndices)}
                >
                  {batchBusy ? "…" : `Approve all (${reviewableIndices.length})`}
                </button>
              )}
              <button
                className="btn sm"
                disabled={batchBusy || pcBusy !== null || applyEligibleIndices.length === 0}
                title={
                  applyEligibleIndices.length > 0
                    ? "Apply every approved change as one all-or-nothing transaction"
                    : "Approve changes first — apply needs an approved change with a baseline hash"
                }
                onClick={() => void applyAll(applyEligibleIndices)}
              >
                {batchBusy ? "Applying…" : `Apply all approved (${applyEligibleIndices.length})`}
              </button>
            </div>
          )}
          {proposedChanges.length > 0 ? (
            <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
              {proposedChanges.map((c, i) => (
                <div key={`${c.path}-${i}`} className="card" style={{ padding: 10 }}>
                  <div className="row" style={{ alignItems: "center", gap: 8 }}>
                    <span
                      className="mono"
                      style={{ fontSize: 12 }}
                      title={isRenameProposedChange(c) ? "Source → destination" : undefined}
                    >
                      {proposedChangePathLabel(c)}
                    </span>
                    <span className="badge backlog" title="The filesystem action this change applies">
                      {proposedChangeActionLabel(c.action)}
                    </span>
                    <span className={`badge ${proposedChangeStatusTone(c.status)}`}>
                      {proposedChangeStatusLabel(c.status)}
                    </span>
                    <span className="muted" style={{ fontSize: 10 }}>
                      {isRenameProposedChange(c)
                        ? "move"
                        : isDeleteProposedChange(c)
                          ? "delete"
                          : `${c.bytes} B`}{" "}
                      · {c.source}
                    </span>
                    <div className="spacer" style={{ flex: 1 }} />
                    {canReviewProposedChange(c) && (
                      <>
                        <button
                          className="btn sm"
                          disabled={pcBusy === i}
                          onClick={() => void reviewChange(i, "approve")}
                        >
                          {pcBusy === i ? "…" : "Approve"}
                        </button>
                        <button
                          className="btn ghost sm"
                          disabled={pcBusy === i}
                          onClick={() => void reviewChange(i, "reject")}
                        >
                          Reject
                        </button>
                      </>
                    )}
                    {c.status === "approved" && (
                      <button
                        className="btn sm"
                        disabled={pcBusy === i || !canApplyProposedChange(c)}
                        title={
                          canApplyProposedChange(c)
                            ? isCreateProposedChange(c)
                              ? "Create the new file in the run's workspace root"
                              : isRenameProposedChange(c)
                                ? "Move the file to its destination in the run's workspace root"
                                : isDeleteProposedChange(c)
                                  ? "Delete the file from the run's workspace root"
                                  : "Write the new content into the run's workspace root"
                            : "Apply needs a baseline hash (none was recorded)"
                        }
                        onClick={() => void applyChange(i)}
                      >
                        {pcBusy === i ? "Applying…" : "Apply"}
                      </button>
                    )}
                  </div>
                  {isCreateProposedChange(c) ? (
                    <div className="muted" style={{ fontSize: 10, marginTop: 4 }}>
                      New file — created only if it does not already exist (no baseline needed).
                    </div>
                  ) : isRenameProposedChange(c) ? (
                    <>
                      <div className="muted" style={{ fontSize: 10, marginTop: 4 }}>
                        Move — applied only if {c.dest_path ?? "the destination"} does not already
                        exist and the source still matches its baseline.
                      </div>
                      {!c.baseline_sha256 && (
                        <div className="muted" style={{ fontSize: 10, marginTop: 4 }}>
                          No baseline hash — apply is refused (no force in v1).
                        </div>
                      )}
                    </>
                  ) : isDeleteProposedChange(c) ? (
                    <>
                      <div className="muted" style={{ fontSize: 10, marginTop: 4 }}>
                        Delete — the file is removed only if it still matches its baseline.
                      </div>
                      {!c.baseline_sha256 && (
                        <div className="muted" style={{ fontSize: 10, marginTop: 4 }}>
                          No baseline hash — apply is refused (no force in v1).
                        </div>
                      )}
                    </>
                  ) : (
                    !c.baseline_sha256 && (
                      <div className="muted" style={{ fontSize: 10, marginTop: 4 }}>
                        No baseline hash — apply is refused (no force in v1).
                      </div>
                    )
                  )}
                  {c.refused_reason && (
                    <div className="banner err" style={{ fontSize: 10, marginTop: 6 }}>
                      Refused: {c.refused_reason}
                    </div>
                  )}
                  {c.status === "applied" && c.applied_at && (
                    <div className="muted" style={{ fontSize: 10, marginTop: 4 }}>
                      Applied at {c.applied_at}.
                    </div>
                  )}
                  {c.review_note && (
                    <div className="muted" style={{ fontSize: 10, marginTop: 4 }}>
                      Note: {c.review_note}
                    </div>
                  )}
                  {/* Read-only preview of the full proposed content. A rename moves
                      the file intact and a delete removes it, so neither has new
                      content to preview. */}
                  {isRenameProposedChange(c) ? (
                    <div className="muted" style={{ fontSize: 10, marginTop: 6 }}>
                      No content change — the file is moved intact.
                    </div>
                  ) : isDeleteProposedChange(c) ? (
                    <div className="muted" style={{ fontSize: 10, marginTop: 6 }}>
                      No content — the file is removed.
                    </div>
                  ) : (
                    <details style={{ marginTop: 6 }}>
                      <summary style={{ cursor: "pointer", fontSize: 11 }}>Preview new content</summary>
                      <pre
                        className="mono"
                        style={{ fontSize: 11, maxHeight: 240, overflow: "auto", whiteSpace: "pre-wrap", marginTop: 6 }}
                      >
                        {c.new_content}
                      </pre>
                    </details>
                  )}
                </div>
              ))}
            </div>
          ) : (
            <div className="empty sm">No proposed changes for this run.</div>
          )}
          {/* The honest availability line: apply is real when this run proposed
              changes (above); otherwise it explains why apply is unavailable
              rather than hiding it or wiring dead controls. */}
          <h5 style={{ marginTop: 16, marginBottom: 8 }}>Review &amp; Apply</h5>
          {reviewApply && (
            <div className="banner" style={{ fontSize: 11, lineHeight: 1.5 }}>
              {reviewApply.reason}
            </div>
          )}
        </div>
      ) : (
        <div className="empty sm">No run details found.</div>
      )}
    </div>
  );
}

function AuditPanel() {
  const { data: auditEntries, loading, error } = useAsync<ReluxAuditEntry[]>(
    () => reluxAudit.list(20),
    [],
  );

  return (
    <div className="card" style={{ marginTop: 24, padding: 16 }}>
      <h4 style={{ marginTop: 0 }}>Recent Audit</h4>
      {loading ? (
        <div className="loading">Loading audit entries...</div>
      ) : error ? (
        <div className="banner err" style={{ fontSize: 12 }}>
          Error loading audit entries: {String(error)}
        </div>
      ) : auditEntries && auditEntries.length > 0 ? (
        <div className="table-scroll" style={{ maxHeight: 300 }}>
          <table className="table sm">
            <thead>
              <tr>
                <th>Time</th>
                <th>Actor</th>
                <th>Action</th>
                <th>Target</th>
                <th>Result</th>
              </tr>
            </thead>
            <tbody>
              {auditEntries.map((entry, index) => (
                <tr key={index}>
                  <td>{new Date(entry.ts).toLocaleString()}</td>
                  <td>{entry.actor}</td>
                  <td>{entry.action}</td>
                  <td>{entry.target}</td>
                  <td>{entry.result}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <div className="empty sm">No audit entries found.</div>
      )}
    </div>
  );
}
