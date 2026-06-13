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

For the durable, mechanism-level, 12-dimension map of how the three reference systems
work and where Relux is implemented / partial / missing (with priorities and concrete
slices), see [`HERMES_OPENCLAW_DEEP_AUDIT.md`](HERMES_OPENCLAW_DEEP_AUDIT.md). This
per-slice ledger records the individual reference reads; that audit is the map.

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

---

## Reference read — Prime brain-assisted task-slot extraction (this slice)

The next brittle part moved off keyword string-slicing: the *slots* of a created
task. Even with brain-mediated intent, the title was still
`crate::prime::task_title` stripping a fixed lead-in list and taking the remainder
verbatim — no normalization, no details, no assignee, no priority. This slice lets
a configured brain *propose* the slots and validates them hard before any task is
created. (Orchestration/plan slots are out of scope here: `plan_orchestration`
already owns steps safely and already has the advisory polish overlay — see the
audit note — so the new validated-slot layer targets task creation, the part still
driven by raw slicing.)

### Hermes — files read

- `reference/hermes-agent-main/agent/conversation_loop.py` (~L3166-3251) — the
  tool-call **argument** path: a string arg is `json.loads`-parsed; a truncated
  object returns partial; a malformed object retries up to 3×, then a `role:tool`
  error result is injected for self-correction. **Pattern: parse the model's
  structured arguments, and on bad JSON fall back / self-correct rather than
  executing junk.**
- `reference/hermes-agent-main/agent/message_sanitization.py` —
  `_repair_tool_call_arguments` (L185-279, a multi-pass JSON repair ending in `{}`),
  `_escape_invalid_chars_in_json_strings` (L143-182, replace literal `0x00-0x1F`
  inside strings), `_sanitize_surrogates` (L31-39), and the tool-error clamp to
  2000 chars (`_sanitize_tool_error`, L515-528). **Pattern: sanitize control chars /
  surrogates and CLAMP length on every model-produced string.**
- `reference/hermes-agent-main/model_tools.py` — `coerce_tool_args` (L535-616) and
  `_coerce_number` / `_coerce_boolean` / `_coerce_json` (L672-728): each argument is
  **coerced to its registered schema type** before dispatch; a value that will not
  coerce is left/dropped, not fatal. **Pattern: coerce-to-schema, tolerate the
  rest.**
- `reference/hermes-agent-main/tools/schema_sanitizer.py` (L40-93) — strip schema
  constructs strict backends reject and guarantee the top-level object shape. Noted;
  it informs our strict field discipline (we reject unsupported fields outright).

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/tools/update-plan-tool.ts` — `readPlanSteps`
  (L39-74) validates a structured plan payload field-by-field, checks `status`
  against the `PLAN_STEP_STATUSES` **allowlist** (L9), and clamps (max one
  `in_progress`). **Pattern: validate a structured payload against an explicit
  schema + status allowlist.**
- `reference/openclaw-main/src/agents/tools/sessions-spawn-tool.ts` —
  `UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS` (L46-55, rejected at L277-284),
  `readStringParam(..., { required: true })`, `maxItems`, the numeric clamp
  `Math.max(0, Math.floor(...))` (L355), and the default fallback
  `cleanup === "keep" | "delete" ? … : "keep"` (L302). **Pattern: reject unsupported
  keys, require/trim strings, clamp ranges, default the rest.**
- `reference/openclaw-main/src/agents/tools/common.ts` — `readStringParam` (L91-122)
  and `ToolInputError` (L57-64): typed extraction that *throws* on bad input rather
  than coercing silently.
- `reference/openclaw-main/src/shared/balanced-json.ts` — `extractBalancedJsonPrefix`
  (L21-69): lift the first balanced `{...}` out of a noisy reply.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| Hermes: **coerce model args to the schema type** (`coerce_tool_args` / `_coerce_number`) | `crates/relux-kernel/src/prime_slots.rs` `coerce_priority` accepts a number OR numeric string → rounds and clamps to `[1,9]`; a non-numeric priority is dropped (only the bad field), never fatal. |
| Hermes: **sanitize control chars + clamp length** on model strings | `sanitize_line` / `sanitize_block` strip control chars, collapse whitespace, and clamp (title forced single-line at 120 chars, details at 600); `sanitize_assignee` keeps only `[a-z0-9-_]`. |
| openclaw: **status allowlist + reject unsupported keys** (`readPlanSteps`, `UNSUPPORTED_*_PARAM_KEYS`) | `parse_task_slots` accepts ONLY `ALLOWED_KEYS` (`title`/`details`/`assignee`/`priority`/`confidence`/`rationale`); ANY other key (a smuggled `run_tool`/`tags`/`action`) fails the whole proposal closed → deterministic slots. |
| openclaw: **required string throws** (`readStringParam` required) | an empty/missing `title` fails the proposal (the create keeps the deterministic title). |
| openclaw: **validate a target against what exists** (cwd-scope / existing ids) | `reconcile_task_slots` honors an `assignee` ONLY when it matches an EXISTING agent in `summary.all_agent_ids`; an unknown id is dropped (the brain can never invent an assignee or smuggle a plugin/tool name in as one). |
| openclaw: **balanced-JSON extraction** | reuse `crate::prime_intent::extract_json_object` (now `pub(crate)`), so a brain that wraps the slot JSON in prose/fences still parses, and the raw `--output-format json` envelope is lifted by `parse_adapter_result` FIRST on the CLI path (`server.rs` `parse_cli_task_slots`). |

**What we deliberately do differently:** the brain *proposes* slots; it executes
nothing. Slots are computed ONLY when the (already brain-reconciled, fail-closed-
gated) intent is a create intent **and** the deterministic path already produced a
real create — so this layer *sharpens* a create, it never mints work from nothing
(casual chat/ideation still cannot reach it). `CreateAndRunTask` may take a
brain title/details/priority but **never** the brain's assignee (the run stays on
Prime, the only agent wired for the required grant). Every durable change still
flows through `decide` → `prime_execute`; the brain authors a *proposal*, the kernel
validates and applies it.

---

## Reference read — Prime brain-assisted agent + admin slots (this slice)

The validated-slot pattern now extends past task creation to the next brittle Prime
paths: **agent creation** (the executable `AgentCreation` → `CreateAgent` `Act`) and
the two risky, approval-gated **admin** subjects — **plugin install** and **permission
grant**. The brittle bits replaced are `crate::prime::derive_agent_name` (named/called
markers + a few hard-coded keywords, else `new-agent`), `derive_plugin_id` (first
`relux-`-prefixed token), and the permission subject slice
(`if message.contains("agent") { derive_agent_name(...) }`).

### Hermes — files read

- `reference/hermes-agent-main/model_tools.py` — `coerce_tool_args` (L535-616) and
  `_coerce_number` / `_coerce_boolean` (L672-728): each tool argument is coerced to its
  registered schema type before dispatch; a non-coercible value is dropped, not fatal.
  Same shape we reuse for the optional slots (a bad adapter/permission field is dropped,
  the rest stands).
- `reference/hermes-agent-main/agent/message_sanitization.py` —
  `_escape_invalid_chars_in_json_strings` (L143-182), the tool-error length clamp
  (`_sanitize_tool_error`, L515-528): sanitize control chars and CLAMP length on every
  model-produced string. Mirrored in the agent/admin slot sanitizers (control chars
  stripped, ids/labels length-clamped).

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/tools/sessions-spawn-tool.ts` — the closest
  analogue to "create a new worker from a conversational request":
  `UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS` (L46-55) rejected at L277-284 **before any
  param is read**; `readStringParam(params, "task", { required: true })` (L285); the
  default-the-rest pattern `params.cleanup === "keep" | "delete" ? … : "keep"` (L302);
  the numeric clamp `Math.max(0, Math.floor(...))` (L355). **Pattern: reject unsupported
  keys up front, require/trim the mandatory string, default/clamp the rest.**
- `reference/openclaw-main/src/agents/tools/common.ts` — `readStringParam` (L91-122) and
  `ToolInputError` (L57-64): typed extraction that *throws* on bad input rather than
  coercing silently.
- `reference/openclaw-main/src/acp/approval-classifier.ts` — the canonical
  *approval-subject* resolution: `resolveToolNameForPermission` (L73-103) pulls the
  subject from multiple sources and **cross-checks them**, `normalizeToolName` (L57-63)
  lowercases + length-bounds + accepts only a strict `^[a-z0-9._-]+$` shape (else
  `undefined`), and `EXEC_CAPABLE_TOOL_IDS` / `CONTROL_PLANE_TOOL_IDS` (L15-23) are
  explicit allowlists that force a NON-auto-approve class. **Pattern: normalize the
  subject to a strict id shape, check its kind against an allowlist, and never
  auto-approve a control-plane subject.**
- `reference/openclaw-main/src/agents/tools/subagents-tool.ts` — `resolveControlled
  SubagentTarget` only acts on a target that resolves to an EXISTING run (L104-115,
  L146-157); an unknown target is an error, never invented. Mirrors honoring a
  permission subject / agent adapter only when it exists.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| openclaw: **reject unsupported keys, require the mandatory string, default the rest** (`sessions-spawn-tool`) | `prime_agent_slots::parse_agent_slots` / `prime_admin_slots::parse_{plugin_ref,permission_slots}` accept ONLY their allowlist; any other key fails the proposal closed; `name`/`plugin_id` are required; role/adapter/notes/permission default to absent. |
| openclaw: **normalize a subject to a strict id shape** (`normalizeToolName`) | `agent_id_form` / `sanitize_plugin_id` / `sanitize_id` lowercase + reduce to `[a-z0-9-]` + clamp; `sanitize_permission` keeps only the `[a-z0-9:_-]` grammar. |
| openclaw: **check the subject KIND against an allowlist** (`CONTROL_PLANE_TOOL_IDS`) | `prime_admin_slots::SUBJECT_KINDS = ["agent"]`; an off-allowlist `subject_kind` (e.g. a smuggled `"plugin"`) fails the whole permission proposal closed. |
| openclaw: **act only on a subject that EXISTS** (`resolveControlledSubagentTarget`, approval cross-check) | `reconcile_agent_slots` honors an adapter only if it's in the live adapter roster and **rejects a duplicate agent id**; `reconcile_permission_slots` honors a subject only if it names an EXISTING agent (`summary.all_agent_ids`) — the brain can never invent/enable an adapter or grant subject. |
| Hermes: **coerce-or-drop, never fatal** (`coerce_tool_args`) | a bad/unknown adapter or an unvalidated permission subject is dropped (the deterministic value stands), never an error. |
| openclaw: **work/control-plane is one explicit, GATED capability** (`tool-policy`) | the brain can sharpen a plugin-install / permission-grant subject, but the action STAYS a `PrimePlan::Propose` behind a human approval — `sharpen_admin_action` reshapes only the *subject the human reviews*, and the kernel still logs an approval and executes nothing. |

**What we deliberately do differently:** agent creation is the only *executable* extension
(a `CreateAgent` `Act`): the brain may sharpen the name/id/description/adapter, but the id
may never collide with an existing agent and the adapter must already exist. Plugin install
and permission grant are **advisory only** — the brain sharpens the subject, but the action
is unchanged in kind (`Propose` → approval), so a brain slot can NEVER execute a protected
install or grant by itself. Every durable change still flows through `decide` →
`prime_execute` (safe `Act`s) or a human approval (risky `Propose`s); the brain authors a
*proposal*, the kernel validates it.

---

## Reference read — brain-assisted clarification wording + agent persona seed (this slice)

The last keyword surfaces the audit flagged are now behind a brain: the **reflect-and-clarify
wording** (the `Clarify` / `Brainstorming` / single-step `PlanRequest` / `TaskUpdate` replies,
previously fixed templates in `crate::prime`) and the created-agent **starter persona** (today
`create_agent` is always handed `None`). The brain may now *re-word* an already-decided
non-actionful turn, and *propose* a bounded persona — both validated hard before anything is
shown or stored.

### Hermes — files read

- `reference/hermes-agent-main/agent/prompt_builder.py` / `agent/system_prompt.py` — the
  `<missing_context>` / `<act_dont_ask>` blocks steer the model to ask **ONE targeted question**
  when context is missing rather than guessing or lecturing. We fold the same instruction into
  `crates/relux-kernel/src/prime_clarify.rs` `build_clarify_prompt` (Clarify ⇒ "EXACTLY ONE
  concrete question"), **and validate the result structurally** (`parse_clarify` enforces a
  single `?` ending the text) rather than trusting the model to obey — the Hermes "prompt steers,
  but a deterministic check decides" shape.
- `reference/hermes-agent-main/agent/message_sanitization.py` —
  `_escape_invalid_chars_in_json_strings` (L143-182) and the tool-error length clamp
  (`_sanitize_tool_error`, L515-528): sanitize control chars and CLAMP length on every
  model-produced string. Mirrored in `prime_clarify::sanitize_line` / `sanitize_block` and in the
  persona sanitizer (`prime_agent_slots`, control chars stripped + length-bounded).
- `reference/hermes-agent-main/agent/conversation_loop.py` — the empty/junk-output fallback
  (~L3466-3480): reuse the last real content when the model misbehaves. Mirrored: any failure
  (no brain, malformed JSON, non-question, low confidence, echo) falls back to the deterministic
  template wording, so the brain is strictly additive.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/tools/sessions-spawn-tool.ts`
  (`UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS` rejected at L277-284 BEFORE any param is read) +
  `src/agents/tools/common.ts` (`readStringParam` required, `ToolInputError`, L57-122 — a required
  string THROWS on bad input). We mirror it: `parse_clarify` / `parse_agent_slots` accept ONLY
  their allowlist (`text`/`confidence`/`rationale`; agent `persona` added to the allowlist) and
  fail the whole proposal closed on any other key; the mandatory `text`/`name` must be non-empty.
- `reference/openclaw-main/src/agents/cli-output.ts` (`parseCliOutput`) +
  `src/shared/balanced-json.ts` (`extractBalancedJsonPrefix`, L21-69): pull the reply out of a
  noisy CLI envelope and surface only the parsed text. The CLI path runs `parse_adapter_result`
  FIRST, then `prime_intent::extract_json_object`, so the raw `--output-format json` envelope
  never reaches the validator or the chat bubble (`server.rs` `parse_cli_clarify`).

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| Hermes: **prompt steers ONE question, a deterministic check decides** | `prime_clarify::build_clarify_prompt` demands one question; `parse_clarify` **structurally enforces** exactly one `?` ending the text (a multi-question lecture or a statement is rejected → deterministic template stands). |
| Hermes: **sanitize control chars + clamp length** | `prime_clarify::sanitize_line`/`sanitize_block` and the new `persona` sanitizer strip control chars and clamp (clarify 240 chars single-line, brainstorm 600, persona 600). |
| openclaw: **reject unsupported keys, require the mandatory string** | `parse_clarify` allowlist = `text`/`confidence`/`rationale`; `parse_agent_slots` allowlist gains `persona`; any other key fails closed; empty `text`/`name` rejected. |
| openclaw: **balanced-JSON extraction, surface only parsed text** | `parse_cli_clarify` runs `parse_adapter_result` then `extract_json_object`; an error envelope / prose / non-question yields `None` and the deterministic wording stands — no raw envelope leak. |
| openclaw: **never silently truncate a sensitive field** | an **overlong persona fails the whole proposal closed** (`MAX_PERSONA_CHARS`), so a created operative gets a bounded persona or the deterministic none — never a clipped one. |

**What we deliberately do differently:** the brain only **re-words** a turn the deterministic
classifier already decided is a non-actionful `Reply`/`Clarify` — it picks no intent, authors no
slot, and runs nothing (`clarify_polish_kind` returns `None` for every actionful turn, so the
wording path is never near an action). A polished reply that asserts a completed action
(`I created…` / `run started` / …) is rejected wholesale, so the brain can never narrate a state
change that did not happen. The persona is the only *durable* contribution here, and it flows
only through the already-validated `AgentCreation` → `create_agent(persona)` seam; the
deterministic path still creates a personaless agent.

---

## Reference read — multi-turn clarification memory (this slice)

The audit's last "Next recommended slice": a `Clarify` turn asked one good question, but the
next user message did not *carry* the prior question's context — "assign this to the researcher"
→ "which task?" → "task_0001" was reclassified from scratch as a bare `DirectAnswer`, so the
original request was lost and Prime felt keyword-shaped rather than like Hermes/Codex. This slice
stores a small, bounded pending-clarification record when Prime asks, and on the next turn decides
— deterministically — whether the follow-up *resolves* it.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/bash-tools.exec-approval-followup-state.ts`
  - `registerExecApprovalFollowupRuntimeHandoff(...)` (L84-111) stores a small *pending handoff*
    in an in-memory `Map<handoffId, {approvalId, sessionKey, idempotencyKey, expiresAtMs}>`;
    `consumeExecApprovalFollowupRuntimeHandoff(...)` (L113-146) looks it up on a LATER turn, checks
    `expiresAtMs <= nowMs` (TTL) and that the keys match, and **deletes the entry after use**.
    `EXEC_APPROVAL_FOLLOWUP_RUNTIME_HANDOFF_TTL_MS = 5 * 60 * 1000` (L7). **Pattern: one small
    pending record keyed by a session/approval id, with an explicit TTL, consumed-and-cleared on
    the next turn — never a record that lingers to steer an unrelated later message.**
- `reference/openclaw-main/src/agents/bash-tools.exec-approval-followup.ts`
  - `sendExecApprovalFollowup(...)` (L271-351) consumes the registration and runs a NEW turn in the
    same session with `buildExecApprovalFollowupPrompt(resultText)` (the stored context injected
    into the prompt). **Pattern: a resolved pending record is continued by running a fresh,
    fully-validated turn with the stored context injected — not by a privileged shortcut.**

### Hermes — files read

- `reference/hermes-agent-main/agent/conversation_loop.py`
  - `run_conversation(...)` builds `messages = list(conversation_history)` then appends the new user
    message (`messages.append(user_msg)`, ~L330-400). A follow-up turn appends the new message to the
    SAME prior history, so the model answers the earlier question *in context* rather than from
    scratch. **Pattern: a follow-up is interpreted against the prior turn's context, not classified
    blind.** We carry only the single pending question's grounding (one bounded record), which is the
    minimal slice of that idea a deterministic kernel needs — not a full transcript.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| openclaw: **small pending record keyed by session id, explicit TTL, consumed-and-cleared** | `KernelState.pending_clarifications: HashMap<conversation_key, PendingClarification>` (`namespace::actor` key), each with `created_at_secs` / `expires_at_secs` (`CLARIFY_TTL_SECS`); persisted via the `meta` JSON seam like `orchestrations`. `resolve_pending` (`crates/relux-kernel/src/prime_clarify_memory.rs`) returns `Expired` past TTL, and a resolved/changed turn clears the record. Bounded: `MAX_PENDING_CLARIFICATIONS` (oldest evicted). |
| openclaw: **continue a resolved pending record by running a fresh, validated turn with the stored context injected** | a bare answer → `combine(original_message, answer)` (length-bounded), and the combined text re-runs the SAME `classify_intent` → `decide` → `prime_execute` pipeline. No privileged shortcut: a risky resolution is still a `Propose` behind a human approval, and an unknown task/agent still fails closed. |
| Hermes: **a follow-up is interpreted against prior context, not classified blind** | when a pending record exists, the kernel reads the next message *through* it: a bare value (`task_0001`, `researcher`) continues the original request; a standalone command/question (`is_standalone_request`) supersedes it; "never mind" cancels it. |
| Hermes / openclaw: **deterministic fallback always exists** | the whole resolver is pure and deterministic; the brain intent/slot proposals (computed on the raw answer) are dropped on a `Continue`, so the combined classification stands. Brain-assisted *extraction* applies only on the `FreshRequest` path (the self-sufficient answer the server already ran slots on) — advisory, never required. |

**What we deliberately do differently:** the memory only decides *how to read* the follow-up; it
executes nothing itself and grants no authority. The combined message flows through the unchanged
`decide` → `prime_execute` (safe `Act`) or human-approval (`Propose`) path, so a continuation can
never run a protected install/grant by itself. Only the intents whose clarify a follow-up can
actually turn into an action are recorded (`AssignTask` / `TaskCreation` / `CreateAndRunTask`) —
a run-start or task-update clarify is NOT recorded, because no by-id action is wired for them and
we never set up a loop that cannot resolve (no faked capability). The record holds only bounded,
non-secret user text and a deterministic intent label — never a provider envelope or a secret.

---

## Reference read — roster-aware fuzzy assignee resolution (this slice)

The multi-turn memory above carries "assign this to the researcher" → "which task?" →
"task_0001" into one combined message, but the assignee extractor then failed it: the
deterministic `extract_agent_id_from_assignment` takes only the FIRST word after "to", so
"the researcher" became the agent id `the`, which exists on no roster — the canonical
continuation dialogue still dead-ended. This slice resolves a *fuzzy* assignee phrase
against the live agent roster so a natural reference ("the researcher", "research bot",
"research") resolves to the existing agent, while a resolved id can only ever be one that
actually exists.

### Hermes — files read

- `reference/hermes-agent-main/agent/conversation_loop.py` (L3114-3162) — the tool-call
  **name** path: a model-chosen tool name not in `agent.valid_tool_names` is first
  *repaired* (`agent._repair_tool_call(name)`); only a name that still fails the allowlist
  after repair is rejected and fed back for self-correction — **an off-allowlist name is
  normalized/fuzzed against the KNOWN set before it is refused, never executed as-is.**
- `reference/hermes-agent-main/agent/agent_runtime_helpers.py` `repair_tool_call`
  (L1566-1636) — the repair itself: lowercase direct match → separator-normalized match →
  CamelCase→snake → suffix-strip (twice) → finally `difflib.get_close_matches(lowered,
  valid_tool_names, n=1, cutoff=0.7)`, **returning a name only when it is in
  `valid_tool_names`, else `None`.** Pattern: normalize/strip, then match against the
  known set in priority order, and resolve only to a member of that set.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/auto-reply/reply/subagents-utils.ts`
  `resolveSubagentTargetFromRuns` (L44-145) — the canonical fuzzy-target resolver: numeric
  index → session-key → **exact alias → exact label → alias prefix → label prefix → runId
  prefix**, where a tier with exactly one match RESOLVES, a tier with more than one is an
  **ambiguity error** (`ambiguousLabel*`), and no match anywhere is `unknownTarget`. Pattern:
  exact → unique-prefix → ambiguous-is-an-error, and the result is always an existing run.
- `reference/openclaw-main/src/agents/subagent-control.ts` `resolveControlledSubagentTarget`
  (L707-729) — wires that resolver to the live run set with the user-facing error strings, so
  a control action only ever lands on a target that EXISTS.
- `reference/openclaw-main/src/acp/approval-classifier.ts` `normalizeToolName` (L57-63) — a
  subject is lowercased, length-bounded, and accepted only against a strict
  `^[a-z0-9._-]+$` shape (else `undefined`). Pattern: normalize a fuzzy subject to a strict
  id shape before matching.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| Hermes: **normalize/strip, then match the known set** (`repair_tool_call`) | `crates/relux-kernel/src/prime.rs` `resolve_assignee` lowercases the phrase, drops `ASSIGNEE_STOPWORDS` (`the`/`a`/`agent`/`our`/…) and sub-2-char noise, and builds candidates (the hyphen/space-joined phrase + each token) before matching. |
| openclaw: **exact → unique-prefix → substring, ambiguity is an error** (`resolveSubagentTargetFromRuns`) | `resolve_assignee` runs the same priority tiers against `summary.all_agent_ids`: exact (case-insensitive) → prefix → substring; exactly one distinct match → `Resolved`, more than one → `Ambiguous` (the decide arm asks "which one?"), none → `Unresolved`. |
| openclaw: **resolve only to a target that EXISTS** (`resolveControlledSubagentTarget`) | a `Resolved` id is taken verbatim from the roster, so the fuzzy phrase can never invent an assignee; an unknown phrase keeps the existing "Agent with ID '…' does not exist" reply (fail closed). |
| openclaw: **normalize a subject to a strict shape** (`normalizeToolName`) | the new `extract_assignee_phrase` keeps the FULL trailing phrase (task-id token stripped) so a multi-word reference resolves, while `extract_agent_id_from_assignment` is kept ONLY as the "did the user name an agent?" presence signal the clarify branches use. |

**What we deliberately do differently:** this is a deterministic change with NO brain in the
loop — it is the fallback the later brain-assisted assignment slot will reconcile against, and
the safety shape (resolve only to an existing agent, ambiguity asked not guessed) holds whether
or not a brain is configured. The `AssignTask` decide arm still produces a `PrimePlan::Act`
through the unchanged `decide` → `prime_execute` path; only the assignee *resolution* got smarter.

---

## Reference read — by-id run start + a resolvable run-start clarification (this slice)

The multi-turn memory above deliberately skipped a run-start clarify ("start it" → "which
one?" → "task_0001") because no by-id `StartRun` was wired. This slice wires it, so that
clarify becomes resolvable.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/subagent-control.ts` `resolveControlledSubagentTarget`
  (L707-729) + `src/auto-reply/reply/subagents-utils.ts` `resolveSubagentTargetFromRuns`
  (L80-145, the numeric-index/active-window filter at L80-92) — a control action lands on a
  target only when it resolves to an EXISTING entry that is also *active/runnable*; an index
  out of range or an unknown target is an error, never coerced. **Pattern: act only on a
  target that both exists AND is in a runnable state.**
- `reference/openclaw-main/src/agents/bash-tools.exec-approval-followup-state.ts`
  (`consumeExecApprovalFollowupRuntimeHandoff`, L113-146) + `.exec-approval-followup.ts`
  (`sendExecApprovalFollowup`) — the consume-and-continue shape the clarification memory
  already mirrors; recording a run-start clarify is now legitimate because the continuation
  has a real by-id action to resolve into (no faked capability).

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| openclaw: **act only on a target that EXISTS and is runnable** | `crates/relux-kernel/src/prime.rs` `RunStart` arm honors an explicit `extract_task_id` only when it is in `summary.queued` (exists AND ready) → `StartRun` `Act`; existing-but-not-ready → an honest "not ready" `Reply`; unknown → "does not exist" (fail closed). |
| openclaw: **consume-and-continue only when a real action backs it** | `prime_clarify_memory::is_resolvable_clarify_intent` now includes `RunStart` (and `clarify_needs_label(RunStart) = "task id"`), so the multi-ready clarify is remembered and a bare task id continues it; `TaskUpdate` stays unrecorded (still no `UpdateTask` action). |

**What we deliberately do differently:** purely deterministic, no brain — the by-id resolution is
validated against the live `summary.queued`/`all_task_ids`, so a continuation can only start a task
that genuinely exists and is ready. This supersedes the earlier slice's note that a run-start clarify
is never recorded (that was true only while no by-id action existed).

---

## Reference read — brain-assisted continuation resolution (this slice)

The deterministic slices above fixed the *common* assignment/run-start continuations. This
slice adds the brain as a strictly-additive fallback for the cases the extractors still miss
("assign the readme task to the helper" — no `task_` token; a continuation where the original
request and the answer only TOGETHER name both task and agent). When a pending clarification is
continued, the brain may now *propose* the missing `{task_id, agent_id}` from the full context,
validated against the live state before any assignment happens — the deterministic combine stays
the fallback.

### Hermes — files read

- `reference/hermes-agent-main/model_tools.py` `coerce_tool_args` (L535-616) /
  `_coerce_number` / `_coerce_json` (L672-728) — each model-proposed argument is coerced to its
  registered schema type before dispatch; a non-coercible value is dropped, not fatal. Mirrored
  in `crates/relux-kernel/src/prime_assign_slots.rs` `parse_assign_slots` (allowlist, sanitize,
  clamp; a bad field drops, an unsupported field fails closed).
- `reference/hermes-agent-main/agent/conversation_loop.py` (`run_conversation`,
  `messages = list(conversation_history)` then append the new user message; ~L330-400) — a
  follow-up is interpreted against the prior turn's context. We carry the single pending
  question's grounding and dispatch the brain on the COMBINED message (the kernel reclassifies
  the same combined text), so the brain answers the earlier question in context, not blind.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/bash-tools.exec-approval-followup.ts`
  (`sendExecApprovalFollowup` → `buildExecApprovalFollowupPrompt`) — a resolved pending handoff
  is continued by running a FRESH, fully-validated turn with the stored context injected into the
  prompt, not by a privileged shortcut. We mirror it: the server computes the combined message,
  dispatches the slot brain on it, and the kernel re-runs the SAME `decide`/validate pipeline;
  the brain authors a proposal, never an action.
- `reference/openclaw-main/src/agents/bash-tools.exec-approval-followup-state.ts`
  (`consumeExecApprovalFollowupRuntimeHandoff`, L113-146) — a pending record is consumed only
  when it matches and has not expired, then cleared. The kernel's `continuation_preview` is the
  read-only counterpart the server consults to learn the combined message + recorded intent
  BEFORE dispatching the (slow, off-lock) slot brain; the kernel re-decides authoritatively under
  its own lock.
- `reference/openclaw-main/src/auto-reply/reply/subagents-utils.ts`
  `resolveSubagentTargetFromRuns` (L80-145) — resolve a fuzzy reference to an EXISTING target;
  reused via `crate::prime::resolve_assignee` for the `agent_id`, with the `task_id` likewise
  honored only when it is in `summary.all_task_ids`.
- `reference/openclaw-main/src/agents/tools/sessions-spawn-tool.ts`
  (`UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS`) + `src/agents/tools/common.ts` (`readStringParam`)
  + `src/shared/balanced-json.ts` (`extractBalancedJsonPrefix`) — reject unsupported keys, trim
  the strings, lift the JSON from a noisy reply; mirrored in `parse_assign_slots` and the CLI
  no-leak seam `parse_cli_assign_slots`.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| openclaw: **continue a resolved pending record by running a FRESH validated turn with the stored context injected** | the server reads `KernelState::continuation_preview` (combined message + recorded intent), dispatches the slot brain on the COMBINED message, and the kernel re-runs `decide`/validate; the brain authors only a proposal. |
| openclaw: **resolve a reference only to an EXISTING target** (`resolveSubagentTargetFromRuns`) | `prime_assign_slots::reconcile_assign_slots` honors `task_id` only when it is in `summary.all_task_ids` and resolves `agent_id` via `resolve_assignee` (always an existing agent); BOTH must validate or the deterministic clarify stands. |
| openclaw: **a bundle is consumed only when it matches** (TTL/key match) | `BrainSlotProposals.continuation` marks slots computed on the combined message; the kernel keeps the bundle ONLY when `continued == slots.continuation`, so a proposal computed for the wrong message can never shape an action. |
| Hermes: **coerce/sanitize, drop the bad field, fail closed on the unsupported one** | `parse_assign_slots` allowlist (`task_id`/`agent_id`/`confidence`/`rationale`), sanitize + clamp; an unsupported field fails the whole proposal closed. |

**What we deliberately do differently:** unlike the create-slot layer (which only *sharpens* an
action the deterministic path already produced), an assignment slot can PROMOTE an
under-specified `AssignTask` turn into a real `AssignTask` action — but ONLY because assignment is
a safe, in-scope action (no approval, no risk gate; the deterministic path already produces it
freely) and BOTH ids are validated against the live state first. The brain authors no risky action
and can name nothing that is not real; a risky intent still becomes an approval-gated `Propose`,
and any failure (no brain, low confidence, unknown id, mismatched continuation flag) leaves the
deterministic clarify in place.

---

## Reference read — safe by-id task UPDATE (this slice)

`TaskUpdate` was the last resolvable-looking clarify with no action behind it: `decide`
could only ask "which task, what field?" and the multi-turn memory deliberately refused to
record it (no faked capability). This slice wires `PrimeAction::UpdateTask { task_id, patch }`
as a REAL, safe mutating action — a deterministic rail for the simple commands plus a
brain-assisted fallback for the references the extractors miss — validated hard before any
mutation, and makes the `TaskUpdate` clarify resolvable.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/tools/update-plan-tool.ts` — `readPlanSteps` (L39-74):
  validate a structured UPDATE payload field-by-field, check `status` against the
  `PLAN_STEP_STATUSES` **allowlist** (L9), and clamp ("at most one `in_progress`"). The
  canonical "validate an update against a schema + a status allowlist" shape.
- `reference/openclaw-main/src/agents/tool-mutation.ts` — `isMutatingToolCall(toolName, args)`
  (L140-181): a single fail-closed classifier that maps a tool+action to read-only vs.
  **mutating**, defaulting an UNKNOWN action to *mutating*. Informs treating the update as an
  explicit mutating action that is applied only after validation (and never auto-inferred from
  chat).
- `reference/openclaw-main/src/agents/tools/common.ts` — `readStringParam` (L91-122) /
  `ToolInputError` (L57-64): typed extraction that *throws* on bad input rather than coercing
  silently; and `sessions-spawn-tool.ts` `UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS` (rejected
  before any param is read) + the `Math.max(0, Math.floor(...))` clamp.
- `reference/openclaw-main/src/auto-reply/reply/subagents-utils.ts`
  `resolveSubagentTargetFromRuns` — resolve a fuzzy reference only to an EXISTING target,
  reused via `crate::prime::resolve_assignee` for the assignee, with the `task_id` honored
  only when it exists.
- `reference/openclaw-main/src/shared/balanced-json.ts` `extractBalancedJsonPrefix` — lift the
  JSON object out of a noisy reply, reused via `crate::prime_intent::extract_json_object`.

### Hermes — files read

- `reference/hermes-agent-main/model_tools.py` `coerce_tool_args` (L535-616) / `_coerce_number`
  (L672-728) — coerce each model arg to its registered schema type before dispatch; a
  non-coercible value is dropped, not fatal. Mirrored in `parse_update_slots` (priority coerced
  and clamped; a non-settable status DROPPED, not fatal; an unsupported field fails closed).
- `reference/hermes-agent-main/agent/message_sanitization.py` — sanitize control chars and
  CLAMP length on every model-produced string. Mirrored in the update title/details sanitizers.
- `reference/hermes-agent-main/agent/conversation_loop.py` (`run_conversation`,
  `messages = list(conversation_history)` then append the new user message) — a follow-up is
  interpreted against prior context; reused via the existing clarify memory, now that a
  `TaskUpdate` clarify is recordable.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| openclaw: **validate an UPDATE against a schema + a status ALLOWLIST** (`readPlanSteps`, `PLAN_STEP_STATUSES`) | `crates/relux-kernel/src/prime_update_slots.rs`: `parse_update_slots` accepts ONLY `ALLOWED_KEYS` (task_id/title/details/priority/status/assignee/confidence/rationale) — any other key fails closed; `parse_settable_status` honors ONLY the `SETTABLE_STATUSES` allowlist (`blocked`/`cancelled`). |
| openclaw: **unknown action defaults to mutating, applied after validation** (`isMutatingToolCall`) | `UpdateTask` is a real `PrimePlan::Act`; `prime_execute` re-checks task existence, enforces a **terminal-state guard** (a completed/failed/cancelled/expired task is never edited), and applies only allowlisted fields (defense in depth) — so even a stale/forged patch can never edit a finished task or set a machine-driven status. |
| openclaw: **reject unsupported keys, require/trim strings, clamp ranges** (`sessions-spawn-tool`/`common.ts`) | `parse_update_slots` sanitizes/clamps title & details, coerces+clamps priority to `[1,9]`, and rejects any unsupported field; the deterministic rail parses a SIMPLE command ("rename task_0001 to X", "set task_0001 priority to 8", "cancel task_0001") and validates it the same way. |
| openclaw: **resolve a reference only to an EXISTING target** (`resolveSubagentTargetFromRuns`) | the `task_id` is honored only when it is in `summary.all_task_ids`; an `assignee` change resolves through `crate::prime::resolve_assignee` and is ALWAYS an existing agent (ambiguity asked, unknown refused). |
| Hermes: **coerce-or-drop, fail closed on the unsupported** (`coerce_tool_args`) | a brain proposal's bad priority / non-settable status / unknown assignee is dropped; an unsupported key fails the whole proposal; on no/low-confidence/unvalidated proposal the deterministic clarify stands. |

**What we deliberately do differently:** like the assignment slot (and unlike the create slot),
a validated update can PROMOTE an under-specified `TaskUpdate` clarify into a real `UpdateTask`
action — but ONLY because a by-id update is a SAFE, in-scope action (it edits the operator's own
task; it is never risk-gated) and every field is validated against the live state, with a
terminal-state guard the brain can never bypass. The promotion is gated on the deterministic path
having genuinely CLARIFIED (not on an honest "task does not exist" / refused-status `Reply`), so an
explicit-but-wrong reference is never silently "corrected". Prime never decrees a `completed` /
`running` status from chat (those flow through the run lifecycle) — that is honestly refused, never
faked. Status synonyms (cancel→cancelled, block→blocked) and the priority/title/details parsing stay
deterministic string helpers: they are the grounding the brain reconciles against and the fallback
when no brain is live.

---

## Reference read — unified Prime brain decision envelope (this slice)

The slices above each added ONE specialized brain call (intent, then task / agent / admin /
assign / update slots, then clarify wording). They are individually correct, but a single Prime
turn could fire the brain TWO or THREE times in series (intent → slots for the resolved intent →
wording for a clarify). That is slow, costly, and less coherent than how Hermes / Codex / Claude
actually work — ONE model response carries both the answer and the structured actions in a single
turn. This slice adds a UNIFIED decision envelope that carries intent + every applicable slot +
optional wording in ONE provider call, while keeping the deterministic/policy execution authority
and every old specialized parser as the fallback.

### Hermes — files read

- `reference/hermes-agent-main/agent/conversation_loop.py` `run_conversation(...)` — a SINGLE
  model response carries both `content` (the answer) and `tool_calls` (the structured actions) in
  one assistant message (`_m.get("tool_calls")`, ~L630-875), and the tool-validation block
  validates the chosen tool against the NAME ALLOWLIST before acting (~L3116-3162). **Pattern: one
  response carries the answer AND the structured actions; each is validated against an allowlist
  before it is used.** We mirror the one-response shape: `crates/relux-kernel/src/prime_decision.rs`
  `parse_decision` lifts ONE envelope carrying the intent AND the slots AND the wording, and each
  piece round-trips through its existing validator before it can shape anything. We deliberately
  differ in that the Relux brain still executes NOTHING — every durable change flows through the
  deterministic kernel path.
- `reference/hermes-agent-main/model_tools.py` `coerce_tool_args` (L535-616) — each argument is
  coerced to its registered schema type, a non-coercible value dropped, not fatal. Mirrored by the
  per-section reuse: a section whose own validator rejects it is DROPPED (its specialized/
  deterministic fallback applies), not fatal to the whole envelope.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/shared/balanced-json.ts` `extractBalancedJsonPrefix` (L21-69) and
  `src/agents/cli-output.ts` `parseCliOutput` — lift the first balanced `{...}` out of a noisy
  reply and surface only the parsed object, never the raw stdout. We reuse the SAME scanner
  (`crate::prime_intent::extract_json_object`); on the CLI path the server runs
  `parse_adapter_result` FIRST (`server.rs` `parse_cli_decision`), so the raw `--output-format json`
  envelope never reaches the parser or the UI.
- `reference/openclaw-main/src/agents/tools/update-plan-tool.ts` `readPlanSteps` (L39-74) — a
  structured payload is validated FIELD-BY-FIELD and COMPOSITIONALLY (each plan step independently
  against its schema + `PLAN_STEP_STATUSES` allowlist; a bad one is an input error). **Pattern:
  validate a composite payload section-by-section against explicit schemas/allowlists.** Mirrored
  by `parse_decision`'s compositional validation (each known section through its own validator).
- `reference/openclaw-main/src/agents/tools/sessions-spawn-tool.ts`
  (`UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS`, rejected before any param is read) — reject unsupported
  keys outright. Mirrored: `parse_decision` rejects any UNKNOWN top-level key and fails the WHOLE
  envelope closed (the brain may not smuggle an un-modeled authority key past the parser).

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| Hermes: **one response carries the answer AND the structured actions**, each allowlist-validated before use | `prime_decision::parse_decision` lifts ONE envelope: `classification` (intent), `task`/`agent`/`plugin`/`permission`/`assign`/`update` slots, and `wording`. Each section is validated by its EXISTING validator (`parse_intent_proposal`, `parse_task_slots`, …) — no weaker duplicate logic. `ai::decide_prime_via_openrouter` / `server.rs` `decide_prime_via_cli` make the one call. |
| openclaw: **`additionalProperties: false`** (reject unsupported keys) | An UNKNOWN top-level key fails the WHOLE envelope closed; the caller then falls back to the specialized paths. |
| openclaw: **compositional, field-by-field validation** (`readPlanSteps`) | A KNOWN section that fails its own validator is DROPPED (that section falls back to its specialized call / the deterministic rail) while the rest of the envelope stands — documented per-section vs. whole-envelope fail-closed policy. |
| openclaw: **balanced-JSON extraction, surface only parsed text** | reuse `extract_json_object`; the CLI path lifts the reply via `parse_adapter_result` FIRST (`parse_cli_decision`), so the raw envelope never leaks. |
| Hermes/openclaw: **deterministic fallback always exists** | the unified call is strictly additive: ANY failure (no brain, malformed/empty envelope, unknown top-level key, zero usable sections) drops the caller to the prior specialized intent/slot/wording calls and the deterministic rails. |

**What we deliberately do differently:** the envelope changes only HOW the brain is asked (one
call) and HOW its reply is parsed (one allowlisted object) — it changes NOTHING about authority.
The fail-closed intent gate (`reconcile_intent`) still runs at the kernel chokepoint, so guarded
chat can never be promoted to work; every slot is still reconciled against the live state, and the
kernel uses ONLY the sections that match the turn it produces (a `task` proposal on an assign turn
is simply ignored). Risky plugin/permission slots are still advisory-only behind a human approval.
The wording is carried raw and validated LATER against the turn's actual `ClarifyKind` through the
SAME `parse_clarify`/`reconcile_clarify` chokepoint, so a clarify is still forced to one question
and an action-claim is still rejected. The brain authors a *proposal*; the kernel validates and
applies it — exactly as before, now in one round-trip. The remaining brain calls (the free-form
conversational reply via `shape_reply`/`run_cli_brain` for non-clarify chat, and the advisory
multi-step plan-card polish) stay specialized: they are not part of the intent+slots+wording
decision, and folding them in is a future slice.

---

## Reference read — folding the conversational reply + plan-polish into the unified envelope (this slice)

The unified envelope above still left TWO brain calls outside it: the free-form conversational
reply (`shape_reply` / `run_cli_brain`) for a non-clarify chat turn, and the advisory multi-step
plan-card polish (`polish_proposal`). So a plain greeting could still cost a decision call **plus**
a reply call, and a multi-step plan turn a decision call **plus** a reply call **plus** a polish
call — slower and less coherent than how Hermes / Codex answer (ONE response carries the natural
text AND the structured actions). This slice folds both, where safe, into the one decision
envelope, preserving the deterministic/policy authority unchanged.

### Hermes — files read

- `reference/hermes-agent-main/agent/conversation_loop.py` `run_conversation(...)` — a SINGLE
  assistant message carries BOTH the natural `content` (the reply the user reads) AND the
  structured `tool_calls` in one turn (`_m.get("role") == "assistant" and _m.get("tool_calls")`,
  L630; `final_response` is set from that same message's content, L528/L967). **Pattern: the one
  model response carries the conversational answer alongside the structured actions — they are not
  two separate calls.** We mirror it: the unified decision now also carries the optional `reply`
  (the conversational answer) next to the intent/slots/wording, so a chat turn is answered in the
  SAME envelope. We still deliberately differ in that the Relux brain executes NOTHING.
- `reference/hermes-agent-main/agent/conversation_loop.py` (the truncation/exhaustion fallback,
  ~L1370-1425) — when the model returns no usable tool_calls / is exhausted, reuse the last real
  content rather than blanking. **Pattern: a deterministic fallback always exists.** Mirrored:
  when the envelope omits `reply`/`plan_polish` or they fail validation, the prior dedicated
  `shape_reply`/`run_cli_brain` and `polish_proposal` calls run as the fallback, byte-for-byte.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/tools/update-plan-tool.ts` `readPlanSteps` (L39-74) — a
  structured UPDATE payload is validated field-by-field against an explicit schema + the
  `PLAN_STEP_STATUSES` allowlist, and clamped. **Pattern: validate a structured payload against a
  fixed schema before honoring it.** The folded `plan_polish` reuses the EXACT existing
  `validate_polish` chokepoint (via `polish_from_cli_text`): a step title is honored ONLY on an
  exact authoritative-index match, so the overlay can change wording but never the step count,
  order, or agent ids.
- `reference/openclaw-main/src/agents/cli-output.ts` `parseCliOutput` +
  `reference/openclaw-main/src/shared/balanced-json.ts` `extractBalancedJsonPrefix` (L21-69) —
  lift the parsed object out of a noisy CLI reply and surface only it, never the raw stdout. The
  folded reply/polish ride inside the same envelope, lifted by `parse_adapter_result` FIRST on the
  CLI path (`parse_cli_decision`), so the raw `--output-format json` envelope never reaches the
  validators or the chat bubble.
- `reference/openclaw-main/src/agents/tools/sessions-spawn-tool.ts`
  (`UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS`, rejected before any param is read) — reject unsupported
  keys. `reply`/`assistant_message`/`plan_polish` are added to the envelope's top-level allowlist;
  any OTHER unknown key still fails the WHOLE envelope closed exactly as before.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| Hermes: **one response carries the conversational answer AND the structured actions** | `prime_decision::PrimeBrainDecision` gains an optional `reply` (free-form conversational answer) and `plan_polish` (advisory plan-card overlay), carried alongside intent/slots/wording in ONE envelope; `build_decision_prompt` describes both sections and their safety rules. |
| openclaw: **surface only the parsed object** | both new sections are carried RAW (re-serialized) and validated LATER — `reply` only after the kernel proves the turn is a non-actionful, non-clarify conversational turn; `plan_polish` only against the post-turn AUTHORITATIVE proposal — because eligibility/grounding is known only after the kernel produces the turn (the same late-validation shape `wording` already uses). |
| Hermes/openclaw: **validate against a fixed schema/allowlist; no weaker duplicate logic** | `validated_reply` reuses the SAME brainstorm chokepoint a clarify reply uses (`prime_clarify::parse_clarify` with `ClarifyKind::Brainstorm` → `reconcile_clarify`): control chars stripped, length clamped (600), an action-claim (`ACTION_CLAIM_MARKERS`) rejected wholesale, low-confidence / pure-echo dropped. `validated_polish` reuses the SAME `validate_polish` chokepoint (`ai::polish_from_cli_text`): titles only on exact index match; summary/questions/risks trimmed + bounded. |
| openclaw: **reject unsupported keys (`additionalProperties:false`)** | `reply`/`assistant_message`/`plan_polish` join the top-level allowlist; any other unknown key still fails the whole envelope closed. A bare-string `reply` is normalized to `{text, confidence}` (stamped just above the honor floor so a deliberately-simple committed reply is honored). |
| Hermes/openclaw: **deterministic fallback always exists** | `run_prime` PREFERS the envelope's `reply`/`plan_polish` (no extra call); on any miss it falls back to the dedicated `shape_reply`/`run_cli_brain` and `polish_proposal`/`polish_proposal_via_cli`, so behavior is byte-for-byte the prior path whenever the fold is unavailable. |

**What we deliberately do differently:** the action-free wall is unchanged — `validated_reply` is
applied ONLY on a NON-actionful, non-clarify conversational turn (the actionful-turn deterministic
reply still never reaches the brain), so the brain can never narrate (or overclaim) a real state
change. As of THIS slice we did **not** implement the "after-action explanation" variant the prompt
permits: the brain composes its reply *before* the kernel executes the turn, so it cannot honestly
narrate the actual result inline, and letting it would breach the long-standing action-free wall —
it stayed a deferred future slice rather than a faked capability. (That variant was implemented in a
LATER slice as a POST-execution re-shaping pass — see "Reference read — safe POST-EXECUTION
after-action reply shaping" below — so the wall is preserved.) `plan_polish` is advisory/presentation only
and runs through the identical `validate_polish` index-match invariant, so it can never change what
"Create these tasks" creates. Both are strictly additive: the envelope changes only HOW the brain
is asked (one call) and HOW its reply is parsed (one allowlisted object) — never authority. The
dedicated specialized calls remain as the fallback; `Local` (no brain) is byte-for-byte unchanged.

---

## Reference read — the first safe Prime tool loop: READ-ONLY context tools (this slice)

Every prior brain stage (intent, slots, wording, the unified decision envelope) is *propose-only*
and answers from ONE static `StateSummary` snapshot baked into the prompt. The brain cannot drill
into a specific task's detail, inspect a run, or enumerate the crew before answering — exactly the
gap the master plan flags: Prime "can classify and propose, but it does not inspect live
control-plane state through a governed tool interface before answering the way Hermes / Codex /
Paperclip-like agents do" (`docs/RELUX_MASTER_PLAN.md` §10.1, §17.1). This slice ships the FIRST
safe piece of that capability: a bounded, governed loop in which a configured brain may request
**read-only context tools**, the kernel validates the requested tool against a read-only allowlist,
executes it deterministically against a state snapshot, injects the result back, and lets the brain
look again or answer. Nothing here mutates state, mints work, or grants authority.

### Hermes — files read

- `reference/hermes-agent-main/agent/conversation_loop.py` `run_conversation(...)`
  - The per-turn agentic loop is bounded by a **max-iterations cap**: `while (api_call_count <
    agent.max_iterations and agent.iteration_budget.remaining > 0) or agent._budget_grace_call:`
    (L598). The loop re-calls the model after each tool execution and ends when no tool is
    requested.
  - **Tool-call detection**: `if assistant_message.tool_calls:` (L3106) branches the reply into "the
    model wants a tool" vs. "the model gave a final answer".
  - **Tool-NAME allowlist validation BEFORE execution + self-correction** (L3114-3162): `if
    tc.function.name not in agent.valid_tool_names:` → `repaired = agent._repair_tool_call(...)`
    (L3117-3118); a name that still fails the allowlist is NOT executed — instead an `available = ",
    ".join(sorted(agent.valid_tool_names))` (L3131) list is built and a `role:tool` message `content
    = f"Tool '{tc.function.name}' does not exist. Available tools: {available}"` (L3152-3153) is fed
    back for the model to self-correct. **Pattern: validate the chosen tool against an explicit
    name allowlist before acting; an off-list name is fed back for self-correction, never executed.**
  - `agent/tool_executor.py` (L445-452) — an executed tool's result is appended back as a
    `{"role":"tool","name":..,"tool_call_id":..,"content":..}` message and the loop continues.
    **Pattern: inject the tool result back into the conversation, then re-call the model.**

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/tool-mutation.ts` `isMutatingToolCall(toolName, args)`
  (L140-181) + `READ_ONLY_ACTIONS` (L39-54: `get`/`list`/`read`/`status`/`show`/`fetch`/`search`/…)
  — a single FAIL-CLOSED classifier: the `default` branch (L165-179) treats `cron`/`gateway`/
  `canvas`/`*_actions` as mutating when `action == null || !READ_ONLY_ACTIONS.has(action)`, i.e. an
  unknown/missing action defaults to *mutating*. **Pattern: a single fail-closed read-only vs.
  mutating gate where the unsafe default wins.**
- `reference/openclaw-main/src/agents/tools/common.ts` `readStringParam(…, {required})` (L91-122)
  + `ToolInputError` (L57-64), and `sessions-spawn-tool.ts` `UNSUPPORTED_*_PARAM_KEYS` (L46-55,
  rejected before any param is read) — typed param extraction that fails on bad input and rejects
  unsupported keys. **Pattern: require/sanitize the mandatory arg; do not coerce junk.**
- `reference/openclaw-main/src/agents/cli-output.ts` `parseCliOutput` +
  `reference/openclaw-main/src/shared/balanced-json.ts` `extractBalancedJsonPrefix` (L21-69) — lift
  the first balanced `{...}` out of a noisy reply and surface only the parsed object, never the raw
  stdout.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| Hermes: **bounded loop with a max-iterations cap** | `crates/relux-kernel/src/prime_tools.rs` `ContextLoop` + `MAX_TOOL_ROUNDS` (the iteration cap); the loop also stops on a repeated no-progress call. The async drivers (OpenRouter / CLI) and the synchronous test twin `run_context_loop` share the SAME stepper, so the control flow is pinned once and never drifts between providers. |
| Hermes: **validate the chosen tool against a NAME allowlist before acting** | `classify_tool(name)` admits ONLY a name on the explicit read-only allowlist (`READ_ONLY_TOOLS`); `interpret_reply` routes an off-list name to `BrainTurn::UnknownTool`, never `Call`. |
| Hermes: **off-list name fed back for self-correction, never executed** | `unknown_tool_feedback(name)` is the `"Tool '…' is not available. These read-only tools are available: …"` message; `ContextLoop::observe` records it as the next prompt's feedback and executes nothing. |
| Hermes: **inject the tool result back, then re-call** | `ContextLoop::observe` pushes the executed `ContextRead` into the gathered set; `build_tools_prompt` re-grounds the next round with every prior read's body. |
| Paperclip: **a single fail-closed read-only vs. mutating gate, unsafe default wins** | the first slice ships read-only tools only, so the allowlist IS the read-only set: `classify_tool` returns `ToolKind::Refused` for ANY name not on it (a plausible-sounding `delete_task`/`run_shell` is refused), mirroring `isMutatingToolCall`'s "unknown ⇒ unsafe" default. |
| Paperclip: **require/sanitize the mandatory arg** | `read_id_arg` requires + sanitizes (control-strip, clamp) a `task_id`/`agent_id`; a missing/empty id is an HONEST `ok:false` read ("provide a task_id" / "does not exist"), never a fabricated record. |
| Paperclip: **balanced-JSON extraction, surface only the parsed object** | `interpret_reply` reuses `prime_intent::extract_json_object`; on the CLI path `lift_cli_tool_text` (`server.rs`) runs `parse_adapter_result` FIRST so the raw `--output-format json` envelope never reaches the parser. |

**What we deliberately do differently:** unlike Hermes (where the model also runs *mutating* tools
and the loop produces the final answer), the Relux loop is **read-only and gather-only**. Every
tool is a pure read of a `ContextSnapshot` (an owned, bounded projection taken ONCE under the kernel
lock, so the brain rounds run lock-free and the executors are unit-testable without a kernel); there
is no path from this module to `prime_execute`, an approval, or any durable change. The gathered
reads only ground the EXISTING action-free conversational reply (folded into `grounded_facts` for
the reply-shaping brain and surfaced as `PrimeContextRead` provenance), and the loop runs ONLY on a
non-actionful inspection/explanation/question turn (`turn_wants_context` ∧ `!is_actionful`). The
brain authors no intent, no slot, and no action; `Local` (no brain) gathers nothing and is
byte-for-byte the prior reply path. This is the first rung — read before you speak — with the
write-capable tool surface deliberately deferred until the read-only loop is proven.

---

## Reference read — dashboard provenance for `context_reads` (this slice)

The read-only tool loop above ships the `PrimeTurn.context_reads` wire field but no UI: the
operator could not *see* what live state Prime inspected before answering, so a brain that
drilled into a task / the crew / the runs read as a hidden, magical action rather than visible
provenance. This slice surfaces it — a compact, bounded provenance chip + a collapsed,
expandable detail — without dumping raw JSON or any provider envelope. This is a
presentation-only change; it adds NO authority and does not touch Prime's behavior, so the
binding "read Hermes/openclaw before changing Prime" rule applies only insofar as the wire it
renders was already produced by the (already reference-grounded) read-only loop. Per the prompt,
the read this time targets the chat-UI result-visibility references.

### open-webui — files read (the closest UI analogue)

- `reference/open-webui-main/src/lib/components/common/ToolCallDisplay.svelte` — the canonical
  "show what a tool did" component:
  - **Collapsed-by-default, click-to-expand** (`export let open = false;` L33; the header
    `on:pointerup={() => { open = !open; }}` L117) — the tool row is a compact summary until the
    user opens it. **Pattern: the provenance is one always-on summary line; the detail is behind a
    disclosure so the chat is never flooded.**
  - **A per-tool STATUS ICON** (L127-139): a spinner while `isExecuting`, an emerald `CheckCircle`
    when `isDone`, a neutral wrench otherwise — the ok/in-flight indicator. **Pattern: a small
    icon distinguishes a succeeded read from one that did not (yet) complete.**
  - **Names the tool** in the label (`Executing **{{NAME}}**...` / `View Result from **{{NAME}}**`,
    L150-160). **Pattern: the summary names the tool(s) that ran.**
  - **The result body is BOUNDED**: `const RESULT_PREVIEW_LIMIT = 10000;` (L37) clamps the output
    `pre` to the first N chars with a `Show all ({{COUNT}} characters)` expander (L230-246).
    **Pattern: never render an unbounded result blob; clamp and offer an explicit expand.**

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| open-webui: **collapsed summary line, detail behind a disclosure** | the Prime turn card renders a `<details>` whose `<summary>` is the always-on chip `🔎 used: <tool>, <tool>` (`contextReadsUsedLabel`); the per-read detail list is collapsed until the operator expands it (`apps/dashboard/src/pages/Prime.tsx`). |
| open-webui: **per-tool status icon (done vs. in-flight)** | a per-read `✓`/`!` icon colored ok/err, plus a subtle "some lookups found nothing" note driven by `contextReadsHadMiss` — the read-only loop's `ok` flag (an honest miss, never fabricated) is the ok/error indicator. |
| open-webui: **the label names the tool(s)** | `contextReadsUsedLabel` lists the DISTINCT tool names in look order, itself bounded (`MAX_TOOLS_IN_LABEL = 4`, the rest collapse into `+N more`) so a long loop never floods the chip. |
| open-webui: **clamp the result body, offer an explicit expand** | the detail is doubly bounded: each read's summary is clamped (`contextReadDetail`, 160 chars + ellipsis) and the list is capped (`boundedContextReads`, `MAX_CONTEXT_READS_SHOWN = 8`, honest `+N more`). |

**What we deliberately do differently:** open-webui renders the tool's **full raw arguments and
result JSON** in the expanded `pre` blocks (clamped only at 10k chars). Relux deliberately ships
**no raw JSON / provider envelope to the UI at all** — only the short, server-clamped `summary`
the kernel already attached to each `PrimeContextRead` (the full result body stayed server-side
grounding, per the read-only-loop slice). So the disclosure shows a bounded one-line provenance
per read, never the raw record — the same no-leak posture as the two CLI-output shaping seams. The
chip is pure presentation: it renders only what the kernel returned, attributes no authority, and
appears only on a turn that genuinely ran the (already governed, fail-closed, read-only) loop.

---

## Reference read — more read-only context tools: runs / plugins / approvals (this slice)

The first read-only loop shipped six tools (`board_summary`/`list_tasks`/`get_task`/`list_agents`/
`get_agent`/`list_runs`). The brain could enumerate runs but not drill into a single run, could not
enumerate the installed plugins/adapters, and could not inspect the approval queue — exactly the
"more read-only tools" rung the audit named as next (`docs/prime-processing-audit.md` "Next
recommended slice"). This slice adds `get_run`, `list_plugins`, and `list_approvals` to the SAME
governed, fail-closed, bounded loop — pure projections of the live control plane, no new authority.

### Hermes — files read

- `reference/hermes-agent-main/agent/conversation_loop.py` `run_conversation(...)` — re-read for the
  bounded tool/result behavior the new tools plug into unchanged: the loop is still bounded by the
  `max_iterations` cap, each tool result is injected back and re-grounds the next round, and the
  chosen tool name is still validated against `agent.valid_tool_names` BEFORE execution (an off-list
  name is fed back, never run). **Pattern reused as-is:** a new read-only tool is just a new
  allowlist member + a pure executor; the loop control flow (cap, allowlist gate, self-correction,
  inject-and-re-call) does not change, so it is pinned ONCE in `prime_tools::ContextLoop` and the
  three new tools inherit it for free.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/tool-mutation.ts` `READ_ONLY_ACTIONS` (L39-54) — the canonical
  read-only verb set: `get`/`list`/`read`/`status`/`show`/`fetch`/`search`/`view`/`inspect`/`check`/…
  Every new tool is one of these verbs (`get_run`, `list_plugins`, `list_approvals`), so all three
  sit squarely in the read-only class `isMutatingToolCall` would classify as non-mutating. **Pattern:
  a `get`/`list` action is read-only; we keep the new tools strictly to that shape and add no
  mutating verb to the allowlist.**
- `reference/openclaw-main/src/agents/tools/common.ts` `readStringParam(…, {required})` (L91-122) +
  `ToolInputError` (L57-64) — typed, required-string extraction that fails on bad input rather than
  coercing. Mirrored by `prime_tools::read_id_arg` for the new `get_run` `run_id` (required +
  sanitized + clamped; a missing/empty id is an HONEST `ok:false` read, never a fabricated run).
- `reference/openclaw-main/src/agents/tools/sessions-spawn-tool.ts`
  (`UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS`, rejected before any param is read) — reject unsupported
  input. The new tools read ONLY their allowlisted arg key (`run_id`, or the optional `status`
  filter); any extra key in the args object is simply ignored (never executed as authority), and an
  unrecognized `status` filter is ignored rather than failing — the same tolerate-the-rest shape the
  existing `list_tasks` filter uses.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| Hermes: **bounded loop, allowlist-validated tool name, inject-and-re-call** (unchanged) | the three new tools are added to `READ_ONLY_TOOLS` and dispatched by `execute_context_tool`; `classify_tool` admits them as `ReadOnly`, and the existing `ContextLoop`/`MAX_TOOL_ROUNDS`/stop-on-repeat driver runs them with NO new control flow. |
| openclaw: **`get`/`list` is the read-only verb class** (`READ_ONLY_ACTIONS`) | `get_run` / `list_plugins` / `list_approvals` are pure reads of the `ContextSnapshot`; there is still no mutating tool on the allowlist, so `classify_tool` stays fail-closed (anything off the list is `Refused`). |
| openclaw: **required string fails on bad input** (`readStringParam`) | `get_run` requires a `run_id` via `read_id_arg`; a missing/empty/unknown id is an honest `ok:false` miss ("Run '…' does not exist."), never a fabricated record. |
| openclaw: **bound the result, surface only the parsed/clamped body** (`cli-output`/`RESULT_PREVIEW_LIMIT`) | lists are `MAX_LIST_ITEMS`-bounded with an honest `(+N more)`, each result is `MAX_RESULT_CHARS`-clamped, and free-text fields (a run summary/error, an approval action/reason) are redacted + bounded at snapshot-build time (`state.rs` `redact_line`). |
| openclaw: **never ship raw provider data** (the no-leak CLI-output seam) | the run projection deliberately OMITS the raw `usage`/`cost` provider envelope and the plugin projection omits the raw `source_label` (a local path / URL) — only the bounded, redacted human fields and the source-kind label are projected. |

**What we deliberately do differently:** the new tools are still **read-only and gather-only** — pure
reads of the owned `ContextSnapshot` taken once under the kernel lock, with no path to
`prime_execute`, an approval, or any durable change. They extend the snapshot with a `plugins`
projection (id/version/kind/enabled/protected/source-kind/tool-count, NO raw source path) and an
`approvals` projection (id/status/risk/requester + a redacted action/reason), and enrich the existing
run projection with the adapter, logical timing, and a redacted summary/error — while deliberately
NOT projecting the raw provider `usage`/`cost` envelope. The loop, the allowlist gate, the bounds,
and the action-free wall are all unchanged; this is the proven read-only rung widened to the rest of
the local control plane, with the write-capable tool surface still deferred.

---

## Reference read — folding the read-only context loop INTO the unified decision (this slice)

The read-only context loop above is a SELF-CONTAINED sidecar: the unified `PrimeBrainDecision`
answers intent + slots + wording from ONE static board snapshot, and THEN — on a non-actionful
inspection turn — a SEPARATE multi-round `ContextLoop` runs to gather live reads before the reply
is shaped. So an inspection turn under a configured brain still costs the unified decision call
PLUS up to `MAX_TOOL_ROUNDS` loop calls PLUS the reply call — more round-trips, and two disjoint
brain interactions, than how Hermes / Codex answer (ONE response carries the answer AND the
structured tool requests). This slice — the audit's named "Read context on the unified decision"
rung — lets the ONE unified decision envelope ALSO carry the brain's **read-only tool requests**,
which Relux then executes deterministically (no second multi-round loop) and grounds the reply in,
while keeping the bounded sidecar loop as the fallback and adding NO mutation path.

### Hermes — files read

- `reference/hermes-agent-main/agent/conversation_loop.py` `run_conversation(...)` — a SINGLE
  assistant message carries BOTH the natural `content` AND the structured `tool_calls`
  (`if assistant_message.tool_calls:`, L3106; `_m.get("tool_calls")`, ~L630-875), and the chosen
  tool name is **validated against `agent.valid_tool_names` BEFORE execution** (L3114-3162) — an
  off-list name is fed back, never run. **Pattern: one response carries the answer AND the tool
  requests; each requested tool is allowlist-validated before it executes.** We mirror it:
  `crates/relux-kernel/src/prime_decision.rs` `parse_decision` now lifts a `tool_requests` array
  ALONGSIDE the intent/slots/wording in the one envelope, and EACH entry is validated through the
  SAME read-only allowlist gate the loop uses (`prime_tools::validate_tool_request` →
  `classify_tool`). We deliberately differ in that the Relux brain executes NOTHING — the validated
  reads run in the deterministic kernel executor, not the model.
- `reference/hermes-agent-main/agent/conversation_loop.py` — the bounded `max_iterations` cap +
  result injection (`agent/tool_executor.py` L445-452, the executed result appended back and the
  loop continues). **Pattern: bound the tool work; inject the result and answer grounded in it.**
  Mirrored: `prime_tools::execute_requested_reads` runs the requested list bounded by the SAME
  `MAX_TOOL_ROUNDS` (extra requests dropped, repeated identical reads skipped), then the observations
  are folded into the existing reply shaper's `grounded_facts` (the bounded follow-up response) —
  the same single-follow-up shape, with the multi-round loop kept only as the fallback.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/tool-mutation.ts` `isMutatingToolCall(toolName, args)` +
  `READ_ONLY_ACTIONS` (L39-54, L140-181) — a single FAIL-CLOSED classifier where an unknown/missing
  action defaults to *mutating*. **Pattern: a single fail-closed read-only-vs-mutating gate, unsafe
  default wins.** `validate_tool_request` reuses `classify_tool`, which admits ONLY a name on the
  read-only allowlist; a mutating / unknown / made-up name (`delete_task`, `run_shell`) is rejected
  at PARSE time and can never reach an executor — so a smuggled mutating request in the unified
  envelope is dropped, not run.
- `reference/openclaw-main/src/agents/tools/update-plan-tool.ts` `readPlanSteps` (L39-74) — a
  composite payload is validated FIELD-BY-FIELD / per-entry against its schema + allowlist; a bad
  entry is an input error, the rest stand. **Pattern: validate each entry of a list section
  independently.** Mirrored: each `tool_requests` entry is validated independently; a refused entry
  is dropped while the valid read-only entries survive, and a `tool_requests` whose ONLY entries are
  refused leaves the section empty (no usable section ⇒ the caller falls back).
- `reference/openclaw-main/src/agents/tools/sessions-spawn-tool.ts`
  (`UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS`, rejected before any param is read) — reject unsupported
  keys. `tool_requests`/`context_reads` join the envelope's top-level allowlist; any OTHER unknown
  top-level key still fails the WHOLE envelope closed exactly as before.
- `reference/openclaw-main/src/agents/cli-output.ts` `parseCliOutput` +
  `reference/openclaw-main/src/shared/balanced-json.ts` `extractBalancedJsonPrefix` (L21-69) — lift
  the parsed object out of a noisy CLI reply, never the raw stdout. The `tool_requests` ride inside
  the same envelope, lifted by `parse_adapter_result` FIRST on the CLI path (`parse_cli_decision`),
  so the raw `--output-format json` envelope never reaches the parser or the request validation.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| Hermes: **one response carries the answer AND the tool requests**, each allowlist-validated before use | `prime_decision::PrimeBrainDecision` gains `context_requests: Vec<ToolCall>`, parsed from a `tool_requests` array in the SAME envelope as intent/slots/wording; each entry is validated through `prime_tools::validate_tool_request` (→ `classify_tool`) at parse time. `build_decision_prompt` describes the section and lists the read-only tool names. |
| openclaw: **single fail-closed read-only gate, unsafe default wins** (`isMutatingToolCall`) | a request naming any tool NOT on the read-only allowlist is dropped at parse time — never executed. A mutating-only `tool_requests` leaves the section empty, so it adds no authority. |
| Hermes: **bounded tool work, inject-and-ground** | `prime_tools::execute_requested_reads` runs the validated list against the pre-taken `ContextSnapshot` bounded by `MAX_TOOL_ROUNDS` (extra dropped, repeated identical reads skipped), then the existing reply shaper grounds the ONE follow-up response in the observations — no second multi-round brain loop. |
| openclaw: **reject unsupported top-level keys** (`UNSUPPORTED_*_PARAM_KEYS`) | `tool_requests`/`context_reads` join the top-level allowlist; any other unknown key still fails the whole envelope closed. |
| openclaw: **no raw envelope leak** (`cli-output`/`balanced-json`) | the CLI path lifts the reply via `parse_adapter_result` FIRST (`parse_cli_decision`); only the validated, bounded `PrimeContextRead` provenance ever ships (the full read bodies stay server-side grounding, exactly as the sidecar loop). |

**What we deliberately do differently:** the fold changes only WHEN the read-only tools are
requested (in the one decision envelope) and removes a duplicate brain interaction — it changes
NOTHING about authority or the read-only-and-gather-only contract. Every requested tool is still a
pure read of the owned snapshot validated against the read-only allowlist; there is no path from
the unified path to `prime_execute`, an approval, or any mutation, and a mutating request is
rejected at parse time. The execution is deterministic (the model runs nothing), bounded by the
same `MAX_TOOL_ROUNDS`, and the reply is the SAME bounded follow-up shaped by the existing
`shape_reply`/`run_cli_brain` grounded in the observations. The kernel uses the requested reads
ONLY on a non-actionful inspection turn (`turn_wants_context` ∧ `!is_actionful`); on a turn that
requested no tools — or any failure / `Local` — the sidecar `ContextLoop` runs exactly as before
(no duplicate execution: the loop runs ONLY when the unified envelope requested nothing). The
write-capable tool surface stays deferred; this slice only unifies the *read-before-you-speak*
gather into the one decision call.

---

## Reference read — the first safe WRITE-capable Prime tool surface (this slice)

The read-only tool loop above proved the governed-tool shape — the brain *requests* an allowlisted
tool, the kernel validates the name fail-closed and executes it deterministically — but every tool
there is a pure READ. The brain still could not ask Prime to *do* anything through a tool contract.
This slice ships the first safe WRITE-capable surface, the audit's named "A WRITE-capable tool
surface" rung (`docs/prime-processing-audit.md`): the brain may request a known mutating tool, but
Relux converts it into an EXISTING Prime action/proposal and enforces every current
validation/approval gate. The brain never writes state directly.

### Hermes — files read

- `reference/hermes-agent-main/agent/conversation_loop.py` `run_conversation(...)` (L3106-3162) —
  a SINGLE assistant message carries BOTH `content` and the structured `tool_calls`, and the chosen
  tool name is **validated against `agent.valid_tool_names` BEFORE execution** (L3114-3162); an
  off-list name is fed back for self-correction, never run. **Pattern: one response carries the
  answer AND the structured action requests; each requested tool name is allowlist-validated before
  it is used.** We mirror it: the unified `PrimeBrainDecision` now also carries a single
  `action_request` alongside intent/slots/wording, and `prime_write_tools::classify_write_tool` is
  the name-allowlist gate — a name not in `WRITE_TOOLS` is refused at parse time. We deliberately
  differ in that the Relux brain executes NOTHING: a write tool is converted into a
  `BrainIntentProposal` + a validated slot that flow through the UNCHANGED `decide` →
  `prime_execute` / approval path.
- `reference/hermes-agent-main/model_tools.py` `coerce_tool_args` (L535-616) — each tool argument is
  coerced/validated against its registered schema before dispatch; a non-coercible value is dropped,
  not fatal. Mirrored by REUSING the existing per-action slot validators on the write tool's `args`
  (no weaker duplicate parsing): an args object that fails its validator fails the whole request
  closed.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/tool-mutation.ts` `isMutatingToolCall(toolName, args)`
  (L140-181) — a single FAIL-CLOSED classifier where an UNKNOWN/missing action defaults to
  *mutating*. **Pattern: a single fail-closed gate where the unsafe default wins, and minting work
  is never inferred from chat.** `classify_write_tool` is the inverse-polarity twin: the write
  allowlist is explicit and tiny, and a write tool is honored ONLY when the deterministic intent gate
  (`reconcile_intent`) agrees the user asked for action — so a mutating tool the brain requests on
  guarded chat is vetoed (a sensitive intent + guarded chat is always kept deterministic).
- `reference/openclaw-main/src/agents/tool-policy.ts`
  `applyOwnerOnlyToolPolicy` / `wrapOwnerOnlyToolExecution` — work / control-plane capabilities
  (spawn, gateway) are ONE explicit, GATED capability, replaced with a hard refusal otherwise.
  **Pattern: a mutating control-plane capability stays explicitly gated, never auto-run.** Mirrored:
  `plugin.install` / `permission.grant` map to the SAME approval-gated `Propose` the deterministic
  path produces — `sharpen_admin_action` reshapes only the *subject the human reviews*; the kernel
  logs an approval and executes nothing.
- `reference/openclaw-main/src/agents/tools/update-plan-tool.ts` `readPlanSteps` (L39-74),
  `src/agents/tools/common.ts` `readStringParam(required)` / `ToolInputError` (L57-122),
  `src/agents/tools/sessions-spawn-tool.ts` `UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS` (rejected before
  any param is read) — validate a structured payload field-by-field against an explicit
  schema/allowlist, require the mandatory string, reject unsupported keys. **Pattern: validate the
  mutating payload hard; reject junk.** Adopted by REUSING the EXISTING validators
  (`parse_task_slots`, `parse_update_slots`, `parse_assign_slots`, `parse_agent_slots`,
  `parse_plugin_ref`/`parse_permission_slots`) on the tool's args — so a write tool inherits the same
  allowlist, sanitization, clamping, and existing-target validation, plus the `task.start` `task_id`
  required-string read.
- `reference/openclaw-main/src/agents/cli-output.ts` `parseCliOutput` +
  `src/shared/balanced-json.ts` `extractBalancedJsonPrefix` — lift the parsed object out of a noisy
  CLI reply, never the raw stdout. The `action_request` rides inside the same envelope, lifted by
  `parse_adapter_result` FIRST on the CLI path (`parse_cli_decision`), so the raw envelope never
  reaches the request validation.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| Hermes: **one response carries the answer AND the structured action**, each allowlist-validated | `prime_decision::PrimeBrainDecision` gains `action_request: Option<ParsedWriteTool>`, parsed from a single `action_request` (`tool_call` alias) object in the SAME envelope; `prime_write_tools::parse_write_tool_request` validates the name via `classify_write_tool` and the args via the existing per-action validator. |
| openclaw: **single fail-closed gate, unsafe default wins** (`isMutatingToolCall`) | `classify_write_tool` admits ONLY a name in `WRITE_TOOLS`; an off-list / mutating-sounding name (`task.delete`, `shell.run`) is dropped at parse time. A write tool acts ONLY when `reconcile_intent` (the unchanged fail-closed gate) accepts its sensitive intent — guarded chat keeps the deterministic non-work intent. |
| openclaw: **a mutating control-plane capability stays GATED** (`tool-policy`) | `plugin.install` / `permission.grant` map to the SAME approval-gated `Propose`; the brain only sharpens the subject the human reviews (`sharpen_admin_action`), and the kernel changes nothing. |
| openclaw/Hermes: **validate the mutating payload hard; no weaker duplicate logic** | each write tool's args are validated by REUSING the existing slot validator; the result feeds the UNCHANGED `prime_turn_with_brain` chokepoint, which still reconciles every id against the live state (an unknown task/agent fails closed) and enforces the terminal-state / readiness guards. |
| openclaw: **balanced-JSON extraction, no raw-envelope leak** | the CLI path lifts the reply via `parse_adapter_result` FIRST; only the validated, bounded provenance (`requested_tool`) ever ships. |
| openclaw: **one explicit, gated capability — never batch** | at most ONE `action_request` per turn; a batched multi-tool request is refused (the turn falls back to the deterministic path, which clarifies), not batch-executed. |

**What we deliberately do differently:** unlike Hermes (where the model runs the mutating tool), the
Relux brain runs NOTHING — a write tool is *desugared* into the EXISTING intent + slot mechanism, so
every durable change still flows through `decide` → `prime_execute` (safe `Act`) or a human approval
(risky `Propose`). The named write tool adds three things over a bare intent+slots proposal: an
explicit, governed allowlist distinct from the read-only set; a one-mutating-tool-per-turn cap; and a
`requested tool: <name>` provenance chip. The safety property "casual chat can never trigger a
mutation" is enforced by the SAME `reconcile_intent` gate the brain-mediated intent already uses (a
write tool's intent is `is_sensitive_intent`, so guarded chat vetoes it), and the `task.create`
sharpen-only invariant holds — a write tool sharpens a create the deterministic path already
produced; only `task.update` / `task.assign` / `task.start` PROMOTE an under-specified clarify, and
only because each is a SAFE, fully-id-validated action (the run-start promotion mirrors the existing
assign/update promotions, honoring only a task that EXISTS and is READY). The mutating-tool surface
is intentionally tiny (`task.create`/`task.update`/`task.assign`/`task.start`/`agent.create` safe;
`plugin.install`/`permission.grant` approval-only); a multi-round write loop, after-action narration,
and richer tools stay deferred.

---

## Reference read — safe POST-EXECUTION after-action reply shaping (this slice)

Every prior brain stage composes its reply BEFORE the kernel executes the turn, so the
action-free wall keeps an ACTIONFUL turn's reply strictly deterministic (`is_actionful` →
`shape_reply` keeps it `DeterministicForAction`). The brain could classify, sharpen slots,
request a governed tool, and re-word a *conversational* turn — but it could never phrase the
confirmation a user reads AFTER a create / update / assign / start / agent.create executes, or
after a plugin.install / permission.grant is proposed. That was the explicitly-deferred
"after-action narration" rung (`docs/prime-processing-audit.md`): "the brain composes its reply
before the kernel executes, so an honest after-action narration needs a post-execution
re-shaping pass that preserves the action-free wall." This slice ships it.

### Hermes — files read

- `reference/hermes-agent-main/agent/tool_executor.py` (L348-452) — the post-execution display
  loop: each executed tool's result is appended back as a `{"role":"tool","name":..,
  "tool_call_id":..,"content":..}` message (L446-452) carrying an `is_error` flag and a BOUNDED
  preview (`result_preview = _err_text[:200]`, L372), and the loop continues so the model produces
  its FINAL answer grounded in that actual result. **Pattern: the final answer is grounded in the
  real, bounded execution result (success vs. `is_error`), injected AFTER the action ran.**
- `reference/hermes-agent-main/agent/conversation_loop.py` `run_conversation(...)` (~L3106-3162,
  L3466-3480) — after the tool result is injected the model is re-called for the final answer; an
  empty/junk model output falls back to the last real content. **Pattern: re-shape the answer over
  the injected result; a deterministic fallback always exists.** Mirrored:
  `crates/relux-kernel/src/prime_after_action.rs` hands the brain the sanitized, bounded
  `ActionEnvelope` (kind = executed / proposed / failed) as the ONLY ground truth and re-words the
  confirmation — but, unlike Hermes, the Relux brain executes NOTHING here: the action already ran
  deterministically; on any failure the grounded deterministic reply stands.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/bash-tools.exec-approval-followup.ts`
  `buildExecApprovalFollowupPrompt` (L64-82) / `buildExecDeniedFollowupPrompt` (L34-48) — the
  canonical "narrate the result after the approved action completed" prompt: it injects the "Exact
  completion details" (the result envelope) and steers "if it succeeded, share the relevant output;
  if it failed, explain what went wrong" (L79-81), while the DENIED variant insists "An async
  command did not run … Do not claim there is new command output" (L36-46). **Pattern: ground the
  follow-up in the exact result, and distinguish succeeded / failed / did-not-run so the model
  never claims work that did not happen.**
- `reference/openclaw-main/src/agents/pi-embedded-helpers/sanitize-user-facing-text.ts`
  `sanitizeUserFacingText` (used at `bash-tools.exec-approval-followup.ts` L102-123) — the result
  body shown to the user is sanitized before display. **Pattern: sanitize the user-facing result.**
- `reference/openclaw-main/src/agents/tools/sessions-spawn-tool.ts` (`UNSUPPORTED_*_PARAM_KEYS`) +
  `src/agents/tools/common.ts` (`readStringParam` required) + `src/shared/balanced-json.ts`
  (`extractBalancedJsonPrefix`) + `src/agents/cli-output.ts` (`parseCliOutput`) — reject unsupported
  keys, require the mandatory string, lift the parsed object out of a noisy CLI reply (never the raw
  stdout). Mirrored in `parse_after_action` (allowlist `text`/`confidence`/`rationale`, require a
  non-empty `text`) and the no-leak `parse_cli_after_action` (`parse_adapter_result` FIRST).

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| Hermes: **final answer grounded in the real, bounded result, injected after the action ran** | `prime_after_action::build_action_envelope` builds a sanitized, bounded `ActionEnvelope` from the ALREADY-executed `PrimeTurn` (kind, action label, the concrete ids it produced/targeted, the durable facts, the redacted grounded reply); `build_after_action_prompt` hands ONLY that to the brain, which runs AFTER the kernel executed. |
| openclaw: **succeeded → share output / failed → explain / did-not-run → do not claim output** (`buildExecApprovalFollowupPrompt` / `buildExecDeniedFollowupPrompt`) | the three `ActionResultKind` prompt variants: `Executed` ("ALREADY done — confirm, you MAY mention the ids"), `Proposed` ("NOT done — awaiting approval — never say installed/granted/created/started"), `Failed` ("did not complete — do NOT claim success"). |
| Hermes: **the result carries an `is_error` flag the answer must honor** | the INVERSE of `prime_clarify`'s blanket action-claim rejection: a completion claim is honored ONLY when the envelope's matching fact is confirmed; a success claim on a `Failed` envelope is rejected; an `installed`/`granted` claim is rejected on every turn (Prime never EXECUTES an install/grant — always an approval-gated `Propose`). |
| openclaw: **sanitize the user-facing result** (`sanitizeUserFacingText`) | `sanitize_block` (control-strip, clamp) + `redact_secrets` (mask secret-prefixed tokens, high-entropy blobs, absolute unix/windows paths) on BOTH the envelope's grounded reply and the brain's reply; an id-shaped token (`task_`/`run_`/`appr_`/`approval_`) not in `envelope.ids` fails the reply closed (an invented id). |
| openclaw: **reject unsupported keys, require the mandatory string, no raw-envelope leak** | `parse_after_action` allowlist + required non-empty `text`; the CLI path lifts the reply via `parse_adapter_result` FIRST (`parse_cli_after_action`), so the raw `--output-format json` envelope never reaches the validator or the chat. |
| Hermes/openclaw: **a deterministic fallback always exists** | strictly additive — no brain (Local), low confidence, malformed JSON, an unsupported field, a contradiction, an invented id, or a pure echo all fall back to the grounded deterministic reply (`shape_reply`'s `DeterministicForAction`) with no provenance. |

**What we deliberately do differently:** unlike Hermes (where the model runs the tool and then
answers in the SAME loop) the Relux action already ran deterministically through `decide` →
`prime_execute` / approval; this stage ONLY re-words the confirmation and changes nothing — there
is no path from `prime_after_action` to a mutation. It runs ONLY on a non-tool ACTIONFUL turn
(`after_action_kind` returns `None` for a tool turn, preserving the long-standing
"never narrate a tool result" wall, and for a non-actionful turn, which the clarify/brainstorm/
free-form paths already shape). A high-risk action is narrated ONLY as a proposal (it is always a
`Propose`, so the envelope kind is `Proposed` and a completion claim is rejected) — Prime never
says installed/granted. This is the post-execution counterpart of the pre-execution wording path
(`prime_clarify`): the same allowlist/sanitize/clamp discipline, but the claim validation is the
INVERSE — a claim grounded in the real result is honored, a claim that contradicts it is refused.
A multi-round write loop and richer tools stay deferred.

---

## Reference read — the bounded observe-then-act decision loop (this slice)

The unified decision was still a SINGLE provider call: the brain had to choose its one governed
action (`action_request`) from the STATIC board snapshot baked into the prompt, with no chance to
drill into a specific task / run / the crew first. The read-only context loop could observe, but
only on a NON-actionful inspection turn and only to ground a reply — never to inform the action. So
a single user turn could not safely do: **inspect live state → choose one governed action →
execute/propose → narrate result**. That is the audit's named "multi-round write loop (act →
observe → act INSIDE the one envelope flow), which needs the decision call itself to loop"
(`docs/prime-processing-audit.md`). This slice makes the decision call LOOP, bounded, with the
observe phase strictly read-only and the act phase still through the unchanged gate.

### Hermes — files read

- `reference/hermes-agent-main/agent/conversation_loop.py` `run_conversation(...)`
  - The per-turn agentic loop is bounded by a **max-iterations cap**: `while (api_call_count <
    agent.max_iterations and agent.iteration_budget.remaining > 0) or agent._budget_grace_call:`
    (L598). Each pass re-calls the model after tool execution and ends when the model stops
    requesting tools (returns its final answer). **Pattern: a bounded loop where each round the
    model either requests tools (loop continues) or gives its final answer (loop ends).**
  - **Tool-call detection** `if assistant_message.tool_calls:` (L3106) branches "the model wants a
    tool" vs. "the model gave a final answer".
  - **Name-allowlist validation BEFORE execution + self-correction** (L3114-3162): a name not in
    `agent.valid_tool_names` is repaired or fed back as a `role:tool` "Tool '…' does not exist.
    Available tools: …" message; it is NEVER executed. **Pattern: the chosen tool is validated
    against an explicit allowlist before it runs; an off-list name is fed back, not executed.**
- `reference/hermes-agent-main/agent/tool_executor.py` (L348-452) — the executed tool's result is
  appended back as a `{"role":"tool","name":..,"tool_call_id":..,"content":..}` message
  (`messages.append(tool_msg)`, ~L450) with an `is_error` flag and a BOUNDED preview
  (`result_preview = _err_text[:200]`, L372), and the loop continues so the model answers grounded in
  the real result. **Pattern: inject the bounded tool result back, then re-call the model.**

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/tool-mutation.ts` `isMutatingToolCall(toolName, args)`
  (L140-181) + `READ_ONLY_ACTIONS` (L39-54: `get`/`list`/`read`/`status`/`show`/`fetch`/`search`/
  `view`/`inspect`/`check`/…) — a single FAIL-CLOSED classifier whose `default` branch treats an
  unknown/missing action as *mutating* (`action == null || !READ_ONLY_ACTIONS.has(action)`).
  **Pattern: a single fail-closed read-only-vs-mutating gate where the unsafe default wins.** The
  observe phase of the loop executes ONLY the read-only `context_requests` (already validated by
  `prime_tools::validate_tool_request` → `classify_tool`, the same fail-closed gate); the one
  mutating action is never run during observation — it is committed only once, at the end, through
  the kernel's existing gate.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| Hermes: **bounded loop; each round requests tools OR gives a final answer** | `crates/relux-kernel/src/prime_decision.rs` `DecisionLoop` + `MAX_DECISION_ROUNDS` (=3): each round the brain returns a `PrimeBrainDecision`; one carrying read-only `context_requests` is an OBSERVE round (the kernel runs the reads and re-calls), one without is the COMMITTED terminal decision. The synchronous test twin `run_decision_loop` and the async provider driver (`server.rs` `decide_prime_with_observation`) share the SAME stepper, so the control flow is pinned once. |
| Hermes: **validate the chosen tool against an allowlist before running; off-list fed back, never executed** | the observe phase runs ONLY `context_requests`, each already validated at parse time against the READ-ONLY allowlist (`validate_tool_request` → `classify_tool`); a mutating/unknown name was dropped in `parse_decision` and can never be a `context_request`. |
| Hermes: **inject the bounded tool result, then re-call** | `DecisionLoop::step` executes the round's requests via `prime_tools::execute_requested_reads` (bounded, deduped, read-only), accumulates the NEW reads, and `build_decision_prompt(message, summary, observations)` re-grounds the next round with the rendered reads + a "commit (omit tool_requests) once you have observed enough" steer. |
| openclaw: **single fail-closed read-only gate, unsafe default wins** | the observe phase is read-only by construction (no path from the loop to `prime_execute`); the lone mutating `action_request` is desugared into the EXISTING intent + slot and flows through the UNCHANGED fail-closed `reconcile_intent` gate + `decide` → `prime_execute` / approval — the loop adds no new authority. |
| Hermes: **bounded; deterministic fallback always exists** | the loop is capped at `MAX_DECISION_ROUNDS` and stops on no progress (a brain re-requesting what it already observed); a provider failure mid-loop keeps the interim decision; ANY failure / `Local` falls back to the specialized per-section stack and the deterministic rails, byte-for-byte. |

**What we deliberately do differently:** unlike Hermes (where the model runs the tools AND the
mutating action and the loop produces the final answer), the Relux loop **observes read-only between
rounds and acts ONCE at the end through the unchanged kernel gate**. The kernel — never the brain —
executes the read-only tools (pure reads of an owned, bounded snapshot taken once under the lock),
and the eventual durable change still flows through `decide` → `prime_execute` (safe `Act`) or a
human approval (risky `Propose`). So a single turn can now inspect live state, choose its one
governed action grounded in what it saw, execute/propose it, and narrate the result (the existing
`prime_after_action` pass), with the fail-closed intent gate still vetoing a mutating action on
guarded chat, every id still validated against the live state, and every approval gate intact. The
first round's prompt is byte-for-byte the prior single-shot prompt (empty observations), so a turn
where the brain commits immediately is unchanged. The loop is intentionally short (a *little*
inspection before one action, not an open-ended agent); a richer multi-action loop stays deferred.

---

## Reference read — a governed ORCHESTRATION write tool (this slice)

The write-capable tool surface ([the first-write slice](#reference-read--the-first-safe-write-capable-prime-tool-surface-this-slice))
shipped seven tools — task create/update/assign/start, agent.create, plugin.install, permission.grant —
but none reached Prime's richest write path: **orchestration** (one goal fanned into several
role-typed briefs assigned across the crew). Prime already has a deterministic multi-agent planner
([`relux_core::plan_orchestration`]) and an executable `OrchestrateGoal` action, but the only way to
invoke them was the keyword `Orchestration` intent whose goal was string-sliced from the raw message
— so a user who explicitly asked Prime to coordinate work but phrased the goal as a single clause got
a clarifying question, not a plan. This slice adds `orchestration.create` to the same governed write
allowlist, mapping it to the EXISTING `OrchestrateGoal` path, with the deterministic planner kept as
the sole authority on the decomposition.

### Hermes — files read

- `reference/hermes-agent-main/agent/conversation_loop.py` `run_conversation(...)` (L3114-3162) — a
  SINGLE assistant message carries both `content` and the structured `tool_calls`, and the chosen
  tool name is **validated against `agent.valid_tool_names` BEFORE execution**; an off-list name is
  fed back, never run. **Pattern: one response carries the answer AND the structured action; each
  requested tool name is allowlist-validated before it is used.** Mirrored: `orchestration.create`
  joins [`crate::prime_write_tools`] `WRITE_TOOLS`, and `classify_write_tool` is the name-allowlist
  gate — the brain executes nothing; the tool desugars into the existing `Orchestration` intent + a
  validated goal slot that flows through the UNCHANGED `decide` → `prime_execute` path.
- `reference/hermes-agent-main/model_tools.py` `coerce_tool_args` (L535-616) + `agent/
  message_sanitization.py` — coerce each argument to its schema type (a non-coercible value is
  dropped, not fatal) and sanitize control chars + clamp length on every model-produced string.
  Mirrored in `prime_orchestration_slots`: the goal/steps sanitizers (control-strip + clamp) and the
  `coerce_confidence` (number-or-numeric-string → clamped, neutral default otherwise).

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/tools/update-plan-tool.ts` `readPlanSteps` (L39-74) — a
  structured plan payload is validated FIELD-BY-FIELD / per-entry against its schema + the
  `PLAN_STEP_STATUSES` allowlist, and clamped (at most one `in_progress`). **Pattern: validate a
  composite payload's list section per-entry against an explicit schema, and clamp it.** Mirrored:
  `parse_orchestration_slots` validates the optional `steps` array STRICTLY — present ⇒ it must be an
  array, and EVERY element must be a string (a non-array, or any non-string element such as a
  smuggled `{"agent":...}` object, fails the WHOLE proposal closed); each step is sanitized + clamped
  and the count is clamped to the planner's own `MAX_STEPS` cap.
- `reference/openclaw-main/src/agents/tools/sessions-spawn-tool.ts`
  `UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS` (rejected before any param is read) + `src/agents/tools/
  common.ts` `readStringParam(required)` / `ToolInputError` (L57-122) — reject unsupported keys
  outright; require/trim the mandatory string. Mirrored: `parse_orchestration_slots` accepts ONLY
  `ALLOWED_KEYS` (`goal`/`steps`/`confidence`/`rationale`) — any other key (a smuggled `agent_id` /
  `role` / `run`) fails closed — and requires a non-empty `goal`.
- `reference/openclaw-main/src/agents/tool-policy.ts`
  `applyOwnerOnlyToolPolicy` / `wrapOwnerOnlyToolExecution` — a work / control-plane capability is
  ONE explicit, GATED capability, never inferred from chat. **Pattern: minting work is an explicit
  capability, never auto-run from casual chat.** Mirrored: `orchestration.create`'s intent
  (`Orchestration`) is `is_sensitive_intent`, so `reconcile_intent` keeps guarded chat deterministic
  — and, because the deterministic classifier itself reads a guarded coordination question ("should
  we split this across agents?") as `Orchestration` (so the gate's veto is a no-op there), the kernel
  promotion is ADDITIONALLY gated on `!is_chat_guarded`, the same boundary, so a question can never
  fan out work; only an explicit orchestrate/build/do-it request promotes.
- `reference/openclaw-main/src/shared/balanced-json.ts` `extractBalancedJsonPrefix` +
  `src/agents/cli-output.ts` `parseCliOutput` — lift the parsed object out of a noisy reply, never
  the raw stdout. The `orchestration.create` args ride inside the unified `action_request`, lifted by
  `parse_adapter_result` FIRST on the CLI path (`parse_cli_decision`), and the goal JSON via the
  shared `extract_json_object` scanner.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| Hermes: **one response carries the action; the tool name is allowlist-validated** | `orchestration.create` joins `prime_write_tools::WRITE_TOOLS` (intent `Orchestration`, `gated:false`); `classify_write_tool` admits ONLY allowlisted names, so a made-up `orchestration.run`/`orchestration.delete` is refused at parse time. The validated slot is `WriteToolSlot::Orchestration(BrainOrchestrationSlots)`. |
| openclaw: **validate the list section per-entry; reject unsupported keys; require the mandatory string** (`readPlanSteps`, `UNSUPPORTED_*`, `readStringParam`) | `prime_orchestration_slots::parse_orchestration_slots` allowlist = `goal`/`steps`/`confidence`/`rationale` (any other key fails closed); `goal` required non-empty + sanitized + clamped; `steps` must be an array of strings (non-array / non-string element fails closed), each sanitized + clamped, count clamped to the planner's `MAX_STEPS`. |
| openclaw: **the deterministic planner owns the dangerous decision** (validate against what exists) | `reconcile_orchestration_slots` composes the goal (steps joined with the planner's own connector, else the goal verbatim) and runs the deterministic `relux_core::plan_orchestration` — returning `None` unless it genuinely splits MULTI-AGENT. The planner still owns role classification, agent grounding (matched ONLY against the live roster), the step cap, and the dependency DAG; `prime_orchestrate` re-checks `is_multi_agent` at apply time. The brain proposes only the goal TEXT — it can never name an agent/role or fan out a goal the planner would not. |
| openclaw: **minting work is an explicit, gated capability — never from chat** (`tool-policy`) | the mapped intent is sensitive, so `reconcile_intent` keeps guarded chat deterministic; the kernel promotion is additionally gated on `!is_chat_guarded` (the deterministic classifier reads a guarded coordination question as `Orchestration`, so that extra guard is load-bearing here). A guarded turn keeps the deterministic clarify and creates nothing. |
| openclaw/Hermes: **balanced-JSON extraction, no raw-envelope leak; deterministic fallback always exists** | the args ride the unified `action_request`, lifted by `parse_adapter_result` FIRST; ANY failure (no brain, low confidence, unsplittable goal, unsupported field, off-allowlist name) leaves the deterministic outcome — a clarify or the keyword-sliced orchestration — in place. The provenance is the existing generic `requested tool: orchestration.create` chip (no new wire field). |

**What we deliberately do differently:** unlike Hermes (where the model runs the mutating tool), the
Relux brain runs NOTHING — `orchestration.create` is *desugared* into the existing `Orchestration`
intent + a validated goal slot, so every brief/task/assignment is still minted by the SAME
deterministic `plan_orchestration` → `prime_orchestrate` path behind the unchanged fail-closed gate.
The brain proposes only the goal text (with advisory step hints); it never authors a brief, names an
agent, picks a role, sets the order, or exceeds the cap — the deterministic planner owns all of that
and the multi-agent gate it can never bypass. Unlike the `task.create` write tool (sharpen-only),
`orchestration.create` may PROMOTE a single-clause clarify whose distinct steps the brain named into
a real orchestration — but ONLY on an explicit (`!is_chat_guarded`) request and ONLY when the
composed goal genuinely decomposes multi-agent through the deterministic planner. Risky work inside
an orchestration is unchanged: each brief is a normal task assigned to an agent and is RUN only by the
separate governed `run_orchestration` batch (no paid CLI is spawned at create time), so a protected
adapter/permission still gates at run time exactly as before. The mutating-tool surface stays small;
a multi-action orchestration loop and richer per-brief tools stay deferred.

---

## Reference read — a governed ORCHESTRATION RUN write tool + run-start memory (this slice)

The [`orchestration.create` slice](#reference-read--a-governed-orchestration-write-tool-this-slice)
let Prime mint a multi-agent orchestration, but the briefs sat `Planned` — the only way to RUN them
was the dashboard button, the blocking `/run` API, or the `prime orchestration run` CLI. A user who
asked Prime to "run the orchestration" got nothing actionable. This slice adds `orchestration.start`
to the same governed write allowlist, mapping it to the EXISTING `run_orchestration` batch, plus the
new `OrchestrationRun` intent/action and a resolvable run-start clarification the multi-turn memory
continues ("run the orchestration" → "which one?" → "orch_0001").

### Hermes — files read

- `reference/hermes-agent-main/agent/conversation_loop.py` `run_conversation(...)` (L3114-3162) — a
  SINGLE assistant message carries both the answer and the structured `tool_calls`, and the chosen
  tool name is **validated against `agent.valid_tool_names` BEFORE execution**; an off-list name is
  fed back, never run. **Pattern: one response carries the action; the tool name is allowlist-validated.**
  Mirrored: `orchestration.start` joins [`crate::prime_write_tools`] `WRITE_TOOLS`, and
  `classify_write_tool` is the name-allowlist gate — a made-up `orchestration.run`/`orchestration.cancel`
  is refused at parse time; the validated slot is `WriteToolSlot::RunOrchestration(BrainRunOrchestration)`.
- `reference/hermes-agent-main/model_tools.py` `coerce_tool_args` (L535-616) +
  `agent/message_sanitization.py` — coerce/sanitize each model-produced string. Mirrored in
  `parse_run_orchestration` (the required `orchestration_id` is sanitized + length-clamped; a missing
  or empty id fails the request closed).

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/subagent-control.ts` `resolveControlledSubagentTarget`
  (L707-729) + `src/auto-reply/reply/subagents-utils.ts` `resolveSubagentTargetFromRuns` (the
  `isActive` runnability filter) — a control action lands ONLY on a target that **EXISTS and is
  runnable**; an unknown/inactive target is an error, never coerced. **Pattern: act only on a target
  that both exists AND is in a runnable state.** Mirrored: `KernelState::runnable_orchestration_id`
  honors an id ONLY when it names an EXISTING orchestration with at least one PENDING brief; an
  unknown id, or one whose briefs are all terminal, fails closed (an honest reply, never a faked run).
- `reference/openclaw-main/src/agents/tool-policy.ts`
  `applyOwnerOnlyToolPolicy` / `wrapOwnerOnlyToolExecution` — running work is ONE explicit, GATED
  capability, never inferred from chat. **Pattern: minting/running work is an explicit capability,
  never auto-run from casual chat.** Mirrored: `OrchestrationRun` is `is_sensitive_intent`, so
  `reconcile_intent` keeps guarded chat deterministic; the deterministic classifier itself routes a
  guarded "should we run the orchestration?" to `Brainstorming` (the conversation guard runs BEFORE
  the run-verb check), so no extra `!is_chat_guarded` rail is needed here (unlike `orchestration.create`).
- `reference/openclaw-main/src/agents/tool-mutation.ts` `isMutatingToolCall` — a single fail-closed
  classifier defaulting an unknown action to *mutating*. Informs treating the run as an explicit
  mutating action validated before execution.
- `reference/openclaw-main/src/agents/bash-tools.exec-approval-followup-state.ts`
  (`consumeExecApprovalFollowupRuntimeHandoff`, TTL + consume-and-clear) +
  `.exec-approval-followup.ts` (`sendExecApprovalFollowup` continue-by-fresh-turn) — the
  consume-and-continue shape the clarify memory already mirrors; recording an `OrchestrationRun`
  clarify is now legitimate because the continuation has a real by-id action to resolve into
  (`is_resolvable_clarify_intent` gains `OrchestrationRun`, `clarify_needs_label` → `"orchestration id"`).

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| Hermes: **one response carries the action; the tool name is allowlist-validated** | `orchestration.start` joins `prime_write_tools::WRITE_TOOLS` (intent `OrchestrationRun`, `gated:false`); `classify_write_tool` admits ONLY allowlisted names; `parse_run_orchestration` requires a non-empty `orchestration_id` (fail closed otherwise). |
| openclaw: **act only on a target that EXISTS and is runnable** (`resolveControlledSubagentTarget`) | `KernelState::runnable_orchestration_id` resolves a candidate id ONLY to an existing orchestration with a pending brief; `prime_execute`'s `RunOrchestration` arm re-checks existence and runs the EXISTING `run_orchestration` batch; an unknown id is an honest, action-free reply. |
| openclaw: **running work is one explicit, gated capability — never from chat** (`tool-policy`) | the mapped intent is sensitive (`is_sensitive_intent`), so `reconcile_intent` keeps guarded chat deterministic; a question ("should we run it?") is routed to `Brainstorming` by the conversation guard before the run-verb classifier, so casual chat can never start a batch. |
| openclaw: **consume-and-continue only when a real action backs it** (`exec-approval-followup`) | `is_resolvable_clarify_intent` includes `OrchestrationRun`, so "run the orchestration" → "which one?" → "orch_0001" continues into a `RunOrchestration` `Act`; the bare-id follow-up reclassifies to `OrchestrationRun` (the combined message carries the verb + the id). |
| openclaw/Hermes: **no raw-envelope leak; deterministic fallback always exists** | the `orchestration.start` args ride the unified `action_request`, lifted by `parse_adapter_result` FIRST on the CLI path; ANY failure (no brain, unknown id, off-allowlist name, missing id) leaves the deterministic outcome — a clarify or an honest reply — in place. Provenance is the existing generic `🛠 requested tool: orchestration.start` chip (no new wire field, no dashboard change). |

**What we deliberately do differently:** unlike Hermes (where the model runs the tool), the Relux brain
runs NOTHING — `orchestration.start` is *desugared* into the existing `OrchestrationRun` intent + a
validated id slot, so the batch is always run by the SAME governed `run_orchestration` engine the
blocking `/run` API and the CLI use, behind the unchanged fail-closed gate. The run is mapped to the
**synchronous** `run_orchestration` (bounded by the blocking endpoint's own defaults — max 25,
concurrency 2), the existing governed path for the CLI/blocking-API surfaces; the dashboard's
non-blocking background **job** (`run-async` + `drive_orchestration_job`) stays the polling-optimized
server path and is unchanged. Each brief still gates at run time through its assigned agent's adapter
(a disabled/unconfigured runtime or a missing permission is recorded `blocked`, never faked), so
`orchestration.start` adds no new run-time authority — it only lets Prime *start* a batch the user
explicitly asked to run. The run turn's reply is the real, grounded batch result, so it is kept
deterministic (excluded from `prime_after_action`, like a tool result), and the brain can never
re-narrate (and overclaim) a per-brief outcome. A multi-action orchestration loop, a `run-async`
(non-blocking) Prime path, and per-brief retry/cancel tools stay deferred.

---

## Reference read — safe in-UI tool configuration for a metadata-only wrapper (this slice)

A source installed without a `relux-plugin.json` is scaffolded as a **metadata-only wrapper**
that declares ZERO tools (`crate::plugin_install::scaffold_manifest`). That is safe but useless:
with no tool definitions, even an enabled HTTP loopback runtime surfaces nothing
(`crates/relux-kernel/src/server.rs` `enabling_a_runtime_on_a_wrapper_surfaces_no_tools`). The only
prior way to add tools was to hand-edit the on-disk manifest and re-install. This slice adds the first
safe **in-UI** path: the operator describes ONE tool and the kernel validates it hard before it enters
the manifest (`docs/RELUX_MASTER_PLAN.md` §7.4 Plugin Kernel Layer, §8.2 ToolSet Plugins).

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/tools/update-plan-tool.ts` — `readPlanSteps` (L39-74): validate a
  structured payload **field-by-field**, check an enum (`status`) against the `PLAN_STEP_STATUSES`
  **allowlist** (L9), and fail closed on a bad value. The canonical "validate a structured payload
  against a schema + an allowlist" shape.
- `reference/openclaw-main/src/agents/tools/sessions-spawn-tool.ts` —
  `UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS` (L46-55), rejected **before any param is read**; the
  `Math.max(0, Math.floor(...))` numeric clamp (L355); the default-the-rest pattern (L302). **Pattern:
  reject unsupported keys up front, require/trim the mandatory string, clamp ranges, default the rest.**
- `reference/openclaw-main/src/agents/tools/common.ts` — `readStringParam` required-throws (L91-122) +
  `ToolInputError` (L57-64): a required string THROWS on bad input rather than coercing silently.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| openclaw: **validate field-by-field against a schema + an enum allowlist** (`readPlanSteps`, `PLAN_STEP_STATUSES`) | `crates/relux-kernel/src/plugin_tool_config.rs` `parse_plugin_tool_input` accepts ONLY `ALLOWED_KEYS` (`name`/`description`/`risk`/`auto_approve`/`timeout_secs`) and validates `risk` against `RISK_LEVELS` (the `RiskLevel` allowlist); anything else fails closed. |
| openclaw: **reject unsupported keys, require/trim the mandatory string, clamp** (`sessions-spawn-tool`, `common.ts`) | any other key fails the whole payload closed (a smuggled raw `permission`/`approval` cannot bypass the derived-permission / risk-driven-approval rules); `name` is required and sanitized to a safe dotted id; the timeout is coerced + clamped to `[1, 300]`s; description is control-char-stripped and length-clamped. |
| openclaw: **a required field THROWS, never coerces silently** (`readStringParam` required) | an empty/missing `name`, a non-allowlist `risk`, or a non-numeric `timeout_secs` is a hard, operator-facing error (the form never silently does the wrong thing). |
| openclaw: **act only on a target that is in the right state** (the bundled/protected refusals across the tool surface) | `KernelState::configure_plugin_tool` refuses a plugin that is not INSTALLED, a BUNDLED/protected fixture, or a non-`ToolSet`; the manifest is mutated transactionally on a clone and re-validated with `relux_core::validate_manifest` before it stands. |

**What we deliberately do differently — and the honesty fix it forced:** the operator **never** supplies a
raw permission; the kernel DERIVES it as `tool:<plugin-id>:<verb>`, so a configured tool can only ever gate
on this plugin's own `tool:` namespace. The mission's "a newly configured tool remains disabled / requires
explicit enable if risk is not low" required a *risk-sensitive, load-bearing* gate — but the manifest's
`approval` field was, until this slice, **decorative** (never enforced at tool execution). We made it
load-bearing: `relux_core::approval_blocks_direct_invocation(approval, risk)` is the single fail-closed
predicate behind both a new `ToolExecutability::NeedsApproval` discovery status and a refusal in
`call_tool`/`invoke_tool` (a non-low-risk tool is `Required` → never runnable just because a loopback
runtime is enabled; a low-risk tool is auto-approved only when the operator opts in). All bundled fixtures
declare `approval: never`, so this changes none of their behavior (verified by the unchanged suite). The
loopback **runtime** stays the separate, explicit run-enabling step, and Relux still never infers a tool or
runs downloaded code — the operator authors the tool, points it at a local server they run, and only then
can it run.

---

## Reference read — honest readiness for the tool-invocation UI (this slice)

The backend invocation path was already complete and tested: the HTTP **loopback runtime**
(`crates/relux-kernel/src/runtime.rs`, bounded/loopback-only/JSON-in-out), `state.rs`
`call_tool`/`invoke_tool` (permission gate → approval gate → runtime, all audited), the
`/v1/relux/tools/invoke` endpoint, and the approval refusal made load-bearing in the prior slice
(`approval_blocks_direct_invocation` → `ToolExecutability::NeedsApproval`). The one remaining gap was
the **UI**: a `ready` tool got an inline invoke form, but every non-runnable tool showed only a terse
"not callable" plus a hover tooltip — not the "clear disabled/refusal state with a reason" the product
bar requires. This slice closes that with a single readiness classifier and an honest inline panel; no
backend behavior changes (`docs/RELUX_MASTER_PLAN.md` §7.4 Plugin Kernel Layer, §8.2 ToolSet Plugins).

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/acp/approval-classifier.ts` — `classifyAcpToolApproval` (L186-225): **ONE
  classifier** maps a tool to a named `AcpApprovalClass`
  (`readonly_scoped`/`readonly_search`/`mutating`/`exec_capable`/`control_plane`/…) and an `autoApprove`
  boolean; **only the safe read classes return `autoApprove: true`** — every other class is non-auto with
  an explicit named class, never a blank/auto path (`EXEC_CAPABLE_TOOL_IDS` / `CONTROL_PLANE_TOOL_IDS`,
  L15-23, force a non-auto class). `normalizeToolName` (L57-63) lowercases + length-bounds + accepts only a
  strict `^[a-z0-9._-]+$` shape. **Pattern: one function, a named class per state, only the safe class is
  runnable — the unsafe states are surfaced with their honest class, never hidden or auto-run.**
- `reference/openclaw-main/src/agents/cli-output.ts` (`parseCliOutput`) — re-confirmed: surface only the
  parsed result, never a raw envelope. The invoke result panel renders `result.output` (the parsed tool
  output), not the wire envelope.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| openclaw: **one classifier → named class, only the safe class auto-runs** (`classifyAcpToolApproval`) | `apps/dashboard/src/plugins.ts` `toolReadiness(t)` maps the kernel's six `executable` states to `{ runnable, label, tone, reason, nextStep }`; `runnable` is true **only** for `ready` — every other state is non-runnable with a concrete reason + next step. `isRunnableTool` now delegates to it (single source of truth). |
| openclaw: **the unsafe states are SURFACED with their class, never hidden/auto-run** | `ToolRow` (`apps/dashboard/src/pages/Plugins.tsx`) renders a `ready` tool's invoke form OR, for any non-ready tool, an inline **"Why not?"** `ToolNotRunnable` panel stating the refusal/disabled reason + next step — never a blank "not callable", never a pretend run. The same refusal the kernel enforces in `call_tool`/`invoke_tool`, rendered honestly. |
| openclaw: **strict, bounded normalization before acting** (`normalizeToolName`) | unchanged from the prior slice — the kernel derives/sanitizes the `tool:<id>:<verb>` permission and validates the loopback URL; the UI only displays what the kernel already validated. |

**What we deliberately do differently:** this is a UI-only, no-backend-change slice — the kernel stays
authoritative (it refuses the same states), and `toolReadiness` is a pure, React-free helper so
`node --test` pins every state without a DOM (`apps/dashboard/test/plugins.test.ts`). The Tools surface is
inline on the Plugins page (no separate route), so a non-ready tool never opens a blank page. The honest
limit is recorded, not papered over: a `needs_approval` tool has no per-call approval flow yet, so it is
shown as blocked with the only real next step (reconfigure as low-risk), never silently run.

---

## Reference read — per-tool-call approval flow (this slice)

The honest-readiness slice above recorded the last real gap: a `needs_approval` tool
was honestly blocked on the direct invoke path, but there was **no per-tool-call
approval flow** — the only way to run a gated tool was to reconfigure it as
low-risk. This slice closes that gap. An operator can now request approval for ONE
specific invocation (tool id + exact arguments), an approver decides it on the
Approvals page, and the approved call executes **exactly once** through the same
runtime — without bypassing any gate (`docs/RELUX_MASTER_PLAN.md` §7.4 Plugin
Kernel Layer, §8.2 ToolSet Plugins).

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/bash-tools.exec-approval-request.ts` —
  `registerExecApprovalRequest` (L116-135) / `requestExecApprovalDecision`
  (L165-173): **two-phase registration**. The approval id is registered server-side
  *before* exec returns `approval-pending`, "otherwise `/approve` can race and
  orphan" (L119-120). The decision is then resolved and only afterwards does the
  action run. **Pattern: register the approval binding first; the action runs only
  after a decision resolves against that registered binding — never a privileged
  shortcut.**
- `reference/openclaw-main/src/agents/bash-tools.exec-approval-followup-state.ts` —
  `registerExecApprovalFollowupRuntimeHandoff` (L84-111) stores a small record keyed
  by an approval id with an explicit TTL; `consumeExecApprovalFollowupRuntimeHandoff`
  (L113-146) looks it up on a LATER turn, **requires every bound field to match**
  (`entry.approvalId !== approvalId || entry.idempotencyKey !== idempotencyKey ||
  entry.sessionKey !== sessionKey` → `undefined`, L137-143), checks it has not
  expired, and **`delete`s the entry after a single use** (L144). **Pattern: a
  pending record bound to the exact approval, matched on every field, consumed and
  cleared on a single use — it can never run twice.**
- `reference/openclaw-main/src/acp/approval-classifier.ts` — `classifyAcpToolApproval`
  (re-read): the gate decides auto vs. non-auto by class; a non-auto class is never
  promoted to auto. Reused as the boundary: only a `needs_approval` tool is eligible
  for the per-call request; a directly-runnable tool is refused (use invoke).

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| openclaw: **register the approval binding FIRST, before anything can run** (`registerExecApprovalRequest`, two-phase) | `crates/relux-kernel/src/state.rs` `request_tool_invocation_approval` creates the generic `Approval` (Pending, audited) AND a `PendingToolInvocation` binding `(plugin, tool, agent, args snapshot + SHA-256)` in one step; nothing executes. Endpoint `POST /v1/relux/tools/request-approval`. |
| openclaw: **record bound to the exact call, matched on EVERY field, consumed-and-cleared once** (`consumeExecApprovalFollowupRuntimeHandoff`) | `execute_approved_tool_invocation` runs only when the approval is `Approved` AND the binding is unconsumed; it re-validates the tool still exists, the subject still holds the permission, and the stored args still hash to the recorded SHA-256; it marks the binding `consumed` on a single attempt (success OR runtime failure) so it can never run again. Endpoint `POST /v1/relux/approvals/:id/execute`. |
| openclaw: **the action runs the stored context, not a fresh client-supplied one** | the stored args snapshot is executed verbatim — the execute endpoint takes only the approval id, never re-supplied args — so an approved call cannot be modified before it runs. |
| openclaw: **a non-auto class is never promoted to auto** (`classifyAcpToolApproval`) | only a `tool_needs_approval` tool is eligible: a directly-runnable (low-risk) tool is refused with `ToolDoesNotRequireApproval` ("invoke it instead"); the execute path bypasses the needs-approval gate ONLY because that is the granted approval, and still runs the full permission gate + audited runtime. |
| Hermes: **sanitize + clamp every operator-facing string** (`message_sanitization.py`) | the args snapshot is bounded to `MAX_TOOL_INVOCATION_ARGS_BYTES` (the loopback request cap), and the Approvals page renders only a bounded, secret-redacted preview (`redact_args_for_preview` masks `token`/`password`/`secret`/`authorization`/… values) — never the raw args; the raw snapshot is stored solely so the approved call runs verbatim. |

**What we deliberately do differently:** the flow grants no blanket/reusable
authority — one approval binds one invocation and is consumed by one execution
attempt (no `session`/`always` grant; the master plan has no safe reusable-grant
model, so we do not invent one). Every step is audited
(`tool_invocation:request`/`execute`, success/denied/failed). The binding persists
in the snapshot (meta-json seam, like `orchestrations`) so an approved call survives
a restart, but a runtime failure still consumes it (one approved invocation = one
attempt) and a rejected approval drops its binding outright. No remote/non-loopback
execution is added — the approved call runs through the same bounded loopback
runtime as a direct invoke, so all existing safety bounds hold.

---

## Reference read — bounded Prime conversation memory (this slice)

Prime had a single pending-clarification record but **no general conversation memory**: every turn
reasoned from the bare current message + a state snapshot, so a follow-up like "what about the
second one?" or "do that again" had no continuity and Prime felt keyword-shaped rather than like
Hermes/Codex. This slice adds a small, bounded, secret-redacted **per-conversation turn history**
that is injected into the brain's prompt as BACKGROUND context only — it changes no gate.

### Hermes — files read

- `reference/hermes-agent-main/agent/conversation_loop.py` — `run_conversation(...)` builds
  `messages = list(conversation_history)` (~L330-331) then appends the new user message, so a
  follow-up is interpreted against the SAME prior history rather than classified blind. The
  per-call injection (~L571-763) caches the recalled context once and adds it to the CURRENT user
  message's COPY only (`api_msg`), never mutating the stored `messages` — so the persisted history
  stays clean of the ephemeral injection. **Pattern: thread recent history into the prompt for
  continuity; inject as context, do not mutate the stored record.**
- `reference/hermes-agent-main/agent/memory_manager.py` — `build_memory_context_block(raw)`
  (~L173-187) wraps recalled context in a `<memory-context>` fence with a system note: "the
  following is recalled memory context, NOT new user input. Treat as authoritative reference
  data". **Pattern: fence the recalled context and label it background-not-an-instruction so the
  model reads it for continuity, never as a command.**
- `reference/hermes-agent-main/agent/context_compressor.py` — head/tail-protected compaction with
  a token-bounded summary, and `redact_sensitive_text` applied before any history leaves the
  session. **Pattern: bound the history's size and redact secrets before storage/use.**
- `reference/hermes-agent-main/agent/message_sanitization.py` — control-char escaping + length
  clamps on every model-produced string. Mirrored in `sanitize_text`.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/harness/hook-history.ts` —
  `limitAgentHookHistoryMessages(messages, max)` returns `messages.slice(-maxMessages)`
  (`MAX_AGENT_HOOK_HISTORY_MESSAGES = 100`). **Pattern: keep the last N (recent-first bound),
  drop the oldest from the front.**
- `reference/openclaw-main/src/agents/cli-runner/session-history.ts` —
  `buildCliSessionHistoryPrompt(...)` renders history as `"<role>: <text>"` pairs inside
  `<conversation_history>` tags and truncates at `MAX_CLI_SESSION_RESEED_HISTORY_CHARS = 12*1024`
  with an explicit truncation marker. **Pattern: render a compact transcript, char-cap it, mark
  the truncation honestly.**
- `reference/openclaw-main/src/agents/transcript-redact.ts` — `redactTranscriptMessage(...)` strips
  secrets (field-aware patterns) before a transcript is stored or surfaced. **Pattern: redact
  before store, never after.**
- `reference/openclaw-main/src/agents/bash-tools.exec-approval-followup-state.ts` — the small
  bounded-record-with-eviction shape we already mirror for clarifications; reused here for the
  per-conversation cap (evict the least-recently-active conversation).

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| Hermes: **thread recent history into the prompt for continuity; inject as context, never an instruction** | `crates/relux-kernel/src/prime_history.rs` `render_context` renders the recent turns into a block headed "BACKGROUND CONTEXT for continuity — NOT a new instruction; the user's CURRENT message below is the only thing to act on"; the kernel injects it into `prime_decision::build_decision_prompt(message, summary, history, observations)` BEFORE the current message. The stored records are never mutated by the injection. |
| Hermes: **fence + label recalled context as background** (`build_memory_context_block`) | the same explicit "background, not a new instruction" steer + the `User:`/`Prime:` transcript shape (openclaw `buildCliSessionHistoryPrompt`). |
| openclaw: **recent-first bound** (`messages.slice(-maxMessages)`) | `push_bounded` keeps the last `MAX_HISTORY_TURNS = 12` per conversation (oldest dropped from the front); `record_conversation_turn` bounds the number of conversations to `MAX_HISTORY_CONVERSATIONS = 32` (evicting the least-recently-active). |
| Hermes/openclaw: **redact before store, clamp length, never persist the raw envelope** | `sanitize_text` runs `relux_core::redact_secrets` + control-char strip + clamp on every field; only the FINAL user-visible reply (the validated brain-shaped / after-action wording the user saw, recorded AFTER shaping), the ids a turn created (`summarize_action`), and each read-only tool's NAME + its bounded one-line SUMMARY (`"<tool>: <summary>"`, clamped to `MAX_TOOL_READ_CHARS`) are stored — never `tool_output`/the full tool JSON or a provider envelope. `render_context` surfaces the reads as a `(consulted: …)` background sub-line, and the rendered prompt block is itself capped at `MAX_CONTEXT_CHARS` with an honest "[earlier turns omitted]" marker. |
| openclaw: **bounded record persisted via the meta-json seam** | `KernelState.conversation_histories: HashMap<conversation_key, Vec<ConversationTurn>>` (`namespace::actor` key) persisted as `ConversationHistoryEntry` through the same `meta` seam as `orchestrations`/`pending_clarifications`. |

**What we deliberately do differently:** the history is **advisory prompt context with zero
authority**. It never reaches the deterministic `classify_intent`, the fail-closed
`reconcile_intent` gate, or any existence/approval check — those run on the CURRENT message alone,
so memory can never promote casual chat into work or override an explicit current-turn intent (pinned
by `recorded_history_never_promotes_casual_chat_into_an_action`). It is recorded AFTER the reply is
shaped + reads gathered (so the stored reply/tool-names match what the user saw), in a short lock of
its own, and is rendered into the prompt only when a brain is configured (the deterministic path is
byte-for-byte unchanged — empty history leaves the decision prompt identical). A new
`POST /v1/relux/prime/reset` drops only this advisory memory (history + any pending clarification);
no durable entity is touched.

---

## Reference read — in-app first-run / operational readiness guide (this slice)

The dashboard had grown many capabilities (Prime brain selection, Claude/Codex
adapters, crew, plugins/tools, approvals) but the operator still had to read
scattered docs to learn how to configure a brain, enable an adapter, add crew,
configure a plugin, and start the first work. The ad-hoc first-run checklist on
Relux Home (`getFirstRunChecklist`) was a static list of counts that nagged with
"todo" even for things that are optional (no agents, no tasks) and lacked the
real readiness signals (real-work adapter, tool/wrapper config state, the single
clear first action). This slice replaces it with a derived, honest readiness
report and a compact, app-like guide.

This is a **dashboard/state-read** slice (it reads existing GET endpoints and
mutates nothing), but the readiness *computation* — turning live booleans into an
honest pass/warn/fail without a faked green check — is exactly the doctor/health
shape the reference systems encode, so it is grounded in them.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/apps/macos/Sources/OpenClaw/HealthStore.swift` —
  `HealthSnapshot` (Codable per-channel `configured` / `linked` / `probe.ok`) and
  the derived `var state: HealthState` (L197-213): a priority cascade that maps
  the live snapshot to `.ok` / `.linkingNeeded` / `.degraded(reason)` / `.unknown`,
  plus `summaryLine` / `detailLine` for a one-line human status. **Pattern: derive a
  small honest state enum from live booleans in priority order, and carry a concrete
  human reason for every non-ok state — never a bare "down".**
- `reference/openclaw-main/apps/ios/Sources/Onboarding/OnboardingStateStore.swift` —
  `shouldPresentOnLaunch(...)`: onboarding shows only when there is no prior/working
  connection (`gatewayServerName == nil`). **Pattern: a first-run surface keys off
  real configuration state, and steps aside once the core path works** — mirrored by
  the "operational" mode that replaces the checklist with a concise summary once
  nothing blocks.
- `reference/openclaw-main/apps/android/.../ui/OnboardingFlow.kt` — `StepRail`
  (L992-1028) renders each step complete/active/future from real state, and
  `canFinishOnboarding(isConnected, isNodeConnected)` (L956-959) gates "done" on
  actual connectivity. **Pattern: a step's completeness is a function of live state,
  and the terminal "ready" is a real predicate, not a click count.**

### Hermes — files read

- `reference/hermes-agent-main/hermes_cli/status.py` — `show_status(args)` (L90-571)
  and the `check_mark(ok)` / `redact_key` helpers: a hierarchical status display where
  every line is a concrete check (model configured, provider resolved, each API key
  set or "(not set)", each adapter logged-in/expired) rendered ✓/✗ with secret-free
  detail. **Pattern: one honest check per capability, the exact configured/missing
  fact, secrets redacted.**
- `reference/hermes-agent-main/hermes_cli/doctor.py` — `run_doctor(args)` (L337+) with
  `check_ok` / `check_warn` / `check_fail` / `check_info` and `_fail_and_issue(...,
  fix, issues)`: a deeper diagnostic that classifies each finding into pass / warn /
  fail / info and attaches the concrete remediation ("Run: hermes auth add …").
  **Pattern: three honest severities (a "warn" for installed-but-unconfigured is
  distinct from a "fail"), each carrying its exact next step.**
- `reference/hermes-agent-main/tui_gateway/server.py` `@method("setup.status")` →
  `{"provider_configured": bool(...)}`: a lightweight readiness RPC the UI calls to
  decide its flow. **Pattern: readiness is a cheap derivation over already-known
  config, not a fresh deep probe per render** — mirrored by deriving the whole report
  from the four reads Home already makes.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| openclaw `HealthStore`: **derive a small honest state from live booleans in priority order, with a concrete reason per non-ok state** | `apps/dashboard/src/readiness.ts` `buildReadiness(inputs)` composes one report from the live reads (`state`, `ai/status`, `adapters`, `plugins`, `tools`); each item carries a `status` (`done`/`todo`/`warn`/`link`/`info`) and an honest `description` + the exact `linkTo` page that fixes it. |
| Hermes `doctor`: **three severities — warn (installed-but-unconfigured) ≠ fail** | a SELECTED-but-broken brain (OpenRouter w/o key, Claude CLI off PATH/disabled) is the only **blocker** (`todo`); a local brain WORKS (`link`, a recommendation); a metadata-only wrapper or a tool needing a loopback runtime is **attention** (`warn`), surfaced but never blocking. Reuses the already-tested `onboarding.ts::primeBrainStep` and `plugins.ts::pluginCategory`/`toolReadiness` so the surfaces never disagree. |
| openclaw onboarding: **step aside once the core path works** | `ready = blockers.length === 0`; `apps/dashboard/src/components/ReadinessGuide.tsx` then shows a concise one-line operational summary + the first action (checks tucked behind a native `<details>`), instead of the full nag. |
| Hermes `status`: **one honest check per capability, secrets redacted** | items cover brain, real-work adapter (Claude/Codex enabled+on-PATH), crew (else the honest "Prime is your built-in operative" local fallback), plugins/tools (configured/needs-runtime/needs-approval/wrapper-needs-config), and pending approvals; the summary is secret-free (brain *label* only, never a key). |
| Hermes `setup.status`: **cheap derivation, not a per-render deep probe** | the report is a pure function of the four GET reads the page already makes; no new endpoint was added. `deriveFirstAction(state)` returns the single clearest next step in priority order (pending approval → active run → start/assign a task → ask Prime). |

**What we deliberately do differently:** the guide is **read-only** — it derives and
displays, mutating nothing, and every "fix" is a `<Link>` to an existing page
(`/health` for the brain, `/crew` for adapters/crew, `/plugins` for tools, `/work`
and `/prime` for work). It never fabricates a green check: an unreachable tools
probe stays an honest `info` ("tool readiness unavailable"), not "no tools
configured". The pure derivation lives in `readiness.ts` (React-free, like
`routing`/`onboarding`/`plugins`) so `node --test` pins all four required states
(`test/readiness.test.ts`) and a render test (`test/readiness-render.test.mjs`)
proves Home mounts and the committed bundle carries the copy.

---

## Reference read — manual Crew create/edit configuration (this slice)

The brain-assisted agent-slot path could already *propose* a created operative's
name/id/role/adapter/persona (`prime_agent_slots`), but the MANUAL surface lagged: the
Crew page offered only a two-field name+role create with no persona, no adapter choice,
and **no edit at all** — for a product where Prime hires/uses crew, an operator could not
configure crew directly. This slice adds a safe, usable manual create/edit workflow that
reuses the same validation discipline the brain path already adopted, so the two surfaces
agree on what a valid agent config is.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/tools/sessions-spawn-tool.ts` (the closest analogue
  to "configure a new worker from an operator request) — `UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS`
  is checked and rejected (L277-284) **before any param is read**; `readStringParam(params,
  "task", { required: true })` (L285) requires the mandatory string; the optional fields
  default-the-rest (`cleanup === "keep" | "delete" ? … : "keep"`, L302-303). **Pattern: reject
  unsupported keys up front, require the mandatory string, default/clamp the rest.**
- `reference/openclaw-main/src/acp/approval-classifier.ts` `normalizeToolName` (L57-63) —
  a subject is lowercased, length-bounded (`> 128 ⇒ undefined`), and accepted only against a
  strict `^[a-z0-9._-]+$` shape. **Pattern: normalize an id/subject to a strict, bounded shape
  before it is honored.**
- `reference/openclaw-main/src/agents/tools/common.ts` `readStringParam` / `ToolInputError`
  (L57-122) — typed extraction that *throws* on bad input rather than coercing silently.

### Hermes — files read

- `reference/hermes-agent-main/agent/message_sanitization.py` —
  `_escape_invalid_chars_in_json_strings` (L143-182) and the tool-error length clamp
  (`_sanitize_tool_error`, L515-528): **sanitize control chars and CLAMP length on every
  operator-/model-produced string.** Mirrored in the agent-config sanitizers.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| openclaw: **reject unsupported keys, require the mandatory string, default the rest** (`sessions-spawn-tool`) | `crates/relux-kernel/src/agent_config.rs` `validate_new_agent` requires a non-empty `name` (`NameRequired`), defaults the adapter to the local Prime adapter when absent, and clamps role/persona; the HTTP request structs accept ONLY the known fields (serde drops the rest). |
| openclaw: **normalize an id to a strict, bounded shape** (`normalizeToolName`) | `normalize_agent_id` lowercases, keeps only `[a-z0-9-]` (separators collapse to one hyphen), trims, and clamps to `MAX_AGENT_ID_CHARS`; an id that normalizes to empty fails (`IdInvalid`). |
| openclaw: **act only on a target that EXISTS** (approval cross-check) | a chosen adapter is honored ONLY when it resolves to an EXISTING roster id from `kernel.adapter_runtime_status()` (case-insensitive, canonical case preserved); an unknown adapter is rejected (`UnknownAdapter`). Id AND display-name uniqueness are checked against the live roster (`DuplicateId`/`DuplicateName`). |
| Hermes: **sanitize control chars + clamp length** (`message_sanitization`) | `sanitize_line`/`sanitize_block` strip control chars, collapse whitespace, and clamp; the persona is additionally run through `relux_core::redact::redact_secrets` so a pasted `sk-ant-…`/`ghp_…`/`key=…` is masked before storage — never persisted verbatim. |
| openclaw: **a settable field is checked against an allowlist** (status/cleanup defaults) | `resolve_status` honors ONLY `{active, paused, disabled}` (operator-settable); the machine-driven `Error` and unknown values are rejected (`InvalidStatus`), so an edit can never forge a lifecycle state. |

**What we deliberately do differently:** this is the MANUAL path, so validation lives at the
HTTP boundary (pure functions in `agent_config.rs`, the kernel hands them the live rosters) and
the result flows through `KernelState::create_agent` / the new `KernelState::update_agent` under
the kernel lock — the brain-seeded create path is **untouched** (it still calls `create_agent`
with its own already-validated slots). Edit is field-granular ("absent ⇒ unchanged"; an empty
`persona` is a deliberate clear) and `update_agent` re-checks the two invariants the kernel owns
(the agent exists; a new adapter is an installed plugin), so a stale/forged patch can never edit a
non-existent agent or point one at a non-adapter plugin. The persona is the only free-text durable
field and it is bounded + secret-redacted; nothing here grants new capability (permission grants
stay on the explicit, approval-gated path).

---

## Reference read — Crew governance: explicit-permission view + safe revoke (this slice)

The manual Crew create/edit slice above deliberately stopped at identity/role/persona/adapter/
status and recorded permission/budget/skills governance as future work. This slice takes the
smallest safe next step of the §9 Permissions panel: **surface each crew member's explicit
permissions and let the operator revoke one.** A *grant* path already existed
(`KernelState::grant_permission_to_agent` + `POST /v1/relux/agents/:id/permissions`); the gap was
that the card showed only a count and there was **no revoke** — so a capability, once granted,
could never be taken back from the console. Skills/tags and budget stay future work (the core
`Agent` has neither field; adding one is more than a minimal slice and §9.1 already defers them).

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/acp/approval-classifier.ts` — `EXEC_CAPABLE_TOOL_IDS` /
  `CONTROL_PLANE_TOOL_IDS` (L15-23) are explicit allowlists that map a subject to a risk **class**
  (`exec_capable` / `control_plane`), and an elevated class forces `autoApprove = false`;
  `normalizeToolName` (L57-63) lowercases, length-bounds, and accepts only a strict
  `^[a-z0-9._-]+$` subject (else `undefined`). **Pattern: classify a capability against an explicit
  control-plane/exec allowlist and never auto-approve an elevated one; normalize the subject to a
  strict id shape first.**
- `reference/openclaw-main/src/agents/tool-policy.ts` — `applyOwnerOnlyToolPolicy` /
  `resolveOwnerOnlyToolApprovalClass` (L18-59): a control-plane capability is one explicit, gated
  thing, added or refused deliberately — never inferred. **Pattern: granting/removing a
  control-plane capability is an explicit, deliberate act.**

### Hermes — files read

- `reference/hermes-agent-main/agent/agent_runtime_helpers.py` `repair_tool_call` (L1566-1636) —
  a model-chosen name is normalized then matched against the KNOWN set, and only a member of that
  set is honored. Mirrors validating a permission string against the canonical prefix allowlist
  before it is sent (client) and again in the kernel (server) — a value off the allowlist is
  refused, never coerced.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| openclaw: **classify a capability against an explicit control-plane/exec allowlist; never auto-approve an elevated one** | `apps/dashboard/src/governance.ts` `ELEVATED_PREFIXES` (`adapter:`/`provider:`/`exec:`/`plugin:`/`agent:`/`approval:`) → `permissionRisk` returns `"elevated"`, and the Crew form requires an explicit `window.confirm` before granting one; `tool:`/`task:`/`audit:` are `"standard"`. This is a UI caution, not an enforcement boundary (the kernel still audits and enforces least privilege). |
| openclaw: **normalize a subject to a strict id shape before acting** (`normalizeToolName`) | `governance.ts` `isValidPermission` / `permissionInvalidReason` reject a permission that does not start with a canonical prefix (mirrored from `relux-core` `VALID_PREFIXES`) BEFORE the API call; `relux_core::Permission::new` re-validates server-side (honest 400 on a bad string). |
| openclaw: **granting/removing a control-plane capability is an explicit, deliberate, audited act** | `KernelState::revoke_permission_from_agent` is the inverse of the existing grant: it removes only an EXPLICIT permission, records an `agent:revoke_permission` audit, and fails closed (`PermissionNotGranted` → 404) when the agent does not hold it — never a silent no-op. `DELETE /v1/relux/agents/:id/permissions` exposes it; the operator console is the human approval (the same gate as clicking the button). |
| Hermes: **honor only a value in the known set** (`repair_tool_call`) | the create/edit form never auto-grants — `create_agent` still grants only the minimal `tool:relux-tools-echo:say`; every other capability is an explicit, warned operator grant. |

**What we deliberately do differently:** revoke is a direct, audited **operator** action (the
human at the console is the approval), not a Prime `Propose` — Prime's own `GrantPermission` stays
approval-gated as before; this is the operator governing their own crew. Revoke can only remove an
explicit grant (least privilege means there are no implicit capabilities to reach), so an agent's
effective power always equals exactly its listed permissions. Skills/tags and per-agent budget
remain future work: the `Agent` model has neither field, and inventing unenforced budget UI or a
core-struct skills field is outside a minimal, safe slice.

---

## Reference read — model-backed crew skills/tags + skill-aware assignment matching (this slice)

The two Crew slices above deferred **skills/tags** ("the `Agent` model has neither field"). This
slice adds a bounded specialty-tag list to the core `Agent`, persisted and backwards-compatible,
and uses it in **assignment matching only** — routing work to a unique specialist, asking when a
skill is shared, never guessing. Skills are *specialty*, not *power*: they never gate a capability
(that stays the explicit, audited permission path).

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/auto-reply/reply/subagents-utils.ts` `resolveSubagentTargetFromRuns`
  (L44-145) — the canonical fuzzy-target resolver: exact alias → exact label → alias-prefix →
  label-prefix → runId-prefix, where a tier with exactly one match RESOLVES, a tier with more than
  one is an **ambiguity error**, and no match is `unknownTarget`; the resolved entry is always an
  EXISTING run. **Pattern: ordered tiers, unique-resolves / multiple-is-ambiguous / none-fails, and
  resolve only to a target that exists.** Mirrored: the skill tier is inserted into
  `resolve_assignee`'s ordered tiers (after exact id/name, before the looser prefix/substring),
  resolving only when exactly one roster agent holds the skill, returning `Ambiguous` for a shared
  skill, and always yielding a real roster id.
- `reference/openclaw-main/src/agents/tools/sessions-spawn-tool.ts` —
  `UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS` rejected before any param is read (L46-55, L277-284),
  `readStringParam(..., { required: true })`, and the numeric clamp `Math.max(0, Math.floor(...))`
  (L355). **Pattern: reject unsupported up front, require the mandatory string, clamp the rest.**
  Mirrored in `agent_config::validate_skills`: each entry is slugified + length-clamped, the list
  is count-bounded, a content-but-unsanitizable entry is rejected (`InvalidSkill`), and overflow is
  rejected (`TooManySkills`).
- `reference/openclaw-main/src/acp/approval-classifier.ts` `normalizeToolName` (L57-63) — lowercase,
  length-bound, accept only a strict `^[a-z0-9._-]+$` shape (else `undefined`). Mirrored:
  `sanitize_skill` reduces an entry to the strict `[a-z0-9-]` slug shape (reusing the
  `normalize_agent_id` discipline) before it is stored or matched.

### Hermes — files read

- `reference/hermes-agent-main/agent/agent_runtime_helpers.py` `repair_tool_call` (L1566-1636) —
  normalize/strip the candidate, then match against the KNOWN set in priority order, resolving only
  to a member of that set. Mirrored: a skill phrase is normalized (stopwords dropped, slugified) and
  matched against the live roster's skill map; a `Resolved` id is always taken verbatim from the
  roster — a skill phrase can never invent an assignee.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| openclaw: **ordered tiers; unique resolves, multiple is ambiguous, none fails; resolve only to an existing target** (`resolveSubagentTargetFromRuns`) | `crates/relux-kernel/src/prime.rs` `resolve_assignee` inserts a **skill tier** after exact id/name and before prefix/substring: exactly one agent tagged with a candidate slug → `Resolved`; more than one → `Ambiguous` (the `AssignTask` decide arm asks "which one?"); none → fall through. The id is taken verbatim from `summary.all_agent_ids` (fail closed). |
| openclaw: **reject unsupported, require, clamp** (`sessions-spawn-tool`) + **strict id shape** (`normalizeToolName`) | `agent_config::{sanitize_skill,validate_skills}`: strict slug, `MAX_SKILL_CHARS`/`MAX_SKILLS` clamps, dedup, `InvalidSkill`/`TooManySkills` honest 400s. Create ⇒ absent is none; edit ⇒ present replaces the whole list (empty clears). |
| Hermes: **normalize then match the known set, resolve only to a member** (`repair_tool_call`) | the skill candidates are the same normalized tokens `resolve_assignee` already builds; an exact id/name match still wins before the skill tier is consulted, so a skill never overrides a direct reference. |
| openclaw: **work/capability is an explicit gated thing, never inferred** (`tool-policy`) | a skill is matched for *routing only* — it grants **no** capability. `relux_core::Agent.skills` is `#[serde(default)]` so existing stored agents load unchanged, and `StateSummary.agent_skills` is a read-only grounding projection the brain never authors. |

**What we deliberately do differently:** this is a deterministic change with no brain in the loop —
it is the fallback the brain-assisted assignment slot already reconciles against (`prime_assign_slots`
/ `prime_update_slots` call the same `resolve_assignee`, now skill-aware), so the safety shape
(resolve only to an existing agent; ambiguity asked, not guessed) holds whether or not a brain is
configured. Skills are validated/sanitized identically on the manual HTTP path and would be on any
future brain-proposed path, and they never widen authority: an agent's effective power still equals
exactly its explicit permissions.

---

## Reference read — safe role-preset bundles for Crew create (this slice)

The last remaining §9.1 Crew gap: **role-preset bundles**. An operator should be able to spin up a
common crew type (researcher, builder, reviewer, planner, operator) without retyping role/persona/
skills each time — but a convenience template must **never** become a backdoor that auto-grants
capability. This slice adds a curated preset list that *suggests* role/persona/skills only, expands
through the existing `agent_config` validators, and grants nothing.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/agents/tools/sessions-spawn-tool.ts` — the closest analogue to
  "spin up a worker of a named kind". When a spawn names a role it becomes a pure context label
  (`const roleContext = requestedAgentId ? { role: requestedAgentId } : {}`, ~L323) attached to the
  reply; the role **never expands the worker's toolset** — capability is governed *separately* by the
  inherited tool allow/deny list. The same file rejects `UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS` before
  any param is read (L46-55, L277-284) and **defaults an unknown enum to a safe fixed value**
  (`params.cleanup === "keep" || params.cleanup === "delete" ? … : "keep"`, ~L301). **Pattern: a role
  is descriptive metadata, not a grant; an unknown selector is rejected/defaulted, a known one expands
  to a fixed shape.**
- `reference/openclaw-main/src/acp/approval-classifier.ts` `normalizeToolName` (L57-63) — lowercase +
  length-bound + accept only a strict id shape, else `undefined`. **Pattern: resolve a selector by a
  strict, case-insensitive id against a fixed allowlist.**

### Hermes — files read

- `reference/hermes-agent-main/agent/system_prompt.py` / `agent/prompt_builder.py` — a persona/role
  steers the model's **system prompt** (operating style) on one axis; the available **tools**
  (capability) are configured on a *separate* axis. **Pattern: persona ≠ permission.** Mirrored: a
  preset contributes a persona/role/skills bundle (the "how it operates" axis) and touches the
  permission grant on no axis at all.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| openclaw: **a named role is descriptive metadata, never a capability grant** (`sessions-spawn-tool` `roleContext`) | `crates/relux-kernel/src/agent_presets.rs` `AgentPreset` holds ONLY `{id,label,summary,role,persona,skills}` — there is no permission/adapter field, so a preset *cannot* widen power by construction. `create_agent` still grants only `tool:relux-tools-echo:say` regardless of preset. |
| openclaw: **resolve a selector by strict id against a fixed allowlist; unknown is rejected** (`normalizeToolName`, `UNSUPPORTED_*` keys) | `find_agent_preset` resolves the id case-insensitively/trimmed against the fixed `AGENT_PRESETS`; an unknown id is an honest `400 unknown preset '…'` (fail closed), never an invented bundle. |
| openclaw/Hermes: **expand to a fixed shape, then validate through the ONE existing path** | the optional `preset` field on `POST /v1/relux/agents` fills only the role/persona/skills the request omitted (the request's own value always wins) and the **merged** input flows through the unchanged `validate_new_agent` — no duplicate validation; a unit test asserts every curated preset passes those validators and its skills survive `validate_skills` unchanged. |
| Hermes: **persona ≠ permission** (system-prompt axis vs. toolset axis) | the preset persona is applied through the same bounded + secret-redacted `agent_config::sanitize_persona` as a hand-typed persona; the UI fills the (still editable) fields and submits the normal create — the operator reviews every field before save. |

**What we deliberately do differently:** the backend is the single source of truth (a read-only
`GET /v1/relux/agent-presets`), and the dashboard *fills the form* from it rather than sending a
`preset` field — so the field a user actually submits is the same validated role/persona/skills any
manual create uses, and Apply confirms before overwriting in-progress edits. The `preset` field on
create is kept for API clients and exercised by a server test, but it is advisory in exactly the same
way: it can only ever produce a config the operator could have typed by hand, and it grants nothing.
Presets are offered in **create** mode only — they seed a new member, they never silently reshape an
existing one.

---

## Reference read — read-only operator Doctor report (this slice)

The Home/Health readiness guide turns the live `/v1/relux` reads into an honest pass/warn/fail
checklist *in the frontend*, but there was no deeper kernel-side diagnostic the operator could run.
This slice adds a single, cheap, **read-only** `GET /v1/relux/doctor` endpoint: the kernel itself
reports structured severity rows (with a message + remediation + in-app action link) from the SAME
inexpensive reads `/v1/relux/health` already makes — no heavy work, no mutation, no path/secret leak.

### Hermes — files read

- `reference/hermes-agent-main/hermes_cli/doctor.py` — `check_ok` / `check_warn` / `check_fail` /
  `check_info` (L185-194) each emit one severity row with a `text` + `detail` string, and
  `_fail_and_issue(text, detail, fix, issues)` (L204-207) pairs a failure with a concrete `fix`
  remediation appended to an issues list. `run_doctor` (L337+) walks a fixed sequence of cheap
  environment/config probes and tallies them. **Pattern: a doctor is an ordered set of cheap checks,
  each a {severity, message} row, and a failure carries a concrete fix — not free-form prose.**
- `reference/hermes-agent-main/hermes_cli/doctor.py` `_PROVIDER_ENV_HINTS` (L33-57) /
  `_has_provider_env_config` — provider readiness is decided by whether auth is *configured*, not by
  making a live call where a cheap signal suffices. Mirrors our "configured?" brain check.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/gateway/server/health-state.ts` — `buildGatewaySnapshot({ includeSensitive })`
  (L21-54): the default snapshot omits resolved filesystem paths; `configPath` / `stateDir` /
  `authMode` are attached **only** when `includeSensitive === true` (an admin caller that already has
  broader access). `refreshGatewayHealthSnapshot` caches the health summary rather than recomputing it
  per request. **Pattern: a status surface defaults to NO resolved paths, and the diagnostic is a
  cheap cached/derived read, never heavy work per call.**

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| Hermes: **a doctor is an ordered set of cheap {severity, message} rows** (`check_*`) | `crates/relux-kernel/src/doctor.rs` `build_doctor_report` returns a fixed-order `Vec<DoctorCheck>` (store → bundle → brain → real-work → tools → crew → approvals), each with a `DoctorSeverity` (`ok`/`info`/`warn`/`fail`) + secret-free `message`. |
| Hermes: **a failure carries a concrete fix** (`_fail_and_issue`) | every non-ok row carries an optional `remediation` string + an in-app `action_link` (`/health`, `/crew`, `/plugins`, `/approvals`) — the dashboard's equivalent of the `fix`. |
| openclaw: **default snapshot omits resolved paths; sensitive only for admins** (`includeSensitive`) | stricter, unconditional: `DoctorInputs` carries NO path at all (booleans/counts/states only), so a db path or resolved binary path can NEVER reach a check message. A redaction test feeds a path-shaped adapter `resolved_path`/`command` and asserts it is absent from the serialized report. |
| openclaw: **cheap derived/cached read, never heavy work** (`refreshGatewayHealthSnapshot`) | `get_doctor` reuses the existing `/v1/relux/health` reads (one serialized store load + already-computed adapter/tool status); no cargo build/test, no network, no mutation. A store open/load failure still returns an honest failing `kernel.store` row instead of a 500. |
| `readiness.ts` parity (no two-surfaces disagreement) | the severity rules mirror the frontend guide exactly: selected-but-broken brain = fail; local brain = healthy info; real-work adapter = optional info/ok; tool needing a runtime = warn. |

**What we deliberately do differently:** the Doctor is **read-only and authoritative on the kernel
side** — it computes severities in Rust from real state and the frontend renders them verbatim
(`apps/dashboard/src/doctor.ts` only maps severity→badge and sorts rows). This is the inverse of the
readiness guide (which derives status in the browser): the guide stays the first-run teacher; the
Doctor is the deeper "what's broken, how to fix it" the operator runs on demand. It never executes a
remediation — every `action_link` is a navigation to the page that fixes it, behind the existing
session gate; nothing here mutates state or runs a tool.

## Reference read — bounded self-correction on a malformed brain decision (this slice)

Chosen from the deep audit (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §1/§7) as the highest-impact safe
agentic-loop gap: the unified decision discarded a malformed-but-correctable brain reply exactly like a
hard provider failure (`parse_decision(&text).ok()` → `None` → fall back), with no re-ask.

### Hermes — files read

- `reference/hermes-agent-main/agent/conversation_loop.py` — the ReAct loop's invalid-output recovery:
  `_invalid_tool_retries` / `_invalid_json_retries` counters inject an explicit recovery tool-result
  message describing what was wrong and re-call the model up to 3× before giving up (the loop does NOT
  silently drop a fixable reply); fuzzy tool-name repair runs BEFORE erroring. The error is fed back as
  context so the model corrects itself rather than the harness crashing.
- `reference/hermes-agent-main/model_tools.py` — `coerce_tool_args` + `_sanitize_tool_error`: the
  recovery message is sanitized/bounded so the framing tokens never leak.

### openclaw — files read

- `reference/openclaw-main/src/agents/pi-embedded-runner/run.ts` — `resolvePlanningOnlyRetryInstruction`,
  `reasoningOnlyRetryAttempts`, `emptyResponseRetryAttempts`, `COMPACTION_CONTINUATION_RETRY_INSTRUCTION`:
  the loop re-prompts with a corrective *instruction* (bounded attempt counters) instead of aborting on
  a degenerate response. A malformed/empty round becomes a bounded corrective re-ask.

### How Relux maps it

- The kernel now distinguishes the two failure modes (`relux-kernel/src/prime_decision.rs`
  `DecisionOutcome::{Malformed(err), ProviderError}`): a reply `parse_decision` rejected is correctable;
  no usable reply at all is not. `DecisionLoop::step_outcome` + `DecisionStep::Retry` re-ask ONCE
  (`MAX_DECISION_CORRECTIONS = 1`), injecting `parse_decision`'s OWN error via
  `build_decision_prompt_with_correction`, before falling back. Both transports surface the distinction
  (`ai::decide_prime_via_openrouter`, kernel `classify_cli_decision`); the async driver
  `decide_prime_with_observation` threads the correction string.
- **Deliberate difference from Hermes:** the Relux brain executes NOTHING. The correction asks only for a
  valid output format; the corrected decision still flows through the unchanged fail-closed gate
  (`reconcile_intent` → slot validators → `decide` → `prime_execute` / approval). The correction message
  is kernel-authored (not user content), so it cannot carry an instruction. Total brain calls stay bounded
  (`MAX_DECISION_ROUNDS + MAX_DECISION_CORRECTIONS`); a provider failure is never retried. Worst case is
  byte-for-byte today's behavior. See the applied-change record in `docs/prime-processing-audit.md`.

---

## Reference read — structured run-failure classifier + bounded transient retry (this slice)

The audit's next P1 (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §7, gap #2): Relux's only
recovery affordance was `prime.retry_run` — a fresh run with NO error taxonomy and NO
backoff. A failed run recorded a free-text `error` string and nothing else, so the
dashboard/Prime/Doctor could not tell a safe-to-retry transient (a provider rate limit)
from a failure that needs an operator (a missing adapter, an auth error). This slice adds a
structured `RunFailureClass`, a bounded transient-retry policy on the Paperclip
`[2m,10m,30m,2h]` schedule, and an honest "retry-ready" state (no faked background timer).

### Hermes — files read

- `reference/hermes-agent-main/agent/error_classifier.py` — `FailoverReason` (enum:
  `auth`/`auth_permanent`/`billing`/`rate_limit`/`overloaded`/`server_error`/`timeout`/
  `context_overflow`/`model_not_found`/`format_error`/…/`unknown`), `ClassifiedError`
  (carries `retryable`/`should_compress`/`should_rotate_credential`/`should_fallback`), and
  `classify_api_error(...)` — a **priority-ordered** pipeline (provider-specific patterns →
  HTTP status → error code → message patterns → transport heuristics → unknown). **Pattern:
  a closed enum of failure reasons + a priority-ordered, pattern-driven classifier whose
  result carries an explicit `retryable` decision; auth/billing/format are NOT retryable,
  rate-limit/overloaded/server-error/timeout ARE.** Crucially, auth patterns set
  `retryable=False` ("the credential is invalid and retrying with the same key will always
  fail").
- `reference/hermes-agent-main/agent/message_sanitization.py` — `_sanitize_tool_error`
  (L515-528, 2000-char clamp) and `_extract_message` (500-char clamp): **CLAMP every
  model/provider-produced string before it is surfaced.** Mirrored in
  `safe_public_message` (redact + single-line + clamp to `MAX_PUBLIC_MESSAGE_CHARS`).

### Paperclip (openclaw) — files read

- `references/paperclip/server/src/services/run-liveness.ts` — `classifyRunActionability`
  (→ `runnable`/`manager_review`/`blocked_external`/`approval_required`/`unknown`) and
  `classifyRunLiveness` (→ `RunLivenessState` `failed`/`completed`/`blocked`/`empty_response`/
  `advanced`/`plan_only`/`needs_followup`, each with a human `livenessReason`). **Pattern:
  classify a finished run into a small closed set + a human reason, and decide auto-continue
  ONLY for the safe class** (`runnable`); a blocker/approval/manager-review is surfaced, not
  auto-continued.
- `references/paperclip/server/src/services/heartbeat.ts` —
  `BOUNDED_TRANSIENT_HEARTBEAT_RETRY_DELAYS_MS = [2m,10m,30m,2h]` (L221-226),
  `BOUNDED_TRANSIENT_HEARTBEAT_RETRY_MAX_ATTEMPTS = delays.length` (L230),
  `computeBoundedTransientHeartbeatRetrySchedule(attempt, now, random)` (L481-499, returns
  `{attempt, baseDelayMs, delayMs, dueAt, maxAttempts}`), and `readHeartbeatRunErrorFamily`
  (L257-268, only `codex_transient_upstream`/`claude_transient_upstream` → the
  `transient_upstream` family retries). **Pattern: ONLY the transient family retries, on that
  exact bounded schedule, capped at the schedule length, with a `dueAt` not-before instant.**

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| Hermes: **closed enum of failure reasons + priority-ordered, pattern-driven classifier with a `retryable` decision** | `crates/relux-core/src/run_failure.rs` `RunFailureClass` (`transient_provider`/`auth_required`/`adapter_missing`/`permission_denied`/`invalid_prompt`/`timeout`/`cancelled`/`output_validation`/`unknown`) + `classify_failure(reason, timed_out)` — priority-ordered (timeout → cancelled → permission → auth → adapter-missing → transient → invalid → output-validation → unknown), matched against the kernel's own reason strings. `retryable()` / `needs_operator_action()` / `remediation()` are the recovery hints. |
| Hermes: **auth/format are NEVER retried (same input fails the same way)** | only `TransientProvider` + `Timeout` are `retryable()`. **Stricter than Hermes:** because a Relux run can MUTATE a workspace, the `unknown` bucket is NON-retryable here (Hermes retries it) — it waits for a deliberate manual retry. |
| Paperclip: **only the transient family retries, bounded `[2m,10m,30m,2h]`, capped at the schedule length** | `RETRY_BACKOFF_SECS = [120,600,1800,7200]`, `MAX_TRANSIENT_RETRIES = 4`, `retry_delay_secs(attempt)`, and `RunRetryState::plan(class, attempt, now_secs)` — schedules `not_before_secs = now + backoff[attempt]` for a retryable class while the budget remains, marks `exhausted` once `attempt` reaches the cap, and returns `None` (no retry block) for every non-retryable class. The attempt index is the run's `retried_from` lineage depth (`transient_attempt_for`). |
| Paperclip: **a `dueAt` not-before instant** | `not_before_secs` is a REAL unix-second instant (kernel `real_now_secs()`, the same wall-clock seam `auth.rs` uses for session expiry) — the only honest representation of a real backoff. The retry-state math stays pure (takes `now_secs`); only one kernel read touches the OS clock. |
| Paperclip: **classify into a small set; auto-continue ONLY the safe class** | `KernelState::transient_retry_ready(now_secs)` is the read-only "retry-ready" projection (newest run per task, failed + retryable class + `eligible_at(now)` + still assigned). It is consumed by the MANUAL `retry_run` or the next autonomy tick — there is **no background scheduler** (the audit's explicit honesty constraint). `one_autonomy_tick` re-attempts eligible transients through the unchanged governed `retry_run` path, bounded by the per-tick cap. |
| Hermes: **clamp + redact every surfaced string** | `safe_public_message` redacts known secret shapes (`redact_secrets`), collapses to one line, and clamps to 500 chars. The run's recorded `error` is already secret-redacted upstream; the class + static `remediation()` never echo the provider envelope. |

**What we deliberately do differently:** the classifier is a pure, deterministic projection
— it grants NO authority and changes no control flow except to mark a transient failure
retry-ready. A retry, whether manual or autonomy-driven, flows through the UNCHANGED governed
run path (`retry_run` → `execute_assigned_run`, re-checking enabled runtime, binary on PATH,
and the permission gate, and stamping `retried_from` so the backoff grows attempt-by-attempt
and exhausts). Auto-retry is restricted to the two unambiguously-safe, upstream-caused classes
(`TransientProvider`, `Timeout`); every other failure — including the `unknown` catch-all —
surfaces a remediation and waits for an operator. There is no faked timer: eligibility is a
real-wall-clock not-before checked only when an operator/cron invokes a tick.

## Reference read — session identity / handoff / resume (this slice)

The audit's §3 P1: Relux had durable agents but **no session/handoff/resume concept beyond a
cold fresh-run retry**. A finished run recorded its output + metrics but threw away the
provider's session id, so there was no handoff record and no way to *continue* a session
(only to start over). This slice captures + persists the adapter session identity, exposes it
on the run detail, and adds a real, governed `run.resume` for the one adapter that supports
safe non-interactive resume (Claude CLI) — refusing honestly everywhere else (no faked resume).

### openclaw — files read

- `reference/openclaw-main/src/agents/acp-spawn.ts` — `resumeSessionId` param;
  `validateAcpResumeSessionOwnership(...)` (a resume is allowed ONLY for a session previously
  recorded for this requester — ownership-gated); `sessionEntryMatchesAcpResumeSessionId`
  (match by `agentSessionId`/`acpxSessionId`); `spawnAcp` modes `"run" | "session"`.
  **Pattern: a session id is a first-class, ownership-checked handle; resume threads it back
  through the same spawn, and is refused when it cannot be validated.**
- `reference/openclaw-main/src/agents/command/attempt-execution.ts` —
  `getCliSessionBinding(sessionEntry, "claude-cli")?.sessionId` (a **per-provider CLI session
  binding**); `runCliWithSession(nextCliSessionId, activeCliSessionBinding)` (threads the
  session id into the CLI invocation); `claudeCliSessionTranscriptHasContent({ sessionId })`
  → **reset to a fresh session when the recorded transcript is missing** rather than fake a
  resume; `FailoverError.reason === "session_expired"` is treated as non-retryable-with-same-
  session. **Pattern: capture the provider session id, resume by passing it back through the
  unchanged gate, and degrade honestly (reset → fresh) when the session is gone.**

### Hermes — files read

- `reference/hermes-agent-main/tools/delegate_tool.py` — `DELEGATE_BLOCKED_TOOLS`, `MAX_DEPTH=1`,
  synchronous subagents **cancelled on interrupt with work discarded**. **Confirms the contrast:**
  Hermes subagents are explicitly *non-durable* and have no resume — so Relux's durable-run +
  captured-session model is the right place to add resume, not a delegate clone.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| openclaw: **per-provider CLI session binding (`getCliSessionBinding(...).sessionId`)** | `relux_core::RunSession { adapter_session_id, source, resume_supported }` captured from the Claude `--output-format json` envelope's `session_id` (`parse_adapter_result` → `AdapterResultSummary.session_id` → `RunSession::from_envelope`), stamped on the `Run` by `KernelState::set_run_session`. Bounded + redacted: `sanitize_session_id` (argv-safe charset, leading-dash rejected, `MAX_SESSION_ID_LEN`). We store ONLY id + source + capability — never a raw envelope/token/log. |
| openclaw: **resume threads the session id back through the same spawn (`runCliWithSession`)** | `build_resume_adapter_args(kind, session_id)` = the unchanged safe Claude argv (`-p --permission-mode default --output-format json`) **plus** `--resume <session_id>`; `prepare_cli_run` threads it ONLY when the run's `resumed_from` lineage is set, so a cold run never gets `--resume`. `resume_run` runs through the SAME governed gate (enabled runtime + PATH probe + permission check + bounded, non-bypass spawn) as a normal run. |
| openclaw: **resume is refused when it cannot be validated/owned** | `relux_core::plan_resume(session, terminal) → ResumeDisposition` (pure, the single source of truth): `Supported` only for a terminal run with a `resume_supported` session; otherwise `NotSupported { reason }` → `KernelError::RunResumeNotSupported` (HTTP 422). `AdapterKind::resume_supported()` is the honest per-kind capability — only the Claude CLI. |
| openclaw: **degrade honestly (reset → fresh) when the session is gone** | Relux does not pre-validate the upstream session (no equivalent of `claudeCliSessionTranscriptHasContent`); instead an invalid/expired `--resume` fails honestly through the existing `finalize_cli_run` classifier (`is_error`/non-zero exit → `OutputValidation`/`TransientProvider`), never a fabricated success. A fresh **retry** (§7) remains the distinct cold-start path. |
| openclaw: **session id is a first-class handle** | `Run.resumed_from` is a distinct lineage field (≠ `retried_from`); the run wire (flattened `RunRecord`) carries `session` + `resumed_from` + a derived `resumable`; the Work page shows a copyable Session id, an honest Handoff label, a Resume-of link, and a **Resume session** button (`canResumeRun`). |

**What we deliberately do differently:** we do not claim a process resume the adapter cannot
do. Only the Claude CLI (`-p --resume <id>`) is wired; Codex `exec` (no captured session id),
a generic `Command`, and the local echo path all return `ResumeNotSupported` with a specific
reason and the operator re-runs fresh. The capability flag, the UI label, and the action all
read from the single `plan_resume` decision, so they can never disagree. No new authority is
granted — resume reuses the unchanged argv-only, non-bypass, bounded, redacted adapter gate.

---

## Reference read — bounded conversation-memory compaction beyond the ring (this slice)

The audit's §6 P1: the bounded conversation memory (above) kept only the last `MAX_HISTORY_TURNS`
turns verbatim and **dropped everything older on the floor** — a long-running Prime thread simply
forgot its earlier work once the ring filled. This slice keeps the recent ring exactly as-is but,
when a turn ages OUT of the front, **folds it into a rolling, bounded, secret-redacted,
deterministic per-conversation summary** that is rendered at the TOP of the same BACKGROUND block,
before the recent turns. It adds no authority, no provider call, and no new gate.

### Hermes — files read

- `reference/hermes-agent-main/agent/context_compressor.py` — compaction kicks in past a window
  threshold: a head/tail-protected pruning pass plus a bounded summary of the middle, with
  `redact_sensitive_text` applied before the summary is produced and an anti-thrash counter so it
  does not re-compress every turn. **Pattern: keep the recent boundary verbatim, summarize the
  older middle into a bounded redacted digest, never re-summarize unboundedly.**
- `reference/hermes-agent-main/agent/memory_manager.py` — `build_memory_context_block` again: the
  summarized recall is fenced + labelled reference-not-instruction. **Pattern: the summary is
  injected with the same background framing as the verbatim turns.**

### openclaw — files read

- `reference/openclaw-main/src/context-engine/types.ts` — `CompactResult.result = { summary,
  firstKeptEntryId, tokensBefore, tokensAfter, ... }`: compaction yields a **summary string plus a
  "first kept entry" boundary** — everything before the boundary is represented by the summary, the
  kept entries stay verbatim. `AssembleResult` then prepends a `systemPromptAddition` (the summary)
  ahead of the kept messages. **Pattern: summary + kept-entries, summary first; the ring is the
  kept entries, the summary stands in for everything older.**

### Paperclip — files read

- `references/paperclip/server/src/services/issue-continuation-summary.ts` — a deterministic,
  char-bounded continuation summary: `ISSUE_CONTINUATION_SUMMARY_MAX_BODY_CHARS = 8_000`,
  per-section caps, `truncateText(value, max)` appends an explicit `\n[truncated]` marker, and the
  salient facts are extracted deterministically (paths, "waiting for review/approval"), NOT by an
  unbounded model call. **Pattern: deterministic extraction first, hard char bounds, honest
  truncation marker.**

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| openclaw: **summary + kept-entries (`CompactResult` `summary` + `firstKeptEntryId`); summary first in the assembled prompt** | `crates/relux-kernel/src/prime_history.rs`: `push_bounded` now RETURNS the turns evicted from the front (openclaw's "everything before `firstKeptEntryId`"); `record_conversation_turn` folds each into the conversation's `relux_core::ConversationSummary`. `render_context_with_summary(summary, ring)` renders the summary line at the TOP of the one BACKGROUND block, then the verbatim recent ring (`render_context` is now the `None`-summary case, so the recent-only path is byte-for-byte unchanged). |
| Hermes: **summarize the older middle into a bounded redacted digest** (`context_compressor`) | `fold_evicted_turn(summary, evicted, now)` is pure + deterministic: an *acting* turn contributes a redacted highlight (the ids it created, `action_summary`), a purely conversational turn contributes only to a count, and the first evicted turn seeds the conversation's `opened_with` anchor (set once). Every field re-runs through `sanitize_text` (`redact_secrets` + control-strip + clamp). |
| Paperclip: **deterministic extraction first, no unbounded model call** | The summary is built ENTIRELY deterministically from data already redacted on the `ConversationTurn` (ids + counts + the opening message). **No provider call** — folding happens under the kernel lock, so it must stay off-network; a brain-generated summary would be a strictly-additive, strictly-validated, off-lock overlay and is deliberately NOT introduced here. |
| Paperclip: **hard char bounds + honest truncation marker** | `MAX_SUMMARY_HIGHLIGHTS = 16` (oldest highlights dropped), per-highlight + `opened_with` clamps, and `render_summary` caps the rendered block at `MAX_SUMMARY_RENDER_CHARS = 600` with a `[summary truncated]` marker. |
| openclaw: **bounded record persisted via the meta-json seam** | `KernelState.conversation_summaries: HashMap<conversation_key, ConversationSummary>` persisted as `ConversationSummaryEntry` through the same `meta` seam as `conversation_histories`; the per-conversation cap evicts a stale conversation's summary alongside its ring, and `clear_conversation` drops it too. |

**What we deliberately do differently:** the summary is **advisory prompt context with zero
authority**, exactly like the ring it compacts. It never reaches `classify_intent`, the fail-closed
`reconcile_intent` gate, or any existence/approval check — the deterministic classifier + gate run
on the CURRENT message alone, so even a summary full of "created task_XXXX" highlights can never
promote a casual musing into work (pinned by
`a_summary_full_of_actions_still_never_promotes_casual_chat_into_work`). It stores only ids +
counts + the opening message — never a raw provider envelope, tool body, or secret — and the
empty-memory decision prompt is byte-for-byte unchanged (no summary + no ring → `""`). A
brain-generated summary is explicitly **out of scope** for this slice (deterministic-first; no new
unbounded call), and remains the documented next extension behind strict validation + a
deterministic fallback.

## Reference read — minimal scoped permission grants (this slice)

The audit's §5 P1 #4: Relux permissions were **exact-string match only** — to let an agent use ten
tools in one plugin an operator had to grant ten separate rows, and there was no way to express
"every tool in this plugin." This slice adds the **smallest safe scope** — a single per-plugin tool
wildcard `tool:<plugin-id>:*` — without touching who may *issue* a grant (still approval-gated) and
without inventing the larger subtree/project graph.

### openclaw — files read

- `reference/openclaw-main/src/acp/permission-relay.ts` — `GatewayExecApprovalDecision =
  "allow-once" | "allow-always" | "deny"`; `buildAcpPermissionOptions` emits exactly those options
  and `resolveGatewayDecisionFromPermissionOutcome` maps a selection back, with
  `FALLBACK_EXEC_APPROVAL_DECISIONS = ["allow-once", "deny"]` as the safe default. **Pattern: a grant
  has a *width* (one-shot vs standing) and the safe default is the narrow one; widening is an
  explicit, enumerated choice, never implicit.**
- `reference/openclaw-main/extensions/tlon/src/monitor/authorization.ts` —
  `resolveChannelAuthorization` resolves a rule to `{ mode: "restricted" | "open", allowedShips }`:
  an allowlist decides membership and the default `mode` is **`"restricted"`** when no rule matches.
  **Pattern: scope matching is an allowlist with a fail-closed default — absence of a matching rule
  denies, it never widens.**

### Paperclip — referenced (not vendored)

- Paperclip's `principal_permission_grants` `(principal, permissionKey, scope)` with `authorization.ts`
  `scopeAllows` + `agentIsInSubtree` is the richer target recorded in the audit's §5 from the original
  read. Its source is **not present** under `reference/`, so this slice deliberately takes only the
  self-containable half (a per-plugin tool scope) and leaves the subtree/project graph — which needs
  the §3 `reports_to` lattice — explicitly open. No feature was justified from the un-vendored source.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| openclaw: **the safe default is the narrowest grant; widening is explicit + enumerated** | `relux_core::Permission::new` accepts a `*` ONLY in the single enumerated shape `tool:<plugin-id>:*`; `*`, `tool:*`, `tool:*:*`, `agent:<id>:*`, and partial globs (`tool:p:cre*`) are all rejected fail-closed (`MalformedScope`). There is no global or namespace-wide widening — the broadest grant expressible is one plugin. |
| openclaw/tlon: **scope matching is an allowlist with a fail-closed default — no match denies** | A new `Permission::authorizes(required)` is the grant-vs-required comparison used at enforcement: true iff exact-equal OR (grant is `tool:<plugin>:*` AND required is a concrete `tool:<plugin>:<tool>` in the **same** plugin). Anything else — different plugin, prefix-of-a-plugin, non-`tool:` kind, wildcard-vs-wildcard, wildcard on the required side — returns false. The two enforcement reads (`agent_holds_permission`, the `start_run` `required_permissions` loop) switched from `matches_exact` to `authorizes`; every tool-invocation path already funnels through `agent_holds_permission`, so one edit covers invoke/approve/per-call-binding/Prime-turn. |
| openclaw: **a grant is a discrete record you can revoke** | Grant dedup and `revoke_permission_from_agent` still use `matches_exact`, so a scope is stored / shown / revoked as exactly one row and a revoke never pattern-expands into the tools it covered (revoking a concrete tool held only via a scope is an honest `PermissionNotGranted`). Pinned by `state.rs::scoped_wildcard_grant_authorizes_plugin_tools_and_revokes_exactly`. |

**What we deliberately do differently / leave out:** no agent-subtree / project / namespace scope
(the larger Paperclip half — deferred until the §3 `reports_to` graph exists), no `allow-always`
persistence beyond the existing standing grant row, no budgets. The scope is intentionally capped at
a single plugin so it can never become a back door to a global `*`. The UI mirrors the exact backend
grammar (`apps/dashboard/src/governance.ts`) so a malformed scope is rejected before the API, and the
Crew Governance panel badges scoped rows + explains the exact-vs-scope rule — no fake budget controls
were added.

## Reference read — the `reports_to` org-lattice / chain-of-command model (this slice)

Implements `HERMES_OPENCLAW_DEEP_AUDIT.md` §3 P2 (and unblocks the §5 subtree-scope half): the
durable **Lead** (`reports_to`) pointer + the pure subtree/chain helpers a future manager-subtree
scoped grant will read. The MODEL ships now; **no permission consults it yet** — enforcement is
unchanged this round.

### openclaw — files read

- `reference/openclaw-main/src/acp/session-lineage-meta.ts` — `toAcpSessionLineageMeta` resolves a
  child's **parent pointer** as `parentSessionId = parentSessionKey ?? spawnedBy`, a **bounded**
  non-negative integer `spawnDepth` (`readInteger` rejects negatives/non-integers), and
  `subagentControlScope: "children" | "none"` (a node's authority is its children subtree, or
  nothing). **Pattern: one parent pointer per node, a hard-bounded depth, and a node's reach is its
  own subtree — default narrow.**

### Hermes — files read

- `reference/hermes-agent-main/tools/delegate_tool.py` — `MAX_DEPTH = 1` ("flat by default: parent
  (0) → child (1); grandchild rejected unless `max_spawn_depth` raised"), per-record
  `{subagent_id, parent_id, depth, …}`, `_get_max_spawn_depth`. **Pattern: a parent/depth lineage
  with a small default depth cap that must be explicitly raised.**

### Paperclip — referenced (not vendored)

- Paperclip's `packages/db/src/schema/agents.ts` `reportsTo` org tree (indexed `(companyId,
  reportsTo)`) + `authorization.ts` `agentIsInSubtree` (a 50-depth upward walk) are summarized in the
  audit's §3/§5 from the original read. The source is **not present** under `reference/`, so only the
  bounded-walk *shape* (depth 50, walk up the `reportsTo` chain) was taken — never the scope
  enforcement, which stays explicitly future.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| **one parent pointer per node** (openclaw `parentSessionId`, Hermes `parent_id`, Paperclip `reportsTo`) | `relux_core::Agent` gains `reports_to: Option<AgentId>` (the **Lead**; internal id stays `reports_to` per the two-layer rule). `#[serde(default)]` ⇒ every pre-existing snapshot loads as top-level (backwards compatible). |
| **a hard-bounded ancestry walk** (Paperclip `agentIsInSubtree` 50-deep; Hermes/openclaw bounded depth) | `relux_core::hierarchy` — `chain_of_command`, `is_in_subtree` (proper-descendant), `would_create_cycle`, all bounded by `MAX_HIERARCHY_DEPTH = 50` with a repeat-guard, so every walk is **total even on a malformed/cyclic map**. |
| **a node's reach is its own subtree** (openclaw `subagentControlScope: "children"`) | `is_in_subtree(manager, child)` is the exact predicate a future manager-subtree grant will read. It is built + tested but **deliberately consulted by nothing** this round — enforcement (`agent_holds_permission`, `start_run`) is byte-for-byte unchanged. |
| **the graph is validated, not assumed** | Create/edit resolve the Lead against the live roster (`agent_config::resolve_manager` → exists + not-self); the kernel owns the graph invariant under its lock — a created leaf needs only existence+self, an **edit additionally rejects a cycle** (`would_create_cycle`). Honest `400`s throughout. |

**What we deliberately do differently / leave out:** the lattice is **display + validation only** this
round — it widens no permission and does not touch orchestration/assignment routing. A Lead may be a
paused/disabled operative (status ⊥ org structure; the edge grants no authority, so there is no safety
reason to forbid it — and any future scoped grant, not the edge, is where a disabled-Lead check would
live). The Crew picker excludes self + the operative's own Branch so an obvious cycle can't be chosen,
but the backend re-validates regardless (the client is convenience, never the authority). The
manager-subtree **scoped permission enforcement** — wiring `is_in_subtree` into a grant (Paperclip
`scopeAllows` + `agentIsInSubtree`) — is the explicitly-deferred next slice (shipped below).

---

## Reference read — the manager-subtree scoped permission grant (this slice)

The follow-up to the `reports_to` lattice: a strict `agent:<manager-id>:subtree:<action>` grant whose
*authority* widens at enforcement time, and only over the holder's **own Branch**, decided by the
bounded `is_in_subtree` walk. One real, narrow enforcement path consults it (a live manager granting a
permission to a subordinate). Audit ref: `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §19 (and §5/§18).

### openclaw — files read

- `reference/openclaw-main/src/acp/session-lineage-meta.ts` — `subagentControlScope: "children" | "none"`:
  a node's authority is its children subtree or nothing, **default narrow**. This is the discipline the
  grant follows — a manager-subtree grant reaches *down* its Branch, never sideways/up, and self is
  excluded (proper-descendant).
- `reference/openclaw-main/src/acp/permission-relay.ts` — `GatewayExecApprovalDecision`
  (`allow-once`/`allow-always`/`deny`), **deny-by-default**: authority is only what an explicit grant
  confers. The kernel chokepoint mirrors this: absent a matching subtree grant (or a non-live manager),
  the answer is deny.

### Hermes — files read

- `reference/hermes-agent-main/tools/delegate_tool.py` — `MAX_DEPTH`, per-record `parent_id`/`depth`,
  flat-by-default delegation: a parent-pointer chain walked under a hard depth bound. Relux's
  `is_in_subtree` is exactly that bounded walk (`MAX_HIERARCHY_DEPTH = 50`).

### Paperclip — referenced (not vendored)

- `principal_permission_grants` scope = `managerAgentId-subtree`, resolved by `authorization.ts`
  `scopeAllows` + `agentIsInSubtree`. **Not vendored** under `reference/` — only the *shape* (a per-grant
  subtree scope; membership decided by the bounded walk; manager-id concrete, never global) was taken,
  from the original audit read. No feature justified from the un-vendored source.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| **a per-grant subtree scope** (Paperclip `managerAgentId-subtree`) | `relux_core::Permission` accepts the strict `agent:<manager-id>:subtree:<action>` grant (`parse_agent_subtree`); `subtree` is a reserved keyword, every malformed variant is rejected fail-closed. No `*`, so no global form. |
| **scope membership decided by the bounded walk** (`scopeAllows` + `agentIsInSubtree`) | `relux_core::permission::manager_subtree_authorizes(grant, holder, action, target, reports_to)` = well-formed grant AND grant's manager == holder AND action matches AND `is_in_subtree(holder, target)`. Self/sibling/ancestor/unrelated all fail; total on a cyclic map. |
| **deny-by-default + a node reaches only its own subtree** (openclaw) | `KernelState::manager_grant_permission_to_subordinate` — the one real path: a manager grants a permission to a subordinate iff the kernel chokepoint `manager_subtree_authorizes` says yes. Layers a fail-closed **liveness** rule (only an `Active` manager wields authority). Denials audited; grants nothing on failure. |
| **the operator path is not widened** | Operator-console `grant_permission_to_agent`/`revoke` stay kernel actions with no actor gate; the manager-grant path is strictly *narrower* (own Branch, `grant_permission` action only, live only). Revoke still `matches_exact` — a subtree grant is one explicit, revocable row. |
| **a permission request is routed to a human authorizer** (openclaw `permission-relay.ts` — a request is relayed to a human who picks allow/deny; authority correlated to a real per-session `sessionKey`/`spawnedBy` in `session-lineage-meta.ts`) | `POST /v1/relux/agents/:id/manager-grant` → `manager_grant_permission_to_subordinate_as_operator(operator, …)`. Relux has no per-agent session identity yet, so the authenticated **operator** stands in for the manager: it supplies the request and is the named, audited authorizer (`operator:authorize_manager_grant` row), but the kernel still enforces the real own-Branch + Active + scope gate — the operator cannot widen what the manager could do. The UI affordance is gated by `governance.ts::managerGrantAvailability` (mirrors the gate; no fake availability). |

**What we deliberately do differently / leave out:** the enforcement primitive, model, **and an
operator-assisted HTTP/UI surface** (`POST /v1/relux/agents/:id/manager-grant` + the Crew "Grant as
manager" affordance) are now real and tested. The genuinely-remaining gap is a **per-agent-authenticated**
actor — a manager driving its own grant *without an operator in the loop* — since Relux has no per-agent
session identity the kernel trusts (openclaw's `sessionKey`/`spawnedBy` analogue). Only the
`grant_permission` action is enforced (assign_task / revoke and project/namespace scopes are future). The
disabled-manager decision is **explicit**: a non-`Active` manager wields no subtree authority
(fail-closed), even over a genuine subordinate — the place a "disabled-Lead can't act" rule lives, exactly
as foreshadowed by the §18 lattice slice.

## Reference read — the first per-agent identity / access-token primitive (this slice)

The follow-up that closes (for one action) the trust-boundary gap the manager-subtree slice left open: a
bounded, hashed-at-rest, revocable **per-agent access token** the local operator mints, so a manager can
authenticate its OWN request and drive the manager-grant path with **no operator in the loop**. Audit ref:
`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §19 (the "still missing: per-agent-authenticated actor" item).

### Paperclip (openclaw) — files read (VENDORED — `references/paperclip/`)

- `references/paperclip/server/src/agent-auth-jwt.ts` — `createLocalAgentJwt(agentId, …)` / `verifyLocalAgentJwt`:
  a per-agent credential whose subject (`sub`) is the agent id, with a bounded `exp`/`iat`, signed
  HMAC-SHA256 and verified with a **timing-safe** compare (`safeCompare` → `crypto.timingSafeEqual`)
  before a request is trusted. This is the per-agent-identity target Relux lacked.
- `references/paperclip/server/src/middleware/auth.ts` — on a valid token the middleware sets
  `req.actor = { type: "agent", agentId: claims.sub, source: "agent_jwt" }`, and **rejects a
  terminated/pending agent**. The request's acting identity comes from the verified token's subject, never
  the body — exactly the discipline Relux's agent-self routes follow.
- `references/paperclip/packages/db/src/schema/principal_permission_grants.ts` +
  `references/paperclip/server/src/services/authorization.ts` (`scopeAllows` + `agentIsInSubtree`) —
  confirm the authenticated agent actor is what the subtree scope is checked against. Relux already has the
  subtree gate (§19); this slice supplies the missing authenticated actor that drives it.

### openclaw — files read

- `reference/openclaw-main/src/acp/session-lineage-meta.ts` — `subagentControlScope: "children" | "none"`,
  **default narrow**: a node's authority is its children subtree or nothing. Relux's token is narrow the
  same way — it unlocks ONLY the agent-self route subset (`/v1/relux/agents/me*`), where the manager's
  reach is still bounded to its own Branch by the unchanged kernel gate; it never reaches the operator
  console.
- `reference/openclaw-main/src/acp/permission-relay.ts` — `allow-once`/`allow-always`/`deny`,
  **deny-by-default**. The agent-token middleware mirrors this: absent a valid token the request is a clean
  401; there is no `RELUX_AUTH_DISABLED` bypass on the agent surface (an agent's identity is meaningless
  without a real token).

### Relux local pattern reused

- `crates/relux-kernel/src/auth.rs` (the operator `SessionStore`) — the proven local discipline: store
  only the **SHA-256 hash** of a high-entropy opaque credential (never the raw value), persist atomically
  to a permission-restricted gitignored file, prune/revoke in place. The new `AgentTokenStore`
  (`crates/relux-kernel/src/agent_auth.rs`) reuses the exact same hashed-at-rest, file-backed shape and the
  shared `atomic_write_restricted`.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| **a per-agent credential whose subject is an agent id** (Paperclip JWT `claims.sub`) | `AgentTokenStore::mint(agent_id, …)` issues an opaque `relux_agt_<hex>` token bound to that agent; `authenticate(raw)` returns `AgentTokenIdentity { agent_id, token_id }`. Relux mints an **opaque hashed token, not a signed JWT** — there is no multi-tenant verifier to satisfy and a hashed-at-rest opaque token is simpler to revoke and impossible to forge from the stored file. |
| **timing-safe verify of a stored secret** (`safeCompare`) | The token is a 256-bit CSPRNG secret stored only as its SHA-256 hash, looked up by hash — preimage resistance makes it unforgeable; there is no low-entropy secret to time-attack (same rationale as `auth.rs` session ids). The raw token is shown **once** at mint and never persisted/returned. |
| **`req.actor = { type: "agent", agentId: claims.sub }`; identity from the token, not the body** (Paperclip middleware) | `require_agent_token` middleware validates the bearer token and inserts `AgentTokenIdentity` into the request extensions; `POST /v1/relux/agents/me/manager-grant` reads the acting manager from that identity, NEVER the body — a token can only ever act as itself. |
| **reject a terminated/pending agent; narrow scope** (Paperclip + openclaw control-scope) | The token grants no authority of its own: the manager-grant-as-self path still flows through the unchanged `manager_subtree_authorizes` gate (own-Branch + `Active` + scope). The route allowlist is tiny (`/agents/me`, `/agents/me/manager-grant`); an agent token is **never** accepted on an operator route (those only check the session cookie). |
| **per-agent grant lifecycle is auditable** | The new `agent:token_authenticated_manager_grant` audit row (token-actor provenance, public `token_id` only) sits on top of the existing agent-actor audit; operator mint/revoke are audited (`agent:mint_token` / `agent:revoke_token`). The raw token never appears in any audit/log, and the redactor masks the `relux_agt_` prefix defensively. |

**What we deliberately do differently / leave out:** Relux mints an **opaque hashed token, not a signed
JWT** (no issuer/audience/HMAC-secret machinery — there is no multi-tenant verifier, and a local hashed
token is simpler + revocable). The agent surface is intentionally a **two-route allowlist**; broadening it
(richer agent self-service, more subtree *actions* than `grant_permission`, an agent minting/rotating its
own token) is future work. The token is still **operator-minted** — there is no agent-driven enrollment /
bootstrap (an agent cannot mint its own first credential), which is the correct trust posture for a
local-first console. This is not an internet auth system: loopback-only, single local operator, no
JWT/OAuth.

## Reference read — a second manager-subtree action: token-authenticated `assign_task` (this slice)

The follow-up that exercises the per-agent-authenticated manager surface for a **second** subtree action
beyond `grant_permission`: a manager that authenticated its own request (the §20 token) may **assign an
existing task** to one of its own-Branch subordinates, with no operator in the loop. Audit ref:
`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §21 (and the §20 "more subtree *actions* than `grant_permission`"
open item).

### Paperclip — files read (VENDORED — `references/paperclip/`)

- `references/paperclip/server/src/services/authorization.ts` (`scopeAllows` + `agentIsInSubtree`) +
  `references/paperclip/packages/db/src/schema/principal_permission_grants.ts` — a manager's authority over
  its Branch is keyed **per `permissionKey`**, not a blanket power: the assignment capability is a distinct
  grant from the grant capability, resolved by the same subtree walk. This is the "second action" shape.
- `references/paperclip/server/src/middleware/auth.ts` — `req.actor = { type: "agent", agentId: claims.sub }`:
  the acting agent is the verified token subject, never the request body — the discipline Relux's
  `agent_self_assign_task` follows (manager id from `AgentTokenIdentity`, target/task from the body).

### openclaw — files read

- `reference/openclaw-main/src/acp/session-lineage-meta.ts` — `subagentControlScope: "children" | "none"`,
  default narrow: a node's authority is its children subtree or nothing. The `assign_task` scope is narrow
  the same way — own-Branch proper descendants only, the exact action only, Active manager only.
- `reference/openclaw-main/src/acp/permission-relay.ts` — deny-by-default: an unauthorized assignment is a
  clean 403 that mutates nothing; a malformed/missing/terminal target is rejected, never silently coerced.

### Relux files read / mapped

- `crates/relux-core/src/permission.rs` — `manager_subtree_authorizes(grant, holder, action, target,
  reports_to)` was **already action-generic**; an `agent:<id>:subtree:assign_task` scope parses/stores/
  revokes with no grammar change and authorizes only its own action.
- `crates/relux-core/src/task.rs` — `Task` / `TaskStatus`; the single-pointer `assigned_agent` model.
- `crates/relux-kernel/src/state.rs` — the `manager_subtree_authorizes` kernel chokepoint (Active-manager
  liveness), `assign_task` (sets `assigned_agent` → `Queued`, audits `task:assign`),
  `prime_update_slots::is_terminal_status`, and the §19/§20 grant primitives the new pair mirrors.
- `crates/relux-kernel/src/agent_auth.rs` — the per-agent `AgentTokenIdentity`.
- `crates/relux-kernel/src/server.rs` — the `agent_router` bearer allowlist + the §20
  `agent_self_manager_grant` handler shape; `crates/relux-kernel/src/lib.rs` — `KernelError`.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| **per-action subtree authority** (Paperclip `permissionKey` per grant) | The existing action-generic matcher needed NO change; `KernelState::manager_assign_task_to_subordinate` calls the same `manager_subtree_authorizes(manager, "assign_task", target)` chokepoint. A `…:grant_permission` scope never authorizes `assign_task` and vice-versa (pinned by `subtree_grant_action_is_exact_and_generic_over_the_action_name`). |
| **actor from the verified token subject** (Paperclip `claims.sub`) | `POST /v1/relux/agents/me/assign-task` reads the acting manager from `AgentTokenIdentity.agent_id`; the body carries only `task_id` + `target_agent_id`. `manager_assign_task_to_subordinate_as_agent` adds an `agent:token_authenticated_manager_assign_task` provenance row (public `token_ref` only). |
| **deny-by-default; reject invalid targets** (openclaw relay) | Authorization is checked **first** (an unauthorized manager never learns the task exists): no-scope / not-Active / out-of-Branch / unknown target → 403, nothing mutated. The one extra guard is **assignability**: a terminal task (`is_terminal_status`) is a 409 `TaskNotAssignable`; a missing task is the kernel's existing `UnknownTask` (400). |
| **narrow, auditable, revocable** | The scope is one explicit `agent:<id>:subtree:assign_task` capability row, granted/revoked through the unchanged operator governance path; the per-agent token surface stays a tiny allowlist that never opens an operator route (boundary check extended). |

**What we deliberately do differently / leave out:** no new permission grammar (the subtree scope was
already action-generic); no change to the operator/Prime assignment path (this is a strictly narrower
agent-authority path); assignment keeps the simple single-pointer model (re-point a live task; refuse a
terminal one); no dedicated manager-token *assignment* UI affordance this round (API + docs, scope
grantable through the existing governance form) rather than a faked control. More subtree actions
(`revoke`, …), project/namespace scopes, and agent-driven enrollment remain future work.

## Reference read — a third manager-subtree action: token-authenticated `revoke_permission` (this slice)

The follow-up that exercises the per-agent-authenticated manager surface for a **third** subtree action
beyond `grant_permission` and `assign_task`: a manager that authenticated its own request (the §20 token)
may **revoke an explicit permission** from one of its own-Branch subordinates, with no operator in the loop.
Audit ref: `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §22 (and the §21 "more subtree *actions*" open item).

### Paperclip — files read (VENDORED — `references/paperclip/`)

- `references/paperclip/server/src/services/authorization.ts` (`scopeAllows` + `agentIsInSubtree`) +
  `references/paperclip/packages/db/src/schema/principal_permission_grants.ts` — a manager's authority over
  its Branch is keyed **per `permissionKey`**: the revoke capability is a distinct grant from grant/assign,
  resolved by the same subtree walk. The "third action" reuses the identical shape with a new action segment.
- `references/paperclip/server/src/middleware/auth.ts` — `req.actor = { type: "agent", agentId: claims.sub }`:
  the acting agent is the verified token subject, never the body — Relux's `agent_self_manager_revoke` reads
  the manager id from `AgentTokenIdentity`, target/permission from the body.

### openclaw — files read

- `reference/openclaw-main/src/acp/session-lineage-meta.ts` — `subagentControlScope: "children" | "none"`:
  a node's authority is its children subtree or nothing; the `revoke_permission` scope is narrow the same way
  (own-Branch proper descendants only, the exact action only, Active manager only).
- `reference/openclaw-main/src/acp/permission-relay.ts` — deny-by-default: an unauthorized revoke is a clean
  403 that mutates nothing; authority is checked before the target's holdings are even consulted.

### Relux files read / mapped

- `crates/relux-core/src/permission.rs` — `manager_subtree_authorizes` is **action-generic**; an
  `agent:<id>:subtree:revoke_permission` scope parses/stores/revokes with no grammar change and authorizes
  only its own action. `Permission::matches_exact` is the exact-only revoke comparison (no pattern expansion).
- `crates/relux-kernel/src/state.rs` — the `manager_subtree_authorizes` kernel chokepoint (Active-manager
  liveness), the existing `revoke_permission_from_agent` primitive (audited `agent:revoke_permission`,
  fail-closed `PermissionNotGranted`), and the §19/§20/§21 manager primitives the new pair mirrors.
- `crates/relux-kernel/src/agent_auth.rs` — the per-agent `AgentTokenIdentity`.
- `crates/relux-kernel/src/server.rs` — the `agent_router` bearer allowlist + the §21 `agent_self_assign_task`
  handler shape; `crates/relux-kernel/src/lib.rs` — `KernelError` (`PermissionDenied`→403,
  `PermissionNotGranted`→404).

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| **per-action subtree authority** (Paperclip `permissionKey` per grant) | The action-generic matcher needed NO change; `KernelState::manager_revoke_permission_from_subordinate` calls the same `manager_subtree_authorizes(manager, "revoke_permission", target)` chokepoint. A `…:revoke_permission` scope never authorizes grant/assign and vice-versa. |
| **actor from the verified token subject** (Paperclip `claims.sub`) | `POST /v1/relux/agents/me/manager-revoke` reads the acting manager from `AgentTokenIdentity.agent_id`; the body carries only `target_id` + `permission`. `manager_revoke_permission_from_subordinate_as_agent` adds an `agent:token_authenticated_manager_revoke_permission` provenance row (public `token_ref` only). |
| **deny-by-default; reject invalid targets** (openclaw relay) | Authorization is checked **first** (an unauthorized manager never learns whether the target holds the permission): no-scope / not-Active / out-of-Branch / unknown target → 403, nothing mutated. The exact-only revoke means a permission not held → honest `PermissionNotGranted` → 404; a malformed permission → 400. |
| **narrow, auditable, revocable** | The scope is one explicit `agent:<id>:subtree:revoke_permission` capability row, granted/revoked through the unchanged operator governance path; the per-agent token surface stays a tiny allowlist that never opens an operator route (the new route is itself bearer-gated). |

**What we deliberately do differently / leave out:** no new permission grammar (the subtree scope is already
action-generic); no change to the operator-console revoke (`DELETE /v1/relux/agents/:id/permissions` stays a
kernel/operator action); the revoke is exact-only (`matches_exact`, no pattern expansion — a `tool:<plugin>:*`
scope is removed only by revoking that exact row); no operator-assisted manager-revoke route this round (the
token-auth path + UI form cover it). More subtree actions (status changes, …), project/namespace scopes, and
agent-driven enrollment remain future work.

---

## Reference read — persistent allow-always grant (this slice)

The approval surfaces so far were all one-shot: a per-call binding executes once, and a generic approval
executes nothing. So a configured gated tool (a non-low-risk operator tool) re-prompted on EVERY invocation —
the openclaw "allow-once vs allow-always" decision had no Relux analogue. This slice adds the first persistent
`allow-always` grant: a standing, explicit, revocable, audited record that lets a FUTURE matching invocation
bypass the per-call approval *prompt*, bounded to one exact `(subject, plugin, tool, permission, risk)`.

### Paperclip (openclaw) — files read

- `reference/openclaw-main/src/acp/permission-relay.ts` — `GatewayExecApprovalDecision =
  "allow-once" | "allow-always" | "deny"`; `buildAcpPermissionOptions` builds three explicit, named options
  (`allow_once` / `allow_always` / `reject_once` kinds); `resolveGatewayDecisionFromPermissionOutcome` maps the
  selected option back to the decision. **Pattern: allow-always is a DISTINCT, operator-chosen decision offered
  alongside allow-once and deny — never a silent default or a blanket "trust everything".**
- `reference/openclaw-main/src/agents/bash-tools.exec-host-gateway.ts` (L609-618) — on `allow-once` the call
  is approved for this run only; on `allow-always` it is approved AND `persistAllowAlwaysPatterns(...)` writes
  a durable record, but ONLY `if (!requiresInlineEvalApproval)`. **Pattern: persist a standing grant ONLY for
  the safe-to-persist case, and allow-always both approves the in-flight call AND persists.**
- `reference/openclaw-main/src/infra/exec-approvals.types.ts` — `ExecAllowlistEntry { id, pattern,
  source: "allow-always", commandText, argPattern, lastUsedAt }`: a persisted, individually-identified record
  bound to a specific pattern (+ optional arg pattern), per-subject. **Pattern: an allow-always grant is an
  individually-revocable row, not a global flag.**
- `reference/openclaw-main/src/infra/exec-approvals.ts` — `hasDurableExecApproval(...)` /
  `hasExactCommandDurableExecApproval` (L1017-1066): a later call bypasses the prompt ONLY when a stored
  `source === "allow-always"` entry matches the EXACT command (pattern OR commandText equal) or every segment
  matches an allow-always entry; any non-matching segment fails closed. `recordAllowlistUse` (L1068+) stamps
  `lastUsedAt` on use. **Pattern: a durable grant authorizes a future call ONLY on an EXACT match (fail closed
  otherwise), and its use is recorded.**

### Hermes — files read

- `reference/hermes-agent-main/tools/approval.py` — `_YOLO_MODE_FROZEN` (a trust setting snapshotted at import
  so a later injection cannot flip it) + the dangerous-pattern / sensitive-write-target detection that forces a
  gate regardless. **Pattern: a standing "skip approval" decision is bounded and cannot be widened at runtime;
  the dangerous case is never auto-allowed.** Mirrored: a Relux grant is bound to an exact tool+risk snapshot,
  is only mintable for a tool that genuinely gates, and a risk escalation invalidates it — the standing
  decision can never silently cover a now-more-dangerous call.

### Files read in Relux (the surface being changed)

- `crates/relux-kernel/src/state.rs` — the per-call gate in `call_tool` / `invoke_tool` (`tool_needs_approval`
  → refuse) and the per-call approval flow (`request_tool_invocation_approval` / `execute_approved_tool_invocation`),
  the `KernelSnapshot` / `from_snapshot` / `KernelCounters` persistence seams.
- `crates/relux-core/src/tool.rs` (`approval_blocks_direct_invocation`), `permission.rs` (`Permission`,
  `RiskLevel`), `approval.rs` — the existing approval/permission model the grant binds to.
- `crates/relux-kernel/src/server.rs` — the approvals routes + `status_for` mapping; `store.rs` — `meta`/counter
  persistence; `apps/dashboard/src/api.ts` + `pages/ReluxApprovals.tsx` — the Approvals UI.

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| openclaw: **allow-always is a distinct, named operator decision** (`permission-relay`) | The Approvals page relabels a gated tool approval's primary button **Approve once** and adds an **Allow always** button; `KernelState::allow_always_from_approval` approves the pending approval AND mints a grant (openclaw's "approve the in-flight call AND persist"). |
| openclaw: **persist ONLY the safe-to-persist case** (`!requiresInlineEvalApproval`) | `grant_persistent_tool_invocation` runs the SAME fail-closed gates as the per-call request path: the tool must exist, the subject must hold its permission, and the tool must GENUINELY gate (`tool_needs_approval`); a directly-runnable low-risk tool is refused (`ToolDoesNotRequireApproval`). |
| openclaw: **an individually-revocable, per-subject record** (`ExecAllowlistEntry { id, … }`) | `relux_core::PersistentGrant { id, subject_agent, plugin_id, tool_name, permission, risk, last_used_at }`, held in the kernel `persistent_grants` map (snapshotted + SQLite-persisted, `next_grant` counter); `revoke_persistent_grant` removes exactly that row, audited `grant:revoke`. |
| openclaw: **bypass ONLY on an EXACT match, else fail closed** (`hasDurableExecApproval`) | `PersistentGrant::authorizes_invocation` compares subject/plugin/tool/permission/risk ALL exactly; `matching_persistent_grant_id` looks up the tool's CURRENT permission + risk, so a changed permission or escalated/changed risk no longer matches → the per-call prompt is required again. |
| openclaw: **record use** (`recordAllowlistUse` stamps `lastUsedAt`) | when a grant lets a call through, `record_persistent_grant_use` stamps `last_used_at` and audits `grant:use` — the use of a standing grant is itself an audit event. |
| Hermes: **the standing skip cannot be widened / never auto-allows the dangerous case** | a grant bypasses ONLY the per-call prompt — never `agent_holds_permission`, the runtime/loopback gate, the manager-subtree boundary, or the per-agent token boundary; there is no wildcard / blanket / global grant form here. |

**What we deliberately do differently:** the grant authorizes a *bypass of the prompt only*, never a real
authorization — the subject must already hold the permission (checked before the grant is consulted and again
on every call), and the runtime gate still applies, so a grant can never make a tool runnable that the subject
could not otherwise run. It is bound to ONE concrete tool (not a `tool:<plugin>:*` scope, even though the
permission model supports that wildcard) to keep the first surface the narrowest safe one. There is no per-grant
expiry yet (the kernel clock is logical, so a real time-bound TTL is deferred; revocation is the control). The
grant minting reuses the per-call request path's exact gates, so allow-always can NEVER widen what per-call
approval already allows — it only removes the repeated prompt for an invocation the operator has explicitly,
revocably blessed.

---

## Reference read — Prime as a Hermes-first general agent (conversation default + suggestion suppression)

A live dashboard screenshot showed Prime still behaving "work-board first": a casual greeting was answered with
the empty board / crew state and "what do you want to set up?", and an insult ("fuck you") rendered **Turn this
into a task** / **Plan this out** buttons. The product intent is the opposite — Prime is a **general local AI
agent / chat companion** that can ALSO drive the Paperclip-style company/control plane **when asked**. This slice
re-grounds the conversational default in how Hermes stays a general agent with tools as optional abilities.

### Hermes — files read

- `reference/hermes-agent-main/agent/prompt_builder.py`
  - `DEFAULT_AGENT_IDENTITY` (L134-142) — the literal identity: "You are Hermes Agent, an intelligent AI
    assistant … helpful, knowledgeable, and direct … assist users with a wide range of tasks … and executing
    actions **via your tools**." Conversation/helpfulness is the framing; tools are a secondary means.
  - `OPENAI_MODEL_EXECUTION_GUIDANCE` (L306-338, `<act_dont_ask>` / `<missing_context>`) — act on an obvious
    default, ask only when the ambiguity genuinely changes the action.
- `reference/hermes-agent-main/agent/system_prompt.py` (L60-77 assembly; L103-118 conditional tool guidance;
  L129-151 optional, per-model tool-use enforcement) — tool guidance is injected ONLY when the tool is
  registered; the base prompt is plain conversation, and tool-use *enforcement* is off by default for capable
  models.
- `reference/hermes-agent-main/agent/chat_completion_helpers.py` `build_api_kwargs` (L233-474) — tools are passed
  with NO `tool_choice="required"`; the model freely returns text or a tool call. `conversation_loop.py` L3106
  (`if assistant_message.tool_calls:`) is the only bifurcation — chat and tool use are symmetric, model-decided.
- `reference/hermes-agent-main/agent/message_sanitization.py` (L1-140) — there is NO profanity / insult /
  sentiment handling; sanitization is technical only (lone surrogates, control chars). The model just replies.

### The exact logic learned

A general agent stays general by **identity + freedom**, not by steering: the system prompt names it a broad
assistant and only *mentions* tools as optional; the API never forces a tool; and the runtime adds no emotional/
profanity special-casing — it trusts the model to answer naturally. Work happens when the model chooses a tool
on an explicit request, not because every turn is nudged toward action.

### How Relux maps it

| Hermes pattern | Relux adaptation |
|---|---|
| Identity = "intelligent AI assistant … via your tools" (conversation-first, tools optional) | The brain prompts (`prime_decision.rs build_decision_prompt`, `ai.rs compose_chat_prompt` + `build_messages`, `prime_intent.rs build_intent_prompt`) now open "You are Prime, a general-purpose local AI agent — a helpful assistant and chat companion, like Codex or Hermes … and WHEN THE USER ASKS FOR WORK you can also drive a local Relux control plane …", and explicitly tell the brain that greetings / small talk / venting / insults / emotional messages / general Q&A are conversation, never work, and not to mention the board/queue/crew or "what to set up" or push tasks on casual chat. |
| No `tool_choice="required"`; the model freely chats vs. acts | Unchanged authority: work is still gated by the explicit-instruction, fail-closed `reconcile_intent`. This slice removes the *prompt's* steering toward work — it does not loosen the gate. The deterministic `Greeting` / `DirectAnswer` fallback wording is likewise re-worded to general-agent framing (`prime::greeting_text`). |
| No profanity / sentiment special-casing — the model replies naturally | Relux adds NO brain-level emotional handling either. Its only addition is a PRESENTATION gate: `prime::is_frustration_or_emotional` + `brainstorm_offers_actionable_work` suppress the brainstorm work CTAs for venting/insults/empty small talk (so "fuck you" never shows "Turn this into a task"); it classifies nothing and acts on nothing. The single CTA source of truth stays `state.rs attach_suggestions`. |

**What we deliberately do differently:** Hermes has a live tool-calling loop where the model executes tools
directly; Relux keeps every durable change behind the deterministic kernel path (`decide` → `prime_execute`)
and the fail-closed intent gate. So "Hermes-first" here means *conversational identity + no work-steering*,
NOT giving the model execution authority. The emotional detector is a conservative presentation-only rail (a
false positive just yields a friendlier, button-free reply); it is explicitly not a new intent or gate.

### Files read in Relux (the surface being changed)

- `crates/relux-kernel/src/prime.rs` — `decide` (`Greeting` / `DirectAnswer` arms), `brainstorm_task_candidate`,
  `is_chat_guarded`; added `greeting_text`, `is_frustration_or_emotional`, `brainstorm_offers_actionable_work`,
  `WORK_INDICATORS`.
- `crates/relux-kernel/src/state.rs` — `attach_suggestions` (the single CTA source).
- `crates/relux-kernel/src/prime_decision.rs`, `crates/relux-kernel/src/ai.rs`,
  `crates/relux-kernel/src/prime_intent.rs` — the brain identity/rule prompts.
- `apps/dashboard/src/prime.ts`, `apps/dashboard/src/pages/Prime.tsx` — the chat-surface copy.

See `docs/prime-processing-audit.md` "Hermes-first general agent" for the per-case behavior table and the
remaining gaps.

## Reference read — bounded run-log / tail surface (this slice)

Runs captured their final, already-redacted, byte-capped stdout/stderr into a single `adapter_output`
transcript event, but there was no dedicated, line-classified, **pollable** log/tail surface — the
operator could not watch a run's stdout/stderr/system output as a first-class, bounded, redacted tail.
This slice adds the first run-log model: a bounded `RunLog` of per-line entries classified
`stdout`/`stderr`/`system`, captured at finalize, served at `GET /v1/relux/runs/:id/logs?since=<seq>`,
and shown in the Work Run Detail. Live per-chunk streaming during the run stays out of scope (the
synchronous spawn has no `onLog` callback yet) and is documented as the remaining gap.

### Paperclip — files read (vendored under `references/paperclip/`)

- `references/paperclip/server/src/services/run-log-store.ts` — the run-log store contract:
  `append(handle, { stream: "stdout" | "stderr" | "system", chunk, ts })` (the **three-stream
  classification**), NDJSON `{ ts, stream, chunk }` per line, and `read({ offset, limitBytes })` →
  `{ content, nextOffset }` with a default `limitBytes: 256_000`. **Pattern: a run log is per-line,
  source-classified, and read as a BOUNDED, OFFSET-CURSORED slice (a pollable tail), not an unbounded
  blob; `nextOffset` is the cursor for the next poll.** `safeSegments` + `resolveWithin` confirm the
  path-safety discipline (not needed here — Relux stores in-process, not on a per-run file).
- `references/paperclip/server/src/adapters/process/execute.ts` — `runChildProcess(runId, command,
  args, { …, onLog })` streams stdout/stderr/system chunks via the `onLog` callback during the run, and
  the result still carries `resultJson: { stdout, stderr }`. **Pattern: LIVE tailing uses an `onLog`
  stream; the final captured stdout/stderr is the fallback.** Relux's synchronous spawn has no `onLog`,
  so Relux captures the FINAL output only — the honest reason the tail is polled, not streamed.

### OpenClaw — files read (vendored under `reference/openclaw-main/`)

- `reference/openclaw-main/src/process/exec.ts` — `runExec`/`execFile` with a `maxBuffer` bound on
  captured output. **Pattern: captured process output is ALWAYS bounded (a hard buffer cap), never
  unlimited** — the discipline behind Relux's per-line and total-line caps.

### The exact logic learned

A run log is (1) per-line, (2) classified into exactly three streams (stdout/stderr/system), (3) bounded
at capture (a byte/line cap, oldest dropped), and (4) read as an offset-cursored slice so a client polls
only the tail past its cursor. LIVE liveness comes from an `onLog` stream during the run; without it, the
honest surface is the captured-at-finalize tail, polled — never a faked live stream.

### How Relux maps it

| Paperclip/OpenClaw pattern | Relux adaptation |
|---|---|
| `stream: "stdout" \| "stderr" \| "system"` per line | `relux_core::RunLogSource { Stdout, Stderr, System }`; each `RunLogLine` carries its source. Stdout/stderr come from the adapter's captured output split per line; `system` lines are kernel-authored (spawn + exit/timeout / spawn-error). |
| NDJSON `{ ts, stream, chunk }` appended per line | `RunLogLine { seq, source, text, truncated }`; `seq` is a dense 1-based cursor (the analogue of the byte `offset`). Built once at finalize by the pure `RunLogBuilder` (no clock, no IO). |
| `read({ offset, limitBytes })` → `{ content, nextOffset }` (bounded, cursored) | `GET /v1/relux/runs/:id/logs?since=<seq>` → `RunLog::since(seq)` returns only lines past the cursor; `latest_seq()` is the next cursor. Bounds are line-based: `MAX_LOG_LINES = 200` (oldest dropped, `dropped_lines` recorded) + `MAX_LOG_LINE_CHARS = 2_000` per line (`truncated` marker). |
| `maxBuffer` (output always bounded) | Two caps (per-line + total-line) plus the upstream adapter byte-cap flags (`stdout_truncated`/`stderr_truncated`) carried onto the log so the UI shows honest truncation markers. Every line is **re-redacted** (`redact_secrets`) on its own account. |
| `onLog` LIVE stream during the run | **Not adopted this round** — the synchronous spawn captures FINAL output only, so the tail is captured at `finalize_cli_run` and POLLED (the in-flight UI re-fetches `?since=<seq>` on the same 1.5s cadence as the transcript). Live streaming is the documented remaining gap. |

**What we deliberately do differently:** Paperclip streams chunks to a per-run NDJSON file via `onLog`;
Relux captures the run's final, already-redacted, byte-capped stdout/stderr at finalize into one bounded
in-process `RunLog` per run (snapshot-persisted). So Relux's tail is honest but POLLED, not streamed, and
stdout lines are grouped then stderr (not interleaved by real time) — both stated plainly in the UI copy
and the audit. A run with no captured log returns an EMPTY tail (not an error), so "No logs" is honest.

### Files read / changed in Relux (the surface being added)

- `crates/relux-core/src/run_log.rs` (new) — `RunLog`/`RunLogLine`/`RunLogSource`/`RunLogBuilder` + the
  bounds + pure tests; exported from `crates/relux-core/src/lib.rs`.
- `crates/relux-kernel/src/state.rs` — `run_logs` store, `capture_cli_run_log` /
  `capture_spawn_error_log` (in `finalize_cli_run`), the `run_log` reader, snapshot export/restore.
- `crates/relux-kernel/src/store.rs` — `run_logs` meta persistence.
- `crates/relux-kernel/src/server.rs` — `GET /v1/relux/runs/:id/logs` (`get_run_logs` + `RunLogsQuery`).
- `apps/dashboard/src/reluxrunlog.ts` (new), `apps/dashboard/src/api.ts` (`getRunLogs` + types),
  `apps/dashboard/src/pages/Work.tsx` (the Logs/Tail section + poll).

See `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §24 for the applied-change record and the remaining streaming gap.

## Reference read — LIVE run-log streaming during off-lock adapter execution (this slice)

### Files read (reference)

- `references/paperclip/server/src/adapters/process/execute.ts` — the process adapter passes an `onLog`
  callback into `runChildProcess(runId, command, args, { …, onLog })`; the final result still carries
  `resultJson.stdout`/`stderr`.
- `references/paperclip/packages/adapter-utils/src/server-utils.ts` `runChildProcess` (~L2075) — the real
  spawn: `child.stdout.on("data", chunk => { stdout = appendWithCap(stdout, text); logChain = logChain
  .then(() => onLog("stdout", text)) … })` and the identical `stderr` handler. **Pattern learned:** each
  read chunk is BOTH appended to a capped buffer AND streamed to the live store, serialized through a
  `logChain` promise so appends never interleave; the captured final output is preserved alongside.
- `references/paperclip/server/src/services/run-log-store.ts` — `append({ ts, stream, chunk })` to a per-run
  NDJSON file + an offset-cursored bounded `read`, so a client polls the during-run tail.
- `reference/openclaw-main/src/process/exec.ts` (~L444) — `child.stdout?.on("data", d => { stdoutChunks.push(…);
  armNoOutputTimer(); })` + a `maxBuffer` bound. **Pattern learned:** the per-chunk read loop is the unit of
  streaming, and captured output is ALWAYS bounded.

### The exact logic learned

Live streaming = feed each stdout/stderr READ CHUNK to a sink as it arrives (not at finalize), while still
keeping the full final capture; serialize the appends; bound the buffer; classify by stream; read the
during-run tail with an offset/seq cursor. Liveness needs the spawn to run where a reader can observe it —
i.e. NOT behind the same lock the reader needs.

### How Relux maps it

| Paperclip/OpenClaw pattern | Relux adaptation |
|---|---|
| `onLog(stream, chunk)` per read chunk | `run_adapter_command_streaming(spec, Option<RunLogSink>)` + `spawn_capped_reader(reader, max, Option<(RunLogSource, RunLogSink)>)`; the sink is fed exactly the KEPT bytes (≤ byte cap), classified by source. `run_adapter_command` delegates with `None` (unchanged). |
| Append chunk to capped buffer AND stream it; preserve final stdout/stderr | The capped reader still returns the final redacted/capped `stdout`/`stderr`; streaming is strictly additive (the `AdapterRunOutcome` is byte-for-byte identical). |
| `logChain` serializes appends | `RunLogSink` wraps `Arc<Mutex<StreamingRunLog>>`; the two reader threads' appends serialize on the mutex. |
| Per-run NDJSON store + offset `read` during the run | In-memory `LiveRunLogs` registry (`Arc<Mutex<HashMap<run_id, Arc<Mutex<StreamingRunLog>>>>>`) on `AppState`, INDEPENDENT of the kernel lock; `get_run_logs` reads `snapshot(run_id, since)` without the kernel lock. The durable per-run `RunLog` (built at finalize) is the canonical source once it exists (`has_run_log` precedence). |
| Chunk may split a line | `StreamingRunLog` line-buffers per source (carry until `\n`), emits only complete lines (re-redacted + clamped), force-emits an over-cap carry; `flush` emits the trailing partial at end. |
| `maxBuffer` (always bounded) | `RunLogBuilder` now enforces `MAX_LOG_LINES` continuously (`enforce_live_cap`, oldest dropped + counted) so a LIVE stream is bounded mid-run; `MAX_LIVE_RUNS` bounds the registry. |

**What we deliberately do differently:** Paperclip persists raw NDJSON chunks to a file and pushes via the
store; Relux keeps the live tail in an in-memory bounded `StreamingRunLog` (per-line, three-stream, redacted)
and serves it by POLL (no SSE/WebSocket). Liveness is wired ONLY for the off-lock parallel-orchestration
driver (the one path that releases the kernel lock during the spawn); the synchronous in-kernel driver holds
the lock across its spawn, so it keeps capturing at finalize — stated plainly in the UI copy and the audit.

### Files changed in Relux

- `crates/relux-core/src/run_log.rs` — `StreamingRunLog` + the continuous `enforce_live_cap` + non-consuming
  `snapshot`; exported from `crates/relux-core/src/lib.rs`. Pure unit tests.
- `crates/relux-kernel/src/live_run_log.rs` (new) — `LiveRunLogs` registry + `RunLogSink`; exported from
  `lib.rs`.
- `crates/relux-kernel/src/adapter.rs` — `run_adapter_command_streaming` + the sink-fed `spawn_capped_reader`.
- `crates/relux-kernel/src/state.rs` — `PreparedBrief::{run_id,run_with_sink}`,
  `run_briefs_in_parallel_streaming`, `KernelState::has_run_log`.
- `crates/relux-kernel/src/server.rs` — `AppState.live_run_logs`, `run_parallel_round` streams + finishes,
  `get_run_logs` durable-vs-live precedence.
- `apps/dashboard/src/pages/Work.tsx` + `reluxrunlog.ts` — copy now describes a LIVE (polled) tail; bundle
  rebuilt.

See `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §25 for the applied-change record and the remaining gaps.

## Reference read — first safe mid-run cancellation for process-backed runs (this slice)

### Files read (reference)

- `reference/openclaw-main/src/process/exec.ts` (vendored) — the process exec threads an `AbortSignal` into
  the child spawn and kills the process when it fires; captured output is `maxBuffer`-bounded. **Pattern
  learned:** an external signal (not just the deadline) can terminate the owned child; the cancel is wired
  into the SAME spawn that owns the process.
- `references/paperclip/server/src/adapters/process/execute.ts` (vendored) — `runChildProcess` kills the child
  on `timeoutSec`/`graceSec` (a `graceSec` window before the hard kill). **Pattern learned:** killing the
  owned child handle on an external stop signal is the mechanism; a grace period is the refinement (Relux's is
  immediate this slice — recorded as a remaining gap).

### The exact logic learned

Mid-run cancellation = a flag the running spawn polls + the spawn's existing kill path fired when the flag is
set. The flag must live where it can reach a child spawned with the orchestration's kernel lock RELEASED, so
it cannot itself take that lock — the same liveness constraint as the live run-log (§25). The result is
recorded as an intentional terminal state distinct from a failure.

### How Relux maps it

| OpenClaw/Paperclip pattern | Relux adaptation |
|---|---|
| `AbortSignal` threaded into the spawn | `run_adapter_command_streaming_cancellable(spec, sink, Option<CancelToken>)`; the existing 40ms `try_wait` loop checks `cancel.is_cancelled()` on the same tick as the deadline. `run_adapter_command{,_streaming}` delegate with `None` (unchanged). |
| Kill the owned child on the signal | Relux reuses its timeout kill path; `kill_child_tree` adds a best-effort Windows tree kill (`taskkill /PID <pid> /T /F`) for the shim→node→… tree, then the owned `child.kill()` fallback. The outcome carries `cancelled: true`. |
| Signal lives outside the spawn, reaches it without the work lock | `RunCancellations` registry (`Arc<Mutex<HashMap<run_id, Arc<CancelState>>>>`) on `AppState`, INDEPENDENT of the kernel lock (mirrors `LiveRunLogs`); `request` sets an `AtomicBool` via a race-free `swap`. The off-lock driver opens a `CancelToken` per brief; the server finishes it after finalize. |
| Terminate → terminal state | `finalize_cli_run` detects `cancelled` and marks the run `RunStatus::Cancelled` + `RunFailureClass::Cancelled`, no retry, `run_cancelled` event + `run:cancel` audit — distinct from a Failed run, excluded from the recovery projections. |
| `graceSec` before hard kill | NOT adopted this slice — the kill is immediate; recorded as a remaining gap. |

**What we deliberately do differently:** cancellation is wired ONLY for the off-lock parallel driver (the one
path that releases the kernel lock during the spawn). The synchronous in-kernel driver holds the lock across
its spawn, so no cancel can interleave there by construction — the API honestly reports `not_running` rather
than faking a cancel. Only an off-lock streaming run has a live `CancelToken`; everything else is honestly
not-cancellable.

### Files changed in Relux

- `crates/relux-kernel/src/run_cancel.rs` (new) — `RunCancellations` registry + `CancelToken`/`CancelState` +
  `CancelOutcome`; exported from `lib.rs`. Pure unit tests.
- `crates/relux-kernel/src/adapter.rs` — `AdapterRunOutcome.cancelled`, `run_adapter_command_streaming_cancellable`,
  the cancel poll + `kill_child_tree`.
- `crates/relux-kernel/src/state.rs` — `PreparedBrief::run_with_sink_cancellable`,
  `run_briefs_in_parallel_streaming(.., cancels)`, the `finalize_cli_run` cancel branch + `cancel_cli_run` /
  `cancel_run`, the `capture_cli_run_log` cancelled line.
- `crates/relux-kernel/src/server.rs` — `AppState.run_cancellations`, the `POST /v1/relux/runs/:id/cancel`
  handler + route, `run_parallel_round` begins/finishes cancel tokens.
- `apps/dashboard/src/{api.ts,runview.ts,pages/Work.tsx}` — `cancelRun` + `ReluxCancelRunResponse`,
  `canCancelRun`, the Cancel-run button + inline honest result; bundle rebuilt.

See `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §26 for the applied-change record and the remaining gaps.

---

## Reference read — Work hierarchy / progress v1 on the board (this slice)

Before surfacing sub-work + progress on the Work board, read how the reference
dashboards model and render a parent's child progress and dependency (blocked-by)
edges, so Relux mirrors a proven shape rather than inventing one. This slice is a
**read-only display** join over two existing reads (it changes no orchestration /
execution / approval behavior), but task/workflow *display* is in scope for the
reference-first rule, so the grounding is recorded.

### Hermes — files read

- `reference/hermes-agent-main/plugins/kanban/dashboard/plugin_api.py` (the `/board`
  endpoint, progress rollup at L386-398): for each parent it computes `{done, total}`
  in ONE pass over `task_links` joined to each child's live status
  (`SELECT l.parent_id, t.status FROM task_links l JOIN tasks t ON t.id = l.child_id`),
  attaching `progress` + `link_counts` per card. **Pattern: a parent's progress is a
  one-pass tally of its children's CURRENT status, not a stored summary field.**
- Same file, `_links_for` (L322-338): parents/children resolved by two directional
  queries over the join table; and the `update_task` "ready" guard (L709-722) — a
  child cannot advance until **all parents are `done`**. **Pattern: blocked-by is a
  status relationship over the dependency edges, surfaced on the row.**
- `reference/hermes-agent-main/hermes_cli/kanban_db.py` `task_links(parent_id, child_id)`
  — parent↔child lives in a join table, not a field on the task. (Relux's analogue is
  the orchestration's `steps[].depends_on` index edges + `steps[].task_id`.)
- `reference/hermes-agent-main/plugins/kanban/dashboard/dist/index.js` (the built board
  bundle — no source ships) — the **native HTML5 drag-to-column** pattern: a private
  MIME `MIME_TASK = "text/x-hermes-task"`, cards `draggable: true` with an `onDragStart`
  that records the dragged id, a column `onDragOver` that `preventDefault()`s + toggles a
  `dragOver` highlight, and an `onDrop` that reads the MIME payload and applies the move,
  with `dataTransfer.effectAllowed/dropEffect = "move"`. **Pattern: column status moves
  ride the browser's native DnD under a private MIME — no DnD library — and the drop
  resolves the target column to a status mutation.**

### How Relux maps it

| Reference pattern | Relux adaptation |
|---|---|
| Hermes: **per-parent `{done,total}` from a one-pass join to each child's LIVE status** | `apps/dashboard/src/workhierarchy.ts` `buildWorkGroups` joins `reluxOrchestration.list()` `steps[].task_id` to the live `reluxWork.listTasks()`, bucketing each child by its current board status via the shared `oversight.ts::taskBucket` (so the strip and the columns agree); `groupProgress` tallies the four board buckets in one pass. A child off the board falls back to the durable `step.outcome`, said so honestly. |
| Hermes: **blocked-by is a status relationship over the dependency edges, shown on the row** | `buildWorkGroups` resolves `step.depends_on` indices to sibling `task_id`s for **blocked-by**, and the reverse (siblings whose `depends_on` names this child) for **blocking**; rendered as chips on each numbered checklist row (`blockedByLabel` / `blockingLabel`). |
| Hermes: **parent↔child in a join table, not a task field** | Relux records **two** real parent→child links: the orchestration's `steps[].depends_on` + `steps[].task_id` (one kind of parent), and the now-**populated** `relux_core::Task.parent_task` edge for hand-made ad-hoc subtasks (the other — see `relix-dashboard-design.md` §6.3). A task in neither is genuinely standalone; no subtree is fabricated. |
| Paperclip `agentIsInSubtree` / Hermes `delegate_tool` `MAX_DEPTH`: **a bounded, cycle-guarded parent-pointer walk** | The org-lattice walk in `relux_core::hierarchy` is mirrored for the **task tree** in `relux_core::task` (`task_ancestors` / `is_in_task_subtree` / `would_create_task_cycle`, bounded by `MAX_TASK_DEPTH`). The kernel's `create_task_with_parent` calls `would_create_task_cycle` before persisting a `parent_task` edge (defence in depth — a fresh create can't cycle; it guards a reparent), and rejects a cross-namespace parent (`TaskParentScope`). |
| In-kernel reparent precedent: the org-lattice **Lead (`reports_to`) change** validates a set parent before mutating (self / existence / `would_create_cycle`) | `KernelState::reparent_task` (`relix-dashboard-design.md` §6.6) is that precedent reused for the **task tree**: it validates a set `parent_task` before any mutation (existence → `UnknownTask`, same namespace → `TaskParentScope`, no self/cycle → `would_create_task_cycle` → `TaskParentCycle`) exactly as `update_agent_with_skills` validates a set Lead, and is **structural only** (the edge + `updated_at`, never status / agent / runs). The UI `candidateParents` (`apps/dashboard/src/reparent.ts`) ports the same bounded subtree walk client-side so the control never offers a parent the kernel would reject. |
| Hermes kanban: **native HTML5 drag-to-column under a private MIME, no DnD library; the drop resolves a column → a status move** | `apps/dashboard/src/pages/Work.tsx` `Column`/`TaskCard` use the same native API under `TASK_DRAG_MIME = "application/x-relux-task"` (`relix-dashboard-design.md` §6.7): cards are `draggable` with `onDragStart` encoding `{id,status}`, the column `onDragOver` `preventDefault()`s + highlights, `onDrop` decodes the payload and resolves the target column via `taskmove.ts::columnDropTarget`. Relux's adaptation is **tighter**: the drop maps only to the operator-settable lanes (Blocked→`blocked`, Done→`cancelled`) and reuses the SAME validated `set_task_status` route + allowlist as the §6.4 select — Open/Running drops, terminal cards, and no-ops are rejected inline, never silently applied. Drag is **additive** over the keyboard select; a foreign (non-task) drop decodes to `null` and is ignored. |

**What we deliberately do differently:** no backend route or authority was added —
the board composes two reads it already makes (the orchestration list + the task
list), so progress is live and honest with zero new state. Hermes nests arbitrary
parent/child trees; Relux's orchestration is a single level today, so the plan
numbers `1..N` and deeper `1.1` nesting is **not** fabricated. See
`docs/relix-dashboard-design.md` §6.2 for the shipped surface + remaining gaps.

---

## Reference read — universal read-only plugin-source capabilities (Plugin Lens, this slice)

**Problem this slice fixes (product contract):** an installed plugin was visible in the UI
but Prime had *nothing it could actually invoke* against it. A manifestless install
scaffolds a metadata-only manifest with an **empty** `capabilities.tools`
(`crates/relux-kernel/src/plugin_install.rs::scaffold_manifest`), and the only paths to a
runnable tool were operator-configured MCP registration / command tools
(`capability_detect.rs` candidates). So a normal GitHub repo / ZIP / folder — which never
ships a `relux-plugin.json` — installed as a dead row. The contract: **if a thing is
installed as a plugin, Prime must be able to discover it and use it somehow** — at minimum
read-only source introspection scoped to that plugin's installed directory.

Before building, read how the reference agents expose a freshly-installed body of source to
their agent and how they keep that read-only and path-confined.

### Hermes — files read

- `reference/hermes-agent-main/tools/skills_tool.py` `_find_all_skills` (L550-624): Hermes
  discovers capabilities by **scanning an installed directory** (`~/.hermes/skills/` +
  configured external dirs) for `SKILL.md` and parsing only the first ~4000 bytes of
  frontmatter. **Pattern: capability discovery is a bounded, read-only scan of the
  installed source tree — the agent never executes the source to learn what it is.**
- `reference/hermes-agent-main/agent/prompt_builder.py` `build_skills_system_prompt`
  (L992-1150) + `reference/hermes-agent-main/agent/system_prompt.py`
  `build_system_prompt_parts` (L60-100): the agent first sees only **metadata** (name,
  ≤1024-char description, category) injected into the stable prompt tier; the full body is
  pulled on demand via a **`skill_view(name=...)` tool** (progressive disclosure). **Pattern:
  a cheap "summary" capability up front, a separate "read this file" capability the agent
  invokes only when it needs the detail — exactly the `plugin.summary` → `plugin.read_file`
  split.**
- `reference/hermes-agent-main/agent/transports/hermes_tools_mcp_server.py` `EXPOSED_TOOLS`
  (L1-160): a curated **read/fetch** subset (`skill_view`, `skills_list`, `web_extract`, …)
  is exposed to a subprocess; `terminal`/shell is **deliberately omitted**. **Pattern:
  read-only inspection tools are safe to expose by default; execution is not.**

### OpenClaw / Paperclip — files read

- `reference/openclaw-main/src/plugins/discovery.ts` `discoverOpenClawPlugins`
  (L1-200): plugin candidates are discovered by directory scan **without requiring**
  `openclaw.plugin.json`; a candidate with `bundledManifest: undefined` still proceeds.
  Each candidate validates that its source **does not escape its root**
  (`checkSourceEscapesRoot`). **Pattern: a manifestless install is a first-class case, and
  every source read is path-confined to the plugin root — no traversal out.**
- `reference/openclaw-main/src/plugins/manifest.ts` (L34-300): the manifest is **optional
  declaration metadata** (activation hints, config contracts), not a precondition for the
  plugin to exist or be introspected. **Pattern: absence of a manifest must not block
  capability exposure; it just means capabilities are derived/registered another way.**

### How Relux maps it

| Reference pattern | Relux adaptation (this slice) |
|---|---|
| Hermes: capability discovery is a **bounded read-only scan of the installed source tree**, never executing it | New `crates/relux-kernel/src/plugin_source.rs` implements four pure, side-effect-free, read-only operations over a plugin's `install_dir`: `inspect` (bounded file tree), `search` (bounded text grep), `read_file` (one bounded text file), `summary` (manifest metadata + `detect_hints` + README excerpt + counts). No process is spawned; nothing in the plugin is executed — this honors `RELUX_MASTER_PLAN.md` §8.2/§18 ("no shelling out, no side effects *from* installed plugins"): reading copied bytes is not running the plugin. |
| Hermes: **`skill_view` is a tool the agent invokes**, gated and audited; metadata up front, detail on demand | The four ops are **real kernel tools** `plugin.summary` / `plugin.inspect` / `plugin.search` / `plugin.read_file`, synthesized for **every non-bundled installed plugin** and routed through the UNCHANGED `invoke_tool` gate (permission check → approval check → runtime → audit). They appear in `discover_tools` → `live_tool_catalog` → `prime_agent_catalog` / `GET /v1/relux/prime/tools`, so the brain sees + picks them like any other tool. |
| Hermes: **read-only inspection is safe to expose; execution is not** | The source tools are `RiskLevel::Low` + `Approval::Never`, so `approval_blocks_direct_invocation` is false → directly `Ready` (no per-call approval), but they still require a real capability — a single `plugin:source:read` grant Prime holds from bootstrap (`ensure_bootstrapped`). They never write, spawn, or reach the network. The existing command-tool / MCP execution paths (which *can* run code) keep their `Required` approval, unchanged. |
| OpenClaw: every source read is **path-confined to the plugin root**, traversal rejected | `plugin_source::resolve_within(base, rel)` canonicalizes both and rejects any path that escapes `install_dir` (absolute paths, `..`, symlink escape), mirroring `checkSourceEscapesRoot`. Unit-tested with traversal payloads. Reads are bounded (max files, max bytes, max matches) so a huge repo cannot exhaust memory. |
| OpenClaw: a **manifestless install is first-class**; the manifest is optional metadata | Source tools are attached at *discovery* time from the installed-plugin record + `install_dir`, **independent of** the manifest's `capabilities.tools`. A manifestless scaffold (empty tools) therefore still exposes the four real capabilities — closing the "dead row" gap — while a manifest plugin gets them *in addition to* its declared tools. |

**What we deliberately do differently:** Relux does NOT auto-generate runnable *execution*
wrappers from source (no inferred argv, no inferred MCP endpoint) — that stays an explicit,
approval-gated operator/Prime action (`capability_detect.rs` candidates + `ConfigureCommandTool`).
The new capabilities are strictly **read-only source introspection**, which is why they can be
`Ready` by default without an approval prompt. Bundled plugins (the shipped adapters/tools)
are excluded from the synthetic source tools — their capabilities are already known and the
contract is about *installed third-party* plugins that were dead rows. See
`docs/plugins.md` "Plugin Lens (read-only source capabilities)" for the product surface.
