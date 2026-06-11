import { test } from "node:test";
import assert from "node:assert/strict";
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
  REVIEW_APPLY_UNAVAILABLE_REASON,
  APPLY_PENDING_DIFF_MODEL_REASON,
  APPLY_AVAILABLE_REASON,
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
  runSession,
  canResumeRun,
  canCancelRun,
  sessionHandoffLabel,
} from "../src/runview.ts";

// The Work page's run-depth view must read HONESTLY: it only formats/classifies
// what the backend recorded, never fabricates progress or metrics, and offers a
// Retry only for runs the backend marked retryable. These assertions pin that.

test("runStatusTone maps known statuses and falls back neutrally", () => {
  assert.equal(runStatusTone("completed"), "done");
  assert.equal(runStatusTone("running"), "running");
  // `failed` carries the shared error-red chip tone (`.badge.blocked`) so a
  // failed run is never rendered in the neutral tone (relix-dashboard-design
  // §12: status vocabulary includes "error"; "No silent failures").
  assert.equal(runStatusTone("failed"), "blocked");
  // `cancelled` is a terminal non-error and stays neutral.
  assert.equal(runStatusTone("cancelled"), "backlog");
  assert.equal(runStatusTone(undefined), "backlog");
});

test("formatRunDuration only renders a real measured value", () => {
  assert.equal(formatRunDuration(undefined), null); // local echo path: no duration
  assert.equal(formatRunDuration(null), null);
  assert.equal(formatRunDuration(-5), null);
  assert.equal(formatRunDuration(450), "450 ms");
  assert.equal(formatRunDuration(8123), "8.1 s");
  assert.equal(formatRunDuration(42000), "42 s");
  assert.equal(formatRunDuration(95000), "1m 35s");
});

test("canRetryRun prefers the backend retryable flag, falls back to failed", () => {
  // Run detail carries the server-derived flag — trust it exactly.
  assert.equal(canRetryRun({ id: "r1", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "failed", retryable: false } as any), false);
  assert.equal(canRetryRun({ id: "r1", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "failed", retryable: true } as any), true);
  // List shape (no flag): only a failed run is retryable.
  assert.equal(canRetryRun({ id: "r1", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "failed" } as any), true);
  assert.equal(canRetryRun({ id: "r1", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "completed" } as any), false);
});

test("runSession defensively parses captured session identity", () => {
  const base = { id: "r1", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "completed" };
  // No session → null (never fabricated).
  assert.equal(runSession({ ...base } as any), null);
  // Malformed payloads render nothing rather than throwing.
  assert.equal(runSession({ ...base, session: "nope" } as any), null);
  assert.equal(runSession({ ...base, session: { source: "claude-cli" } } as any), null);
  // Well-formed session round-trips; a missing resume_supported reads false.
  assert.deepEqual(
    runSession({ ...base, session: { adapter_session_id: "sess-1", source: "claude-cli", resume_supported: true } } as any),
    { adapter_session_id: "sess-1", source: "claude-cli", resume_supported: true },
  );
  assert.equal(
    runSession({ ...base, session: { adapter_session_id: "sess-2", source: "codex-cli" } } as any)?.resume_supported,
    false,
  );
});

test("canResumeRun prefers backend resumable flag, falls back honestly", () => {
  const base = { id: "r1", task_id: "t", agent_id: "a", adapter_plugin: "p" };
  // Run detail carries the server-derived flag — trust it exactly.
  assert.equal(canResumeRun({ ...base, status: "completed", resumable: true } as any), true);
  assert.equal(canResumeRun({ ...base, status: "completed", resumable: false } as any), false);
  // List shape (no flag): a terminal run with a resumable session qualifies.
  assert.equal(
    canResumeRun({ ...base, status: "completed", session: { adapter_session_id: "s", source: "claude-cli", resume_supported: true } } as any),
    true,
  );
  // A non-resumable session, or a still-running run, does not qualify.
  assert.equal(
    canResumeRun({ ...base, status: "completed", session: { adapter_session_id: "s", source: "codex-cli", resume_supported: false } } as any),
    false,
  );
  assert.equal(
    canResumeRun({ ...base, status: "running", session: { adapter_session_id: "s", source: "claude-cli", resume_supported: true } } as any),
    false,
  );
  // No session at all → not resumable.
  assert.equal(canResumeRun({ ...base, status: "failed" } as any), false);
});

test("canCancelRun offers Cancel only for a running run", () => {
  const base = { id: "r1", task_id: "t", agent_id: "a", adapter_plugin: "p" };
  // An in-flight (running) run is offered Cancel; the backend is the honest
  // authority on whether it is actually a cancellable off-lock process run.
  assert.equal(canCancelRun({ ...base, status: "running" } as any), true);
  // A terminal or not-yet-running run is never offered Cancel.
  assert.equal(canCancelRun({ ...base, status: "completed" } as any), false);
  assert.equal(canCancelRun({ ...base, status: "failed" } as any), false);
  assert.equal(canCancelRun({ ...base, status: "cancelled" } as any), false);
  assert.equal(canCancelRun({ ...base, status: "pending" } as any), false);
});

test("sessionHandoffLabel is honest about resume support", () => {
  const base = { id: "r1", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "completed" };
  assert.equal(sessionHandoffLabel({ ...base } as any), null);
  const supported = sessionHandoffLabel({ ...base, session: { adapter_session_id: "s", source: "claude-cli", resume_supported: true } } as any);
  assert.match(supported ?? "", /resume supported/);
  const unsupported = sessionHandoffLabel({ ...base, session: { adapter_session_id: "s", source: "codex-cli", resume_supported: false } } as any);
  assert.match(unsupported ?? "", /resume not supported/);
  assert.match(unsupported ?? "", /handoff\/audit/);
});

test("runMetricsLine only shows metrics the adapter actually reported", () => {
  assert.equal(runMetricsLine({ id: "r", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "completed" } as any), null);
  assert.equal(
    runMetricsLine({ id: "r", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "completed", cost: 0.0125, usage: { num_turns: 3, output_tokens: 210 } } as any),
    "$0.0125 · 3 turns · 210 output tokens",
  );
});

test("phaseLabel humanizes event kinds and falls back to status", () => {
  assert.equal(phaseLabel("adapter_spawn", "running"), "Spawning adapter");
  assert.equal(phaseLabel("run_failed", "failed"), "Failed");
  assert.equal(phaseLabel(undefined, "running"), "running");
  assert.equal(phaseLabel("some_future_kind", "running"), "some_future_kind");
});

test("isRunInFlight is true only for non-terminal states", () => {
  assert.equal(isRunInFlight("running"), true);
  assert.equal(isRunInFlight("pending"), true);
  assert.equal(isRunInFlight("completed"), false);
  assert.equal(isRunInFlight("failed"), false);
});

test("eventPayloadPreview drops bulky stdout/stderr and nulls", () => {
  assert.equal(eventPayloadPreview(null), null);
  assert.equal(eventPayloadPreview({ stdout: "huge", stderr: "" }), null);
  const preview = eventPayloadPreview({ stdout: "huge", exit_code: 0, structured: true });
  assert.ok(preview && preview.includes("exit_code"));
  assert.ok(preview && !preview.includes("huge"));
});

const ev = (kind: string): any => ({ id: "e", run_id: "r", ts: "t", kind, source: "kernel", message: "" });

test("toolCallSummary counts only real tool events, and is null when there are none", () => {
  assert.equal(toolCallSummary(undefined), null);
  assert.equal(toolCallSummary([]), null);
  // run_started/adapter_output are NOT tool calls — never fabricate one from them.
  assert.equal(toolCallSummary([ev("run_started"), ev("adapter_output"), ev("run_completed")]), null);
  assert.equal(toolCallSummary([ev("tool_call")]), "1 tool call");
  assert.equal(toolCallSummary([ev("tool_call"), ev("tool_call")]), "2 tool calls");
  assert.equal(
    toolCallSummary([ev("tool_call"), ev("tool_call_denied"), ev("tool_call_failed"), ev("tool_call_failed")]),
    "1 tool call · 1 denied · 2 failed",
  );
});

test("reviewApplyAvailability is honestly unavailable for a Relux run with no artifacts", () => {
  const base = { id: "r", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "completed" } as any;
  const verdict = reviewApplyAvailability(base);
  assert.equal(verdict.available, false);
  assert.equal(verdict.reason, REVIEW_APPLY_UNAVAILABLE_REASON);
  // The reason must name where the capability actually lives and why ids don't cross.
  assert.match(verdict.reason, /read-only execution record/);
  assert.match(verdict.reason, /legacy Runs surface/);
  assert.match(verdict.reason, /not Relux run ids/);
});

test("reviewApplyAvailability stays unavailable even WITH artifacts (no diff/apply model yet)", () => {
  // Capturing read-only references must NEVER enable apply — there is no Relux
  // diff/apply model. The reason adapts to explain the references are read-only.
  const withArtifacts = {
    id: "r", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "completed",
    artifacts: [{ name: "main.rs", type: "file", source: "claude-cli" }],
  } as any;
  const verdict = reviewApplyAvailability(withArtifacts);
  assert.equal(verdict.available, false);
  assert.equal(verdict.reason, APPLY_PENDING_DIFF_MODEL_REASON);
  assert.match(verdict.reason, /read-only artifact references/);
  assert.match(verdict.reason, /apply is unavailable until then/);
  // An empty artifact array falls back to the no-data reason.
  assert.equal(
    reviewApplyAvailability({ ...withArtifacts, artifacts: [] }).reason,
    REVIEW_APPLY_UNAVAILABLE_REASON,
  );
});

test("reviewApplyAvailability IS available when the run captured proposed changes", () => {
  // Proposed changes ARE the Relux diff/apply model: apply is real for them, and
  // takes precedence over the read-only-references reason.
  const withChanges = {
    id: "r", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "completed",
    artifacts: [{ name: "main.rs", type: "file", source: "claude-cli" }],
    proposed_changes: [
      { path: "src/main.rs", new_content: "x", new_sha256: "h", bytes: 1, source: "claude-cli", status: "proposed" },
    ],
  } as any;
  const verdict = reviewApplyAvailability(withChanges);
  assert.equal(verdict.available, true);
  assert.equal(verdict.reason, APPLY_AVAILABLE_REASON);
  assert.match(verdict.reason, /controlled workspace root/);
});

test("runProposedChanges returns only well-formed changes and normalizes status", () => {
  const run = {
    id: "r", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "completed",
    proposed_changes: [
      { path: "a.txt", new_content: "hi", new_sha256: "h", bytes: 2, source: "claude-cli", status: "approved", baseline_sha256: "b" },
      { path: "b.txt", new_content: "yo", new_sha256: "h", bytes: 2, source: "x", status: "made-up" }, // unknown → proposed
      { new_content: "no path", source: "x", status: "proposed" }, // no path → dropped
      { path: "c.txt", source: "x", status: "proposed" },          // no content → dropped
      "nope",                                                       // wrong shape → dropped
      null,
    ],
  } as any;
  const cs = runProposedChanges(run);
  assert.equal(cs.length, 2);
  assert.equal(cs[0].path, "a.txt");
  assert.equal(cs[0].status, "approved");
  assert.equal(cs[0].baseline_sha256, "b");
  assert.equal(cs[0].action, "replace"); // no action field → default replace
  assert.equal(cs[1].status, "proposed"); // unknown normalized
});

test("runProposedChanges parses the action field and normalizes unknown/absent to replace", () => {
  const run = {
    id: "r", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "completed",
    proposed_changes: [
      { path: "new.txt", action: "create", new_content: "hi", new_sha256: "h", bytes: 2, source: "x", status: "proposed" },
      { path: "old.txt", action: "replace", new_content: "yo", new_sha256: "h", bytes: 2, source: "x", status: "proposed", baseline_sha256: "b" },
      { path: "weird.txt", action: "purge", new_content: "z", new_sha256: "h", bytes: 1, source: "x", status: "proposed" }, // unknown → replace
      { path: "legacy.txt", new_content: "q", new_sha256: "h", bytes: 1, source: "x", status: "proposed" }, // absent → replace
      { path: "old.rs", action: "rename", dest_path: "new.rs", new_content: "", new_sha256: "h", bytes: 0, source: "x", status: "proposed", baseline_sha256: "b" },
      { path: "dead.rs", action: "delete", new_content: "", new_sha256: "h", bytes: 0, source: "x", status: "proposed", baseline_sha256: "b" },
    ],
  } as any;
  const cs = runProposedChanges(run);
  assert.equal(cs.length, 6);
  assert.equal(cs[0].action, "create");
  assert.equal(cs[1].action, "replace");
  assert.equal(cs[2].action, "replace"); // unknown normalized
  assert.equal(cs[3].action, "replace"); // absent normalized
  assert.equal(cs[4].action, "rename");
  assert.equal(cs[4].dest_path, "new.rs"); // destination is carried through
  assert.equal(cs[5].action, "delete");
  assert.equal(cs[5].dest_path, undefined); // a delete has no destination
});

test("proposedChangeActionLabel and isCreate/isRename/isDelete classify the action honestly", () => {
  assert.equal(proposedChangeActionLabel("create"), "Create");
  assert.equal(proposedChangeActionLabel("replace"), "Replace");
  assert.equal(proposedChangeActionLabel("rename"), "Rename");
  assert.equal(proposedChangeActionLabel("delete"), "Delete");
  assert.equal(proposedChangeActionLabel(undefined), "Replace");
  assert.equal(proposedChangeActionLabel("zzz"), "Replace");
  assert.equal(isCreateProposedChange({ action: "create" } as any), true);
  assert.equal(isCreateProposedChange({ action: "replace" } as any), false);
  assert.equal(isCreateProposedChange({} as any), false); // missing → replace
  assert.equal(isRenameProposedChange({ action: "rename" } as any), true);
  assert.equal(isRenameProposedChange({ action: "replace" } as any), false);
  assert.equal(isDeleteProposedChange({ action: "delete" } as any), true);
  assert.equal(isDeleteProposedChange({ action: "replace" } as any), false);
  assert.equal(isDeleteProposedChange({ action: "rename" } as any), false);
});

test("proposedChangePathLabel shows source → destination only for a rename", () => {
  assert.equal(
    proposedChangePathLabel({ action: "rename", path: "old.rs", dest_path: "new.rs" } as any),
    "old.rs → new.rs",
  );
  // A rename missing its destination falls back to just the source path.
  assert.equal(
    proposedChangePathLabel({ action: "rename", path: "old.rs" } as any),
    "old.rs",
  );
  assert.equal(
    proposedChangePathLabel({ action: "replace", path: "keep.rs" } as any),
    "keep.rs",
  );
  assert.equal(
    proposedChangePathLabel({ action: "create", path: "new.rs" } as any),
    "new.rs",
  );
  // A delete shows just its path (no destination).
  assert.equal(
    proposedChangePathLabel({ action: "delete", path: "dead.rs" } as any),
    "dead.rs",
  );
});

test("canApply gates a rename on approval AND a source baseline (like a replace)", () => {
  const mkRename = (status: string, baseline?: string) =>
    ({ path: "old", action: "rename", dest_path: "new", new_content: "", new_sha256: "h", bytes: 0, source: "x", status, baseline_sha256: baseline }) as any;
  assert.equal(canApplyProposedChange(mkRename("approved", "abc")), true);
  assert.equal(canApplyProposedChange(mkRename("approved")), false); // no baseline → not applyable
  assert.equal(canApplyProposedChange(mkRename("proposed", "abc")), false);
});

test("canApply gates a delete on approval AND a source baseline (like a replace)", () => {
  const mkDelete = (status: string, baseline?: string) =>
    ({ path: "dead", action: "delete", new_content: "", new_sha256: "h", bytes: 0, source: "x", status, baseline_sha256: baseline }) as any;
  assert.equal(canApplyProposedChange(mkDelete("approved", "abc")), true);
  assert.equal(canApplyProposedChange(mkDelete("approved")), false); // no baseline → not applyable
  assert.equal(canApplyProposedChange(mkDelete("proposed", "abc")), false);
});

test("runProposedChanges is empty for a run with none or a bad shape", () => {
  assert.deepEqual(runProposedChanges(undefined), []);
  assert.deepEqual(runProposedChanges({ id: "r" } as any), []);
  assert.deepEqual(runProposedChanges({ proposed_changes: "nope" } as any), []);
});

test("proposedChangeStatusLabel and tone map known states honestly", () => {
  assert.equal(proposedChangeStatusLabel("applied"), "Applied");
  assert.equal(proposedChangeStatusLabel("zzz"), "Proposed");
  assert.equal(proposedChangeStatusTone("applied"), "done");
  assert.equal(proposedChangeStatusTone("approved"), "done");
  assert.equal(proposedChangeStatusTone("proposed"), "running");
  assert.equal(proposedChangeStatusTone("rejected"), "backlog");
});

test("canReview/canApply gate on status, action, and a baseline hash", () => {
  const mk = (status: string, baseline?: string) =>
    ({ path: "f", new_content: "x", new_sha256: "h", bytes: 1, source: "x", status, baseline_sha256: baseline }) as any;
  const mkCreate = (status: string) =>
    ({ path: "f", action: "create", new_content: "x", new_sha256: "h", bytes: 1, source: "x", status }) as any;
  assert.equal(canReviewProposedChange(mk("proposed")), true);
  assert.equal(canReviewProposedChange(mk("approved")), false);
  // A replace apply needs an approved change WITH a baseline hash.
  assert.equal(canApplyProposedChange(mk("approved", "abc")), true);
  assert.equal(canApplyProposedChange(mk("approved")), false); // no baseline → not applyable
  assert.equal(canApplyProposedChange(mk("proposed", "abc")), false);
  // A create apply needs only approval — no baseline.
  assert.equal(canApplyProposedChange(mkCreate("approved")), true);
  assert.equal(canApplyProposedChange(mkCreate("proposed")), false);
});

test("batch helpers select reviewable/apply-eligible indices and gate the toolbar", () => {
  const mk = (status: string, baseline?: string) =>
    ({ path: "f", new_content: "x", new_sha256: "h", bytes: 1, source: "x", status, baseline_sha256: baseline }) as any;
  const mkCreate = (status: string) =>
    ({ path: "f", action: "create", new_content: "x", new_sha256: "h", bytes: 1, source: "x", status }) as any;
  const changes = [
    mk("proposed"),            // 0: reviewable, not apply-eligible
    mk("approved", "abc"),     // 1: apply-eligible (replace + baseline)
    mk("approved"),            // 2: approved replace but NO baseline → not apply-eligible
    mk("applied", "abc"),      // 3: terminal — neither
    mk("rejected"),            // 4: terminal — neither
    mkCreate("approved"),      // 5: apply-eligible (create, no baseline needed)
    mkCreate("proposed"),      // 6: reviewable, not apply-eligible
  ];
  assert.deepEqual(reviewableProposedChangeIndices(changes), [0, 6]);
  assert.deepEqual(applyEligibleProposedChangeIndices(changes), [1, 5]);
  // The batch toolbar shows only when there is MORE THAN ONE change.
  assert.equal(showBatchProposedChangeControls(changes), true);
  assert.equal(showBatchProposedChangeControls([mk("approved", "abc")]), false);
  assert.equal(showBatchProposedChangeControls([]), false);
});

test("runArtifacts returns only well-formed references and normalizes unknown types", () => {
  const run = {
    id: "r", task_id: "t", agent_id: "a", adapter_plugin: "p", status: "completed",
    artifacts: [
      { name: "main.rs", type: "file", source: "claude-cli", path: "src/main.rs", bytes: 42 },
      { name: "weird", type: "made-up", source: "claude-cli" }, // unknown → "other"
      { type: "file", source: "x" },           // no name → dropped
      { name: "no-source", type: "file" },      // no source → dropped
      "not-an-object",                          // wrong shape → dropped
      null,                                     // null → dropped
    ],
  } as any;
  const arts = runArtifacts(run);
  assert.equal(arts.length, 2);
  assert.equal(arts[0].name, "main.rs");
  assert.equal(arts[0].path, "src/main.rs");
  assert.equal(arts[0].bytes, 42);
  assert.equal(arts[1].type, "other"); // unknown normalized
});

test("runArtifacts is empty for a run with no artifacts or a bad shape", () => {
  assert.deepEqual(runArtifacts(undefined), []);
  assert.deepEqual(runArtifacts(null), []);
  assert.deepEqual(runArtifacts({ id: "r" } as any), []);
  assert.deepEqual(runArtifacts({ artifacts: "nope" } as any), []);
});

test("artifactTypeLabel maps known kinds and falls back to Other", () => {
  assert.equal(artifactTypeLabel("file"), "File");
  assert.equal(artifactTypeLabel("diff"), "Diff");
  assert.equal(artifactTypeLabel("url"), "URL");
  assert.equal(artifactTypeLabel("mystery"), "Other");
});

// --- Failure class + recovery surface (HERMES_OPENCLAW_DEEP_AUDIT.md §7) -----

test("failureClassLabel maps known classes and falls back without going blank", () => {
  assert.equal(failureClassLabel("transient_provider"), "Transient provider error");
  assert.equal(failureClassLabel("auth_required"), "Authentication required");
  assert.equal(failureClassLabel("adapter_missing"), "Adapter not available");
  assert.equal(failureClassLabel("timeout"), "Timed out");
  assert.equal(failureClassLabel("unknown"), "Unknown failure");
  // A class the UI doesn't know yet still renders the raw token, never blank.
  assert.equal(failureClassLabel("some_new_class"), "some_new_class");
  assert.equal(failureClassLabel(undefined), null);
});

test("failureClassTone: transient auto-recovers (running), others block, cancel is neutral", () => {
  assert.equal(failureClassTone("transient_provider"), "running");
  assert.equal(failureClassTone("timeout"), "running");
  assert.equal(failureClassTone("auth_required"), "blocked");
  assert.equal(failureClassTone("permission_denied"), "blocked");
  assert.equal(failureClassTone("cancelled"), "backlog");
});

test("recoveryStatusLine distinguishes scheduled / due / exhausted / operator-action", () => {
  // No class → no line (a successful or in-flight run).
  assert.equal(recoveryStatusLine({} as any, 1000), null);

  // A scheduled transient retry waiting on its not-before instant.
  const scheduled = {
    failure_class: "transient_provider",
    retry: { attempt: 0, max_attempts: 4, not_before_secs: 1120, exhausted: false },
  } as any;
  const sLine = recoveryStatusLine(scheduled, 1000);
  assert.ok(sLine && sLine.includes("Retry 1/4"));
  assert.ok(sLine && /auto-recovering/.test(sLine));

  // The same retry once its instant has passed → due.
  const dueLine = recoveryStatusLine(scheduled, 5000);
  assert.ok(dueLine && /is due/.test(dueLine));

  // Exhausted budget → manual-retry guidance, never another auto attempt.
  const exhausted = {
    failure_class: "timeout",
    retry: { attempt: 4, max_attempts: 4, exhausted: true },
  } as any;
  const eLine = recoveryStatusLine(exhausted, 5000);
  assert.ok(eLine && /exhausted/.test(eLine));

  // A non-retryable class → operator action.
  const authLine = recoveryStatusLine({ failure_class: "auth_required" } as any, 5000);
  assert.equal(authLine, "Needs operator action before it can succeed.");

  // An intentional cancel is neither a retry nor an operator-action problem.
  const cancelLine = recoveryStatusLine({ failure_class: "cancelled" } as any, 5000);
  assert.ok(cancelLine && /Cancelled/.test(cancelLine));
});
