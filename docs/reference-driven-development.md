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
