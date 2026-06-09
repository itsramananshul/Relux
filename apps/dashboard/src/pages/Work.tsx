import { useState, useMemo } from "react";
import { reluxWork, type ReluxTask, type ReluxRun, type ReluxAgent } from "../api";
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
              <Column title="Open" tasks={columns.open} onAction={handleReload} agents={agents || []} />
              <Column title="Running" tasks={columns.running} onAction={handleReload} agents={agents || []} />
              <Column title="Done" tasks={columns.done} onAction={handleReload} agents={agents || []} />
            </div>

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
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              ) : (
                <div className="empty sm">No runs yet.</div>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function Column({ title, tasks, onAction, agents }: { title: string; tasks: ReluxTask[]; onAction: () => void; agents: ReluxAgent[] }) {
  return (
    <div style={{ flex: 1, minWidth: 280 }}>
      <h4 style={{ marginBottom: 8, fontSize: 13, textTransform: "uppercase", letterSpacing: "0.05em" }}>
        {title} <span className="muted" style={{ fontWeight: 400 }}>{tasks.length}</span>
      </h4>
      <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        {tasks.map(t => (
          <TaskCard key={t.id} task={t} onAction={onAction} agents={agents} />
        ))}
        {tasks.length === 0 && <div className="empty sm" style={{ padding: 16 }}>No {title.toLowerCase()} tasks</div>}
      </div>
    </div>
  );
}

function TaskCard({ task, onAction, agents }: { task: ReluxTask; onAction: () => void; agents: ReluxAgent[] }) {
  const [busy, setBusy] = useState(false);
  const [selectedAgent, setSelectedAgent] = useState(task.assigned_agent || "");
  
  async function start() {
    setBusy(true);
    try {
      await reluxWork.startTask(task.id);
      onAction();
    } catch (e) {
      alert(e instanceof Error ? e.message : "Start failed");
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

  const isStartable = task.status === "created" || task.status === "queued";

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
        {task.assignee_name ? (
          <div className="mono muted" style={{ fontSize: 10 }}>Assigned: {task.assignee_name}</div>
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
        {isStartable && (
          <button className="btn sm" style={{ height: 24, padding: "0 8px" }} onClick={() => void start()} disabled={busy}>
            {busy ? "..." : "Start"}
          </button>
        )}
      </div>
    </div>
  );
}
