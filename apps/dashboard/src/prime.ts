import type { ReluxPrimeProposal, ReluxPrimeProposalStep, ReluxPrimeTaskSlots, ReluxReplyPolish } from "./api";

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
