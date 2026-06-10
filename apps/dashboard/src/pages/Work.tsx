import { useState, useMemo, useEffect } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { runIdFromSearch } from "../routing";
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
} from "../runview";

// Relux Work page: standalone surface for tasks and runs.
// BACKED BY: /v1/relux/tasks, /v1/relux/runs.

export function Work() {
  const location = useLocation();
  const navigate = useNavigate();
  const queryParams = useMemo(() => new URLSearchParams(location.search), [location.search]);
  const filterAgentId = queryParams.get("agentId");
  const filterStatus = queryParams.get("status");
  // Run detail is URL-driven: `/work?run=<id>` opens that run's panel. Making the
  // URL the source of truth (rather than only local state) lets a deep link from
  // an orchestration step's run_id, plus browser back/forward/refresh, restore the
  // same view. A missing/empty param simply shows no run panel.
  const selectedRunId = runIdFromSearch(location.search);

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
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);

  // Point the URL at a run (or clear it), preserving any other Work filters in the
  // querystring. This is the single way run selection changes, so it stays in sync
  // with deep links and the browser history.
  const setSelectedRunId = (runId: string | null) => {
    if ((runId ?? null) === selectedRunId) return; // no-op: don't push a duplicate history entry
    const next = new URLSearchParams(location.search);
    if (runId) next.set("run", runId);
    else next.delete("run");
    const search = next.toString();
    navigate({ search: search ? `?${search}` : "" }, { replace: false });
  };

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

  const handleInspectTask = (taskId: string) => {
    setSelectedTaskId(taskId);
    setSelectedRunId(null);
  };

  const handleInspectRun = (runId: string) => {
    setSelectedRunId(runId);
    setSelectedTaskId(null);
  };

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
                                retry of {run.retried_from}
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

function RunDetailPanel({ runId, onClose, onRetried }: { runId: string; onClose: () => void; onRetried: (newRunId: string) => void }) {
  const { data: run, loading: loadingRun, error: errorRun, reload: reloadRun } = useAsync<ReluxRunDetail>(
    () => reluxWork.getRun(runId),
    [runId],
  );
  const { data: events, loading: loadingEvents, error: errorEvents, reload: reloadEvents } = useAsync<ReluxRunEvent[]>(
    () => reluxWork.getRunEvents(runId),
    [runId],
  );
  const [retrying, setRetrying] = useState(false);

  // Light polling while the run is still in flight. Execution is synchronous, so
  // a run is usually already terminal when this panel opens; this only keeps a
  // panel left open during a long CLI run fresh. No fake progress: we just
  // re-fetch the real recorded run + transcript.
  const inFlight = isRunInFlight(run?.status);
  useEffect(() => {
    if (!inFlight) return;
    const t = setInterval(() => {
      reloadRun();
      reloadEvents();
    }, 1500);
    return () => clearInterval(t);
  }, [inFlight, reloadRun, reloadEvents]);

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

  return (
    <div style={{ paddingBottom: 16 }}>
      <div className="row" style={{ alignItems: "center", marginBottom: 12 }}>
        <h4 style={{ margin: 0 }}>Run Detail</h4>
        {run && <span className={`badge ${runStatusTone(run.status)}`} style={{ marginLeft: 8 }}>{run.status}</span>}
        {inFlight && <span className="muted" style={{ fontSize: 11, marginLeft: 8 }}>live · refreshing…</span>}
        <div className="spacer" style={{ flex: 1 }} />
        {run && canRetryRun(run) && (
          <button className="btn sm" style={{ marginRight: 8 }} onClick={() => void retry()} disabled={retrying}>
            {retrying ? "Retrying…" : "Retry"}
          </button>
        )}
        <button className="btn ghost sm" onClick={onClose}>Close</button>
      </div>
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
          {run.retried_from && <div className="kv"><span>Retry of:</span><span className="mono">{run.retried_from}</span></div>}
          {/* Logical-sequence timestamps (ordering, not wall-clock). Real timing is "Duration" above. */}
          <div className="kv"><span>Sequence:</span><span className="mono">{run.started_at ?? "—"} → {run.ended_at ?? "(in progress)"}</span></div>
          {run.failure_reason && (
            <div className="kv stretch"><span>Failure reason:</span>
              <pre className="code" style={{ whiteSpace: "pre-wrap", color: "var(--err, #b00)" }}>{run.failure_reason}</pre>
            </div>
          )}
          {run.summary && <div className="kv stretch"><span>Summary:</span><pre className="code" style={{ whiteSpace: "pre-wrap" }}>{run.summary}</pre></div>}
          {run.output_excerpt && (
            <div className="kv stretch"><span>Output excerpt:</span>
              <pre className="code" style={{ whiteSpace: "pre-wrap", maxHeight: 240, overflow: "auto" }}>{run.output_excerpt}</pre>
            </div>
          )}
          <h5 style={{ marginTop: 16, marginBottom: 8 }}>Transcript</h5>
          {loadingEvents && !events ? (
            <div className="loading">Loading events...</div>
          ) : errorEvents ? (
            <div className="banner err" style={{ fontSize: 12 }}>
              Error loading events: {String(errorEvents)}
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
