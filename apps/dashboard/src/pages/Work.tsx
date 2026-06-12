import { useState, useMemo, useEffect, useRef } from "react";
import { useLocation, useNavigate, Link } from "react-router-dom";
import { runIdFromSearch, taskIdFromSearch, workRunShareUrl } from "../routing";
import {
  bucketTasks,
  oversightCountChips,
  hasOversightAttention,
  continuationActionLabel,
} from "../oversight";
import {
  buildWorkGroups,
  nonEmptyGroups,
  progressSegments,
  groupProgressLabel,
  blockedByLabel,
  blockingLabel,
  groupForTask,
  bucketTone,
  bucketColorVar,
  type WorkGroup,
  type GroupProgress,
} from "../workhierarchy";
import {
  childrenOfTask,
  adhocSubtaskProgress,
  subtaskCounts,
} from "../adhocsubtrees";
import {
  rollupRuns,
  runRollupChips,
  adhocSubtreeTaskIds,
  type RollupChip,
} from "../runrollup";
import { operatorStatusMoves, canMoveStatus } from "../taskmove";
import { candidateParents } from "../reparent";
import { orchestrationStatusTone } from "../orchestration";
import { approvalInlineActions } from "../approvalactions";
import {
  latestReluxEventId,
  mergeReluxRunEvents,
  noActivityLabel,
} from "../reluxruntranscript";
import {
  latestRunLogSeq,
  mergeRunLog,
  runLogIsEmpty,
  runLogSourceLabel,
  runLogTruncationNote,
} from "../reluxrunlog";
import { reluxWork, reluxAudit, reluxOversight, reluxPrime, reluxApprovals, reluxOrchestration, type ReluxTask, type ReluxRun, type ReluxAgent, type ReluxTaskDetail, type ReluxRunDetail, type ReluxAuditEntry, type ReluxRunEvent, type ReluxRunLog, type ReluxOversight, type ReluxApproval, type ReluxOrchestration } from "../api";
import { useAsync } from "../components/common";
import {
  runStatusTone,
  formatRunDuration,
  canRetryRun,
  canResumeRun,
  canCancelRun,
  runSession,
  sessionHandoffLabel,
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
  // The composed Board Oversight summary (counts + in-flight/attention runs +
  // pending approvals + any resumable Prime continuation). A failure here must NOT
  // blank the board — the kanban + create + runs all still work — so its error is
  // surfaced inline in the strip and excluded from the page-level error gate.
  const { data: oversight, error: errorOversight, reload: reloadOversight } = useAsync<ReluxOversight>(
    () => reluxOversight.get(),
    [],
  );
  // The multi-agent orchestrations (the ONLY real parent→child grouping the kernel
  // records today — see workhierarchy.ts). Joined to the live task list below to
  // surface sub-work + progress on the board. A failure here must NOT blank the
  // board, so its error is surfaced inline in the hierarchy card and excluded from
  // the page-level error gate (an older kernel without the route just shows the
  // honest empty/degraded state).
  const { data: orchestrations, error: errorOrchestrations, reload: reloadOrchestrations } = useAsync<ReluxOrchestration[]>(
    () => reluxOrchestration.list(),
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

  // Four board columns, one per WorkBucket. Every task status maps to exactly one
  // column (oversight.ts::taskBucket), so blocked / waiting-on-approval / failed
  // work is now VISIBLE on the board (the prior "other" bucket was computed but
  // never rendered — the reported gap). Filters apply before bucketing.
  const columns = useMemo(() => {
    let list = tasks ?? [];

    if (filterAgentId) {
      list = list.filter(t => t.assigned_agent === filterAgentId);
    }
    if (filterStatus) {
      list = list.filter(t => t.status === filterStatus);
    }

    return bucketTasks(list);
  }, [tasks, filterAgentId, filterStatus]);

  // Real parent→child groups for the board: each orchestration goal joined to the
  // live task list (workhierarchy.buildWorkGroups). Built from the UNFILTERED task
  // list so a group's progress reflects the whole subtree even when the board is
  // filtered. Only groups with committed steps are surfaced as parents.
  const groups = useMemo(
    () => nonEmptyGroups(buildWorkGroups(orchestrations ?? [], tasks ?? [])),
    [orchestrations, tasks],
  );
  const selectedTaskGroup = useMemo(
    () => (selectedTaskId ? groupForTask(groups, selectedTaskId) : null),
    [groups, selectedTaskId],
  );
  // Ad-hoc subtask counts per parent (design §6.2): the second real parent→child link,
  // the `parent_task` edge the kernel now populates. Used to mark board cards with
  // sub-work and to render a parent's subtree in the task detail.
  const subCounts = useMemo(() => subtaskCounts(tasks ?? []), [tasks]);

  const error = errorTasks || errorRuns || errorAgents;
  const loading = (loadingTasks && !tasks) || (loadingRuns && !runs) || (loadingAgents && !agents);

  const handleReload = () => {
    reloadTasks();
    reloadRuns();
    reloadAgents();
    reloadOversight();
    reloadOrchestrations();
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

            <OversightStrip
              oversight={oversight}
              error={errorOversight ? String(errorOversight) : null}
              agents={agents || []}
              onInspectRun={handleInspectRun}
              onReload={handleReload}
            />

            <WorkHierarchy
              groups={groups}
              runs={runs || []}
              error={errorOrchestrations ? String(errorOrchestrations) : null}
              loading={!orchestrations && !errorOrchestrations}
              agents={agents || []}
              onInspectTask={handleInspectTask}
            />

            <div className="row wrap" style={{ gap: 16, alignItems: "flex-start" }}>
              <Column title="Open" tasks={columns.open} onAction={handleReload} onInspectTask={handleInspectTask} agents={agents || []} subtaskCounts={subCounts} />
              <Column title="Running" tasks={columns.running} onAction={handleReload} onInspectTask={handleInspectTask} agents={agents || []} subtaskCounts={subCounts} />
              <Column title="Blocked / Failed" tasks={columns.blocked} onAction={handleReload} onInspectTask={handleInspectTask} agents={agents || []} subtaskCounts={subCounts} />
              <Column title="Done" tasks={columns.done} onAction={handleReload} onInspectTask={handleInspectTask} agents={agents || []} subtaskCounts={subCounts} />
            </div>

            {(selectedTaskId || selectedRunId) && (
              <div className="card" style={{ marginTop: 24, padding: 16 }}>
                {selectedTaskId && (
                  <TaskDetailPanel
                    taskId={selectedTaskId}
                    group={selectedTaskGroup}
                    agents={agents || []}
                    tasks={tasks || []}
                    runs={runs || []}
                    onInspectTask={handleInspectTask}
                    onChanged={handleReload}
                    onClose={() => setSelectedTaskId(null)}
                  />
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

// Board Oversight v1 strip (docs/relix-dashboard-design.md §5 Inbox / §11 Active
// Runs): one composed, dense panel at the top of Work that makes live work visible
// and controllable without opening each run. It shows the operational counts, the
// in-flight runs (Inspect / Cancel), the runs needing attention (Inspect / Retry),
// the pending approvals (the common Approve & run / Allow always / Deny decisions
// INLINE, plus Open → for the detailed Approvals surface), and any resumable Prime
// continuation (Continue). Every control reuses an EXISTING backend route — nothing
// new is executed here. A read failure degrades to an inline note, never a blank.
function OversightStrip({
  oversight,
  error,
  agents,
  onInspectRun,
  onReload,
}: {
  oversight: ReluxOversight | null;
  error: string | null;
  agents: ReluxAgent[];
  onInspectRun: (runId: string) => void;
  onReload: () => void;
}) {
  const [continuing, setContinuing] = useState(false);
  const [busyRun, setBusyRun] = useState<string | null>(null);
  // The honest one-line result of the last cross-cutting action (continue / cancel /
  // retry), so a click is never a silent no-op. Cleared when a new action starts.
  const [note, setNote] = useState<string | null>(null);

  const agentName = (id: string) => agents.find(a => a.id === id)?.name || id;

  // Resume the paused Prime agent loop from its stored continuation. A loop still
  // waiting on a tool approval is NOT resumed here (the operator must approve the
  // tool first via Approvals) — the button is disabled with that reason.
  async function continueLoop(id: string) {
    setContinuing(true);
    setNote(null);
    try {
      const turn = await reluxPrime.continue(id, false);
      setNote(turn.reply ? `Resumed: ${turn.reply.slice(0, 160)}` : "Resumed the paused loop.");
      onReload();
    } catch (e) {
      setNote(e instanceof Error ? e.message : "Continue failed.");
    } finally {
      setContinuing(false);
    }
  }

  async function cancel(runId: string) {
    setBusyRun(runId);
    setNote(null);
    try {
      const res = await reluxWork.cancelRun(runId);
      setNote(res.message);
      onReload();
    } catch (e) {
      setNote(e instanceof Error ? e.message : "Cancel failed.");
    } finally {
      setBusyRun(null);
    }
  }

  async function retry(runId: string) {
    setBusyRun(runId);
    setNote(null);
    try {
      const res = await reluxWork.retryRun(runId);
      onReload();
      onInspectRun(res.run_id); // jump to the fresh attempt
    } catch (e) {
      setNote(e instanceof Error ? e.message : "Retry failed.");
    } finally {
      setBusyRun(null);
    }
  }

  if (error) {
    return (
      <div className="card" style={{ marginBottom: 20, padding: 12 }}>
        <div className="row" style={{ alignItems: "center", marginBottom: 6 }}>
          <h4 style={{ margin: 0 }}>Oversight</h4>
        </div>
        <div className="muted" style={{ fontSize: 11 }}>
          Oversight summary unavailable ({error}). The board below still works.
        </div>
      </div>
    );
  }
  if (!oversight) {
    return (
      <div className="card" style={{ marginBottom: 20, padding: 12 }}>
        <h4 style={{ margin: "0 0 6px" }}>Oversight</h4>
        <div className="muted" style={{ fontSize: 12 }}>Loading oversight…</div>
      </div>
    );
  }

  const chips = oversightCountChips(oversight.counts);
  const cont = oversight.continuation;
  const showAttention = hasOversightAttention(oversight);

  return (
    <div className="card" style={{ marginBottom: 20, padding: 12 }}>
      <div className="row" style={{ alignItems: "center", marginBottom: 10 }}>
        <h4 style={{ margin: 0 }}>Oversight</h4>
        <span className="muted" style={{ fontSize: 11, marginLeft: 8 }}>
          live work at a glance — composed from runs, approvals &amp; the paused-loop continuation
        </span>
      </div>

      {/* Dense count chips: what is live → what is stuck → what needs me. */}
      <div className="row wrap" style={{ gap: 6, marginBottom: showAttention ? 12 : 0 }}>
        {chips.map(c => (
          <span
            key={c.label}
            className={`badge ${c.value > 0 ? c.tone : "backlog"}`}
            style={{ fontSize: 10, fontWeight: 600 }}
            title={c.label}
          >
            {c.label}: {c.value}
          </span>
        ))}
      </div>

      {note && (
        <div className="muted" style={{ fontSize: 11, margin: "8px 0", wordBreak: "break-word" }}>{note}</div>
      )}

      {/* Resumable Prime continuation (survives a refresh). Continue resumes a
          limit-paused loop; one awaiting a tool approval routes the operator to
          Approvals first (resume proceeds only after the tool is approved). */}
      {cont && (
        <div className="card sm" style={{ padding: 10, marginBottom: 10, border: "1px solid var(--border)" }}>
          <div className="row" style={{ alignItems: "center", gap: 8, flexWrap: "wrap" }}>
            <span className="badge in_progress" style={{ fontSize: 9, fontWeight: 600 }}>paused agent loop</span>
            <span className="mono muted" style={{ fontSize: 10 }}>{cont.id}</span>
            <div className="spacer" style={{ flex: 1 }} />
            {cont.awaiting_approval ? (
              <Link to="/approvals" className="link" style={{ fontSize: 12 }}>Open Approvals →</Link>
            ) : (
              <button className="btn sm" disabled={continuing} onClick={() => void continueLoop(cont.id)}>
                {continuing ? "Continuing…" : "Continue"}
              </button>
            )}
          </div>
          <div className="muted" style={{ fontSize: 11, marginTop: 6 }}>{continuationActionLabel(cont)}</div>
        </div>
      )}

      {showAttention && (
        <div className="row wrap" style={{ gap: 16, alignItems: "flex-start" }}>
          {/* In-flight runs — the live work to watch (Inspect, Cancel when cancellable). */}
          {oversight.active_runs.length > 0 && (
            <div style={{ flex: 1, minWidth: 280 }}>
              <h5 style={{ margin: "0 0 6px", fontSize: 12, textTransform: "uppercase", letterSpacing: "0.05em" }}>
                In flight <span className="muted" style={{ fontWeight: 400 }}>{oversight.active_runs.length}</span>
              </h5>
              <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                {oversight.active_runs.map(run => (
                  <OversightRunRow
                    key={run.id}
                    run={run}
                    agentName={agentName(run.agent_id)}
                    busy={busyRun === run.id}
                    onInspect={() => onInspectRun(run.id)}
                    action={canCancelRun(run) ? { label: "Cancel", ghost: true, run: () => cancel(run.id) } : null}
                  />
                ))}
              </div>
            </div>
          )}

          {/* Runs needing attention — failed/cancelled (Inspect, Retry when retryable). */}
          {oversight.attention_runs.length > 0 && (
            <div style={{ flex: 1, minWidth: 280 }}>
              <h5 style={{ margin: "0 0 6px", fontSize: 12, textTransform: "uppercase", letterSpacing: "0.05em" }}>
                Needs attention <span className="muted" style={{ fontWeight: 400 }}>{oversight.attention_runs.length}</span>
              </h5>
              <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                {oversight.attention_runs.map(run => (
                  <OversightRunRow
                    key={run.id}
                    run={run}
                    agentName={agentName(run.agent_id)}
                    busy={busyRun === run.id}
                    onInspect={() => onInspectRun(run.id)}
                    action={canRetryRun(run) ? { label: "Retry", ghost: false, run: () => retry(run.id) } : null}
                  />
                ))}
              </div>
            </div>
          )}

          {/* Pending approvals — the gate list, now with the common low-friction
              decisions INLINE (Approve & run / Allow always / Deny for a per-call
              tool invocation; Approve / Deny for a generic approval). Each button
              drives the SAME reluxApprovals route the dedicated Approvals page and
              the Prime approval card use — no new authority. "Open →" stays the
              link to the detailed Approvals audit surface (typed payload, grants,
              permissions). The action set per row is decided by approvalInlineActions. */}
          {oversight.pending_approvals.length > 0 && (
            <div style={{ flex: 1, minWidth: 280 }}>
              <h5 style={{ margin: "0 0 6px", fontSize: 12, textTransform: "uppercase", letterSpacing: "0.05em" }}>
                Pending approvals <span className="muted" style={{ fontWeight: 400 }}>{oversight.pending_approvals.length}</span>
              </h5>
              <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                {oversight.pending_approvals.map(a => (
                  <OversightApprovalRow key={a.id} approval={a} onReload={onReload} />
                ))}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// One compact run row in the oversight strip: status, ids, a one-line summary/error,
// an Inspect button (opens the full Run Detail panel with the deep controls) and at
// most one inline action (Cancel for in-flight, Retry for attention).
function OversightRunRow({
  run,
  agentName,
  busy,
  onInspect,
  action,
}: {
  run: ReluxRun;
  agentName: string;
  busy: boolean;
  onInspect: () => void;
  action: { label: string; ghost: boolean; run: () => void } | null;
}) {
  return (
    <div className="card sm" style={{ padding: 8, border: "1px solid var(--border)" }}>
      <div className="row" style={{ alignItems: "center", gap: 8 }}>
        <span className={`badge ${runStatusTone(run.status)}`} style={{ fontSize: 9 }}>{run.status}</span>
        <span className="mono muted" style={{ fontSize: 10 }}>{run.id}</span>
        <span className="muted" style={{ fontSize: 10 }}>· {agentName}</span>
        <div className="spacer" style={{ flex: 1 }} />
        <button className="btn ghost sm" style={{ height: 22, padding: "0 8px" }} onClick={onInspect}>Inspect</button>
        {action && (
          <button
            className={`btn sm ${action.ghost ? "ghost" : ""}`}
            style={{ height: 22, padding: "0 8px" }}
            disabled={busy}
            onClick={() => void action.run()}
          >
            {busy ? "…" : action.label}
          </button>
        )}
      </div>
      {(run.summary || run.error) && (
        <div className="muted" style={{ fontSize: 10, marginTop: 4, wordBreak: "break-word" }}>
          {run.error || run.summary}
        </div>
      )}
    </div>
  );
}

// One pending-approval row in the oversight strip with the common decisions INLINE.
// The action set is decided by approvalInlineActions (a per-call tool invocation
// gets Approve & run / Allow always / Deny; a generic approval gets Approve / Deny;
// anything else degrades to Open → only with an honest reason). Each button drives
// the SAME reluxApprovals route the dedicated Approvals page and the Prime approval
// card use — decide / execute / allow-always — so this invents NO new authority and
// runs nothing the operator did not choose. After any decision the strip refreshes
// (onReload re-reads the composed oversight in place) and a compact, shaped one-line
// result/error is shown — never the raw tool envelope. "Open →" always links to the
// detailed Approvals surface (typed payload, grants, permissions).
export function OversightApprovalRow({
  approval,
  onReload,
}: {
  approval: ReluxApproval;
  onReload: () => void;
}) {
  const [working, setWorking] = useState<null | "approve" | "always" | "deny">(null);
  // The honest one-line result of the last decision, so a click is never a silent
  // no-op. No raw JSON — just the shaped confirmation or the backend's error.
  const [note, setNote] = useState<string | null>(null);
  const a = approval;
  const ti = a.tool_invocation;
  const actions = approvalInlineActions(a);
  const locked = working !== null;

  // Approve: for a per-call tool invocation this is the exact two-step the Approvals
  // page + Prime card use (decide(approved) then execute once); for a generic
  // approval it is just decide(approved) — it records the decision and runs nothing.
  async function approve() {
    setWorking("approve");
    setNote(null);
    try {
      await reluxApprovals.decide(a.id, "approved");
      if (actions.approve?.kind === "approve_run") {
        await reluxApprovals.execute(a.id);
        setNote(`Approved & ran ${a.action} once.`);
      } else {
        setNote(`Approved ${a.action}.`);
      }
      onReload();
    } catch (e) {
      setNote(e instanceof Error ? e.message : "Approve failed.");
    } finally {
      setWorking(null);
    }
  }

  // Allow always: approves AND persists a standing allow-always grant for this exact
  // (agent, tool), then runs the bound call once — future matching calls skip the
  // prompt. Only offered for a tool-invocation approval (the route 404s otherwise).
  async function allowAlways() {
    setWorking("always");
    setNote(null);
    try {
      await reluxApprovals.allowAlways(a.id);
      await reluxApprovals.execute(a.id);
      setNote(`Allowed ${a.action} always & ran it once.`);
      onReload();
    } catch (e) {
      setNote(e instanceof Error ? e.message : "Allow-always failed.");
    } finally {
      setWorking(null);
    }
  }

  // Deny: decide(rejected). A bound invocation is dropped and cannot run without a
  // fresh approval.
  async function deny() {
    setWorking("deny");
    setNote(null);
    try {
      await reluxApprovals.decide(a.id, "rejected");
      setNote(`Denied ${a.action}.`);
      onReload();
    } catch (e) {
      setNote(e instanceof Error ? e.message : "Deny failed.");
    } finally {
      setWorking(null);
    }
  }

  return (
    <div className="card sm" style={{ padding: 8, border: "1px solid var(--border)" }}>
      <div className="row" style={{ alignItems: "center", gap: 8 }}>
        <span className={`badge ${a.risk === "critical" || a.risk === "high" ? "failed" : "in_progress"}`} style={{ fontSize: 9 }}>
          {a.risk}
        </span>
        <span style={{ fontSize: 12, fontWeight: 600 }}>{a.action}</span>
        <div className="spacer" style={{ flex: 1 }} />
        <Link to="/approvals" className="link" style={{ fontSize: 12 }}>Open →</Link>
      </div>
      {a.reason && <div className="muted" style={{ fontSize: 10, marginTop: 4 }}>{a.reason}</div>}
      {ti && (
        <div className="muted mono" style={{ fontSize: 9, marginTop: 4, wordBreak: "break-all" }}>
          {ti.tool_name} on {ti.plugin_id} as {ti.agent_id}
        </div>
      )}
      {actions.actionable ? (
        <div className="row wrap" style={{ gap: 6, marginTop: 8 }}>
          {actions.approve && (
            <button
              className="btn sm"
              style={{ height: 22, padding: "0 8px" }}
              disabled={locked}
              onClick={() => void approve()}
              title={
                actions.approve.kind === "approve_run"
                  ? "Approve this single call and run it once through the existing per-call execute path"
                  : "Approve this request — it records the decision; nothing runs here"
              }
            >
              {working === "approve" ? "…" : actions.approve.label}
            </button>
          )}
          {actions.allowAlways && (
            <button
              className="btn ghost sm"
              style={{ height: 22, padding: "0 8px" }}
              disabled={locked}
              onClick={() => void allowAlways()}
              title={ti ? `Allow ${ti.tool_name} for ${ti.agent_id} without asking again, then run it once` : undefined}
            >
              {working === "always" ? "…" : "Allow always"}
            </button>
          )}
          {actions.deny && (
            <button
              className="btn ghost sm"
              style={{ height: 22, padding: "0 8px" }}
              disabled={locked}
              onClick={() => void deny()}
              title="Deny this request — it is dropped and cannot run without a fresh approval"
            >
              {working === "deny" ? "…" : "Deny"}
            </button>
          )}
        </div>
      ) : (
        actions.reason && (
          <div className="muted" style={{ fontSize: 10, marginTop: 8, fontStyle: "italic" }}>
            {actions.reason}
          </div>
        )
      )}
      {actions.actionable && actions.reason && (
        <div className="muted" style={{ fontSize: 9, marginTop: 6, fontStyle: "italic" }}>{actions.reason}</div>
      )}
      {note && (
        <div className="muted" style={{ fontSize: 10, marginTop: 6, wordBreak: "break-word" }}>{note}</div>
      )}
    </div>
  );
}

// Work hierarchy/progress v1 (docs/relix-dashboard-design.md §6 "A progress strip
// on a parent" + §6.1 sub-issue nesting / workflow-checklist). Surfaces the REAL
// parent→child grouping the kernel records — the multi-agent orchestration — right
// on the board: each goal joined to the live task list (workhierarchy.buildWorkGroups),
// with a compact segmented progress strip, the brief count, and an expandable
// numbered workflow checklist (role + live status + blocked-by/blocking chips +
// Inspect). NO fake hierarchy: a planned orchestration with no committed steps is
// dropped (nonEmptyGroups upstream), tasks in no orchestration stay standalone flat
// cards in the columns below, and a failed/empty read degrades to an honest state.
export function WorkHierarchy({
  groups,
  runs,
  error,
  loading,
  agents,
  onInspectTask,
}: {
  groups: WorkGroup[];
  runs: ReluxRun[];
  error: string | null;
  loading: boolean;
  agents: ReluxAgent[];
  onInspectTask: (taskId: string) => void;
}) {
  const agentName = (id: string | null) =>
    id ? agents.find(a => a.id === id)?.name || id : "unassigned";

  return (
    <div className="card" style={{ marginBottom: 20, padding: 12 }}>
      <div className="row" style={{ alignItems: "center", marginBottom: 10 }}>
        <h4 style={{ margin: 0 }}>Work groups</h4>
        <span className="muted" style={{ fontSize: 11, marginLeft: 8 }}>
          multi-agent goals decomposed into sub-work — progress &amp; blockers from live task state
        </span>
      </div>
      {error ? (
        <div className="muted" style={{ fontSize: 11 }}>
          Work groups unavailable ({error}). The board below still works.
        </div>
      ) : loading ? (
        <div className="muted" style={{ fontSize: 12 }}>Loading work groups…</div>
      ) : groups.length === 0 ? (
        <div className="empty sm">
          No sub-work yet — no multi-agent goal has been decomposed into a grouped plan.
          Start one from Prime's orchestration view; its briefs appear here grouped with progress.
        </div>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
          {groups.map(g => (
            <WorkGroupCard key={g.id} group={g} runs={runs} agentName={agentName} onInspectTask={onInspectTask} />
          ))}
        </div>
      )}
    </div>
  );
}

// One parent group on the board: the goal + status, a compact segmented progress
// strip and brief count, and an expandable numbered workflow checklist. When no
// child is on the current board view, progress comes from the durable orchestration
// record — said so honestly rather than implying live state.
function WorkGroupCard({
  group,
  runs,
  agentName,
  onInspectTask,
}: {
  group: WorkGroup;
  runs: ReluxRun[];
  agentName: (id: string | null) => string;
  onInspectTask: (taskId: string) => void;
}) {
  const g = group;
  const plural = g.progress.total === 1 ? "" : "s";
  // The run/cost rollup for this group = the runs under its child tasks (design §6).
  const groupTaskIds = useMemo(() => g.children.map(c => c.taskId), [g.children]);
  return (
    <div className="card sm" style={{ padding: 10, border: "1px solid var(--border)" }}>
      <div className="row" style={{ alignItems: "center", gap: 8, flexWrap: "wrap", marginBottom: 8 }}>
        <span className={`badge ${orchestrationStatusTone(g.status)}`} style={{ fontSize: 9, fontWeight: 600 }}>
          {g.status}
        </span>
        <span style={{ fontWeight: 600, fontSize: 13 }}>{g.goal}</span>
        <span className="mono muted" style={{ fontSize: 10 }}>{g.id}</span>
        <div className="spacer" style={{ flex: 1 }} />
        <span className="muted" style={{ fontSize: 11 }}>{groupProgressLabel(g.progress)}</span>
      </div>
      <div className="row" style={{ alignItems: "center", gap: 8 }}>
        <SegmentedBar progress={g.progress} />
        <span className="muted" style={{ fontSize: 10, whiteSpace: "nowrap" }}>
          {g.progress.total} brief{plural}
        </span>
      </div>
      {!g.hasLiveChildren && (
        <div className="muted" style={{ fontSize: 10, marginTop: 6, fontStyle: "italic" }}>
          Progress is from the orchestration record — these briefs are not on the current board view.
        </div>
      )}
      <div style={{ marginTop: 8 }}>
        <RunRollupChips runs={runs} taskIds={groupTaskIds} />
      </div>
      <details style={{ marginTop: 8 }}>
        <summary style={{ cursor: "pointer", fontSize: 11 }}>Show the {g.progress.total}-brief plan</summary>
        <div style={{ marginTop: 8 }}>
          <WorkChecklist group={g} agentName={agentName} onInspectTask={onInspectTask} />
        </div>
      </details>
    </div>
  );
}

// The segmented progress strip (design §6): one slice per non-empty bucket, width
// proportional to its share, painted with the bucket's semantic CSS var (color is
// meaning-only — design §12). The full counts read in the title tooltip.
function SegmentedBar({ progress }: { progress: GroupProgress }) {
  const segs = progressSegments(progress);
  return (
    <div className="seg-bar" title={groupProgressLabel(progress)} aria-label={groupProgressLabel(progress)}>
      {segs.map(s => (
        <span key={s.bucket} style={{ width: `${s.pct}%`, background: bucketColorVar(s.bucket) }} />
      ))}
    </div>
  );
}

// The per-subtree RUN / COST ROLLUP strip (design §6 "live cost (tokens + spend)
// for the subtree"). A compact row of chips computed PURELY on the client by joining
// the live run list (reluxWork.listRuns — each run carries task_id + the optional
// measured cost/duration/usage) to the subtree's task ids (runrollup.ts). It is
// scrupulously honest: cost/duration/tokens are summed ONLY over runs that reported
// them, "cost unavailable" is shown when none did (never a fabricated $0.00), and the
// chip tooltips disclose partial coverage. Run Detail remains the source of full logs;
// each chip is a glance signal, not a drill-down. Renders nothing when there is no
// rollup data to show (a subtree whose tasks have never run shows a single
// "no runs yet" chip, so the strip is never silently blank where work exists).
export function RunRollupChips({ runs, taskIds }: { runs: ReluxRun[]; taskIds: string[] }) {
  const chips = useMemo<RollupChip[]>(
    () => runRollupChips(rollupRuns(runs, taskIds)),
    [runs, taskIds],
  );
  const toneClass = (tone: RollupChip["tone"]) =>
    tone === "failed" ? "blocked" : tone === "active" ? "in_progress" : "backlog";
  return (
    <div className="rollup-strip" role="group" aria-label="Run and cost rollup">
      {chips.map((c, i) => (
        <span
          key={`${c.label}-${i}`}
          className={`badge ${toneClass(c.tone)} rollup-chip`}
          title={c.title}
        >
          {c.label}
        </span>
      ))}
    </div>
  );
}

// The dense, B&W, numbered workflow checklist for one group's children (design
// §6/§6.1) — reused on the board card and in the task detail's parent context.
// Each row: the 1-based step number, the title (→ Inspect), the specialist role,
// the LIVE board status (or the durable outcome when the task is off-board), the
// assignee, and the blocked-by / blocking dependency chips. `highlightTaskId`
// marks the row for the currently-open task in the detail panel.
function WorkChecklist({
  group,
  agentName,
  onInspectTask,
  highlightTaskId,
}: {
  group: WorkGroup;
  agentName: (id: string | null) => string;
  onInspectTask: (taskId: string) => void;
  highlightTaskId?: string;
}) {
  return (
    <div className="plan-list">
      {group.children.map(c => {
        const blockedBy = blockedByLabel(c);
        const blocking = blockingLabel(c);
        return (
          <div key={c.taskId} className={`plan-row${highlightTaskId === c.taskId ? " selected" : ""}`}>
            <div className="plan-num mono">{c.index + 1}</div>
            <div className="plan-main">
              <div className="plan-title-row">
                <span className="plan-title" onClick={() => onInspectTask(c.taskId)}>{c.title}</span>
                <span className="badge backlog" style={{ fontSize: 9 }} title="specialist role">{c.role}</span>
                <span
                  className={`badge ${bucketTone(c.bucket)}`}
                  style={{ fontSize: 9 }}
                  title={c.status ? "live board status" : "from the durable orchestration record (task not on the board)"}
                >
                  {c.status ?? `${c.bucket} (recorded)`}
                </span>
              </div>
              <div className="row wrap" style={{ gap: 8, fontSize: 10, alignItems: "center" }}>
                <span className="mono muted">{c.taskId}</span>
                <span className="muted">· {agentName(c.assignedAgent)}</span>
                {blockedBy && (
                  <span className="badge blocked" style={{ fontSize: 9 }} title="this brief waits on an upstream brief">
                    {blockedBy}
                  </span>
                )}
                {blocking && (
                  <span className="badge backlog" style={{ fontSize: 9 }} title="downstream briefs wait on this one">
                    {blocking}
                  </span>
                )}
                <div className="spacer" style={{ flex: 1 }} />
                <button
                  className="btn ghost sm"
                  style={{ height: 20, padding: "0 8px" }}
                  onClick={() => onInspectTask(c.taskId)}
                >
                  Inspect
                </button>
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );
}

function Column({ title, tasks, onAction, onInspectTask, agents, subtaskCounts }: { title: string; tasks: ReluxTask[]; onAction: () => void; onInspectTask: (taskId: string) => void; agents: ReluxAgent[]; subtaskCounts: Map<string, number> }) {
  return (
    <div style={{ flex: 1, minWidth: 280 }}>
      <h4 style={{ marginBottom: 8, fontSize: 13, textTransform: "uppercase", letterSpacing: "0.05em" }}>
        {title} <span className="muted" style={{ fontWeight: 400 }}>{tasks.length}</span>
      </h4>
      <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        {tasks.map(t => (
          <TaskCard key={t.id} task={t} onAction={onAction} onInspectTask={onInspectTask} agents={agents} subtaskCount={subtaskCounts.get(t.id) ?? 0} />
        ))}
        {tasks.length === 0 && <div className="empty sm" style={{ padding: 16 }}>No {title.toLowerCase()} tasks</div>}
      </div>
    </div>
  );
}

// Compact, design-system status MOVE control (design §6 "Drag a card to a column →
// status mutation, with transition validation"). A small select offering ONLY the
// operator-settable moves taskmove.ts allows for the task's live status (Block /
// Cancel) — the SAME set the backend route accepts, so it never offers a rejected
// move. Renders nothing for a terminal task (no move is possible). On a rejected move
// (state changed underneath) it surfaces the honest backend reason inline, never a
// silent no-op. Calls onMoved() after a successful move so the board refreshes (the
// card re-buckets and any parent progress strip updates).
export function StatusMoveControl({
  taskId,
  status,
  onMoved,
}: {
  taskId: string;
  status: string;
  onMoved: () => void;
}) {
  const moves = useMemo(() => operatorStatusMoves(status), [status]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  if (moves.length === 0) return null;

  async function move(target: string) {
    if (!target) return;
    setBusy(true);
    setErr(null);
    try {
      await reluxWork.setTaskStatus(taskId, target);
      onMoved();
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Move failed.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <span className="status-move" style={{ display: "inline-flex", flexDirection: "column", gap: 2 }}>
      <select
        className="input sm"
        aria-label="Move task status"
        title="Move this task to a new status"
        value=""
        disabled={busy}
        style={{ fontSize: 10, padding: "4px 8px", minWidth: 92, height: 24 }}
        onChange={(e) => void move(e.target.value)}
      >
        <option value="">{busy ? "Moving…" : "Move…"}</option>
        {moves.map((m) => (
          <option key={m.status} value={m.status}>
            {m.label}
          </option>
        ))}
      </select>
      {err && (
        <span className="badge failed" style={{ fontSize: 9, whiteSpace: "normal" }} title={err}>
          {err}
        </span>
      )}
    </span>
  );
}

// SAFE REPARENT control (design §6.6): a compact "Move under…" selector + a "Remove
// parent" button on the task detail. Candidate parents come from reparent.ts, which
// excludes self, all descendants (no cycle), the current parent (no-op), and any
// cross-namespace task — the SAME safety the kernel enforces, so the control never
// offers a parent the backend would reject. When nothing qualifies it says so honestly
// rather than presenting an empty control. A selection control, not drag-and-drop —
// reliable v1 (free-form drag/reorder stays a §6/§7 target). On success it calls
// onReparented() so the panel + board refresh; a rejection surfaces the real reason.
export function ReparentControl({
  task,
  tasks,
  onReparented,
}: {
  task: ReluxTask;
  tasks: ReluxTask[];
  onReparented: () => void;
}) {
  const candidates = useMemo(() => candidateParents(tasks, task.id), [tasks, task.id]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const hasParent = !!task.parent_task;

  async function reparent(parentTask: string | null) {
    setBusy(true);
    setErr(null);
    try {
      await reluxWork.reparentTask(task.id, parentTask);
      onReparented();
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Reparent failed.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <span className="reparent-control" style={{ display: "inline-flex", flexDirection: "column", gap: 2 }}>
      <span className="row" style={{ alignItems: "center", gap: 6, flexWrap: "wrap" }}>
        {candidates.length > 0 ? (
          <select
            className="input sm"
            aria-label="Move task under a new parent"
            title="Move this task under another task"
            value=""
            disabled={busy}
            style={{ fontSize: 10, padding: "4px 8px", minWidth: 120, height: 24 }}
            onChange={(e) => e.target.value && void reparent(e.target.value)}
          >
            <option value="">{busy ? "Moving…" : "Move under…"}</option>
            {candidates.map((c) => (
              <option key={c.id} value={c.id}>
                {c.title} ({c.id})
              </option>
            ))}
          </select>
        ) : (
          <span className="muted" style={{ fontSize: 10 }}>
            No other task can be its parent.
          </span>
        )}
        {hasParent && (
          <button
            className="btn ghost sm"
            style={{ height: 24, padding: "0 8px", fontSize: 10 }}
            disabled={busy}
            title="Make this a top-level task"
            onClick={() => void reparent(null)}
          >
            Remove parent
          </button>
        )}
      </span>
      {err && (
        <span className="badge failed" style={{ fontSize: 9, whiteSpace: "normal" }} title={err}>
          {err}
        </span>
      )}
    </span>
  );
}

function TaskCard({ task, onAction, onInspectTask, agents, subtaskCount }: { task: ReluxTask; onAction: () => void; onInspectTask: (taskId: string) => void; agents: ReluxAgent[]; subtaskCount: number }) {
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
      <div style={{ fontWeight: 600, fontSize: 13, marginBottom: 6, lineHeight: 1.4 }}>{task.title}</div>
      {/* Ad-hoc subtree markers (design §6.2): a card shows when it is itself a parent
          (has sub-work) and/or a subtask of another task, from the real `parent_task`
          edge — the second hierarchy beside orchestration. Color is meaning-only. */}
      {(subtaskCount > 0 || task.parent_task) && (
        <div className="row" style={{ gap: 6, marginBottom: 8, flexWrap: "wrap", alignItems: "center" }}>
          {subtaskCount > 0 && (
            <span
              className="badge backlog"
              style={{ fontSize: 9, cursor: "pointer" }}
              title="this task has ad-hoc subtasks — open it to see the subtree"
              onClick={() => onInspectTask(task.id)}
            >
              ↳ {subtaskCount} subtask{subtaskCount === 1 ? "" : "s"}
            </span>
          )}
          {task.parent_task && (
            <span
              className="badge backlog"
              style={{ fontSize: 9, cursor: "pointer" }}
              title={`subtask of ${task.parent_task}`}
              onClick={() => onInspectTask(task.parent_task!)}
            >
              ↑ subtask of <span className="mono">{task.parent_task}</span>
            </span>
          )}
        </div>
      )}
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
        {/* Status MOVE (design §6): a compact Block / Cancel control, offered only for
            a non-terminal task (taskmove.ts). On success the board reloads so the card
            re-buckets into its new column. */}
        <StatusMoveControl taskId={task.id} status={task.status} onMoved={onAction} />
        <button className="btn ghost sm" style={{ height: 24, padding: "0 8px" }} onClick={() => onInspectTask(task.id)}>Inspect</button>
      </div>
    </div>
  );
}

function TaskDetailPanel({
  taskId,
  group,
  agents,
  tasks,
  runs,
  onInspectTask,
  onChanged,
  onClose,
}: {
  taskId: string;
  group: WorkGroup | null;
  agents: ReluxAgent[];
  tasks: ReluxTask[];
  runs: ReluxRun[];
  onInspectTask: (taskId: string) => void;
  onChanged: () => void;
  onClose: () => void;
}) {
  const { data: task, loading, error, reload: reloadTask } = useAsync<ReluxTaskDetail>(
    () => reluxWork.getTask(taskId),
    [taskId],
  );
  const agentName = (id: string | null) =>
    id ? agents.find(a => a.id === id)?.name || id : "unassigned";
  // A status move from the detail refreshes BOTH this panel (so the shown status is
  // live) and the board (so the card re-buckets + any parent progress updates).
  const onStatusMoved = () => {
    reloadTask();
    onChanged();
  };

  return (
    <div style={{ paddingBottom: 16 }}>
      <div className="row" style={{ alignItems: "center", marginBottom: 12 }}>
        <h4 style={{ margin: 0 }}>Task Detail</h4>
        <div className="spacer" style={{ flex: 1 }} />
        <button className="btn ghost sm" onClick={onClose}>Close</button>
      </div>
      {/* Parent context (design §6.1): when this task is a brief inside a multi-agent
          orchestration, show its goal, the group's segmented progress, and the full
          numbered plan (siblings + blocked-by/blocking), with this task highlighted.
          Absent when the task is standalone (in no group) — no fabricated parent. */}
      {group && (
        <div className="card sm" style={{ padding: 10, marginBottom: 12, border: "1px solid var(--border)" }}>
          <div className="row" style={{ alignItems: "center", gap: 8, flexWrap: "wrap", marginBottom: 6 }}>
            <span className="badge backlog" style={{ fontSize: 9 }}>part of</span>
            <span style={{ fontWeight: 600, fontSize: 12 }}>{group.goal}</span>
            <span className="mono muted" style={{ fontSize: 10 }}>{group.id}</span>
            <div className="spacer" style={{ flex: 1 }} />
            <span className="muted" style={{ fontSize: 11 }}>{groupProgressLabel(group.progress)}</span>
          </div>
          <SegmentedBar progress={group.progress} />
          <div style={{ marginTop: 8 }}>
            <RunRollupChips runs={runs} taskIds={group.children.map(c => c.taskId)} />
          </div>
          <details style={{ marginTop: 8 }}>
            <summary style={{ cursor: "pointer", fontSize: 11 }}>
              Show the {group.progress.total}-brief plan
            </summary>
            <div style={{ marginTop: 8 }}>
              <WorkChecklist
                group={group}
                agentName={agentName}
                onInspectTask={onInspectTask}
                highlightTaskId={taskId}
              />
            </div>
          </details>
        </div>
      )}
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
          <div className="kv">
            <span>Status:</span>
            <span className="row" style={{ alignItems: "center", gap: 8, flexWrap: "wrap" }}>
              <span>{task.status}</span>
              {/* Status MOVE (design §6): the same compact Block / Cancel control the
                  board cards show, offered only for a non-terminal task. */}
              {canMoveStatus(task.status) && (
                <StatusMoveControl taskId={task.id} status={task.status} onMoved={onStatusMoved} />
              )}
            </span>
          </div>
          {/* Parent edge + SAFE REPARENT (design §6.6): show the current parent (if any,
              click to inspect) and the compact Move-under… / Remove-parent control. The
              candidate list (reparent.ts) excludes self + descendants + cross-namespace,
              so it never offers a move the kernel would reject. */}
          <div className="kv">
            <span>Parent:</span>
            <span className="row" style={{ alignItems: "center", gap: 8, flexWrap: "wrap" }}>
              {task.parent_task ? (
                <span
                  className="mono"
                  style={{ cursor: "pointer", textDecoration: "underline" }}
                  title={`subtask of ${task.parent_task}`}
                  onClick={() => onInspectTask(task.parent_task!)}
                >
                  ↑ {task.parent_task}
                </span>
              ) : (
                <span className="muted">top-level (no parent)</span>
              )}
              <ReparentControl task={task} tasks={tasks} onReparented={onStatusMoved} />
            </span>
          </div>
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
      {/* Ad-hoc subtasks (design §6.2): break this task down by hand, outside any
          orchestration. Renders the parent's children with the SAME progress strip +
          numbered list the orchestration groups use, plus an Add-subtask form. */}
      <AdhocSubtaskSection
        taskId={taskId}
        tasks={tasks}
        runs={runs}
        agentName={agentName}
        onInspectTask={onInspectTask}
        onChanged={onChanged}
      />
    </div>
  );
}

// The ad-hoc subtree of one parent task (design §6.2): the children joined from the
// flat task list by the real `parent_task` edge, shown as a segmented progress strip
// + a numbered checklist (the same shape as an orchestration group), with an inline
// "Add subtask" form. Honest empty state when the task has no sub-work yet.
export function AdhocSubtaskSection({
  taskId,
  tasks,
  runs,
  agentName,
  onInspectTask,
  onChanged,
}: {
  taskId: string;
  tasks: ReluxTask[];
  runs: ReluxRun[];
  agentName: (id: string | null) => string;
  onInspectTask: (taskId: string) => void;
  onChanged: () => void;
}) {
  const children = useMemo(() => childrenOfTask(tasks, taskId), [tasks, taskId]);
  const progress = useMemo(() => adhocSubtaskProgress(children), [children]);
  // The ad-hoc subtree's run/cost rollup spans the parent task itself plus its direct
  // children (the parent is a real task that may have runs) — design §6.
  const subtreeTaskIds = useMemo(
    () => adhocSubtreeTaskIds(taskId, children.map(c => c.taskId)),
    [taskId, children],
  );
  const [title, setTitle] = useState("");
  const [adding, setAdding] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  async function addSubtask() {
    const t = title.trim();
    if (!t) return;
    setAdding(true);
    setErr(null);
    try {
      await reluxWork.createTask(t, { parent_task: taskId });
      setTitle("");
      onChanged(); // reload the board so the new child appears in the strip + columns
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Add subtask failed.");
    } finally {
      setAdding(false);
    }
  }

  return (
    <div className="card sm" style={{ padding: 10, marginTop: 14, border: "1px solid var(--border)" }}>
      <div className="row" style={{ alignItems: "center", gap: 8, marginBottom: 8 }}>
        <h5 style={{ margin: 0 }}>Subtasks</h5>
        {children.length > 0 && (
          <>
            <span className="muted" style={{ fontSize: 11 }}>{groupProgressLabel(progress)}</span>
            <div className="spacer" style={{ flex: 1 }} />
            <span className="muted" style={{ fontSize: 10, whiteSpace: "nowrap" }}>
              {progress.total} subtask{progress.total === 1 ? "" : "s"}
            </span>
          </>
        )}
      </div>
      {children.length > 0 ? (
        <>
          <SegmentedBar progress={progress} />
          <div style={{ marginTop: 8 }}>
            <RunRollupChips runs={runs} taskIds={subtreeTaskIds} />
          </div>
          <div className="plan-list" style={{ marginTop: 8 }}>
            {children.map(c => (
              <div key={c.taskId} className="plan-row">
                <div className="plan-num mono">{c.index + 1}</div>
                <div className="plan-main">
                  <div className="plan-title-row">
                    <span className="plan-title" onClick={() => onInspectTask(c.taskId)}>{c.title}</span>
                    <span className={`badge ${bucketTone(c.bucket)}`} style={{ fontSize: 9 }} title="live board status">
                      {c.status}
                    </span>
                  </div>
                  <div className="row wrap" style={{ gap: 8, fontSize: 10, alignItems: "center" }}>
                    <span className="mono muted">{c.taskId}</span>
                    <span className="muted">· {agentName(c.assignedAgent)}</span>
                    <div className="spacer" style={{ flex: 1 }} />
                    <button
                      className="btn ghost sm"
                      style={{ height: 20, padding: "0 8px" }}
                      onClick={() => onInspectTask(c.taskId)}
                    >
                      Inspect
                    </button>
                  </div>
                </div>
              </div>
            ))}
          </div>
        </>
      ) : (
        <div className="empty sm" style={{ padding: 12 }}>
          No sub-work yet — add a subtask to break this task down.
        </div>
      )}
      <div className="row" style={{ gap: 8, marginTop: 10 }}>
        <input
          className="input sm"
          placeholder="Add a subtask..."
          value={title}
          onChange={e => setTitle(e.target.value)}
          onKeyDown={e => e.key === "Enter" && void addSubtask()}
          disabled={adding}
          style={{ flex: 1 }}
        />
        <button className="btn sm" onClick={() => void addSubtask()} disabled={adding || !title.trim()}>
          {adding ? "..." : "Add subtask"}
        </button>
      </div>
      {err && <div className="banner err" style={{ fontSize: 11, marginTop: 8 }}>{err}</div>}
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
  // Bounded, redacted run-log tail (stdout/stderr/system). Same incremental
  // pattern as the transcript: keep the accumulated lines and re-fetch only the
  // tail past `logCursorRef` (the highest line seq we hold), merging on. The
  // first load (and Refresh) fetches the full bounded tail with no cursor.
  const [runLog, setRunLog] = useState<ReluxRunLog | null>(null);
  const [logsLoading, setLogsLoading] = useState(true);
  const [logsError, setLogsError] = useState<string | null>(null);
  const logCursorRef = useRef<number | null>(null);
  // Wall-clock instant of the last observed activity (a new transcript event or
  // a run phase/status change). Drives the honest "no activity" stalled signal —
  // the Relux event `ts` is a logical clock, so staleness must be measured here
  // against real time, never derived from `ts`.
  const [lastActivityAt, setLastActivityAt] = useState<number | null>(null);
  const [nowMs, setNowMs] = useState<number>(() => Date.now());
  const [retrying, setRetrying] = useState(false);
  const [resuming, setResuming] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  // The honest one-line result of the last cancel request (e.g. "requested" or
  // "not a cancellable in-flight process run"), shown inline so the button is
  // never a silent no-op. Cleared when the panel switches runs.
  const [cancelNote, setCancelNote] = useState<string | null>(null);
  // Copy-link state: the shareable absolute `/work?run=` URL is the same one a
  // deep link restores, so an operator can hand a run to a teammate. Reset when
  // the panel switches runs so a stale "copied" note never sticks.
  const [shareNote, setShareNote] = useState<string | null>(null);
  useEffect(() => { setShareNote(null); setCancelNote(null); }, [runId]);

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

  // First load (and on run switch): fetch the FULL bounded log tail and seed the
  // log cursor. Resets accumulated lines so a different run never shows the
  // prior run's logs. A run with no captured log returns an empty tail (the
  // honest "No logs" state) — not an error.
  useEffect(() => {
    let on = true;
    setRunLog(null);
    setLogsLoading(true);
    setLogsError(null);
    logCursorRef.current = null;
    reluxWork
      .getRunLogs(runId)
      .then((log) => {
        if (!on) return;
        setRunLog(log);
        logCursorRef.current = latestRunLogSeq(log);
      })
      .catch((e) => {
        if (on) setLogsError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (on) setLogsLoading(false);
      });
    return () => {
      on = false;
    };
  }, [runId]);

  // Manual Refresh / Poll for the log tail: fetch only the lines past our cursor
  // and merge them on (a full re-fetch when we hold no cursor yet). For an
  // in-flight off-lock (parallel) run the backend streams lines into a live tail
  // as the process produces them, so this poll surfaces them before the run
  // finalizes; a synchronous run still shows its tail at finalize. No WebSocket —
  // pollable. Never clears the last good tail on a transient error.
  async function refreshLogs() {
    setLogsError(null);
    try {
      const tail = await reluxWork.getRunLogs(runId, logCursorRef.current ?? undefined);
      setRunLog((prev) => mergeRunLog(prev, tail));
      const next = latestRunLogSeq(tail);
      if (next != null) logCursorRef.current = next;
    } catch (e) {
      setLogsError(e instanceof Error ? e.message : String(e));
    }
  }

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
      // Poll the log tail on the same cadence (only the lines past our cursor).
      reluxWork
        .getRunLogs(runId, logCursorRef.current ?? undefined)
        .then((tail) => {
          if (tail.lines.length === 0) return; // nothing new
          setRunLog((prev) => mergeRunLog(prev, tail));
          const next = latestRunLogSeq(tail);
          if (next != null) logCursorRef.current = next;
        })
        .catch(() => {
          // Transient: keep the last good tail; the next tick retries.
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

  // Resume continues the run's captured provider session (distinct from retry's
  // cold re-run). An honest 422 refusal (no resumable session) surfaces its real
  // reason here rather than silently doing nothing.
  async function resume() {
    setResuming(true);
    try {
      const res = await reluxWork.resumeRun(runId);
      onRetried(res.run_id);
    } catch (e) {
      alert(e instanceof Error ? e.message : "Resume failed");
    } finally {
      setResuming(false);
    }
  }

  // Request mid-run cancellation of an in-flight, process-backed run
  // (HERMES_OPENCLAW_DEEP_AUDIT §8/§26). The backend is the honest authority: a run
  // that is not a cancellable off-lock process run returns `not_running` and we show
  // that message inline rather than implying a stop that never happened. On a real
  // request we reload the run + logs so the Cancelled status and the cancellation
  // system log line surface as the spawn finalizes.
  async function cancel() {
    setCancelling(true);
    setCancelNote(null);
    try {
      const res = await reluxWork.cancelRun(runId);
      setCancelNote(res.message);
      reloadRun();
      void refreshLogs();
    } catch (e) {
      setCancelNote(e instanceof Error ? e.message : "Cancel failed");
    } finally {
      setCancelling(false);
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
          {run && canResumeRun(run) && (
            <button
              className="btn sm"
              title="Continue this run's captured provider session (threads --resume through the governed adapter gate). Distinct from Retry, which starts a fresh run."
              onClick={() => void resume()}
              disabled={resuming}
            >
              {resuming ? "Resuming…" : "Resume session"}
            </button>
          )}
          {run && canCancelRun(run) && (
            <button
              className="btn ghost sm"
              title="Cancel this in-flight run: kills the adapter process mid-flight (only an off-lock parallel run is cancellable; the result tells you honestly)."
              onClick={() => void cancel()}
              disabled={cancelling}
            >
              {cancelling ? "Cancelling…" : "Cancel run"}
            </button>
          )}
          <button className="btn ghost sm" onClick={onClose}>Close</button>
        </div>
      </div>
      {cancelNote && (
        <div className="muted" style={{ fontSize: 11, marginBottom: 8 }}>{cancelNote}</div>
      )}
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
          {run.resumed_from && (
            <div className="kv"><span>Resume of:</span>
              <a
                className="link mono"
                href={`?run=${encodeURIComponent(run.resumed_from)}`}
                onClick={(e) => { e.preventDefault(); onOpenRun(run.resumed_from!); }}
              >
                {run.resumed_from}
              </a>
            </div>
          )}
          {/* Durable session identity / handoff captured from the adapter envelope
              (HERMES_OPENCLAW_DEEP_AUDIT §3). The id is mono + copyable; the label
              is honest about whether resume is supported here. */}
          {(() => {
            const session = runSession(run);
            if (!session) return null;
            return (
              <>
                <div className="kv"><span>Session:</span>
                  <span
                    className="mono"
                    title="Copy the provider session id"
                    style={{ cursor: "copy", wordBreak: "break-all" }}
                    onClick={() => void navigator.clipboard?.writeText(session.adapter_session_id)}
                  >
                    {session.adapter_session_id}
                  </span>
                </div>
                <div className="kv"><span>Handoff:</span>
                  <span className="muted">{sessionHandoffLabel(run)}</span>
                </div>
              </>
            );
          })()}
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
          {/* Bounded, redacted run-log / tail: the adapter's stdout/stderr split
              into per-line entries, framed by kernel `system` lines
              (spawn/exit/timeout). LIVE for an off-lock (parallel) run — the
              spawn streams each line as it is read and the in-flight poll merges
              the `?since=<seq>` tail, so lines appear BEFORE the run finalizes;
              once finalized the canonical persisted log is served. Polled (no
              WebSocket). Shows truncation + redaction markers honestly and never
              blanks when there are no logs
              (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§10). */}
          <h5 style={{ marginTop: 16, marginBottom: 8 }}>
            Logs / Tail
            <button
              type="button"
              className="btn xs ghost"
              style={{ marginLeft: 8, fontSize: 10, verticalAlign: "middle" }}
              onClick={() => { void refreshLogs(); }}
              title="Re-fetch the bounded log tail (live for an in-flight parallel run; merged incrementally by the poll)"
            >
              ↻ Refresh
            </button>
            {(() => {
              const note = runLogTruncationNote(runLog);
              return note ? (
                <span
                  className="badge in_progress"
                  style={{ fontSize: 9, fontWeight: 600, marginLeft: 8, verticalAlign: "middle" }}
                  title="this tail is a bounded, redacted excerpt — earlier lines and/or byte-capped streams are not shown in full"
                >
                  {note}
                </span>
              ) : null;
            })()}
          </h5>
          <div className="muted" style={{ fontSize: 10, marginBottom: 6 }}>
            stdout/stderr/system lines — already secret-redacted and bounded. Live tail for an
            in-flight parallel run (lines appear before it finalizes); polled, merged incrementally.
          </div>
          {logsLoading && !runLog ? (
            <div className="loading">Loading logs...</div>
          ) : logsError ? (
            <div className="banner err" style={{ fontSize: 12 }}>
              Error loading logs: {String(logsError)}
            </div>
          ) : !runLogIsEmpty(runLog) ? (
            <div className="table-scroll" style={{ maxHeight: 300 }}>
              <table className="table sm">
                <thead>
                  <tr>
                    <th>#</th>
                    <th>Source</th>
                    <th>Line</th>
                  </tr>
                </thead>
                <tbody>
                  {runLog!.lines.map((line) => (
                    <tr key={line.seq}>
                      <td className="mono" style={{ fontSize: 10 }}>{line.seq}</td>
                      <td>
                        <span
                          className={`badge ${line.source === "stderr" ? "failed" : line.source === "system" ? "in_progress" : "queued"}`}
                          style={{ fontSize: 9 }}
                        >
                          {runLogSourceLabel(line.source)}
                        </span>
                      </td>
                      <td className="mono" style={{ fontSize: 11, whiteSpace: "pre-wrap" }}>
                        {line.text}
                        {line.truncated && (
                          <span className="muted" title="this line was clamped to the per-line length cap"> …[line truncated]</span>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <div className="empty sm">
              {inFlight ? "No logs yet for this run." : "No logs captured for this run."}
            </div>
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
