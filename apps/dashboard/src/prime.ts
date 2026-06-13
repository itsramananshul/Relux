import type { ReluxPendingClarification, ReluxPrimeAction, ReluxPrimeContextRead, ReluxPrimeProposal, ReluxPrimeProposalStep, ReluxPrimeTaskSlots, ReluxPrimeTaskUpdate, ReluxReplyPolish } from "./api";

// A typed view of the GitHub plugin-import action Prime proposes for an
// "install owner/repo as a plugin" / "import https://github.com/… as plugin" turn
// (the kernel `PrimeAction::InstallPluginFromGithub`). `repo_url` is the canonical,
// credential-free clone URL; `plugin_id` is the PROPOSED local id (finalized by the
// installer). Pure + defensive so the chat card never trusts an unshaped action.
export interface GithubPluginInstallAction {
  repoUrl: string;
  pluginId: string;
}

// Extract the GitHub plugin-import descriptor from a Prime action, or null when the
// action is absent / a different type / missing its repo URL. The action shape is
// `{ type, [k]: unknown }`, so every field is validated before use.
export function githubPluginInstallAction(
  action: ReluxPrimeAction | null | undefined,
): GithubPluginInstallAction | null {
  if (!action || action.type !== "install_plugin_from_github") return null;
  const repoUrl = typeof action.repo_url === "string" ? action.repo_url.trim() : "";
  const pluginId = typeof action.plugin_id === "string" ? action.plugin_id.trim() : "";
  if (!repoUrl) return null;
  return { repoUrl, pluginId };
}

// Prime's chat-surface copy (RELUX_MASTER_PLAN §11.1; `docs/prime-processing-audit.md`
// "Hermes-first general agent"). Prime is presented as a GENERAL local AI agent —
// a chat companion that can ALSO drive the Relux control plane — not a company /
// work-board manager. The intro, hint, placeholder, and example chips lead with
// normal conversation; the work/crew/plugin abilities are secondary and optional.
// Kept here (a pure .ts module) so they are unit-testable without rendering the page.

// The opening line shown above the conversation. General-agent framing; it does
// NOT lead with the board/queue/crew or "what do you want to set up".
export const PRIME_GREETING =
  "I'm Prime, your local AI agent. Talk to me like you would any assistant — ask a question, " +
  "think something through, or just chat. When you want work done, I can also drive your Relux " +
  "control plane (tasks, runs, agents, plugins) and I always ask before anything risky.";

// The honest one-line contract under the header. Conversation is the default; work
// happens only on an explicit ask, via the buttons under a reply.
export const PRIME_HINT =
  "Chat freely — ask anything, brainstorm, or vent; Prime won't create or run anything from " +
  "casual conversation. When you actually want work done, just ask, or use the buttons under a " +
  "reply to turn an idea into a task or start a run.";

// The input placeholder. A general prompt, not a work-board command.
export const PRIME_PLACEHOLDER = "Message Prime — ask anything, or tell it what to do";

// Discoverable example chips. General-chat prompts come FIRST; the control-plane
// examples are kept but secondary, so Prime never reads as work-board-only.
export const PRIME_SUGGESTIONS = [
  "what can you do?",
  "help me think through an idea",
  "what is going on?",
  "what tools can you use?",
  "create a task to summarize the README",
  "orchestrate research the options, build a prototype, and write the docs",
  "start it",
  "why did it fail?",
];

// Pure helpers for rendering Prime's reviewable plan proposal as a card
// (RELUX_MASTER_PLAN §10 planning layer, §11.1 "Prime Chat"). The proposal is a
// PREVIEW: these functions only describe it. Nothing here commits work — the card
// surfaces Prime's existing "Create these tasks" suggestion as the lone commit
// path, and these helpers never fabricate a step or agent the proposal did not
// carry (§17.1: the UI renders only what Prime returned).

// "1 step" / "3 steps", "1 agent" / "2 agents" — a count with a correctly
// pluralized noun, so the summary line reads naturally for any size.
export function countNoun(n: number, noun: string): string {
  return `${n} ${noun}${n === 1 ? "" : "s"}`;
}

// The one-line summary under the proposal title. A genuine multi-step plan reads
// as "N steps across M agents"; a goal that does not split is steered to the
// one-task path honestly, never fanned into a storm (§10.5).
export function proposalSummary(p: ReluxPrimeProposal): string {
  if (!p.multi_step) {
    return "Reads as a single task — not a multi-step plan.";
  }
  return `${countNoun(p.steps.length, "step")} across ${countNoun(p.agents.length, "agent")}.`;
}

// Whether the card should render the step list. A single-step proposal carries no
// steps, so the card shows just the goal + the one-task route — no empty table.
export function hasSteps(p: ReluxPrimeProposal): boolean {
  return p.multi_step && p.steps.length > 0;
}

// The title to DISPLAY for a step: the LLM-polished wording when one is present
// for this exact step index, otherwise the authoritative deterministic title. The
// polished string is presentation-only — it never changes which step this is, its
// order, or the agent it lands on (§10 planning layer, §17.1). The kernel only
// emits a polished title keyed to a real step index, so this is a safe override.
export function stepDisplayTitle(p: ReluxPrimeProposal, step: ReluxPrimeProposalStep): string {
  const polished = p.polish?.step_titles?.find((t) => t.index === step.index);
  return polished?.title?.trim() ? polished.title : step.title;
}

// The summary line to display: the polished one-line summary when the brain
// supplied one, otherwise the deterministic "N steps across M agents" line. Both
// are presentation only; nothing about the plan changes either way.
export function proposalDisplaySummary(p: ReluxPrimeProposal): string {
  const polished = p.polish?.summary?.trim();
  return polished ? polished : proposalSummary(p);
}

// The provenance to DISPLAY for an AI-refined plan: which brain refined the wording,
// so the operator sees the source at a glance rather than only on hover. The kernel
// stamps `polish.model` through the same `validate_polish` chokepoint for every
// brain — the OpenRouter model id (e.g. "anthropic/claude-3.5-haiku") on the HTTP
// path, or the local CLI brain's label ("Claude CLI" / "Codex CLI") on the adapter
// path. Returns null when there is no polish overlay (nothing to attribute), and a
// generic "AI brain" when an overlay carries no recorded source (older kernels that
// predate the `model` stamp). Presentation only — provenance never alters the plan
// (§10 planning layer, §11.1, §17.1).
export function polishProvenance(p: ReluxPrimeProposal): string | null {
  if (!p.polish) return null;
  const model = p.polish.model?.trim();
  return model ? model : "AI brain";
}

// Honest provenance for HOW Prime classified the turn's INTENT. Returns a short
// label only when a configured brain genuinely decided the intent
// (`intent_source === "brain"`) — i.e. the brain understood a request keyword
// rules would have missed. Deterministic turns (no brain, a low-confidence or
// safety-gate-vetoed proposal) return null, so the card shows nothing extra and
// never overclaims a brain decision. The kernel only stamps "brain" when the
// fail-closed gate accepted the proposal (§10.1, §17.1).
export function intentProvenance(source: string | undefined): string | null {
  return source === "brain" ? "brain-classified" : null;
}

// Honest provenance for the brain-assisted task SLOTS that shaped a created task.
// The kernel attaches `slots` ONLY when a configured brain genuinely sharpened the
// task (normalized title, details, an existing-agent assignee, or a priority) and
// every field passed validation; the server stamps `source` with the model id / CLI
// brain label. Returns that label (falling back to a neutral "AI brain" when an
// older kernel left `source` unset), or null when the brain did not assist — so the
// chip never overclaims a brain decision (§10.1, §10.2, §17.1).
export function slotProvenance(slots: ReluxPrimeTaskSlots | undefined): string | null {
  if (!slots) return null;
  return brainSourceLabel(slots.source);
}

// The shared provenance-label rule for every brain-assisted slot card (task, agent,
// admin): return the stamped model id / CLI brain label, falling back to a neutral
// "AI brain" when an older kernel left `source` unset. Callers render this ONLY when
// the kernel attached a slot object — and the kernel only attaches one when the
// fail-closed validators accepted the slots — so the label always reflects a genuine,
// validated brain contribution (§10.1, §10.2, §17.1).
export function brainSourceLabel(source: string | undefined): string {
  const trimmed = source?.trim();
  return trimmed ? trimmed : "AI brain";
}

// A one-line summary of what a by-id task update changed, e.g.
// "priority → 8, status → blocked". The kernel attaches `update` ONLY on a successful
// TaskUpdate turn and already validated every change, so this just renders the applied
// rows — it never fabricates a field. Returns "" for an (impossible) empty change set.
export function updateChangeSummary(update: ReluxPrimeTaskUpdate | undefined): string {
  if (!update || update.changes.length === 0) return "";
  return update.changes.map((c) => `${c.field} → ${c.value}`).join(", ");
}

// The brain-provenance chip label for a by-id update, present ONLY when a configured
// brain resolved the change the deterministic extractors missed (the kernel stamped
// `source`). Returns null for a deterministically-parsed update (no chip), so the chip
// only ever appears on a genuine, validated brain contribution.
export function updateProvenance(update: ReluxPrimeTaskUpdate | undefined): string | null {
  if (!update || !update.source) return null;
  return brainSourceLabel(update.source);
}

// Honest provenance for a brain-polished clarify / brainstorm REPLY. The server attaches
// `reply_polish` ONLY when a configured brain re-worded the turn through the validated
// wording path (one schema-checked question / short summary — never a free-form lecture
// or an action claim, both rejected server-side). Returns a short human label
// ("brain-worded question · <source>" for a clarification, "brain-worded reply · <source>"
// for a brainstorm), or null when the wording was the deterministic template — so the chip
// only ever appears when the brain genuinely shaped the wording (§10.5, §17.1). The turn
// itself stays action-free; this is presentation/provenance only.
export function replyPolishLabel(rp: ReluxReplyPolish | undefined): string | null {
  if (!rp) return null;
  const noun = rp.kind === "clarification" ? "question" : "reply";
  return `brain-worded ${noun} · ${brainSourceLabel(rp.source)}`;
}

// The label for the small "one brain decision" chip. The server attaches `decision_source`
// ONLY when a SINGLE unified brain call carried more than one proposal this turn (intent +
// slots + wording answered together), so the chip honestly names the one decision behind the
// per-section chips. Returns null when the turn used the prior serial calls or no brain, so
// the chip never overclaims a unified decision. Provenance only; the turn's authority is
// unchanged.
export function decisionSourceLabel(source: string | undefined): string | null {
  const trimmed = source?.trim();
  return trimmed ? `one brain decision · ${trimmed}` : null;
}

// The label for the small governed WRITE-tool provenance chip. The server attaches
// `requested_tool` ONLY when the brain requested a write-capable tool that genuinely drove
// this turn (the turn is actionful and its intent matches the tool), so the chip honestly
// names the governed tool behind a real action/approval — a write tool the fail-closed gate
// vetoed attributes nothing. Returns null otherwise. Provenance only: the mutation still
// flowed through the unchanged decide → execute / approval path; the brain wrote nothing.
export function requestedToolLabel(tool: string | undefined): string | null {
  const trimmed = tool?.trim();
  return trimmed ? `requested tool: ${trimmed}` : null;
}

// The label for the small POST-EXECUTION (after-action) wording chip. The server attaches
// `after_action_source` ONLY when a configured brain re-worded an ACTIONFUL turn's confirmation
// AFTER the kernel already executed (or proposed) the action — grounded in a sanitized result
// envelope and validated against it (no claim of unexecuted work, no invented id, no
// "installed"/"granted" on a still-pending proposal). Returns a short "after-action wording ·
// <source>" label, or null when the reply stayed the grounded deterministic one (no brain / any
// failure). Provenance only: the action already ran through the unchanged decide → execute /
// approval path; the brain changed no state, only the confirmation wording.
export function afterActionLabel(source: string | undefined): string | null {
  const trimmed = source?.trim();
  return trimmed ? `after-action wording · ${brainSourceLabel(trimmed)}` : null;
}

// Compact provenance for the READ-ONLY context tools Prime consulted before answering
// this turn (the governed read-only tool loop). Returns a short "used: get_task,
// list_agents" label naming the DISTINCT tools in look order, bounded so a long loop
// never floods the chip (the overflow collapses into "+N more"). Returns null when no
// tool was consulted, so the chip only ever appears on a turn that genuinely inspected
// live state. Provenance only — every read was a fabricate-nothing inspection that
// changed nothing (§10.1, §17.1).
const MAX_TOOLS_IN_LABEL = 4;
export function contextReadsUsedLabel(reads: ReluxPrimeContextRead[] | undefined): string | null {
  if (!reads || reads.length === 0) return null;
  const tools: string[] = [];
  for (const r of reads) {
    const t = r.tool?.trim();
    if (t && !tools.includes(t)) tools.push(t);
  }
  if (tools.length === 0) return null;
  const shown = tools.slice(0, MAX_TOOLS_IN_LABEL);
  const extra = tools.length - shown.length;
  return `used: ${shown.join(", ")}${extra > 0 ? `, +${extra} more` : ""}`;
}

// Whether ANY consulted read was an honest MISS (`ok === false`) — e.g. a task id that
// did not exist or an empty result. The chip uses this for a subtle ok/partial indicator
// so the operator can see at a glance that not every lookup found what Prime asked for.
// Prime never fabricates a record, so a miss is reported, never hidden (§17.1).
export function contextReadsHadMiss(reads: ReluxPrimeContextRead[] | undefined): boolean {
  return !!reads && reads.some((r) => r.ok === false);
}

// A bounded one-line detail string for a single context read, for the expandable detail
// list. The summary is already short and server-clamped; we clamp again defensively so
// the UI never renders an unbounded blob and never dumps raw JSON (§17.1). The ok/miss
// status is the caller's to render as an icon — this returns only the text, with an
// honest fallback when a read carried no summary.
const MAX_DETAIL_CHARS = 160;
export function contextReadDetail(read: ReluxPrimeContextRead): string {
  const summary = (read.summary ?? "").trim();
  const clamped = summary.length > MAX_DETAIL_CHARS ? summary.slice(0, MAX_DETAIL_CHARS - 1) + "…" : summary;
  return clamped || (read.ok ? "(no detail)" : "(not found)");
}

// The reads to SHOW in the expandable detail, bounded so even a pathological loop never
// floods the card. Returns the first `max` reads plus the count hidden, for an honest
// "+N more" note. The loop is already server-bounded (MAX_TOOL_ROUNDS), so this is a
// defensive second cap on the client.
export const MAX_CONTEXT_READS_SHOWN = 8;
export function boundedContextReads(
  reads: ReluxPrimeContextRead[] | undefined,
  max: number = MAX_CONTEXT_READS_SHOWN,
): { shown: ReluxPrimeContextRead[]; hidden: number } {
  if (!reads || reads.length === 0) return { shown: [], hidden: 0 };
  const shown = reads.slice(0, max);
  return { shown, hidden: reads.length - shown.length };
}

// The label for the small "waiting for: …" chip shown while Prime is still expecting an
// answer to a clarifying question (multi-turn clarify memory). The kernel attaches
// `pending_clarification` ONLY when an actionable request is awaiting a missing field, and
// already named what is needed ("task id" / "agent" / "task description"). Returns
// "waiting for: <needs>", or null when nothing is pending — so the chip only ever appears
// when the NEXT message will actually be read as the answer. The next message continues the
// original request through the same grounded pipeline; this is just a context hint.
export function pendingClarificationLabel(pc: ReluxPendingClarification | undefined): string | null {
  if (!pc) return null;
  const needs = pc.needs?.trim();
  return needs ? `waiting for: ${needs}` : "waiting for your answer";
}

// Render a tool's output for the chat in a CHAT-NATURAL, bounded way — used by both
// the ran-tool result (a turn's `tool_output`) and the post-approval result inside
// the approval card. The kernel already returns a SHAPED, secret-redacted result and
// never the raw JSON-RPC envelope (`docs/mcp.md` "Invocation"); this just presents it
// so the operator is never left staring at wrapper braces:
//   - a plain string  -> shown as-is;
//   - the shaped `{ result: <text>, structuredContent?: … }` envelope (the Hermes
//     `mcp_tool.py` shape) -> the human `result` text is surfaced directly, with the
//     machine `structuredContent` appended as compact JSON only when present;
//   - anything else -> pretty-printed JSON (a plain plugin tool's structured output).
// The result is clamped so a pathological tool can never flood the chat. Returns ""
// for an empty/absent output (the caller then renders no result block). It fabricates
// nothing — it only reshapes what the turn already carried.
const MAX_TOOL_OUTPUT_CHARS = 4000;
export function formatToolOutput(output: unknown): string {
  if (output === undefined || output === null) return "";
  let text: string;
  if (typeof output === "string") {
    text = output;
  } else if (typeof output === "object") {
    const o = output as Record<string, unknown>;
    if (typeof o.result === "string") {
      text = o.result;
      if (o.structuredContent !== undefined && o.structuredContent !== null) {
        try {
          text += `\n\n${JSON.stringify(o.structuredContent, null, 2)}`;
        } catch {
          /* a non-serializable structuredContent is simply omitted */
        }
      }
    } else {
      try {
        text = JSON.stringify(output, null, 2);
      } catch {
        text = String(output);
      }
    }
  } else {
    text = String(output);
  }
  text = text.trimEnd();
  if (text.length > MAX_TOOL_OUTPUT_CHARS) {
    text = `${text.slice(0, MAX_TOOL_OUTPUT_CHARS - 1)}…`;
  }
  return text;
}
