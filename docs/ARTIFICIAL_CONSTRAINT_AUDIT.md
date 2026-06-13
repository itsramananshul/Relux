# Artificial Constraint Audit + Fix

> Scope: the shipping **relux-\*** product layer (`relux-core`, `relux-kernel`,
> `apps/dashboard`). The legacy `relix-*` crates are a separate, older surface and are
> out of scope for this audit. Spec refs: `docs/RELUX_MASTER_PLAN.md` §10.5/§17.1;
> `docs/mcp.md` "Prime Agent Loop".

## Why this exists

The Prime Agent Loop originally shipped with a toy 3-tool / 3-round hard cap that made
Prime feel like a demo. That was replaced with a real configurable autonomy policy
(`relux_core::PrimeAgentPolicy`) and resumable continuation. This audit sweeps the rest of
the product layer for the **same class of mistake**: places where earlier work
accidentally constrained Relux into toy / demo / MVP mode instead of the serious,
configurable, production-capable product it is meant to be.

The bar used to triage every finding:

- **FIX NOW** — an artificial toy/demo constraint that hurts usability and is safe to
  remove, raise, or make configurable in this slice without weakening a real guardrail.
- **KEEP (with reason)** — a genuine safety / DoS / resource / least-privilege guardrail
  that must remain. Where it is operator-facing it should be visible/configurable; where
  it is an internal anti-DoS clamp it stays fixed and that is correct.
- **LATER** — a real constraint worth lifting but too large for this slice; recorded with
  the exact next step.

A guardrail is NOT a toy cap. Char/byte clamps, request/response size limits, loopback-only
MCP, fail-closed approval defaults, and the *existence* of a finite agent-loop ceiling are
real and stay. A toy cap is a tiny magic number that throttles normal product behavior with
no safety rationale (a 6-step orchestration, a 4-round read loop, a hidden echo fixture
presented as a capability).

---

## FIX NOW — applied in this slice

### 1. Orchestration step cap `6` → `16` (named, still bounded)
- **Was:** `plan_orchestration` carried a function-local `const MAX_STEPS: usize = 6`, and
  `prime_orchestration_slots.rs` *duplicated* the literal `6`. A real multi-part goal
  ("research the options, build a prototype, write tests, document it, wire CI, and ship
  it") was silently truncated to the first 6 briefs.
- **Now:** a single named, documented `relux_core::MAX_ORCHESTRATION_STEPS = 16`. The
  deterministic planner and the brain-proposal path both reference it (the duplicated
  literal is gone, so they can never drift). The cap is still a **real safety rail** — a
  pathological run-on sentence cannot fan out without bound, and overflow clauses are still
  reported in an honest "only the first N were planned" note, never dropped silently.
- **Files:** `crates/relux-core/src/orchestration.rs`, `crates/relux-core/src/lib.rs`,
  `crates/relux-kernel/src/prime_orchestration_slots.rs`.

### 2. Read-only context loop `MAX_TOOL_ROUNDS` `4` → `8`
- **Was:** Prime's read-only context-gathering loop (Hermes `run_conversation`'s
  `max_iterations` analog) was capped at **4** rounds with the comment "kept small". Hermes'
  own default is **90**. Four reads made Prime give up gathering live state too early on a
  genuinely multi-part question.
- **Now:** **8**, with an honest comment that this is a finite anti-spin rail on a
  **read-only** loop (it changes nothing; the only cost it bounds is brain-call count). The
  loop still stops early on a repeated / no-progress read, and a brain that has not finished
  simply answers with what it has. Every dependent test is symbolic (`<= MAX_TOOL_ROUNDS`),
  so the bound stays enforced at the new value.
- **Files:** `crates/relux-kernel/src/prime_tools.rs`.

### 3. Tool-plan step cap `5` → configurable policy (standard 16 / extended 64, ceiling 64)
- **Was:** `relux_core::MAX_TASK_TOOL_PLAN_STEPS = 5` — a hidden toy constant baked into
  `TaskToolPlan::validate`, `parse_task_tool_plan`, the Prime tool-plan proposal, the
  task-create route, the dashboard builder copy, and several tests. A serious operator
  multi-tool plan ("search, then read, then summarize, then file, then notify, then …")
  was rejected at the sixth step with no way to raise it.
- **Now:** a real **configurable policy**, folded into the existing operator-facing
  autonomy surface (`relux_core::PrimeAgentPolicy`) alongside the agent-loop limits — the
  Hermes `iteration_budget.py` precedent (a tunable bound, not a tiny constant) applied to
  operator-authored plans. Two new fields: `max_tool_plan_steps` (standard, default **16**,
  aligned with `MAX_ORCHESTRATION_STEPS`) and `extended_max_tool_plan_steps` (default
  **64**), both clamped to the absolute hard backstop `MAX_TASK_TOOL_PLAN_STEPS_CEIL`
  (**64**). `TaskToolPlan::validate_with_limit(max)` (clamped to the ceiling) is the new
  operator-facing validator; the no-arg `validate()` keeps the conservative static default
  (`MAX_TASK_TOOL_PLAN_STEPS`, now **16**) for tests/CLI. The configured limit is applied
  consistently at: the **Prime tool-plan proposal** (`build_tool_plan_proposal`), the
  **UI-created tool-run task** route (`create_task`), and **any policy route** (the new
  `max_tool_plan_steps` / `extended_max_tool_plan_steps` fields on
  `/v1/relux/prime/agent-policy` + the `prime agent-policy configure` CLI flags). The
  permissive **run-driven read path** (`parse_task_tool_plan`) bounds only at the ceiling
  so a plan created under a raised limit still reads back. An over-limit plan is a clean
  `400` / blocking issue that **names the limit and how to raise it** — never silently
  truncated. Dashboard: a **Tool plan** row in the Prime Autonomy Limits panel; the
  builder takes the live limit (`MAX_TOOL_RUN_STEPS` fallback raised 5 → 16).
- **Files:** `crates/relux-core/src/task.rs`, `crates/relux-core/src/prime.rs`,
  `crates/relux-core/src/lib.rs`, `crates/relux-kernel/src/state.rs`,
  `crates/relux-kernel/src/server.rs`, `crates/relux-kernel/src/main.rs`,
  `apps/dashboard/src/api.ts`, `apps/dashboard/src/components/PrimeAgentPolicyPanel.tsx`,
  `apps/dashboard/src/toolruntask.ts`.

### 4. Orchestration width + read-only context rounds → configurable policy (one autonomy dial)
- **Was:** the two items below previously sat in **LATER** — orchestration fan-out width
  (`MAX_ORCHESTRATION_STEPS`) and the read-only context-loop round budget (`MAX_TOOL_ROUNDS`)
  were each a bare module constant. They were already *raised* (6→16 and 4→8) and honest, but
  they could not be tuned per deployment, and the planner / brain-proposal paths read the
  constant directly, so there was no single operator dial.
- **Now:** both are folded into the existing operator-facing autonomy surface
  (`relux_core::PrimeAgentPolicy`) alongside the agent-loop + tool-plan limits — the same
  `iteration_budget.py` precedent (a tunable bound, not a tiny constant). Four new fields:
  `max_orchestration_steps` (standard, default **16**) / `extended_max_orchestration_steps`
  (default **64**), clamped to the shared `MAX_ORCHESTRATION_STEPS_CEIL` (**64**); and
  `max_context_rounds` (standard, default **8**, aligned with `MAX_TOOL_ROUNDS`) /
  `extended_max_context_rounds` (default **32**), clamped to `MAX_CONTEXT_ROUNDS_CEIL` (**64**).
  - **Orchestration:** the planner now takes the configured width as an argument
    (`relux_core::plan_orchestration_with_limit`; the bare `plan_orchestration` keeps the
    default constant for callers without a policy). Both authoritative create-paths read the
    SAME resolved width (`PrimeAgentPolicy::orchestration_steps`): `prime_orchestrate` (the
    deterministic create) and `reconcile_orchestration_slots` (the brain-proposal path), so they
    can never fan out to different widths. The preview route resolves the same width so the
    previewed brief count matches what "Create" produces. Beyond the width the overflow note
    **names the active limit and how to raise it** (autonomy limits / extended mode) — never a
    silent drop.
  - **Read-only context loop:** `ContextLoop` / the up-front `execute_requested_reads` executor
    now take the resolved round budget (`PrimeAgentPolicy::context_rounds`), threaded from the
    server preview block into the observe-then-act `DecisionLoop` and the sidecar `ContextLoop`.
    The parse path bounds the request list at the absolute ceiling (so a list authored under a
    raised/extended policy still reads back); the configured budget is applied at RESOLVE time.
    The no-progress / repeat early-stop safety is preserved exactly.
  - **Surface:** the four fields are on `/v1/relux/prime/agent-policy` (GET resolves them per
    profile; PUT/PATCH clamps them) and the `prime agent-policy configure` CLI
    (`--max-orchestration-steps` / `--ext-…`, `--max-context-rounds` / `--ext-…`), with compact
    chips + controls in the dashboard Prime Autonomy Limits panel.
- **Files:** `crates/relux-core/src/orchestration.rs`, `crates/relux-core/src/prime.rs`,
  `crates/relux-core/src/lib.rs`, `crates/relux-kernel/src/prime_tools.rs`,
  `crates/relux-kernel/src/prime_decision.rs`,
  `crates/relux-kernel/src/prime_orchestration_slots.rs`, `crates/relux-kernel/src/state.rs`,
  `crates/relux-kernel/src/server.rs`, `crates/relux-kernel/src/main.rs`,
  `crates/relux-kernel/src/lib.rs`, `apps/dashboard/src/api.ts`,
  `apps/dashboard/src/components/PrimeAgentPolicyPanel.tsx`.

### 5. Background-job concurrency `MAX_ACTIVE_JOBS = 4` → configurable policy (named resource guardrail)
- **Was:** the async `run-async` orchestration-job path carried a hidden
  `const MAX_ACTIVE_JOBS: usize = 4` in `server.rs`. It is a **real** resource guardrail —
  each active job drives live adapter processes on a dedicated OS thread, so unbounded jobs
  would exhaust the host — but it was a fixed, invisible `4`: a busy operator could not raise
  it and a constrained host could not lower it, and the over-limit 429 named the constant
  internally (`{n}/{MAX_ACTIVE_JOBS}`) rather than an operator-visible knob. This was the last
  **LATER** item.
- **Now:** a real **configurable policy**, folded into the same operator-facing autonomy
  surface (`relux_core::PrimeAgentPolicy`) as the other dials — the Hermes precedent here is
  the api-server's configurable `max_concurrent` admission knob (`reference/hermes-agent-main/
  .plans/openai-api-server.md`: a named, raisable concurrency limit, not a hidden wall). Two
  new fields: `max_active_jobs` (standard, default **4** — the practical value the retired
  constant held) and `extended_max_active_jobs` (default **16**), both clamped to the absolute
  hard backstop `MAX_ACTIVE_JOBS_CEIL` (**64**). It stays a real guardrail — even "extended"
  is bounded, never unlimited — so a request burst can never spawn unbounded workers.
  - **Admission:** `JobRegistry::start` takes the resolved cap as an argument (the registry no
    longer hard-codes a number); the `run-async` route reads it from the policy via
    `PrimeAgentPolicy::active_jobs(extended)`. A request may opt into the higher profile with
    `{"extended": true}`.
  - **Honest refusal:** the over-limit `429` now **names the configured limit and how to raise
    it** — `"background-job concurrency limit reached: {active}/{limit} jobs active under the
    standard admission profile. Wait for one to finish, retry with {"extended": true} …, or
    raise max_active_jobs on PUT /v1/relux/prime/agent-policy (clamped to 64)."` — never a
    generic "too many".
  - **Surface:** the two fields are on `/v1/relux/prime/agent-policy` (GET resolves them per
    profile; PUT/PATCH clamps them) and the `prime agent-policy configure` CLI
    (`--max-active-jobs` / `--ext-max-active-jobs`), with an **Active jobs** row + a resolved
    `jobs std/ext active` chip in the dashboard Prime Autonomy Limits panel.
- **Files:** `crates/relux-core/src/prime.rs`, `crates/relux-kernel/src/server.rs`,
  `crates/relux-kernel/src/main.rs`, `apps/dashboard/src/api.ts`,
  `apps/dashboard/src/components/PrimeAgentPolicyPanel.tsx`.

### 6. Prime tool use was keyword-gated + MCP-discovery was `"mcp:"`-token-gated + the brain saw no tool inventory
- **Was:** three artificial constraints meant an installed/configured tool was largely a dead
  row on the Plugins page rather than something Prime would actually use from chat:
  1. The **Prime Agent Loop entry** was gated on the *deterministic keyword classifier*
     (`classify_intent(message) == ToolInvocation`) — directly contradicting §10.1 and
     `docs/reference-driven-development.md` ("the keyword classifier is a fallback safety rail,
     not the primary brain"). A natural-language tool request the keyword rail did not pattern-match
     (e.g. *"summarise this repo with the readme tool"*) never entered the governed tool path,
     even with a capable brain configured.
  2. The off-lock **live MCP `tools/list` discovery** only ran when the message literally
     contained the substring `"mcp:"` — a toy gate that made MCP tools usable only if the user
     typed the internal reference syntax, never from plain language.
  3. The **decision brain was never handed the installed-tool inventory**, so it could not know
     which tools existed, could not reliably classify a tool request, and could not answer
     "what tools do you have?" from the real runnable set.
- **Now:** all three lifted **without weakening any gate** (`docs/prime-tool-use.md`):
  1. The agent-loop entry is **brain-driven** — the brain proposes the intent and the
     **unchanged fail-closed `reconcile_intent`** decides, with the keyword classifier as the
     fallback rail. `reconcile_intent` already treats `tool_invocation` / `tool_plan_request` as
     **sensitive**, so guarded chat (a greeting, a question, frustration, a vague musing) can
     **never** be promoted into the loop — the safety wall holds, but an explicit request the
     brain recognises now enters it.
  2. The MCP discovery gate is the brain-derived `effective_is_tool_turn` (or a literal `mcp:`
     ref), still requiring an enabled server — so a plausible tool turn discovers the live MCP
     tools and normal chat pays nothing. Still fail-closed: an unreachable/disabled server
     contributes no tools.
  3. `render_tool_inventory` injects the **runnable** installed tools (+ enabled MCP server
     names) into the decision prompt — only `ready` / `needs_approval` tools, the same set the
     agent loop offers, so the brain is never told it can run a tool the kernel would refuse.
     `GET /v1/relux/prime/tools` exposes the full runnable catalog (incl. live MCP) for the
     dashboard "Tools Prime can use" panel.
  Every execution still flows through the single `prime_invoke_tool` → `invoke_tool` chokepoint
  (permission → risk/approval + per-call/allow-always grant → audit); there is no second
  security model and nothing is auto-approved.
- **Files:** `crates/relux-kernel/src/prime_decision.rs`, `crates/relux-kernel/src/server.rs`,
  `crates/relux-kernel/src/ai.rs`, `crates/relux-kernel/src/lib.rs`,
  `apps/dashboard/src/api.ts`, `apps/dashboard/src/pages/Prime.tsx`, `docs/prime-tool-use.md`.

---

## KEEP (with reason) — real guardrails, not toy caps

- **Prime Agent Loop ceilings** (`relux_core::PrimeAgentPolicy` → `AgentLimits`): standard
  12/18, extended 64/96, clamped at tool-calls ≤ 512 / brain-rounds ≤ 1024 / duration ≤ 24h.
  Already configurable (`/v1/relux/prime/agent-policy`, dashboard panel) and explicitly
  *not* infinite. This is the model the rest of the audit aspires to. **Visible + configurable.**
- **Echo fixture demoted to internal-only** (`relux-tools-echo`, `is_internal_plugin`): the
  trivial input-echo ToolSet is hidden from the Plugins list, the Tools list, Prime's
  "what tools can you use?" catalog, and the proposal picker (`server.rs`, `state.rs` filters
  on `is_internal_plugin`). It remains installed only so the dev smoke / test harness can
  exercise the tool/run path, and is revealed only behind `?include_internal=true`. **This is
  already the correct posture** — echo is a dev/test fixture, not product proof. No change
  needed; verified, not rebuilt.
- **`create_agent` grants only the minimal tool, never a preset capability**
  (`agent_presets.rs`): a role preset shapes *description/persona/skills* only and structurally
  carries no permission/adapter field; elevated grants stay on the deliberate, audited
  Governance path. This is **least-privilege by design** (mirrors openclaw `sessions-spawn-tool`
  role-as-context), not a toy cap.
- **MCP loopback bounds** (`relux-core/src/mcp.rs`): loopback-only endpoints, per-call timeout
  clamp `100..60_000ms`, request 256 KiB / response 1 MiB caps, ≤256 discovered tools, text
  clamp 20 000 chars, fail-closed Medium+Required default classification. Anti-DoS + safety;
  mirrors Hermes. **Keep.**
- **Char / byte clamps** across `prime_history`, `prime_*_slots`, `proposed_change`,
  `artifact`, `run_log`, `run_failure`, redaction (`MAX_*_CHARS`, `MAX_*_BYTES`): bound prompt
  context and persisted records against a runaway reply / DoS. **Keep** (internal clamps; not
  operator-facing knobs).
- **HTTP body caps** (`runtime.rs` 256 KiB request / 1 MiB response; `server.rs` 64 MiB
  upload): standard request-size guardrails. **Keep.**
- **`MAX_DECISION_ROUNDS = 3` / `MAX_DECISION_CORRECTIONS = 1`** (`prime_decision.rs`): the
  unified-decision reconcile loop, not an agentic execution loop — it bounds how many times the
  kernel re-prompts the brain to repair one malformed decision envelope. Small is correct here.
  **Keep.**
- **`hierarchy.rs MAX_HIERARCHY_DEPTH = 50`**: cycle/run-away guard on the org tree. **Keep.**

## LATER — real lifts, too large for this slice

- **None.** The last LATER item — `MAX_ACTIVE_JOBS = 4` (`server.rs`, async run-job
  concurrency) — was completed as **FIX NOW #5** above: it is now the configurable
  `PrimeAgentPolicy::max_active_jobs` / `extended_max_active_jobs` (default 4/16, clamped to
  a 64 ceiling), surfaced on the agent-policy route + CLI + dashboard, with an honest
  over-limit message. No artificial toy-cap items remain open in the relux-\* product layer.

---

## Method

Repo-wide ripgrep over the relux layer for: `echo`, `demo`, `mock`, `fixture`,
`placeholder`, `toy`, `mvp`, `v1`, `for now`, `stub`, `fake`, `local-only`, and every
`const *(MAX|LIMIT|CAP)* = <n>`; plus a dashboard sweep for toy/stub/"coming soon" copy
and blank routes. Each numeric constant was read in context and triaged against the bar
above. Reference code read before touching loop/planning behavior (per
`docs/reference-driven-development.md`, BINDING): Hermes `agent/conversation_loop.py`
(`run_conversation` `max_iterations`), `agent/iteration_budget.py` (configurable budget,
default 90/50 — the precedent for "high configurable ceiling, not a tiny constant"), and
the existing `docs/REFERENCE_CODE_MAP.md` mappings.

**Findings that turned out clean (no toy constraint):** Prime's chat framing copy
(`apps/dashboard/src/prime.ts`) already leads with general-agent conversation, not
board/crew-only behavior; the dashboard "todo" hits are kanban board-status labels, not
unfinished stubs; `builtin.rs`'s "runtime not implemented" for un-bundled plugin tools is
an honest capability boundary, not a fake.
</content>
</invoke>
