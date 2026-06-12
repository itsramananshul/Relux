// Pure, testable action model for the INLINE approval controls in the Board
// Oversight strip on the Work page (docs/relix-dashboard-design.md §"Approvals" /
// §11; RELUX_MASTER_PLAN §7.4 per-call approval). It decides which of the EXISTING
// approval decisions are honest and applicable for one approval — it invents no new
// authority. Each action it green-lights maps to a route the Approvals page and the
// Prime approval card already drive (reluxApprovals.{decide,execute,allowAlways}).
//
// Kept dependency-free (no React, no DOM) so the action model can be unit-tested
// under `node --strip-types` without the esbuild render harness (see docs note
// dashboard-test-tsx-vs-ts-split). The Work oversight strip renders these.

import type { ReluxApproval } from "./api";

// The inline action set the oversight strip may offer for one approval. Derived
// ONLY from the approval's existing shape, so the strip can never offer a decision
// the backend would reject for that record.
export interface ApprovalInlineActions {
  // The affirmative decision, or null when none is offered inline:
  //  - "approve_run": a bound per-call tool invocation → decide(approved) then
  //    execute once (the exact two-step the Approvals page + Prime card use).
  //  - "approve": a generic approval → decide(approved). It records the decision
  //    but runs nothing here (a generic approval has no bound call to execute).
  approve: { kind: "approve_run" | "approve"; label: string } | null;
  // Allow-always is offered ONLY for a tool-invocation approval (the allow-always
  // route 404s for a generic approval); it persists a standing grant and runs once.
  allowAlways: boolean;
  // Deny (decide(rejected)) is offered for any pending approval.
  deny: boolean;
  // True when at least one inline decision is offered. When false, the strip shows
  // "Open details" only, with `reason` explaining why.
  actionable: boolean;
  // An honest one-line note: why the set is reduced (or details-only). Empty when
  // the full per-call set is offered and no caveat applies.
  reason: string;
}

// Decide the inline action set for one approval. The oversight strip only ever feeds
// PENDING approvals, but a non-pending record is handled defensively (details-only).
export function approvalInlineActions(a: ReluxApproval): ApprovalInlineActions {
  if (a.status !== "pending") {
    // Already decided elsewhere (or stale) — no inline decision is honest; the
    // detailed Approvals surface owns the full, decided record.
    return {
      approve: null,
      allowAlways: false,
      deny: false,
      actionable: false,
      reason: `Already ${a.status} — open details for the full record.`,
    };
  }
  if (a.tool_invocation) {
    // A bound per-call tool invocation: the full common set — approve & run, allow
    // always, deny — all through the existing per-call approval routes.
    return {
      approve: { kind: "approve_run", label: "Approve & run" },
      allowAlways: true,
      deny: true,
      actionable: true,
      reason: "",
    };
  }
  // A generic approval (no bound call): decide only. Approve records the decision
  // but executes nothing here, and allow-always does not apply (no tool to grant).
  // Deny is still available.
  return {
    approve: { kind: "approve", label: "Approve" },
    allowAlways: false,
    deny: true,
    actionable: true,
    reason: "Records the decision — nothing runs here. Open details for the full record.",
  };
}
