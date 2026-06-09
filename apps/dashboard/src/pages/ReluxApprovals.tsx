// apps/dashboard/src/pages/ReluxApprovals.tsx
import React, { useEffect, useState } from "react";
import {
  ReluxApproval,
  reluxApprovals,
  ReluxAgentPermissions,
  reluxPermissions,
  ReluxAgent,
  reluxWork,
} from "../api";
import { Link } from "react-router-dom";

const ReluxApprovals: React.FC = () => {
  const [approvals, setApprovals] = useState<ReluxApproval[]>([]);
  const [agentPermissions, setAgentPermissions] = useState<ReluxAgentPermissions[]>([]);
  const [agents, setAgents] = useState<ReluxAgent[]>([]);
  const [loading, setLoading] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);

  const [selectedAgentId, setSelectedAgentId] = useState<string>("");
  const [permissionInput, setPermissionInput] = useState<string>("");
  const [grantPermissionError, setGrantPermissionError] = useState<string | null>(null);
  const [grantPermissionSuccess, setGrantPermissionSuccess] = useState<string | null>(null);

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

  useEffect(() => {
    fetchApprovals();
    fetchAgentPermissions();
    fetchAgents();
    // Poll for updates every 5 seconds
    const interval = setInterval(() => {
      fetchApprovals();
      fetchAgentPermissions();
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
      <div className="flex-1 overflow-auto p-8">
        <h2 className="text-2xl font-bold mb-4">Approvals & Permissions</h2>
        <p>Loading...</p>
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex-1 overflow-auto p-8">
        <h2 className="text-2xl font-bold mb-4">Approvals & Permissions</h2>
        <p className="text-red-500">{error}</p>
      </div>
    );
  }

  return (
    <div className="flex-1 overflow-auto p-8">
      <h2 className="text-2xl font-bold mb-4">Approvals & Permissions</h2>

      {/* Approvals Panel */}
      <div className="bg-gray-800 p-6 rounded-lg shadow-md mb-8">
        <h3 className="text-xl font-semibold mb-4 text-white">Approvals</h3>
        {approvals.length === 0 ? (
          <p className="text-gray-400">No approvals found.</p>
        ) : (
          <div className="overflow-x-auto">
            <table className="min-w-full divide-y divide-gray-700">
              <thead className="bg-gray-700">
                <tr>
                  <th
                    scope="col"
                    className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase tracking-wider"
                  >
                    ID
                  </th>
                  <th
                    scope="col"
                    className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase tracking-wider"
                  >
                    Requested By
                  </th>
                  <th
                    scope="col"
                    className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase tracking-wider"
                  >
                    Status
                  </th>
                  <th
                    scope="col"
                    className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase tracking-wider"
                  >
                    Created At
                  </th>
                  <th
                    scope="col"
                    className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase tracking-wider"
                  >
                    Actions
                  </th>
                </tr>
              </thead>
              <tbody className="bg-gray-800 divide-y divide-gray-700">
                {approvals.map((approval) => (
                  <tr key={approval.id}>
                    <td className="px-6 py-4 whitespace-nowrap text-sm font-medium text-gray-300">
                      {approval.id}
                    </td>
                    <td className="px-6 py-4 whitespace-nowrap text-sm text-gray-400">
                      {approval.requested_by}
                    </td>
                    <td className="px-6 py-4 whitespace-nowrap text-sm text-gray-400">
                      {approval.status}
                    </td>
                    <td className="px-6 py-4 whitespace-nowrap text-sm text-gray-400">
                      {new Date(approval.created_at).toLocaleString()}
                    </td>
                    <td className="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
                      {approval.status === "Pending" && (
                        <>
                          <button
                            onClick={() => handleDecideApproval(approval.id, "approved")}
                            className="text-green-500 hover:text-green-700 mr-2"
                          >
                            Approve
                          </button>
                          <button
                            onClick={() => handleDecideApproval(approval.id, "rejected")}
                            className="text-red-500 hover:text-red-700"
                          >
                            Reject
                          </button>
                        </>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      {/* Permissions Panel */}
      <div className="bg-gray-800 p-6 rounded-lg shadow-md mb-8">
        <h3 className="text-xl font-semibold mb-4 text-white">Agent Permissions</h3>
        {agentPermissions.length === 0 ? (
          <p className="text-gray-400">No agent permissions found.</p>
        ) : (
          <div className="overflow-x-auto">
            <table className="min-w-full divide-y divide-gray-700">
              <thead className="bg-gray-700">
                <tr>
                  <th
                    scope="col"
                    className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase tracking-wider"
                  >
                    Agent ID
                  </th>
                  <th
                    scope="col"
                    className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase tracking-wider"
                  >
                    Permissions
                  </th>
                </tr>
              </thead>
              <tbody className="bg-gray-800 divide-y divide-gray-700">
                {agentPermissions.map((ap) => (
                  <tr key={ap.agent_id}>
                    <td className="px-6 py-4 whitespace-nowrap text-sm font-medium text-gray-300">
                      <Link to={`/agents/${ap.agent_id}`} className="text-blue-400 hover:underline">
                        {ap.agent_id}
                      </Link>
                    </td>
                    <td className="px-6 py-4 whitespace-nowrap text-sm text-gray-400">
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
      <div className="bg-gray-800 p-6 rounded-lg shadow-md">
        <h3 className="text-xl font-semibold mb-4 text-white">Grant Permission to Agent</h3>
        <form onSubmit={handleGrantPermission} className="space-y-4">
          <div>
            <label htmlFor="agent-select" className="block text-sm font-medium text-gray-300">
              Select Agent:
            </label>
            <select
              id="agent-select"
              value={selectedAgentId}
              onChange={(e) => setSelectedAgentId(e.target.value)}
              className="mt-1 block w-full pl-3 pr-10 py-2 text-base border-gray-600 focus:outline-none focus:ring-blue-500 focus:border-blue-500 sm:text-sm rounded-md bg-gray-700 text-white"
            >
              {agents.map((agent) => (
                <option key={agent.id} value={agent.id}>
                  {agent.name} ({agent.id})
                </option>
              ))}
            </select>
          </div>
          <div>
            <label htmlFor="permission-input" className="block text-sm font-medium text-gray-300">
              Permission String:
            </label>
            <input
              type="text"
              id="permission-input"
              value={permissionInput}
              onChange={(e) => setPermissionInput(e.target.value)}
              className="mt-1 block w-full p-2 border border-gray-600 rounded-md shadow-sm bg-gray-700 text-white focus:ring-blue-500 focus:border-blue-500"
              placeholder="e.g., tool:relux-tools-github:read_repo"
            />
          </div>
          {grantPermissionError && (
            <p className="text-red-500 text-sm">{grantPermissionError}</p>
          )}
          {grantPermissionSuccess && (
            <p className="text-green-500 text-sm">{grantPermissionSuccess}</p>
          )}
          <button
            type="submit"
            className="inline-flex justify-center py-2 px-4 border border-transparent shadow-sm text-sm font-medium rounded-md text-white bg-blue-600 hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500"
          >
            Grant Permission
          </button>
        </form>
      </div>
    </div>
  );
};

export default ReluxApprovals;
