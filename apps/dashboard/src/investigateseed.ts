// Pure builder + storage handoff for "Investigate with Prime" (the §3.3b recovery
// card's "Investigate → chat companion pre-loaded with the diagnosis" choice;
// docs/relix-dashboard-design.md §6.10 remaining gap).
//
// When the operator clicks Investigate on a Recovery decision card, the dashboard
// seeds Prime with a SAFE, bounded, redacted investigation prompt and navigates to
// the chat. The prompt carries only the context the recovery model already holds —
// the task/run identity, the structured failure class + failure text, and the
// deterministic root cause + recommendation — framed as a DEBUGGING QUESTION that
// explicitly tells Prime NOT to create tasks, start runs, or change anything. So
// Prime answers like a Hermes-style debugging partner (a normal "answered" turn),
// never silently materializing work (RELUX_MASTER_PLAN §10.5, §17.1).
//
// The handoff is a one-shot sessionStorage entry: stash on click, CONSUME (read +
// remove) once on the Prime page's mount. No new backend route, no giant
// architecture — the seed is built from data already on the client, and Prime can
// fetch deeper logs/transcript on its own through its existing read-only context
// tools if it needs more.
//
// Kept dependency-free of React/DOM (it only type-imports the api + recovery shapes,
// which are erased) so the builder + redaction + bounding + consume-once semantics
// are unit-tested under `node --strip-types` (see dashboard-test-tsx-vs-ts-split).

import type { RecoveryAssessment } from "./recovery";
import type { ReluxRun, ReluxRunDetail, ReluxTask } from "./api";

// The sessionStorage key the card stashes the seed under and the Prime page consumes.
export const INVESTIGATION_SEED_KEY = "relux.prime.investigation-seed";

// Bounds so a pathological log tail / failure blob can never flood the chat seed.
// The tail keeps the MOST RECENT chars (the failure is at the end); single fields
// keep the head. Both are defensive — the kernel already bounds server-side.
export const MAX_LOG_TAIL_CHARS = 1200;
export const MAX_FIELD_CHARS = 500;

// The normalized, framework-free input the seed is built from. Each field is
// optional/nullable so a partial record (e.g. a blocked task with no failed run)
// still produces an honest seed naming only what is known.
export interface InvestigationSeedInput {
  subject: "run" | "task";
  task?: {
    id: string;
    title?: string | null;
    status?: string | null;
    assignee?: string | null;
  } | null;
  run?: {
    id: string;
    status?: string | null;
    failureClass?: string | null;
    // The run's failure text (error first, else summary). Bounded + redacted here.
    summary?: string | null;
    adapter?: string | null;
  } | null;
  // The deterministic recovery diagnosis (recovery.ts), folded in as context.
  classLabel: string;
  rootCause: string;
  recommendation: string;
  // An already-available client-side log/transcript excerpt, if any. Bounded +
  // redacted here. When absent, the seed points Prime at the ids to fetch its own.
  logTail?: string | null;
}

// Defensive client-side redaction of obvious secret shapes that might appear in a
// failure blob or log tail (the kernel redacts server-side; this is a belt-and-
// braces pass before text leaves for the chat). Targeted patterns only — it never
// blanket-scrubs normal prose, so the diagnosis stays readable.
export function redactSecrets(text: string): string {
  if (!text) return text;
  return text
    // Provider API keys: sk-… / sk-ant-… / pk-… / rk-… (keep the prefix as a hint).
    .replace(/\b(sk|pk|rk)-[A-Za-z0-9_-]{6,}/g, "$1-[REDACTED]")
    // Bearer / token auth headers.
    .replace(/\bBearer\s+[A-Za-z0-9._-]+/gi, "Bearer [REDACTED]")
    // key=value / key: value for sensitive keys (api_key, secret, token, password, …).
    .replace(
      /\b(api[_-]?key|secret|token|password|passwd|pwd|authorization|auth[_-]?token)\b(\s*[:=]\s*)("?)[^\s"]+\3/gi,
      (_m, key: string, sep: string) => `${key}${sep}[REDACTED]`,
    );
}

// Clamp a single field to a head-bounded length with an ellipsis when truncated.
function clampHead(text: string, max: number): string {
  const t = text.trim();
  return t.length > max ? `${t.slice(0, max - 1)}…` : t;
}

// Clamp a log tail to its MOST RECENT `max` chars (the failure is at the end),
// prefixing an honest "earlier lines omitted" marker when truncated.
function clampTail(text: string, max: number): string {
  const t = text.replace(/\s+$/g, "");
  if (t.length <= max) return t;
  return `… (earlier lines omitted)\n${t.slice(t.length - max)}`;
}

// Build the safe investigation seed text from the normalized input. Pure: no clock,
// network, or DOM. The leading instruction makes Prime a read-only debugging
// partner; the body lists only what is known, bounded + redacted.
export function buildInvestigationSeed(input: InvestigationSeedInput): string {
  const what = input.subject === "run" ? "a failed run" : "a blocked task";
  const lines: string[] = [];

  lines.push(
    `I'm investigating ${what} in Relux and want your help debugging it. ` +
      `Please DON'T create tasks, start runs, change any status, or run any tools — ` +
      `just help me understand what went wrong and what to check next, like a debugging partner.`,
  );
  lines.push("");
  lines.push("Context (already gathered for you):");

  if (input.task) {
    const t = input.task;
    const parts = [`id ${t.id}`];
    if (t.title) parts.push(`title "${clampHead(t.title, 160)}"`);
    if (t.status) parts.push(`status ${t.status}`);
    if (t.assignee) parts.push(`assignee ${t.assignee}`);
    lines.push(`- Task: ${parts.join(", ")}`);
  }

  if (input.run) {
    const r = input.run;
    const parts = [`id ${r.id}`];
    if (r.status) parts.push(`status ${r.status}`);
    if (r.adapter) parts.push(`adapter ${r.adapter}`);
    if (r.failureClass) parts.push(`failure class ${r.failureClass}`);
    lines.push(`- Run: ${parts.join(", ")}`);
    if (r.summary && r.summary.trim()) {
      lines.push(`- Failure text: ${redactSecrets(clampHead(r.summary, MAX_FIELD_CHARS))}`);
    }
  }

  lines.push(`- Diagnosis (Relix recovery model): ${input.classLabel}.`);
  lines.push(`  Root cause: ${redactSecrets(clampHead(input.rootCause, MAX_FIELD_CHARS))}`);
  lines.push(`  Recommended: ${redactSecrets(clampHead(input.recommendation, MAX_FIELD_CHARS))}`);

  if (input.logTail && input.logTail.trim()) {
    lines.push("");
    lines.push("Recent log tail (most recent lines, bounded + redacted):");
    lines.push("```");
    lines.push(redactSecrets(clampTail(input.logTail, MAX_LOG_TAIL_CHARS)));
    lines.push("```");
  } else if (input.run) {
    lines.push("");
    lines.push(
      `If you need more, the transcript and full log tail for run ${input.run.id} are available ` +
        `through your read-only run tools — read them before concluding.`,
    );
  }

  return lines.join("\n");
}

// Adapter: build the seed input for a FAILED RUN's recovery card from the live run
// record + its deterministic assessment (+ an optional already-available log tail).
export function runInvestigationInput(
  run: ReluxRun | ReluxRunDetail,
  assessment: RecoveryAssessment,
  logTail?: string | null,
): InvestigationSeedInput {
  const detail = run as ReluxRunDetail;
  return {
    subject: "run",
    task: {
      id: run.task_id,
      title: detail.task_title ?? null,
      assignee: run.agent_id ?? null,
    },
    run: {
      id: run.id,
      status: run.status ?? null,
      failureClass: run.failure_class ?? null,
      // Prefer the explicit error text; fall back to the run summary.
      summary: run.error ?? run.summary ?? null,
      adapter: run.adapter_plugin ?? null,
    },
    classLabel: assessment.classLabel,
    rootCause: assessment.rootCause,
    recommendation: assessment.recommendation,
    logTail: logTail ?? null,
  };
}

// Adapter: build the seed input for a BLOCKED TASK's recovery card from the task +
// its latest run (folded in only when that run actually failed) + the assessment.
export function taskInvestigationInput(
  task: ReluxTask,
  latestRun: ReluxRun | null,
  assessment: RecoveryAssessment,
  logTail?: string | null,
): InvestigationSeedInput {
  const failed = !!latestRun && (latestRun.status === "failed" || !!latestRun.failure_class);
  return {
    subject: "task",
    task: {
      id: task.id,
      title: task.title ?? null,
      status: task.status ?? null,
      assignee: task.assignee_name ?? task.assigned_agent ?? null,
    },
    run: failed
      ? {
          id: latestRun!.id,
          status: latestRun!.status ?? null,
          failureClass: latestRun!.failure_class ?? null,
          summary: latestRun!.error ?? latestRun!.summary ?? null,
          adapter: latestRun!.adapter_plugin ?? null,
        }
      : null,
    classLabel: assessment.classLabel,
    rootCause: assessment.rootCause,
    recommendation: assessment.recommendation,
    logTail: logTail ?? null,
  };
}

// The minimal Storage surface the handoff needs — so the consume-once logic can be
// unit-tested with a plain fake (no real Window/sessionStorage).
export interface SeedStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

// Stash the seed for the Prime page to pick up. Best-effort: a storage failure
// (private mode / quota) is swallowed so the navigation still happens (Prime then
// just opens normally).
export function stashInvestigationSeed(storage: SeedStorage, seed: string): void {
  try {
    storage.setItem(INVESTIGATION_SEED_KEY, seed);
  } catch {
    /* storage unavailable — fall through; Prime opens without a seed */
  }
}

// Consume the seed exactly ONCE: read it, then remove it so a refresh / re-mount
// never re-sends it. Returns the seed text, or null when none is pending (so normal
// Prime chat is untouched). A storage error degrades to null.
export function consumeInvestigationSeed(storage: SeedStorage): string | null {
  try {
    const seed = storage.getItem(INVESTIGATION_SEED_KEY);
    if (seed !== null) storage.removeItem(INVESTIGATION_SEED_KEY);
    return seed && seed.trim() ? seed : null;
  } catch {
    return null;
  }
}
