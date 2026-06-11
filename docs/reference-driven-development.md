# Reference-driven development (BINDING)

> Companion rule to `CLAUDE.md` and the design docs. This governs **how** Prime,
> plugins, agents/crew, orchestration, adapter execution, approvals, and
> task/workflow behavior get built — not just **what** the design docs specify.

Relux's whole reason to exist is to be *all three at once*: the secure mesh (its
own), the company (from Paperclip), staffed by self-improving employees (from
Hermes) — see `docs/hermes-vs-paperclip-vs-relix.md`. We have **complete local
clones** of the reference systems under `reference/` (gitignored, never tracked):

- `reference/hermes-agent-main/` — Hermes, the conversational self-improving agent.
- `reference/openclaw-main/` — the system the docs codename **Paperclip**: the
  TypeScript coding-agent / control-plane with the tool/approval/process model.
- `reference/open-webui-main/` — a chat UI reference.

The user's standing complaint is the thing this rule exists to kill: **Relux keeps
feeling like brittle hard-coded keyword rules instead of a real intelligent
operator like Hermes / Paperclip / Codex / Claude.** That happens when we build
from vibes or from two hard-coded examples instead of from how the reference
systems actually solve the problem.

## The hard rule

**Before changing Prime, plugins, agents/crew, orchestration, adapter execution,
approvals, or task/workflow behavior, the implementer MUST first read the
corresponding Hermes and Paperclip (openclaw) code paths.** Then, in the change's
write-up (PR/commit body and, for a substantive slice, a "Reference read" block
like the one below), record:

1. **Which files were read** — exact paths under `reference/…`, with the function
   or class names that matter.
2. **The exact logic / pattern learned** — the concrete mechanism, not "it's
   conversational." E.g. "the model's chosen tool is validated against a name
   allowlist before execution, and an off-list name is fed back for
   self-correction rather than crashing."
3. **How Relux maps / adapts that pattern** — the specific Relux file and the
   adaptation (including what we deliberately do *differently* and why).

Additional standing requirements:

- **No more feature work justified by vague vibes or two hard-coded examples.** If
  the reference systems do not inform the design, the work is not ready.
- **Relux Prime must be an intelligent chat operator with abilities**, not a
  keyword router (`docs/RELUX_MASTER_PLAN.md` §10.1, §17.1):
  - normal chat stays chat,
  - action requests act,
  - ambiguous intent asks,
  - plans are proposed when useful,
  - risky work needs approval.
- **Keyword rules may exist only as fallback safety rails, never as the primary
  brain.** When a real brain is configured it decides; the deterministic rules are
  the floor we fall back to when no brain is available or the brain fails — and the
  fail-closed safety gate the brain can never override.

## What "read first" buys us (the safety shape)

The reference systems are not just inspirational; they encode *how to let a model
decide without letting it do damage*. Two patterns we adopt everywhere:

- **Validate the model's choice against an allowlist/schema before acting**
  (Hermes). The model proposes; a deterministic check accepts only known-good
  shapes; anything else is rejected and the system falls back or self-corrects.
- **One fail-closed gate, work-creation as an explicit gated capability**
  (Paperclip/openclaw). A single classifier decides auto-approve vs. gate
  (unknown ⇒ gated), and minting work is one explicit capability — never inferred
  from casual chat.

---

## Reference read — Prime brain-mediated intent (this slice)

The first application of this rule: moving Prime's intent classification off the
brittle keyword cascade and onto a brain-mediated decision stage, safely.

### Hermes — files read

- `reference/hermes-agent-main/agent/conversation_loop.py`
  - `run_conversation(...)` — the one-turn loop: append the user message, call the
    model, and if it returned `tool_calls`, execute and loop; otherwise set the
    final reply. **Intent is the model's own structured choice (emit tool_calls vs.
    plain text), not a keyword pre-classifier.**
  - The tool-validation block (~L3116–3162): **the model's chosen tool name is
    validated against `agent.valid_tool_names` BEFORE execution**; an off-list name
    is *repaired* (fuzzy) or fed back as a `role:tool` "Tool X does not exist.
    Available: …" message for self-correction (up to 3 rounds) — never executed.
  - The empty/junk-output fallback (~L3466–3480): reuse the last real content
    instead of looping or blanking — a **deterministic fallback** when the model
    misbehaves.
- `reference/hermes-agent-main/agent/prompt_builder.py` / `agent/system_prompt.py`
  - `TOOL_USE_ENFORCEMENT_GUIDANCE`, the `<act_dont_ask>` / `<missing_context>`
    blocks — **chat-vs-act-vs-clarify is steered by conditional system-prompt
    instructions tied to which tools are loaded**, letting the model emit a
    structured choice. This is *why* Hermes feels conversational rather than rule-bound.
- `reference/hermes-agent-main/agent/error_classifier.py`
  - `classify_api_error(...)` → a priority-ordered classifier returning action
    hints (`retryable` / `should_fallback` / …). Pattern noted for future adapter
    error handling; not yet adopted here.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/tool-mutation.ts`
  - `isMutatingToolCall(toolName, args)` — **a single fail-closed classifier**:
    maps tool + `action` to read-only vs. mutating, defaulting an unknown action to
    *mutating*. The chokepoint that decides auto-approve vs. gate.
- `reference/openclaw-main/src/agents/tool-policy.ts`
  - `applyOwnerOnlyToolPolicy(...)` / `wrapOwnerOnlyToolExecution(...)` — **work /
    control-plane capabilities (spawn, cron, gateway) are one explicit, gated
    capability**, replaced with a hard refusal for non-owners. Work is never
    *inferred* from chat.
- `reference/openclaw-main/src/agents/cli-output.ts`
  - `parseCliOutput` / `extractBalancedJsonFragments` — **pull JSON out of a noisy
    CLI reply with a balanced-brace scan, and surface only the parsed `.text`** —
    never the raw stdout/envelope.
- `reference/openclaw-main/src/acp/approval-classifier.ts`
  - `classifyAcpToolApproval(...)` — resolves the tool name from multiple sources
    and cross-checks them before assigning an approval class; only cwd-scoped reads
    auto-approve. Pattern noted for Relux's Claim/approval provenance.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| Hermes: model's choice validated against a **name allowlist** before acting | `crates/relux-kernel/src/prime_intent.rs` `parse_intent_proposal` accepts an intent label only if it round-trips through `PrimeIntent`'s own deserializer — an off-allowlist label is rejected and the turn falls back to the deterministic classifier. |
| Hermes: **prompt-steered** chat-vs-act-vs-clarify | `build_intent_prompt` lists the allowed labels and the conversational-safety rules (musing/questions stay chat; only an explicit instruction is work; ambiguous ⇒ ask) and demands JSON-only output. |
| Paperclip: **one fail-closed gate**, work as an explicit gated capability | `reconcile_intent` is the single gate: on guarded chat (ideation/questions w/o an explicit command) the brain may **never** be promoted to a work intent, a low-confidence proposal keeps the deterministic intent, and a `create_and_run` without explicit run language is downgraded to `create` (no silent auto-run). |
| Paperclip: **balanced-JSON extraction, surface only parsed text** | `extract_json_object` lifts the first balanced `{...}`; the CLI path runs `parse_adapter_result` FIRST so the raw `--output-format json` envelope never reaches the classifier or the UI (`crates/relux-kernel/src/server.rs` `parse_cli_intent`). |
| Hermes: **deterministic fallback** on bad model output | The brain is strictly additive — no key, disabled, timeout, error envelope, off-allowlist label, or low confidence all fall back to `crate::prime::classify_intent`. |

**What we deliberately do differently:** the brain decides **intent only**. Unlike
Hermes (where the model also chooses and runs tools) Relux still derives every slot
(task title, agent, goal) deterministically from the message and executes every
durable change through `decide` → `prime_execute`. The model can sharpen a misread
intent; it authors no slots and runs no action. This keeps the master-plan safety
contract (`crates/relux-kernel/src/ai.rs`) intact while lifting the "keyword-rules"
feel the user complained about.
