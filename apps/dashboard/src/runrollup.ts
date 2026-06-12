// Pure, framework-free helpers for the per-subtree RUN / COST ROLLUP on the board
// (docs/relix-dashboard-design.md §6 "A progress strip on a parent: … and live
// cost (tokens + spend) for the subtree" + the §6.1/§6.2/§6.3 still-pending
// "per-subtree cost rollup" target).
//
// THE DATA (all REAL, never fabricated). A `relux_core::Run`
// (crates/relux-core/src/run.rs) carries `task_id`, a lifecycle `status`, and
// three OPTIONAL measured fields the adapter only reports when it emitted a
// machine-readable result envelope:
//   - `cost` (USD) — the envelope's `total_cost_usd`; absent otherwise.
//   - `duration_ms` — the REAL wall-clock of the adapter subprocess; absent for the
//     deterministic local-echo path (which touches no process) and any run with no
//     measured subprocess.
//   - `usage` — the token object; absent without a structured envelope.
// So cost / duration / tokens are frequently ABSENT, and this module is scrupulous:
// it sums ONLY the runs that reported a figure and records how many did, so the UI
// can say "cost unavailable" honestly instead of printing a fabricated $0.00.
// A genuine $0.00 (a run that reported cost 0) is distinct from "unavailable".
//
// This is a CLIENT join over two EXISTING reads — `reluxWork.listRuns()` (all runs,
// each with `task_id` + the optional metrics) and a subtree's child task ids (from
// workhierarchy / adhocsubtrees). No backend route is added; the data is already on
// the Work page. Kept dependency-free (no React/DOM) so it runs under
// `node --strip-types` (see docs note dashboard-test-tsx-vs-ts-split).

import type { ReluxRun } from "./api";

// The health bucket for one run's lifecycle status.
//   active = pending / running / waiting_for_approval (and any unknown in-flight)
//   done   = completed
//   failed = ended WITHOUT completing (failed or cancelled)
// Note this differs from the task-board `taskBucket` (which sends a cancelled TASK
// to "done"): a cancelled RUN did not complete its work, so for a run-health signal
// it counts as not-completed alongside failures (the "failed" chip's tooltip says so).
export type RunBucket = "active" | "done" | "failed";

export function runBucket(status: string): RunBucket {
  switch (status) {
    case "completed":
      return "done";
    case "failed":
    case "cancelled":
      return "failed";
    default:
      // pending / running / waiting_for_approval and any future in-flight status
      return "active";
  }
}

// A read-only rollup of the runs under one subtree (an orchestration group's child
// tasks, or an ad-hoc parent + its children). Every figure is real; the `*Known`
// flags + `*Runs` coverage counts let the UI be honest about absent metrics.
export interface RunRollup {
  // Run counts across the subtree's tasks.
  runs: number;
  active: number;
  done: number;
  failed: number;
  // Cost (USD). `costUsd` sums ONLY runs that reported a cost; `costRuns` is how many
  // did. `costKnown === costRuns > 0` — when false the UI shows "cost unavailable",
  // never a fabricated $0.00. (A reported cost of exactly 0 IS known and real.)
  costUsd: number;
  costRuns: number;
  costKnown: boolean;
  // Real measured wall-clock duration (ms), summed over runs that reported one.
  durationMs: number;
  durationRuns: number;
  durationKnown: boolean;
  // Token totals (input + output), summed over runs whose `usage` carried numbers.
  tokens: number;
  tokenRuns: number;
  tokensKnown: boolean;
}

// Sum input + output tokens from a run's `usage` object, or null when neither field
// is a real number (so the run does not count toward token coverage). Tolerant of
// the varying envelope shapes — only the two canonical fields are read.
function usageTokens(usage: Record<string, unknown> | undefined): number | null {
  if (!usage || typeof usage !== "object") return null;
  const input = usage["input_tokens"];
  const output = usage["output_tokens"];
  const hasInput = typeof input === "number" && Number.isFinite(input);
  const hasOutput = typeof output === "number" && Number.isFinite(output);
  if (!hasInput && !hasOutput) return null;
  return (hasInput ? (input as number) : 0) + (hasOutput ? (output as number) : 0);
}

// The empty rollup (no runs) — the honest zero state for a subtree whose tasks have
// never been executed. Nothing is "known" because nothing was measured.
function emptyRollup(): RunRollup {
  return {
    runs: 0,
    active: 0,
    done: 0,
    failed: 0,
    costUsd: 0,
    costRuns: 0,
    costKnown: false,
    durationMs: 0,
    durationRuns: 0,
    durationKnown: false,
    tokens: 0,
    tokenRuns: 0,
    tokensKnown: false,
  };
}

// Roll up every run whose `task_id` is in `taskIds`. Sums cost / duration / tokens
// only for the runs that actually reported each, tracking coverage so the UI can be
// honest about absent metrics. Pure — no fabricated values, never reads the network.
export function rollupRuns(runs: ReluxRun[], taskIds: Iterable<string>): RunRollup {
  const ids = taskIds instanceof Set ? (taskIds as Set<string>) : new Set(taskIds);
  const r = emptyRollup();
  for (const run of runs) {
    if (!ids.has(run.task_id)) continue;
    r.runs += 1;
    switch (runBucket(run.status)) {
      case "done":
        r.done += 1;
        break;
      case "failed":
        r.failed += 1;
        break;
      default:
        r.active += 1;
    }
    if (typeof run.cost === "number" && Number.isFinite(run.cost)) {
      r.costUsd += run.cost;
      r.costRuns += 1;
    }
    if (typeof run.duration_ms === "number" && Number.isFinite(run.duration_ms)) {
      r.durationMs += run.duration_ms;
      r.durationRuns += 1;
    }
    const tok = usageTokens(run.usage);
    if (tok !== null) {
      r.tokens += tok;
      r.tokenRuns += 1;
    }
  }
  r.costKnown = r.costRuns > 0;
  r.durationKnown = r.durationRuns > 0;
  r.tokensKnown = r.tokenRuns > 0;
  return r;
}

// Format a USD cost compactly. Sub-cent costs keep 4 decimals (a typical agent run
// costs a fraction of a cent); larger costs use 2. Only called when cost is known —
// the caller shows "cost unavailable" otherwise.
export function formatCostUsd(usd: number): string {
  if (usd === 0) return "$0.00";
  // Agent run costs are typically a fraction of a cent, so keep 4 decimals below $1
  // (so a $0.0123 run is not flattened to $0.01); dollars use the usual 2.
  if (usd < 1) return `$${usd.toFixed(4)}`;
  return `$${usd.toFixed(2)}`;
}

// Format a measured duration: ms under 1s, one-decimal seconds under a minute, then
// `Nm Ss`.
export function formatDurationMs(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const totalSecs = ms / 1000;
  if (totalSecs < 60) return `${totalSecs.toFixed(1)}s`;
  const mins = Math.floor(totalSecs / 60);
  const secs = Math.round(totalSecs % 60);
  return `${mins}m ${secs}s`;
}

// Format a token count: raw under 1k, `N.Nk` under a million, then `N.NM`.
export function formatTokens(n: number): string {
  if (n < 1000) return `${n}`;
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

// One compact rollup chip: short visible text, an honest tooltip, and a semantic
// tone (color is meaning-only — design §12; the component maps tone → a B&W badge).
export interface RollupChip {
  label: string;
  title: string;
  tone: "neutral" | "active" | "failed";
}

// Build the compact chip strip for a subtree's run/cost rollup. Order: run count →
// active → failed → cost → duration → tokens. Absent metrics surface honestly
// ("cost unavailable"), never as a fabricated zero. An empty subtree shows a single
// "no runs yet" chip.
export function runRollupChips(r: RunRollup): RollupChip[] {
  if (r.runs === 0) {
    return [
      {
        label: "no runs yet",
        title: "No execution attempts recorded for this subtree's tasks.",
        tone: "neutral",
      },
    ];
  }
  const chips: RollupChip[] = [];
  const runWord = r.runs === 1 ? "run" : "runs";
  chips.push({
    label: `${r.runs} ${runWord}`,
    title: `${r.runs} ${runWord} across this subtree — ${r.active} active · ${r.done} done · ${r.failed} not completed.`,
    tone: "neutral",
  });
  if (r.active > 0) {
    chips.push({
      label: `${r.active} active`,
      title: "Runs pending, running, or waiting on a tool approval.",
      tone: "active",
    });
  }
  if (r.failed > 0) {
    chips.push({
      label: `${r.failed} failed`,
      title: "Runs that ended without completing (failed or cancelled).",
      tone: "failed",
    });
  }
  if (r.costKnown) {
    const coverage = r.costRuns < r.runs ? ` (from ${r.costRuns}/${r.runs} runs)` : "";
    chips.push({
      label: formatCostUsd(r.costUsd),
      title: `Reported run cost${coverage}. Only adapter runs that emitted a structured result envelope report a cost.`,
      tone: "neutral",
    });
  } else {
    chips.push({
      label: "cost unavailable",
      title:
        "No run in this subtree reported a cost — the local-echo path and plain-text adapters emit no cost envelope. Not zero; unknown.",
      tone: "neutral",
    });
  }
  if (r.durationKnown) {
    const coverage = r.durationRuns < r.runs ? ` (from ${r.durationRuns}/${r.runs} runs)` : "";
    chips.push({
      label: formatDurationMs(r.durationMs),
      title: `Measured adapter wall-clock${coverage}.`,
      tone: "neutral",
    });
  }
  if (r.tokensKnown) {
    const coverage = r.tokenRuns < r.runs ? `${r.tokenRuns}/${r.runs} runs` : "all runs";
    chips.push({
      label: `${formatTokens(r.tokens)} tok`,
      title: `Reported tokens (input + output) from ${coverage}.`,
      tone: "neutral",
    });
  }
  return chips;
}

// The task ids whose runs make up an ad-hoc subtree's rollup: the parent itself
// (it is a real task that may have runs) plus its direct children's ids.
export function adhocSubtreeTaskIds(parentId: string, childTaskIds: string[]): string[] {
  return [parentId, ...childTaskIds];
}
