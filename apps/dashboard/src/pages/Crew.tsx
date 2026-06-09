import { useState, useEffect, useMemo } from "react";
import { useLoaderData, Link } from "react-router-dom";
import { fetchJson, postJson, reluxWork, type ReluxAgent, type ReluxTask } from "../api";
import { useAsync } from "../components/common";

type Agent = ReluxAgent;

export async function loader() {
  return await fetchJson("/v1/relux/agents");
}

export function Crew() {
  const initialAgents = useLoaderData() as Agent[];
  const [agents, setAgents] = useState<Agent[]>(initialAgents);
  const [name, setName] = useState("");
  const [role, setRole] = useState("");
  const [error, setError] = useState<string | null>(null);

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


  const fetchAgents = async () => {
    try {
      const data = (await fetchJson("/v1/relux/agents")) as Agent[];
      setAgents(data);
      void reloadTasks(); // Also reload tasks to update counts for agents
    } catch (err) {
      console.error("Failed to fetch agents:", err);
      setError("Failed to load agents.");
    }
  };

  const handleCreateAgent = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    try {
      const newAgent = (await postJson("/v1/relux/agents", { name, role })) as Agent;
      setAgents((prev) => [...prev, newAgent]);
      setName("");
      setRole("");
    } catch (err: any) {
      console.error("Failed to create agent:", err);
      setError(err.message || "Failed to create agent.");
    }
  };

  useEffect(() => {
    fetchAgents();
  }, []);

  return (
    <div className="crew-page">
      <div className="section">
        <h2>Your Crew</h2>
        {error && <div className="error-message">{error}</div>}
        {tasksError && (
          <div className="error-message">
            Error loading tasks: {String(tasksError)}
          </div>
        )}
        <div className="agent-list">
          {agents.length === 0 ? (
            <p>No agents found. Create one below!</p>
          ) : (
            agents.map((agent) => (
              <div key={agent.id} className="agent-card">
                <h3>{agent.name} ({agent.id})</h3>
                <p><strong>Role:</strong> {agent.description || "N/A"}</p>
                <p><strong>Status:</strong> {agent.status}</p>
                <p><strong>Adapter:</strong> {agent.adapter_plugin}</p>
                <p><strong>Permissions:</strong> {agent.permissions_summary}</p>
                <p>
                  <strong>Queued Tasks:</strong>{" "}
                  <Link to={`/work?agentId=${agent.id}&status=queued`} className="link">
                    {agentTaskCounts[agent.id]?.queued || 0}
                  </Link>
                </p>
                <p>
                  <strong>Running Tasks:</strong>{" "}
                  <Link to={`/work?agentId=${agent.id}&status=running`} className="link">
                    {agentTaskCounts[agent.id]?.running || 0}
                  </Link>
                </p>
                <p className="created-at">Created: {new Date(agent.created_at).toLocaleString()}</p>
              </div>
            ))
          )}
        </div>
      </div>

      <div className="section">
        <h2>Create New Crew Member</h2>
        <form onSubmit={handleCreateAgent} className="create-agent-form">
          <div className="form-group">
            <label htmlFor="agent-name">Name:</label>
            <input
              id="agent-name"
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              required
            />
          </div>
          <div className="form-group">
            <label htmlFor="agent-role">Role/Description (optional):</label>
            <input
              id="agent-role"
              type="text"
              value={role}
              onChange={(e) => setRole(e.target.value)}
            />
          </div>
          <button type="submit" className="btn primary">Create Agent</button>
        </form>
      </div>
    </div>
  );
}
