// Pure, framework-free recovery recommendation model for failed / blocked work
// (docs/relix-execution-and-issue-design.md §3.3b "Diagnosis-driven escalation +
// the Inbox decision card"; docs/relix-dashboard-design.md §6.9 "remaining gaps").
//
// §3.3b's decision card carries (1) a plain-language ROOT CAUSE and (2) a
// RECOMMENDATION plus one-click CHOICES (Retry / Block / Reassign / Investigate /
// Dismiss). This module builds that card's CONTENT deterministically from data the
// kernel already records — the run's structured `failure_class` + retry/session
// state, and a blocked task's reopen eligibility — so it is an HONEST read of what
// happened, never a fabricated AI guess. The actions it proposes are limited to
// affordances backed by an EXISTING route; when no action is available from the
// data on hand, it says what information is missing instead of inventing one.
//
// Kept dependency-free of React/DOM (it only type-imports the api shapes, which are
// erased) so the classification is unit-tested under `node --strip-types` (see the
// docs note dashboard-test-tsx-vs-ts-split). The Work page renders these.

import type { ReluxRun, ReluxRunDetail, ReluxTask } from "./api";
import { canRetryRun, canResumeRun } from "./runview.ts";
import { reopenEligibility, type ReopenableTask } from "./taskmove.ts";

// One proposed recovery action. The `kind` is a stable key the renderer maps to an
// EXISTING affordance/route (it never invents authority); `label` is the button
// text and `hint` the one-line "what this does / why". `primary` marks the single
// recommended first action (the one the operator should reach for).
export type RecoveryActionKind =
  | "retry_run" // POST /v1/relux/runs/:id/retry — a fresh cold run of a failed run
  | "resume_session" // POST /v1/relux/runs/:id/resume — continue the captured session
  | "reopen" // POST /v1/relux/tasks/:id/reopen — re-queue a blocked task
  | "reopen_and_run" // POST /v1/relux/tasks/:id/reopen-and-run — re-queue + run
  | "reassign" // POST /v1/relux/tasks/:id/assign — hand to another operative
  | "open_approval" // the Approvals surface — decide a pending gate
  | "configure_agent" // the Crew / Settings surface — adapter / credential / permission
  | "inspect"; // the run transcript + logs already on this surface

export interface RecoveryActionSpec {
  kind: RecoveryActionKind;
  label: string;
  hint: string;
  // The recommended first action. Exactly one action is primary when any is.
  primary?: boolean;
}

export interface RecoveryAssessment {
  // What was diagnosed.
  subject: "run" | "task";
  // A short human label for the failure / hold class (the card's heading chip).
  classLabel: string;
  // The structured class this came from, when any (run `failure_class`); used for the
  // badge tone. Absent for a hold with no underlying failed run.
  failureClass?: string;
  // Plain-language root cause (§3.3b "the tester couldn't start because …").
  rootCause: string;
  // Plain-language recommendation (§3.3b "add the secret and retry, or block …").
  recommendation: string;
  // Proposed actions in priority order, each backed by an existing route. May be
  // empty — then `missingInfo` explains what is needed before anything can be done.
  actions: RecoveryActionSpec[];
  // When the data on hand offers no safe action: what is missing / where to look.
  // Null when at least one action is proposed.
  missingInfo: string | null;
}

// The known run failure classes (relux_core::run_failure::RunFailureClass). Kept
// local so a future server class falls through to the honest "unknown" branch.
const KNOWN_FAILURE_CLASSES = new Set([
  "transient_provider",
  "auth_required",
  "adapter_missing",
  "permission_denied",
  "invalid_prompt",
  "timeout",
  "cancelled",
  "output_validation",
  "unknown",
]);

// A retry action spec, present only when the run is actually retry-eligible (the
// backend `retryable` flag / the honest local rule). `primary` is set by the caller.
function retryAction(label: string, hint: string, primary = false): RecoveryActionSpec {
  return { kind: "retry_run", label, hint, primary };
}

const INSPECT_ACTION: RecoveryActionSpec = {
  kind: "inspect",
  label: "Inspect logs",
  hint: "Read this run's transcript and log tail to see exactly what happened.",
};

// Build the recovery assessment for a run, or null when there is nothing to recover
// (the run is not failed/cancelled and carries no failure class — e.g. it is still
// running or completed cleanly). Pure: it reads only the run record + the derived
// retry/resume eligibility, never a clock or the network.
export function assessRunRecovery(
  run: ReluxRun | ReluxRunDetail,
): RecoveryAssessment | null {
  const failed = run.status === "failed";
  const cancelled = run.status === "cancelled";
  const failureClass = run.failure_class;
  if (!failed && !cancelled && !failureClass) return null;

  const retryable = canRetryRun(run);
  const resumable = canResumeRun(run);
  const retry = run.retry;

  // The actions every escalating failure can at least offer: inspect, plus retry
  // and/or resume when eligible. The class-specific branch orders + augments these.
  const resumeAction: RecoveryActionSpec | null = resumable
    ? {
        kind: "resume_session",
        label: "Resume session",
        hint: "Continue the captured provider session where it stopped (not a cold restart).",
      }
    : null;

  const cls = failureClass && KNOWN_FAILURE_CLASSES.has(failureClass) ? failureClass : "unknown";

  switch (cls) {
    case "adapter_missing":
      return {
        subject: "run",
        classLabel: "Adapter not available",
        failureClass: cls,
        rootCause:
          "The operative's adapter (its CLI/runtime) isn't installed or enabled, so the run couldn't start.",
        recommendation:
          "Enable or install the adapter for this operative, then retry the run.",
        actions: [
          {
            kind: "configure_agent",
            label: "Configure adapter",
            hint: "Open Crew / Settings to enable or install the operative's adapter.",
            primary: true,
          },
          ...(retryable ? [retryAction("Retry", "Re-run once the adapter is available.")] : []),
          INSPECT_ACTION,
        ],
        missingInfo: null,
      };

    case "auth_required":
      return {
        subject: "run",
        classLabel: "Authentication required",
        failureClass: cls,
        rootCause:
          "The adapter's provider credential is missing or expired, so the run was rejected before doing any work.",
        recommendation:
          "Add or refresh the provider credential (a stored secret), then retry the run.",
        actions: [
          {
            kind: "configure_agent",
            label: "Fix credentials",
            hint: "Open Settings to add or refresh the provider key for this adapter.",
            primary: true,
          },
          ...(retryable ? [retryAction("Retry", "Re-run once the credential is set.")] : []),
          INSPECT_ACTION,
        ],
        missingInfo: null,
      };

    case "permission_denied":
      return {
        subject: "run",
        classLabel: "Permission denied",
        failureClass: cls,
        rootCause:
          "The run tried something the operative isn't permitted to do — a tool or action outside its grants.",
        recommendation:
          "Grant the missing permission to the operative, or reassign the work to one that already has it. Inspect the transcript to see which action was blocked.",
        actions: [
          {
            kind: "configure_agent",
            label: "Review permissions",
            hint: "Open Crew governance to grant the operative the blocked capability.",
            primary: true,
          },
          {
            kind: "reassign",
            label: "Reassign",
            hint: "Hand the work to a different operative that holds the needed permission.",
          },
          INSPECT_ACTION,
          ...(retryable ? [retryAction("Retry", "Re-run once the permission is granted.")] : []),
        ],
        missingInfo: null,
      };

    case "transient_provider":
    case "timeout": {
      const exhausted = retry?.exhausted === true;
      const scheduled = !!retry && !exhausted;
      const classLabel = cls === "timeout" ? "Timed out" : "Transient provider error";
      const rootCause =
        cls === "timeout"
          ? "The run timed out — usually a slow or overloaded provider, not a defect in the work."
          : "A transient provider error (a hiccup or rate-limit on the model provider), not a defect in the work.";
      const recommendation = exhausted
        ? "The bounded automatic retries are spent. Retry manually if the work is still wanted, or inspect the logs if it keeps failing."
        : scheduled
          ? "A bounded automatic retry is scheduled — it usually self-heals. You can also retry now."
          : "Retry now; transient errors usually clear on a second attempt.";
      return {
        subject: "run",
        classLabel,
        failureClass: cls,
        rootCause,
        recommendation,
        actions: [
          ...(retryable
            ? [retryAction("Retry now", "Start a fresh attempt immediately.", true)]
            : []),
          ...(resumeAction ? [resumeAction] : []),
          INSPECT_ACTION,
        ],
        missingInfo: retryable
          ? null
          : "This run is not retry-eligible from here (a retry already exists, or the task moved on). Inspect the logs to decide.",
      };
    }

    case "invalid_prompt":
      return {
        subject: "run",
        classLabel: "Invalid request",
        failureClass: cls,
        rootCause:
          "The request the operative sent was rejected as invalid — usually the task's input/prompt needs adjusting.",
        recommendation:
          "Inspect the transcript to see what was rejected, revise the task input, then retry — or reassign to a different operative.",
        actions: [
          { ...INSPECT_ACTION, primary: true },
          ...(retryable ? [retryAction("Retry", "Re-run after revising the task input.")] : []),
          {
            kind: "reassign",
            label: "Reassign",
            hint: "Hand the work to a different operative.",
          },
        ],
        missingInfo: null,
      };

    case "output_validation":
      return {
        subject: "run",
        classLabel: "Output validation failed",
        failureClass: cls,
        rootCause:
          "The run produced output that failed validation, so its result was not accepted.",
        recommendation:
          "Inspect the transcript to see what failed validation, then retry.",
        actions: [
          { ...INSPECT_ACTION, primary: true },
          ...(retryable ? [retryAction("Retry", "Re-run for a fresh attempt.")] : []),
        ],
        missingInfo: null,
      };

    case "cancelled":
      return {
        subject: "run",
        classLabel: "Cancelled",
        failureClass: cls,
        rootCause:
          "This run was cancelled — a terminal non-error, not a failure of the work itself.",
        recommendation: "Start a fresh run if the work is still wanted.",
        actions: [
          ...(retryable
            ? [retryAction("Run again", "Start a fresh run of this work.", true)]
            : []),
          INSPECT_ACTION,
        ],
        missingInfo: retryable
          ? null
          : "A fresh run isn't offered from this record. Re-run the task from its board card if it is still wanted.",
      };

    default: {
      // "unknown" or an unrecognized/absent class on a failed run.
      const hasClass = !!failureClass;
      return {
        subject: "run",
        classLabel: "Unknown failure",
        failureClass: failureClass,
        rootCause: hasClass
          ? "The run failed with an unclassified error, so the cause can't be determined automatically."
          : "The run failed without a recorded failure class, so the cause can't be determined automatically.",
        recommendation:
          "Inspect the transcript and log tail to diagnose, then retry — or reassign if a different operative might fare better.",
        actions: [
          { ...INSPECT_ACTION, primary: true },
          ...(retryable ? [retryAction("Retry", "Re-run for a fresh attempt.")] : []),
          ...(resumeAction ? [resumeAction] : []),
          {
            kind: "reassign",
            label: "Reassign",
            hint: "Hand the work to a different operative.",
          },
        ],
        missingInfo:
          "No structured failure class was recorded — read the transcript/logs to diagnose the cause.",
      };
    }
  }
}

// The latest run for a task (highest id wins — run ids are monotonic), or null when
// the task has no runs. Pure helper so the task recovery card can fold in the most
// recent run's diagnosis. `runs` is the flat run list the board already holds.
export function latestRunForTask(
  runs: ReluxRun[],
  taskId: string,
): ReluxRun | null {
  let latest: ReluxRun | null = null;
  for (const r of runs) {
    if (r.task_id !== taskId) continue;
    if (!latest || r.id > latest.id) latest = r;
  }
  return latest;
}

// Build the recovery assessment for a task. A task surfaces a recovery card when it
// is BLOCKED (held work that needs a lifecycle decision) — the §6.9 gap. The
// recommendation is the reopen path (re-queue → run), and when the task's latest run
// FAILED, that run's diagnosed root cause is folded in as context so the operator
// knows WHY it stalled. A non-blocked task returns null (run-level recovery handles a
// failed run on its own surface). Pure: reads the task + its latest run + reopen
// eligibility, no clock/network.
export function assessTaskRecovery(
  task: ReluxTask,
  latestRun: ReluxRun | null,
): RecoveryAssessment | null {
  if (task.status !== "blocked") return null;

  const elig = reopenEligibility(task as ReopenableTask);
  // The latest run's diagnosis, when it failed, gives the honest "why blocked" context.
  const runFail =
    latestRun && (latestRun.status === "failed" || latestRun.failure_class)
      ? assessRunRecovery(latestRun)
      : null;

  const rootCause = runFail
    ? `This task is on hold. Its last run stalled: ${lowerFirst(runFail.rootCause)}`
    : "This task is on hold (blocked) — it was parked by an operator or by an unmet blocker, so it isn't runnable until it's reopened.";

  if (!elig.eligible) {
    // Blocked but no assignee — the kernel reopen guard rejects this. The only safe
    // next step is to assign an operative first (the same honest reason §6.9 shows).
    return {
      subject: "task",
      classLabel: "Blocked",
      rootCause,
      recommendation:
        "Assign an operative to this task before reopening — a run needs an assignee. Then reopen it to put it back in the run lifecycle.",
      actions: [
        {
          kind: "reassign",
          label: "Assign operative",
          hint: "Pick an operative; assigning re-queues the task so it can run.",
          primary: true,
        },
      ],
      missingInfo: elig.reason,
    };
  }

  // Blocked + assigned: the reopen lifecycle action is the recommended path.
  return {
    subject: "task",
    classLabel: "Blocked",
    rootCause,
    recommendation: runFail
      ? `${runFail.recommendation} If the underlying cause is addressed, reopen the task to put it back in the run lifecycle.`
      : "Reopen the task to put it back in the run lifecycle, then run it again — or reassign it to a different operative.",
    actions: [
      {
        kind: "reopen_and_run",
        label: "Reopen & run",
        hint: "Re-queue the task and run it now through the same run gate (no bypass).",
        primary: true,
      },
      {
        kind: "reopen",
        label: "Reopen",
        hint: "Re-queue the task so its operative can run it again (running stays a separate step).",
      },
      {
        kind: "reassign",
        label: "Reassign",
        hint: "Hand the work to a different operative (re-queues it).",
      },
    ],
    missingInfo: null,
  };
}

// Lowercase the first character of a sentence so it reads cleanly when spliced after
// "Its last run stalled: …". Leaves an all-caps acronym start alone.
function lowerFirst(s: string): string {
  if (!s) return s;
  const first = s[0];
  const second = s[1];
  // Don't de-capitalize an acronym (two leading capitals, e.g. "URL").
  if (second && first === first.toUpperCase() && second === second.toUpperCase()) return s;
  return first.toLowerCase() + s.slice(1);
}
