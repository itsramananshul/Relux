// The INLINE approval decision controls (Approve & run / Allow always / Deny),
// shared by the Work board oversight strip (OversightApprovalRow) and the cross-Guild
// Inbox row (docs/relix-dashboard-design.md §"Approvals" / §6.11). The action set is
// decided by the pure `approvalInlineActions` model, and every button drives the SAME
// reluxApprovals route the dedicated Approvals page and the Prime approval card use
// (decide / execute / allow-always). It invents NO new authority and runs nothing the
// operator did not choose. After any decision it calls `onDecided` so the host surface
// refreshes in place, and shows a compact, shaped one-line result/error — never the
// raw tool envelope.
//
// This is the action core ONLY (the buttons + the result note). The host renders the
// approval's identity (action / reason / risk / bound tool) above it, so the same
// controls drop into either surface without duplicating their layout.

import { useState } from "react";
import { reluxApprovals, type ReluxApproval } from "../api";
import { approvalInlineActions } from "../approvalactions";

export function ApprovalInlineDecisions({
  approval,
  onDecided,
}: {
  approval: ReluxApproval;
  onDecided: () => void;
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
      onDecided();
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
      onDecided();
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
      onDecided();
    } catch (e) {
      setNote(e instanceof Error ? e.message : "Deny failed.");
    } finally {
      setWorking(null);
    }
  }

  return (
    <>
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
    </>
  );
}
