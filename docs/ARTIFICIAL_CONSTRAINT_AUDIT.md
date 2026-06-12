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

- **`MAX_TASK_TOOL_PLAN_STEPS = 5`** (operator-authored multi-tool plan): a documented bound
  surfaced in `docs/mcp.md`, the Plugins "Create a tool-run task" form, `toolruntask.ts`, and
  several tests. Raising it is reasonable (the plan is operator-authored and gated per step),
  but it touches doc copy + UI labels + builder validation + tests. **Next step:** lift the
  constant, then sweep the "1–5 steps" / "at most 5" copy in `docs/mcp.md`,
  `apps/dashboard/src/{pages/Plugins.tsx,toolruntask.ts}`, and pin the new bound in the
  builder + kernel `validate` tests.
- **Orchestration step cap as an operator policy** (not just a raised constant): fold
  `MAX_ORCHESTRATION_STEPS` into a configurable policy alongside `PrimeAgentPolicy` so an
  operator can tune fan-out width per deployment. **Next step:** add an
  `orchestration` section to the agent-policy surface (`/v1/relux/prime/agent-policy`) with a
  clamped `max_steps`, thread it into `plan_orchestration` (which is pure today and would take
  the limit as an argument), and add a dashboard control.
- **`MAX_TOOL_ROUNDS` as a policy field**: same idea — the read-only context loop bound could
  read from the operator's `PrimeAgentPolicy` (brain-rounds) rather than a module constant, so
  there is one autonomy dial. **Next step:** thread the resolved `AgentLimits` into
  `run_context_loop` / `execute_requested_reads` (both pure today) instead of the constant.
- **`MAX_ACTIVE_JOBS = 4`** (`server.rs`, async run-job concurrency): a real concurrency
  guardrail, but `4` may be low for a busy operator. **Next step:** make it an operator
  setting with a sane clamp, surfaced in the dashboard, rather than a fixed constant.

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
