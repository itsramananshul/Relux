// apps/dashboard/src/pages/ReluxApprovals.tsx
import React, { useEffect, useState } from "react";
import {
  ReluxApproval,
  reluxApprovals,
  ReluxAgentPermissions,
  reluxPermissions,
  ReluxAgent,
  reluxWork,
  ReluxPersistentGrant,
  reluxGrants,
} from "../api";
import { Link } from "react-router-dom";

// Relux Approvals & Permissions (relix-dashboard-design.md §"Approvals": the gate
// list + detail with Approve/Reject, and RELUX_MASTER_PLAN §7.4/§7.5 per-call tool
// approvals + allow-always grants). This surface is part of the standalone Relux
// shell, so it uses the SAME B&W design system as every other Relux page (`card`,
// `table`, `badge`, `btn`, `banner`) — not an off-aesthetic utility-class theme.
// All behavior is unchanged: the same fetch/poll, decide, execute-once,
// allow-always, revoke, and grant routes drive it; only presentation conforms.

// Approval lifecycle status → the shared badge tone (matches the board/run tones).
const APPROVAL_STATUS_TONE: Record<string, string> = {
  pending: "in_review",
  approved: "done",
  rejected: "blocked",
};

const ReluxApprovals: React.FC = () => {
  const [approvals, setApprovals] = useState<ReluxApproval[]>([]);
  const [agentPermissions, setAgentPermissions] = useState<ReluxAgentPermissions[]>([]);
  const [agents, setAgents] = useState<ReluxAgent[]>([]);
  const [grants, setGrants] = useState<ReluxPersistentGrant[]>([]);
  const [loading, setLoading] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);

  const [selectedAgentId, setSelectedAgentId] = useState<string>("");
  const [permissionInput, setPermissionInput] = useState<string>("");
  const [grantPermissionError, setGrantPermissionError] = useState<string | null>(null);
  const [grantPermissionSuccess, setGrantPermissionSuccess] = useState<string | null>(null);
  // Per-approval execution feedback for the per-call tool-invocation flow, keyed
  // by approval id: a success line (the structured output) or an honest error.
  const [execResult, setExecResult] = useState<Record<string, string>>({});
  const [execError, setExecError] = useState<Record<string, string>>({});

  const fetchApprovals = async () => {
    try {
      setLoading(true);
      const data = await reluxApprovals.list();
      setApprovals(data);
    } catch (err) {
      console.error("Failed to fetch approvals:", err);
      setError("Failed to load approvals.");
    } finally {
      setLoading(false);
    }
  };

  const fetchAgentPermissions = async () => {
    try {
      const data = await reluxPermissions.list();
      setAgentPermissions(data);
    } catch (err) {
      console.error("Failed to fetch agent permissions:", err);
      setError("Failed to load agent permissions.");
    }
  };

  const fetchAgents = async () => {
    try {
      const data = await reluxWork.listAgents();
      setAgents(data);
      if (data.length > 0) {
        setSelectedAgentId(data[0].id);
      }
    } catch (err) {
      console.error("Failed to fetch agents:", err);
      setError("Failed to load agents.");
    }
  };

  const fetchGrants = async () => {
    try {
      const data = await reluxGrants.list();
      setGrants(data);
    } catch (err) {
      console.error("Failed to fetch persistent grants:", err);
      setError("Failed to load persistent grants.");
    }
  };

  useEffect(() => {
    fetchApprovals();
    fetchAgentPermissions();
    fetchAgents();
    fetchGrants();
    // Poll for updates every 5 seconds
    const interval = setInterval(() => {
      fetchApprovals();
      fetchAgentPermissions();
      fetchGrants();
    }, 5000);
    return () => clearInterval(interval);
  }, []);

  const handleDecideApproval = async (
    approvalId: string,
    decision: "approved" | "rejected",
  ) => {
    try {
      await reluxApprovals.decide(approvalId, decision);
      fetchApprovals(); // Refresh the list
    } catch (err) {
      console.error(`Failed to ${decision} approval ${approvalId}:`, err);
      setError(`Failed to ${decision} approval.`);
    }
  };

  const handleExecuteApproval = async (approvalId: string) => {
    setExecError((m) => ({ ...m, [approvalId]: "" }));
    setExecResult((m) => ({ ...m, [approvalId]: "" }));
    try {
      const res = await reluxApprovals.execute(approvalId);
      setExecResult((m) => ({
        ...m,
        [approvalId]: JSON.stringify(res.output),
      }));
      fetchApprovals(); // Refresh so the binding shows as consumed.
    } catch (err: any) {
      console.error(`Failed to execute approval ${approvalId}:`, err);
      setExecError((m) => ({
        ...m,
        [approvalId]: err?.message || "Execution failed.",
      }));
    }
  };

  const handleAllowAlways = async (approvalId: string) => {
    setExecError((m) => ({ ...m, [approvalId]: "" }));
    setExecResult((m) => ({ ...m, [approvalId]: "" }));
    try {
      await reluxApprovals.allowAlways(approvalId);
      // Refresh both: the approval is now approved, and a new grant exists.
      fetchApprovals();
      fetchGrants();
    } catch (err: any) {
      console.error(`Failed to allow-always approval ${approvalId}:`, err);
      setExecError((m) => ({
        ...m,
        [approvalId]: err?.message || "Allow-always failed.",
      }));
    }
  };

  const handleRevokeGrant = async (grantId: string) => {
    try {
      await reluxGrants.revoke(grantId);
      fetchGrants();
    } catch (err) {
      console.error(`Failed to revoke grant ${grantId}:`, err);
      setError("Failed to revoke grant.");
    }
  };

  const handleGrantPermission = async (e: React.FormEvent) => {
    e.preventDefault();
    setGrantPermissionError(null);
    setGrantPermissionSuccess(null);
    if (!selectedAgentId || !permissionInput.trim()) {
      setGrantPermissionError("Please select an agent and enter a permission.");
      return;
    }
    try {
      await reluxPermissions.grant(selectedAgentId, permissionInput.trim());
      setGrantPermissionSuccess(
        `Permission '${permissionInput.trim()}' granted to agent '${selectedAgentId}'.`,
      );
      setPermissionInput("");
      fetchAgentPermissions(); // Refresh agent permissions
    } catch (err: any) {
      console.error("Failed to grant permission:", err);
      setGrantPermissionError(err.message || "Failed to grant permission.");
    }
  };

  if (loading) {
    return (
      <div className="grid">
        <div className="card">
          <h3>Approvals &amp; Permissions</h3>
          <div className="loading">Loading…</div>
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="grid">
        <div className="card">
          <h3>Approvals &amp; Permissions</h3>
          <div className="banner err" style={{ fontSize: 12, marginBottom: 0 }}>{error}</div>
        </div>
      </div>
    );
  }

  return (
    <div className="grid">
      {/* Approvals Panel */}
      <div className="card">
        <h3>Approvals</h3>
        {approvals.length === 0 ? (
          <div className="empty">No approvals found.</div>
        ) : (
          <div className="table-scroll">
            <table className="table sm">
              <thead>
                <tr>
                  <th>ID</th>
                  <th>Action</th>
                  <th>Risk</th>
                  <th>Status</th>
                  <th style={{ textAlign: "right" }}>Actions</th>
                </tr>
              </thead>
              <tbody>
                {approvals.map((approval) => {
                  const ti = approval.tool_invocation;
                  return (
                    <tr key={approval.id}>
                      <td style={{ verticalAlign: "top" }}>
                        <div className="mono" style={{ fontSize: 11 }}>{approval.id}</div>
                        <div className="muted" style={{ fontSize: 10 }}>
                          {approval.requested_by} ·{" "}
                          {new Date(approval.created_at).toLocaleString()}
                        </div>
                      </td>
                      <td style={{ verticalAlign: "top" }}>
                        <div>{approval.action}</div>
                        <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>{approval.reason}</div>
                        {ti && (
                          <div style={{ marginTop: 6, fontSize: 11 }}>
                            <div className="muted">
                              tool <span className="mono">{ti.tool_name}</span> on{" "}
                              <span className="mono">{ti.plugin_id}</span> as{" "}
                              <span className="mono">{ti.agent_id}</span>
                              {ti.consumed && (
                                <span className="muted" style={{ marginLeft: 6 }}>(executed)</span>
                              )}
                            </div>
                            <pre
                              className="code"
                              style={{ marginTop: 4, fontSize: 11, whiteSpace: "pre-wrap", overflowX: "auto" }}
                            >
                              {ti.args_preview}
                            </pre>
                            <div className="muted" style={{ fontSize: 10 }}>
                              args sha256 {ti.args_sha256.slice(0, 16)}…
                            </div>
                            {execResult[approval.id] && (
                              <div style={{ marginTop: 4, fontSize: 11, color: "var(--ok)" }}>
                                Output: <span className="mono">{execResult[approval.id]}</span>
                              </div>
                            )}
                            {execError[approval.id] && (
                              <div className="banner err" style={{ marginTop: 4, fontSize: 11 }}>
                                {execError[approval.id]}
                              </div>
                            )}
                          </div>
                        )}
                      </td>
                      <td style={{ verticalAlign: "top" }}>
                        <span className="badge backlog">{approval.risk}</span>
                      </td>
                      <td style={{ verticalAlign: "top" }}>
                        <span className={"badge " + (APPROVAL_STATUS_TONE[approval.status] ?? "backlog")}>
                          {approval.status}
                        </span>
                      </td>
                      <td style={{ verticalAlign: "top", textAlign: "right" }}>
                        <div className="row wrap" style={{ gap: 6, justifyContent: "flex-end" }}>
                          {approval.status === "pending" && (
                            <>
                              <button
                                className="btn sm"
                                onClick={() => handleDecideApproval(approval.id, "approved")}
                              >
                                {ti ? "Approve once" : "Approve"}
                              </button>
                              {/* "Allow always" is offered ONLY for a gated tool-invocation
                                  approval: it approves this call AND persists a standing grant
                                  so future calls of THIS tool by THIS agent skip the prompt.
                                  Scope is narrow on purpose — not a blanket trust. */}
                              {ti && (
                                <button
                                  className="btn ghost sm"
                                  title={`Allow ${ti.tool_name} for ${ti.agent_id} without asking again`}
                                  onClick={() => handleAllowAlways(approval.id)}
                                >
                                  Allow always
                                </button>
                              )}
                              <button
                                className="btn ghost sm"
                                onClick={() => handleDecideApproval(approval.id, "rejected")}
                              >
                                Reject
                              </button>
                            </>
                          )}
                          {/* An approved, not-yet-executed per-call tool invocation can be
                              run exactly once. The kernel enforces one-shot consumption;
                              the button just drives it. */}
                          {ti?.executable && (
                            <button
                              className="btn sm"
                              onClick={() => handleExecuteApproval(approval.id)}
                            >
                              Execute once
                            </button>
                          )}
                          {ti && ti.consumed && approval.status === "approved" && (
                            <span className="muted" style={{ fontSize: 11 }}>Executed</span>
                          )}
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </div>

      {/* Persistent allow-always grants Panel */}
      <div className="card">
        <h3>Allow-always grants</h3>
        <p className="muted" style={{ marginTop: -4, fontSize: 12 }}>
          A grant lets one agent run one specific gated tool without asking each time.
          It only matches that exact tool, agent, and risk level — revoke it to require
          approval again.
        </p>
        {grants.length === 0 ? (
          <div className="empty">No persistent grants. Use “Allow always” on a tool approval to add one.</div>
        ) : (
          <div className="table-scroll">
            <table className="table sm">
              <thead>
                <tr>
                  <th>Tool</th>
                  <th>Agent</th>
                  <th>Risk</th>
                  <th>Last used</th>
                  <th style={{ textAlign: "right" }}>Actions</th>
                </tr>
              </thead>
              <tbody>
                {grants.map((g) => (
                  <tr key={g.id}>
                    <td>
                      <div className="mono">{g.tool_name}</div>
                      <div className="muted mono" style={{ fontSize: 10 }}>{g.plugin_id}</div>
                    </td>
                    <td className="mono">{g.agent_id}</td>
                    <td><span className="badge backlog">{g.risk}</span></td>
                    <td className="muted" style={{ fontSize: 11 }}>
                      {g.last_used_at ? new Date(g.last_used_at).toLocaleString() : "never"}
                    </td>
                    <td style={{ textAlign: "right" }}>
                      <button className="btn ghost sm" onClick={() => handleRevokeGrant(g.id)}>
                        Revoke
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      {/* Permissions Panel */}
      <div className="card">
        <h3>Agent Permissions</h3>
        {agentPermissions.length === 0 ? (
          <div className="empty">No agent permissions found.</div>
        ) : (
          <div className="table-scroll">
            <table className="table sm">
              <thead>
                <tr>
                  <th>Agent ID</th>
                  <th>Permissions</th>
                </tr>
              </thead>
              <tbody>
                {agentPermissions.map((ap) => (
                  <tr key={ap.agent_id}>
                    <td>
                      {/* Agent governance lives on the Crew surface inside the Relux
                          shell; link there (not the legacy /agents console, which
                          would leave the shell and dead-end). */}
                      <Link to="/crew" className="link mono" title="Manage this agent on the Crew page">
                        {ap.agent_id}
                      </Link>
                    </td>
                    <td className="muted" style={{ fontSize: 12 }}>
                      {ap.permissions.length > 0 ? ap.permissions.join(", ") : "None"}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      {/* Grant Permission Form */}
      <div className="card">
        <h3>Grant Permission to Agent</h3>
        <form onSubmit={handleGrantPermission} className="grid" style={{ gap: 12 }}>
          <label className="field">
            <span>Select Agent:</span>
            <select
              className="input"
              value={selectedAgentId}
              onChange={(e) => setSelectedAgentId(e.target.value)}
            >
              {agents.map((agent) => (
                <option key={agent.id} value={agent.id}>
                  {agent.name} ({agent.id})
                </option>
              ))}
            </select>
          </label>
          <label className="field">
            <span>Permission String:</span>
            <input
              className="input mono"
              type="text"
              value={permissionInput}
              onChange={(e) => setPermissionInput(e.target.value)}
              placeholder="e.g., tool:relux-tools-github:read_repo"
            />
          </label>
          {grantPermissionError && (
            <div className="banner err" style={{ fontSize: 12, marginBottom: 0 }}>{grantPermissionError}</div>
          )}
          {grantPermissionSuccess && (
            <div className="muted" style={{ fontSize: 12, color: "var(--ok)" }}>{grantPermissionSuccess}</div>
          )}
          <div>
            <button type="submit" className="btn">Grant Permission</button>
          </div>
        </form>
      </div>
    </div>
  );
};

export default ReluxApprovals;
