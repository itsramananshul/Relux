import { useState, useMemo } from "react";
import { reluxWork, reluxAudit, type ReluxTask, type ReluxRun, type ReluxAgent, type ReluxTaskDetail, type ReluxRunDetail, type ReluxAuditEntry, type RunEvent } from "../api";
import { useAsync } from "../components/common";

// Relux Work page: standalone surface for tasks and runs.
// BACKED BY: /v1/relux/tasks, /v1/relux/runs.

export function Work() {
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
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null);

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
    const list = tasks ?? [];
    return {
      open: list.filter(t => t.status === "created" || t.status === "queued"),
      running: list.filter(t => t.status === "running"),
      done: list.filter(t => t.status === "completed"),
      other: list.filter(t => !["created", "queued", "running", "completed"].includes(t.status)),
    };
  }, [tasks]);

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
                  <RunDetailPanel runId={selectedRunId} onClose={() => setSelectedRunId(null)} />
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
                          <td className="mono" style={{ fontSize: 11 }}>{run.id}</td>
                          <td className="mono" style={{ fontSize: 11 }}>{run.task_id}</td>
                          <td className="mono" style={{ fontSize: 11 }}>
                            {agents?.find(a => a.id === run.agent_id)?.name || run.agent_id}
                          </td>
                          <td>
                            <span className={`badge ${run.status === "completed" ? "done" : run.status === "running" ? "running" : "backlog"}`}>
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
  const isRunnableByAssignedAgent = isAssigned && (task.status === "queued" || task.status === "running");

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

function RunDetailPanel({ runId, onClose }: { runId: string; onClose: () => void }) {
  const { data: run, loading: loadingRun, error: errorRun } = useAsync<ReluxRunDetail>(
    () => reluxWork.getRun(runId),
    [runId],
  );
  const { data: events, loading: loadingEvents, error: errorEvents } = useAsync<RunEvent[]>(
    () => reluxWork.getRunEvents(runId),
    [runId],
  );

  const error = errorRun || errorEvents;
  const loading = loadingRun || loadingEvents;

  return (
    <div style={{ paddingBottom: 16 }}>
      <div className="row" style={{ alignItems: "center", marginBottom: 12 }}>
        <h4 style={{ margin: 0 }}>Run Detail</h4>
        <div className="spacer" style={{ flex: 1 }} />
        <button className="btn ghost sm" onClick={onClose}>Close</button>
      </div>
      {loading ? (
        <div className="loading">Loading run details...</div>
      ) : error ? (
        <div className="banner err" style={{ fontSize: 12 }}>
          Error loading run: {String(error)}
        </div>
      ) : run ? (
        <div className="grid" style={{ gap: 8, fontSize: 12 }}>
          <div className="kv"><span>ID:</span><span className="mono">{run.id}</span></div>
          <div className="kv"><span>Task ID:</span><span className="mono">{run.task_id}</span></div>
          <div className="kv"><span>Agent ID:</span><span className="mono">{run.agent_id}</span></div>
          <div className="kv"><span>Adapter Plugin:</span><span>{run.adapter_plugin}</span></div>
          <div className="kv"><span>Status:</span><span>{run.status}</span></div>
          <div className="kv"><span>Started At:</span><span>{run.started_at ? new Date(run.started_at).toLocaleString() : "N/A"}</span></div>
          <div className="kv"><span>Ended At:</span><span>{run.ended_at ? new Date(run.ended_at).toLocaleString() : "N/A"}</span></div>
          {run.summary && <div className="kv stretch"><span>Summary:</span><pre className="code" style={{ whiteSpace: "pre-wrap" }}>{run.summary}</pre></div>}
          {run.error && <div className="kv stretch"><span>Error:</span><pre className="code" style={{ whiteSpace: "pre-wrap" }}>{run.error}</pre></div>}
          <h5 style={{ marginTop: 16, marginBottom: 8 }}>Events</h5>
          {loadingEvents ? (
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
                    <th>Time</th>
                    <th>Kind</th>
                    <th>Source</th>
                    <th>Message</th>
                  </tr>
                </thead>
                <tbody>
                  {events.map((event, index) => (
                    <tr key={index}>
                      <td>{event.ts ? new Date(event.ts * 1000).toLocaleTimeString() : "N/A"}</td>
                      <td>{event.kind}</td>
                      <td>{event.source}</td>
                      <td className="muted" style={{ fontSize: 11 }}>
                        {event.message}
                        {event.payload_json && (
                          <pre className="code" style={{ whiteSpace: "pre-wrap", marginTop: 4 }}>
                            {JSON.stringify(JSON.parse(event.payload_json), null, 2)}
                          </pre>
                        )}
                      </td>
                    </tr>
                  ))}
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
