// Pure, dependency-free derivation of the first-run / operational readiness of a
// local Relux instance (RELUX_MASTER_PLAN §11 Dashboard / §22 Home: the dynamic
// first-run checklist reflects live health). Given the four control-plane reads
// the dashboard already makes — `state` (counts), `ai/status` (Prime brain),
// `adapters` (CLI runtimes) and `plugins`/`tools` (capability surface) — it
// returns one honest report: a compact checklist of what is/ isn't ready, the
// blockers vs. the things that merely need attention, a one-line operational
// summary, and the single clearest first action.
//
// The hard discipline this encodes (mirrored from the reference doctors —
// Hermes `hermes_cli/status.py`/`doctor.py` and openclaw's HealthStore
// `var state: HealthState`, where live booleans are turned into honest pass/
// warn/fail, never a faked green check): every item is computed from real state.
// A brain that is SELECTED but unusable is a blocker; a local-only brain WORKS
// (it is a recommendation, not a failure); an installed-but-unconfigured wrapper
// or runtime is "attention", not "ready". When nothing blocks, the caller shows
// a concise summary instead of nagging.
//
// Kept React-free (like ./routing, ./onboarding and ./plugins) so `node --test`
// can assert the guidance without a DOM. The component renders whatever this
// returns; it invents nothing.

import type {
  ReluxState,
  ReluxAiStatus,
  ReluxAdapterStatus,
  ReluxPlugin,
  ReluxToolDescriptor,
  ReluxPrimeBrain,
  ReluxPrimeContinuation,
} from "./api";
import {
  primeBrainStep,
  CLAUDE_ADAPTER_ID,
  CODEX_ADAPTER_ID,
} from "./onboarding.ts";
import { pluginCategory } from "./plugins.ts";

// done — ready / nothing to do.        todo — a BLOCKER: a selected capability
// link — works, an optional upgrade.   cannot work until the user acts.
// warn — installed but needs setup before it does anything (attention).
// info — neutral context (counts), never an action.
export type ReadinessStatus = "done" | "todo" | "warn" | "link" | "info";

export interface ReadinessItem {
  id: string;
  label: string;
  status: ReadinessStatus;
  description: string;
  // Where the fix lives (an existing dashboard page). Omitted only for the pure
  // context lines.
  linkTo?: string;
  // A short button label for the action, when there is a concrete one.
  cta?: string;
  // true when the honest fix is to re-run the failed read — the component wires
  // this to its Refresh handler so an "… unavailable" row offers a Retry.
  retry?: boolean;
}

export interface FirstAction {
  label: string;
  linkTo: string;
}

export interface ReadinessReport {
  items: ReadinessItem[];
  // status === "todo": a selected capability is broken and blocks normal use.
  blockers: ReadinessItem[];
  // status === "warn": installed but not configured; surfaced, never blocking.
  attention: ReadinessItem[];
  // true when there are no blockers AND no read failed — the caller shows the
  // operational summary. A degraded report is never "ready" (it must not fake a
  // green "operational" badge from partial data).
  ready: boolean;
  // true when one or more secondary reads FAILED (settled to null), so some rows
  // are explicit "… unavailable" placeholders rather than real readiness. The
  // caller shows an honest "degraded" banner instead of the operational summary.
  degraded: boolean;
  firstAction: FirstAction;
  // A one-line, secret-free operational summary for the "all set" state.
  summary: string;
}

// Which reads have SETTLED to null because the request FAILED, as opposed to
// null because the request is still in flight. The caller can only set a flag
// once it knows the read completed (not loading) — that is what lets the report
// distinguish "Checking readiness…" (loading) from "State unavailable" (failed).
export interface ReadinessFailed {
  state?: boolean;
  ai?: boolean;
  adapters?: boolean;
  plugins?: boolean;
  tools?: boolean;
}

export interface ReadinessInputs {
  state: ReluxState | null;
  ai: ReluxAiStatus | null;
  adapters: ReluxAdapterStatus[] | null;
  plugins: ReluxPlugin[] | null;
  // null when the tools probe failed/has not resolved (we then stay honest
  // rather than claim "no tools configured").
  tools: ReluxToolDescriptor[] | null;
  // A paused Prime agent-loop continuation, when one exists (from the composed
  // oversight read). Supplementary, not a primary readiness signal: absent (or a
  // failed oversight read) simply means "no paused work surfaced here" — it never
  // degrades the report or blocks setup.
  continuation?: ReluxPrimeContinuation | null;
  // Which of the above are null because the read FAILED (not still loading).
  // Omitted/empty means "treat every null as still-loading" — the prior contract.
  failed?: ReadinessFailed;
}

function adapterFor(
  adapters: ReluxAdapterStatus[] | null,
  id: string,
): ReluxAdapterStatus | undefined {
  return (adapters ?? []).find((a) => a.plugin_id === id);
}

// A human, secret-free label for the selected brain (used in the summary line).
export function brainLabel(ai: ReluxAiStatus | null): string {
  const brain: ReluxPrimeBrain | null = ai ? ai.brain : null;
  switch (brain) {
    case "claude_cli":
      return "Claude CLI";
    case "codex_cli":
      return "Codex CLI";
    case "openrouter":
      return "OpenRouter";
    case "local":
      return "Local (deterministic)";
    default:
      return "unknown";
  }
}

// "Prime brain" item — reuse the already-tested onboarding derivation so the two
// surfaces never disagree on what "connected" means.
function brainItem(
  ai: ReluxAiStatus | null,
  adapters: ReluxAdapterStatus[] | null,
): ReadinessItem {
  const step = primeBrainStep(ai, adapters);
  const cta =
    step.status === "todo"
      ? "Set up brain"
      : step.status === "link"
        ? "Configure brain"
        : undefined;
  return {
    id: step.id,
    label: step.label,
    status: step.status,
    description: step.description,
    linkTo: step.linkTo,
    cta,
  };
}

// "Run real work" item — whether a Claude/Codex CLI adapter can actually execute
// assigned tasks (DISTINCT from the brain item, which is who answers Prime's
// chat). This is the recommended-but-optional real-work path, so an unavailable
// adapter is a "link" (one click on Crew), never a blocker: Prime tracks work
// without it.
export function adapterWorkItem(
  adapters: ReluxAdapterStatus[] | null,
  failed = false,
): ReadinessItem {
  const base = { id: "run-real-work", linkTo: "/crew" } as const;

  // The adapter list read failed — say so rather than claiming "no CLI detected,
  // install one" (which would be a guess we cannot stand behind).
  if (adapters === null && failed) {
    return {
      ...base,
      label: "Adapter status unavailable",
      status: "warn",
      description:
        "Could not read the real-work adapter status from the control plane, so whether a Claude/Codex CLI is runnable is unknown. Retry, or open Crew → Adapters.",
      cta: "Open Crew",
      retry: true,
    };
  }

  const claude = adapterFor(adapters, CLAUDE_ADAPTER_ID);
  const codex = adapterFor(adapters, CODEX_ADAPTER_ID);
  const cli = [claude, codex].filter(Boolean) as ReluxAdapterStatus[];

  const ready = cli.find((a) => a.state === "available");
  if (ready) {
    const name = ready.plugin_id === CLAUDE_ADAPTER_ID ? "Claude CLI" : "Codex CLI";
    return {
      ...base,
      label: "Real-work adapter ready",
      status: "done",
      description: `${name} is enabled and on your PATH — Prime can run assigned tasks through it.`,
      cta: "Manage on Crew",
    };
  }

  const onPath = cli.find((a) => a.available_on_path);
  if (onPath) {
    const name = onPath.plugin_id === CLAUDE_ADAPTER_ID ? "Claude CLI" : "Codex CLI";
    return {
      ...base,
      label: "Enable a real-work adapter",
      status: "link",
      description: `${name} is detected on your PATH but its adapter is not enabled. Enable it on Crew → Adapters to run real work (it runs in a safe, non-bypass mode).`,
      cta: "Enable on Crew",
    };
  }

  return {
    ...base,
    label: "Connect a real-work adapter (optional)",
    status: "link",
    description:
      "Install and sign in to the Claude CLI (`claude`) or Codex CLI (`codex`) so it is on your PATH, then enable its adapter on Crew → Adapters to execute tasks. Optional — Prime creates and tracks work without it.",
    cta: "Open Crew",
  };
}

// "Crew" item — at least one agent, else explain that Prime itself is the
// built-in operative (the local fallback). Not a blocker.
export function crewItem(state: ReluxState | null): ReadinessItem {
  const n = state ? state.agents : 0;
  if (n > 0) {
    return {
      id: "crew",
      label: "Crew configured",
      status: "done",
      description: `${n} agent${n === 1 ? "" : "s"} configured — Prime can delegate work to ${n === 1 ? "it" : "them"}.`,
      linkTo: "/crew",
      cta: "Manage crew",
    };
  }
  return {
    id: "crew",
    label: "Add crew (optional)",
    status: "link",
    description:
      "No additional agents yet — Prime is your built-in operative and can do the work itself. Add specialized agents on Crew to delegate to.",
    linkTo: "/crew",
    cta: "Add crew",
  };
}

// "Plugins & tools" item — the honest capability surface. A metadata-only
// wrapper (generated, zero tools) or a tool that still needs a loopback runtime
// is "attention" (installed but does nothing yet); ready tools are "done";
// approval-gated tools are noted but not a failure (they are gated by design).
export function pluginToolItem(
  plugins: ReluxPlugin[] | null,
  tools: ReluxToolDescriptor[] | null,
  failed: { plugins?: boolean; tools?: boolean } = {},
): ReadinessItem {
  const base = { id: "plugins-tools", linkTo: "/plugins" } as const;

  // The plugin list read failed — the whole capability surface (wrappers + tools)
  // is unknown, so don't infer "no plugins / no config needed" from a null list.
  if (plugins === null && failed.plugins) {
    return {
      ...base,
      label: "Plugins unavailable",
      status: "warn",
      description:
        "Could not read the installed plugin list from the control plane, so tool readiness is unknown. Retry, or open Plugins to review what is installed.",
      cta: "Open Plugins",
      retry: true,
    };
  }

  // Wrappers needing config are knowable from the plugin list ALONE — surface
  // them first even when the tools probe is unavailable.
  const wrappers = (plugins ?? []).filter(
    (p) => pluginCategory(p) === "wrapper" && (p.tool_count ?? 0) === 0,
  ).length;
  if (wrappers > 0) {
    return {
      ...base,
      label: "Configure installed plugins",
      status: "warn",
      description: `${wrappers} plugin${wrappers === 1 ? " is" : "s are"} installed as metadata-only wrapper${wrappers === 1 ? "" : "s"} — add tool definitions to make ${wrappers === 1 ? "it" : "them"} runnable. Relux never infers tools from downloaded content.`,
      cta: "Configure",
    };
  }

  // The tools probe FAILED (settled to null) — an explicit, retryable row, not
  // the indefinite "right now" loading text below.
  if (tools === null && failed.tools) {
    return {
      ...base,
      label: "Tools unavailable",
      status: "warn",
      description:
        "Could not read tool readiness from the control plane. Retry, or open Plugins to review installed plugins and tool status.",
      cta: "Open Plugins",
      retry: true,
    };
  }

  // The tools probe has not resolved yet — stay honest rather than claim "no
  // tools configured", but do not assert failure either (it may still arrive).
  if (tools === null) {
    return {
      ...base,
      label: "Plugins & tools",
      status: "info",
      description: "Tool readiness is unavailable right now. Open Plugins to review installed plugins and tool status.",
      cta: "Open Plugins",
    };
  }

  const ready = tools.filter((t) => t.executable === "ready").length;
  const needsApproval = tools.filter((t) => t.executable === "needs_approval").length;
  const needsRuntime = tools.filter(
    (t) => t.executable === "runtime_not_configured" || t.executable === "runtime_disabled",
  ).length;

  if (needsRuntime > 0) {
    return {
      ...base,
      label: "Configure a tool runtime",
      status: "warn",
      description: `${needsRuntime} tool${needsRuntime === 1 ? "" : "s"} need a loopback runtime before ${needsRuntime === 1 ? "it" : "they"} can run. Point Relux at the local HTTP server you run on the plugin's row.`,
      cta: "Configure",
    };
  }

  if (ready > 0) {
    const approvalNote =
      needsApproval > 0
        ? ` ${needsApproval} more ${needsApproval === 1 ? "is" : "are"} gated behind per-call approval (by design).`
        : "";
    return {
      ...base,
      label: "Tools ready",
      status: "done",
      description: `${ready} tool${ready === 1 ? "" : "s"} ready to invoke.${approvalNote}`,
      cta: "View tools",
    };
  }

  return {
    ...base,
    label: "Add tools (optional)",
    status: "info",
    description:
      "No extra tools configured yet — Prime's built-in capabilities are available. Install plugins on Plugins to add tools and adapters.",
    cta: "Browse plugins",
  };
}

// Whether Prime's SELECTED brain is currently unusable — the one true blocker
// (an OpenRouter brain with no key, a CLI brain whose binary is gone). A local
// brain is never blocked. Used so the guided "try Prime" step is not offered
// before the brain that would answer it is connected, and so the first action
// routes to fixing the brain first.
export function brainIsBlocked(
  ai: ReluxAiStatus | null,
  adapters: ReluxAdapterStatus[] | null,
): boolean {
  if (!ai) return false; // unknown — don't assert a block we can't stand behind
  return primeBrainStep(ai, adapters).status === "todo";
}

// "Try Prime" item — the guided first useful turn, the answer to "what do I do?".
// It only appears once the brain that would answer is connected (else the brain
// item IS the action). Whether the operator has actually used Prime is inferred
// honestly from real state: any task created or any run recorded means the first
// turn happened (done); otherwise it is the recommended next step (a link, never
// a blocker — nothing is broken, there is simply nothing to show yet).
export function tryPrimeItem(
  state: ReluxState | null,
  ai: ReluxAiStatus | null,
  adapters: ReluxAdapterStatus[] | null,
): ReadinessItem | null {
  // Don't invite a first prompt before the brain is connected — the brain item
  // already carries that action, and a dead "Ask Prime" here would just fail.
  if (brainIsBlocked(ai, adapters)) return null;

  const base = { id: "try-prime", linkTo: "/prime" } as const;
  const tasks = state ? state.tasks : 0;
  const runs = state ? state.runs : 0;

  if (tasks > 0 || runs > 0) {
    return {
      ...base,
      label: "Used Prime",
      status: "done",
      description:
        "You've put Prime to work — ask it “what tools do you have?” any time, or give it a new goal to plan and run.",
      cta: "Open Prime",
    };
  }

  return {
    ...base,
    label: "Ask Prime what it can do",
    status: "link",
    description:
      "Your brain is connected but Prime hasn't done anything yet. Open Prime and ask “what tools do you have?”, or give it a goal — it can inspect state, create tasks, and start runs.",
    cta: "Ask Prime",
  };
}

// "Resume paused work" item — a Prime agent-loop that paused with work still to
// do (an autonomy ceiling reached, or a gated tool waiting on approval). Surfaced
// as attention because it needs the operator to act, never a blocker. The resume
// (and any pending approval) lives on the Work board's oversight strip.
export function continuationItem(
  continuation: ReluxPrimeContinuation | null | undefined,
): ReadinessItem | null {
  if (!continuation) return null;
  const reason = continuation.reason ? ` (${continuation.reason})` : "";
  const description = continuation.awaiting_approval
    ? `Prime paused a tool call that is waiting on your approval${reason}. Approve it, then resume the loop from where it left off — no work is re-run.`
    : `Prime paused with work still to do${reason}. Resume the loop to continue from the ${continuation.observation_count} step${continuation.observation_count === 1 ? "" : "s"} it already gathered.`;
  return {
    id: "paused-continuation",
    label: continuation.awaiting_approval
      ? "Approve, then resume paused work"
      : "Resume paused work",
    status: "warn",
    description,
    linkTo: "/work",
    cta: "Open Work",
  };
}

// "Work needs attention" item — blocked or failed work that is sitting waiting
// for a human (a blocked task, a failed run the watchdog surfaced). Attention,
// not a blocker: the rest of Relux still works. Routes to the unified Inbox,
// which is purpose-built for this queue.
export function attentionWorkItem(state: ReluxState | null): ReadinessItem | null {
  if (!state) return null;
  const blocked = state.blocked ?? 0;
  const failed = state.failed ?? 0;
  if (blocked <= 0 && failed <= 0) return null;
  const parts: string[] = [];
  if (blocked > 0) parts.push(`${blocked} blocked task${blocked === 1 ? "" : "s"}`);
  if (failed > 0) parts.push(`${failed} failed run${failed === 1 ? "" : "s"}`);
  return {
    id: "work-attention",
    label: "Work needs attention",
    status: "warn",
    description: `${parts.join(" and ")} ${blocked + failed === 1 ? "is" : "are"} waiting for you. Open the Inbox to inspect the cause and recover, retry, or reopen.`,
    linkTo: "/inbox",
    cta: "Open Inbox",
  };
}

// "State unavailable" item — the primary control-plane read (counts, tasks,
// approvals) failed, so crew/approvals/first-action are all guesses. Surface it
// honestly and retryably at the top instead of leaving the guide stuck on
// "Checking readiness…". A warn (not a todo): the kernel may simply be busy.
export function stateUnavailableItem(): ReadinessItem {
  return {
    id: "state-unavailable",
    label: "State unavailable",
    status: "warn",
    description:
      "Could not read live state (task, run and approval counts) from the control plane, so the readiness below is partial. Retry, or open Health to diagnose the kernel.",
    linkTo: "/health",
    cta: "Open Health",
    retry: true,
  };
}

// "Approvals" item — only present when something is actually waiting on the
// operator. Surfaced as attention (it needs a decision) and folded into the
// first action below.
function approvalsItem(state: ReluxState | null): ReadinessItem | null {
  const n = state ? state.pending_approvals : 0;
  if (n <= 0) return null;
  return {
    id: "approvals",
    label: "Pending approvals",
    status: "warn",
    description: `${n} approval${n === 1 ? "" : "s"} ${n === 1 ? "is" : "are"} waiting on your decision.`,
    linkTo: "/approvals",
    cta: "Review",
  };
}

// The single clearest next action, in priority order: fix a broken brain first
// (nothing works without it), then a pending decision, then paused/stuck work,
// then in-flight work, then starting the first work. Always lands on a real page
// (Prime is always available, so the fresh state still has an action).
//
// `ai`/`adapters`/`continuation` are optional so existing one-argument callers
// keep their exact behaviour: with them omitted the brain-blocker and paused-work
// branches simply do not fire (we don't assert a state we weren't given).
export function deriveFirstAction(
  state: ReluxState | null,
  ai: ReluxAiStatus | null = null,
  adapters: ReluxAdapterStatus[] | null = null,
  continuation: ReluxPrimeContinuation | null | undefined = null,
): FirstAction {
  // A selected-but-unusable brain blocks everything Prime does — fix it first.
  if (brainIsBlocked(ai, adapters)) {
    return { label: "Set up Prime's brain", linkTo: "/health" };
  }
  if (!state) return { label: "Talk to Prime", linkTo: "/prime" };
  if (state.pending_approvals > 0) {
    const n = state.pending_approvals;
    return { label: `Review ${n} pending approval${n === 1 ? "" : "s"}`, linkTo: "/approvals" };
  }
  if (continuation) {
    return {
      label: continuation.awaiting_approval ? "Approve & resume paused work" : "Resume paused work",
      linkTo: "/work",
    };
  }
  if ((state.blocked ?? 0) > 0 || (state.failed ?? 0) > 0) {
    return { label: "Inspect work that needs attention", linkTo: "/inbox" };
  }
  if (state.active_runs > 0) {
    const n = state.active_runs;
    return { label: `Watch ${n} active run${n === 1 ? "" : "s"}`, linkTo: "/work" };
  }
  if (state.open_tasks > 0) {
    return { label: "Start or assign a task", linkTo: "/work" };
  }
  if (state.tasks === 0) {
    return { label: "Ask Prime to start your first task", linkTo: "/prime" };
  }
  return { label: "Talk to Prime", linkTo: "/prime" };
}

function operationalSummary(inputs: ReadinessInputs): string {
  const { state, ai, tools } = inputs;
  const agents = state ? state.agents : 0;
  const ready = (tools ?? []).filter((t) => t.executable === "ready").length;
  const open = state ? state.open_tasks : 0;
  const running = state ? state.active_runs : 0;
  return (
    `Brain: ${brainLabel(ai)}. ` +
    `${agents} agent${agents === 1 ? "" : "s"}, ${ready} tool${ready === 1 ? "" : "s"} ready. ` +
    `${open} open task${open === 1 ? "" : "s"}, ${running} running.`
  );
}

// Compose the full readiness report from the live control-plane reads. Order is
// the natural guided journey: brain → real-work adapter → crew → plugins/tools →
// try Prime (the first useful turn) → (approvals / paused work / stuck work when
// present). When `state` is null the control plane was not reachable; the caller
// renders its own honest error and a loading report here.
export function buildReadiness(inputs: ReadinessInputs): ReadinessReport {
  const { state, ai, adapters, plugins, tools, continuation = null, failed = {} } = inputs;

  const items: ReadinessItem[] = [];

  // A failed primary state read is surfaced explicitly (the counts that drive
  // crew/approvals/first-action are unknown) instead of leaving the guide stuck
  // on the indefinite "Checking readiness…" text.
  if (state === null && failed.state) items.push(stateUnavailableItem());

  items.push(
    brainItem(ai, adapters),
    adapterWorkItem(adapters, !!failed.adapters),
    crewItem(state),
    pluginToolItem(plugins, tools, { plugins: failed.plugins, tools: failed.tools }),
  );
  // The guided first turn — only once the brain that would answer it is connected.
  const tryPrime = tryPrimeItem(state, ai, adapters);
  if (tryPrime) items.push(tryPrime);
  // Things that need the operator to act, in escalating order of urgency.
  const approvals = approvalsItem(state);
  if (approvals) items.push(approvals);
  const paused = continuationItem(continuation);
  if (paused) items.push(paused);
  const stuck = attentionWorkItem(state);
  if (stuck) items.push(stuck);

  const blockers = items.filter((i) => i.status === "todo");
  const attention = items.filter((i) => i.status === "warn");

  // A read genuinely failed (null AND flagged) → the report is degraded: some
  // rows are placeholders, so it must not present a green "operational" summary.
  const degraded =
    (state === null && !!failed.state) ||
    (adapters === null && !!failed.adapters) ||
    (plugins === null && !!failed.plugins) ||
    (tools === null && !!failed.tools);

  return {
    items,
    blockers,
    attention,
    ready: blockers.length === 0 && !degraded,
    degraded,
    firstAction: deriveFirstAction(state, ai, adapters, continuation),
    summary: operationalSummary(inputs),
  };
}
